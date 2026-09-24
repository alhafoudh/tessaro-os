//! Durable writes. The same recipe as tessaro-agent's store, without the
//! lock: nothing else writes the staging directory while an update is being
//! applied, and the agent serialises its own writers.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// `body` into `path`, all or nothing: a temporary file, fsync, rename,
/// fsync of the directory.
pub fn write_atomic(path: &Path, body: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temporary = dir.join(format!(".{name}.tmp"));
    {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(body)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    sync_dir(dir)
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut body = serde_json::to_vec_pretty(value)?;
    body.push(b'\n');
    write_atomic(path, &body)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|err| format!("{}: {err}", path.display()))
}

pub fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

pub fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Bytes a non-root writer could still put on the filesystem holding `path`.
pub fn available(path: &Path) -> io::Result<u64> {
    usage(path).map(|usage| usage.available)
}

/// The filesystem holding `path`, in bytes, as `df` reports it: `size` is
/// what it holds after its own metadata, `free` includes root's reserve,
/// `available` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub size: u64,
    pub free: u64,
    pub available: u64,
}

// The statvfs fields are u64 on the 64-bit targets, 32-bit on others.
#[allow(clippy::unnecessary_cast)]
pub fn usage(path: &Path) -> io::Result<Usage> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: c_path is NUL terminated and stat is a valid out pointer.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs succeeded, so it filled the struct.
    let stat = unsafe { stat.assume_init() };
    let block = stat.f_frsize as u64;
    Ok(Usage {
        size: stat.f_blocks as u64 * block,
        free: stat.f_bfree as u64 * block,
        available: stat.f_bavail as u64 * block,
    })
}

/// `BLKRRPART` on `disk`: the kernel drops its partitions and reads the
/// table again. Fails with EBUSY while any of them is mounted.
pub fn reread_partitions(disk: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // _IO(0x12, 95), from <linux/fs.h>.
    const BLKRRPART: u32 = 0x125f;
    let file = File::open(disk)?;
    // SAFETY: a valid open descriptor, and an ioctl that takes no argument.
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), BLKRRPART as _) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Drop `file`'s pages from the page cache, so the next read comes from the
/// device and not from what was just written. Best effort.
pub fn drop_cache(file: &File) {
    use std::os::fd::AsRawFd;
    // SAFETY: a valid open descriptor; the call has no memory effects.
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
}
