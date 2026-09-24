//! Write a prepared update to the disk. Runs in the initramfs, before the
//! root filesystem is mounted, so nothing has the blocks it overwrites open.
//!
//! The order is what makes a power cut survivable:
//!
//! 1. `check`: the upload against the SHA-256 the agent's dry run recorded,
//!    and for a root update the staged kernel and room for it on the ESP. A
//!    failure here is `Untouched`: nothing was written, the old system boots.
//! 2. `write`: decompress the upload again, check each chunk against its
//!    hash and write it, sync, drop the device's page cache, read it all
//!    back. The upload decompresses to the same bytes the dry run checked,
//!    since it is the same file, so a chunk failing here means the disk or
//!    the RAM is failing.
//! 3. `install_kernel`, for a root update: a `.new` file, fsync, rename.
//!
//! Anything that goes wrong in 2 or 3 is `Partial`. For a root update the
//! caller keeps the marker so the next boot does all of it again; every step
//! is idempotent. A disk update has nothing to go back to.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::image::{short, skip, Image};
use crate::manifest::{self, Chunk, Manifest, Mode};
use crate::{fsutil, megabytes, KERNEL, MANIFEST, UPLOAD};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Refused before the first write; the old system is intact.
    Untouched(String),
    /// Refused after writing began; the old system is gone. Retry.
    Partial(String),
}

impl Failure {
    pub fn message(&self) -> &str {
        match self {
            Failure::Untouched(message) | Failure::Partial(message) => message,
        }
    }
}

/// The staged manifest, if this tessaro-flash can apply it.
pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let manifest: Manifest = fsutil::read_json(&dir.join(MANIFEST))
        .map_err(|err| format!("the staged manifest is unreadable: {err}"))?;
    if manifest.format != manifest::FORMAT {
        return Err(format!(
            "the update was prepared in format {}, this initramfs applies {}",
            manifest.format,
            manifest::FORMAT
        ));
    }
    Ok(manifest)
}

/// A root update from the staging directory `dir`: `root` is the root
/// partition's device node, `esp` where the boot partition is mounted.
/// `starting` runs once, just before the first write, and must make that
/// fact durable.
pub fn apply(
    dir: &Path,
    root: &Path,
    esp: &Path,
    console: &mut dyn Write,
    starting: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Manifest, Failure> {
    use Failure::{Partial, Untouched};

    let manifest = read_manifest(dir).map_err(Untouched)?;
    if manifest.mode != Mode::Root {
        return Err(Untouched(
            "the update rewrites the whole disk, not the root partition".to_string(),
        ));
    }
    let upload = dir.join(UPLOAD);
    check(&manifest, &upload, dir, root, Some(esp), console).map_err(Untouched)?;
    write(&manifest, &upload, root, console, starting)?;
    install_kernel(&manifest, dir, esp, console).map_err(Partial)?;
    say(console, "update written");
    Ok(manifest)
}

/// Everything that can be known before the first write. `dir` holds the
/// staged kernel and `esp` is where it goes, both for a root update only.
pub fn check(
    manifest: &Manifest,
    upload: &Path,
    dir: &Path,
    device: &Path,
    esp: Option<&Path>,
    console: &mut dyn Write,
) -> Result<(), String> {
    say(console, "checking the upload");
    match crate::sha256_file(upload) {
        Ok((size, sha256)) if size == manifest.upload.size && sha256 == manifest.upload.sha256 => {}
        Ok(_) => return Err("the upload is damaged; nothing was written".to_string()),
        Err(err) => return Err(format!("the upload is unreadable: {err}")),
    }

    let mut target = OpenOptions::new()
        .read(true)
        .write(true)
        .open(device)
        .map_err(|err| format!("{}: {err}", device.display()))?;
    let capacity = target
        .seek(SeekFrom::End(0))
        .map_err(|err| format!("{}: {err}", device.display()))?;
    if capacity < manifest.target.size {
        return Err(format!(
            "{} holds {}, the update needs {}",
            device.display(),
            megabytes(capacity),
            megabytes(manifest.target.size)
        ));
    }

    let Some(kernel) = &manifest.kernel else {
        return Ok(());
    };
    match crate::sha256_file(&dir.join(KERNEL)) {
        Ok((size, sha256)) if size == kernel.size && sha256 == kernel.sha256 => {}
        Ok(_) => return Err("the staged kernel is damaged; nothing was written".to_string()),
        Err(err) => return Err(format!("the staged kernel is unreadable: {err}")),
    }
    let esp = esp.ok_or("the update installs a kernel, but no boot partition was given")?;
    if !kernel_current(esp, kernel) {
        // The .new copy sits next to the old kernel until the rename.
        let free = fsutil::available(esp).unwrap_or(0);
        if free < kernel.size {
            return Err(format!(
                "the boot partition has {} free, the new kernel needs {}",
                megabytes(free),
                megabytes(kernel.size)
            ));
        }
    }
    Ok(())
}

/// Decompress `upload` and write the manifest's chunks to `device`: the
/// root partition, or the whole disk.
pub fn write(
    manifest: &Manifest,
    upload: &Path,
    device: &Path,
    console: &mut dyn Write,
    starting: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), Failure> {
    use Failure::{Partial, Untouched};

    let mut image = Image::open(upload).map_err(|err| Untouched(format!("the upload: {err}")))?;
    let target_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(device)
        .map_err(|err| Untouched(format!("{}: {err}", device.display())))?;
    starting().map_err(Untouched)?;

    let what = match manifest.mode {
        Mode::Root => "the root filesystem",
        Mode::Disk => "the whole disk",
    };
    let total = manifest.target.mapped();
    say(
        console,
        &format!("writing {what}, {} - do not power off", megabytes(total)),
    );
    let (mut position, mut written, mut shown) = (0u64, 0u64, 0u64);
    let mut buffer = Vec::new();
    for chunk in &manifest.target.chunks {
        let at = manifest.target.start + chunk.offset;
        skip(&mut image, at - position).map_err(Partial)?;
        buffer.resize(chunk.len as usize, 0);
        image
            .read_exact(&mut buffer)
            .map_err(|err| Partial(short(err)))?;
        position = at + chunk.len;
        if crate::sha256(&buffer) != chunk.sha256 {
            return Err(Partial(format!(
                "the upload decompressed differently at offset {at} than when it was checked"
            )));
        }
        target_file
            .write_all_at(&buffer, chunk.offset)
            .map_err(|err| Partial(format!("writing {}: {err}", device.display())))?;
        written += chunk.len;
        let percent = written * 100 / total.max(1);
        if percent >= shown + 10 {
            shown = percent - percent % 10;
            say(console, &format!("writing {shown}%"));
        }
    }
    target_file
        .sync_all()
        .map_err(|err| Partial(format!("syncing {}: {err}", device.display())))?;
    fsutil::drop_cache(&target_file);
    drop(target_file);

    say(console, &format!("reading {what} back"));
    let target_file =
        File::open(device).map_err(|err| Partial(format!("{}: {err}", device.display())))?;
    for chunk in &manifest.target.chunks {
        read_chunk(&target_file, chunk, &mut buffer)
            .map_err(|err| Partial(format!("reading {} back: {err}", device.display())))?;
        if crate::sha256(&buffer) != chunk.sha256 {
            return Err(Partial(format!(
                "{} reads back differently at offset {}",
                device.display(),
                chunk.offset
            )));
        }
    }
    Ok(())
}

/// The kernel, last: the new root's modules are already in place when the
/// new kernel first boots.
pub fn install_kernel(
    manifest: &Manifest,
    dir: &Path,
    esp: &Path,
    console: &mut dyn Write,
) -> Result<(), String> {
    let Some(kernel) = &manifest.kernel else {
        return Ok(());
    };
    if kernel_current(esp, kernel) {
        say(console, "the kernel is unchanged");
        return Ok(());
    }
    say(
        console,
        &format!("installing the new kernel as {}", kernel.name),
    );
    install(&dir.join(KERNEL), esp, &kernel.name, &kernel.sha256)
}

fn kernel_current(esp: &Path, kernel: &manifest::File) -> bool {
    crate::sha256_file(&esp.join(&kernel.name))
        .is_ok_and(|(size, sha256)| size == kernel.size && sha256 == kernel.sha256)
}

fn read_chunk(file: &File, chunk: &Chunk, buffer: &mut Vec<u8>) -> io::Result<()> {
    buffer.resize(chunk.len as usize, 0);
    file.read_exact_at(buffer, chunk.offset)
}

/// Copy the staged kernel to `<esp>/<name>.new`, check it, rename it over
/// the old one. vfat has no atomic replace, but the window is one directory
/// entry, not a file's worth of data.
fn install(staged: &Path, esp: &Path, name: &str, sha256: &str) -> Result<(), String> {
    let fresh = esp.join(format!("{name}.new"));
    let body = fs::read(staged).map_err(|err| format!("reading the staged kernel: {err}"))?;
    {
        let mut file = File::create(&fresh).map_err(|err| format!("{}: {err}", fresh.display()))?;
        file.write_all(&body)
            .and_then(|()| file.sync_all())
            .map_err(|err| format!("{}: {err}", fresh.display()))?;
    }
    let (_, written) =
        crate::sha256_file(&fresh).map_err(|err| format!("{}: {err}", fresh.display()))?;
    if written != sha256 {
        return Err(format!("{} reads back differently", fresh.display()));
    }
    fs::rename(&fresh, esp.join(name)).map_err(|err| format!("installing {name}: {err}"))?;
    fsutil::sync_dir(esp).map_err(|err| format!("syncing the boot partition: {err}"))
}

fn say(console: &mut dyn Write, line: &str) {
    let _ = writeln!(console, "tessaro-update: {line}");
    let _ = console.flush();
}

/// Only for tests: whether `file` holds every chunk of `manifest`.
#[cfg(test)]
fn holds(file: &Path, manifest: &Manifest) -> bool {
    let file = File::open(file).unwrap();
    let mut buffer = Vec::new();
    manifest.target.chunks.iter().all(|chunk| {
        read_chunk(&file, chunk, &mut buffer).unwrap();
        crate::sha256(&buffer) == chunk.sha256
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{staged, staged_as, DISK_SECTORS, MIB};

    struct Device {
        _dir: tempfile::TempDir,
        staging: std::path::PathBuf,
        root: std::path::PathBuf,
        esp: std::path::PathBuf,
        manifest: Manifest,
    }

    fn device() -> Device {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("update");
        let esp = dir.path().join("esp");
        fs::create_dir(&staging).unwrap();
        fs::create_dir(&esp).unwrap();
        fs::write(esp.join("bzImage"), b"the old kernel").unwrap();
        let (_, manifest) = staged(&staging);
        // The old root: the right size, full of something else.
        let root = dir.path().join("sda2");
        fs::write(&root, vec![0x5au8; (6 * MIB) as usize]).unwrap();
        Device {
            _dir: dir,
            staging,
            root,
            esp,
            manifest,
        }
    }

    fn run(device: &Device) -> Result<Manifest, Failure> {
        apply(
            &device.staging,
            &device.root,
            &device.esp,
            &mut io::sink(),
            &mut || Ok(()),
        )
    }

    #[test]
    fn the_mapped_blocks_and_the_kernel_are_written() {
        let device = device();
        run(&device).unwrap();

        assert!(holds(&device.root, &device.manifest));
        assert_eq!(fs::read(device.esp.join("bzImage")).unwrap(), b"a kernel");
        assert!(!device.esp.join("bzImage.new").exists());
        // Unmapped blocks are left alone: free space in the new filesystem.
        let root = fs::read(&device.root).unwrap();
        assert_eq!(root[(4600 * 1024) as usize], 0x5a);
    }

    #[test]
    fn applying_twice_is_the_same_as_once() {
        let device = device();
        run(&device).unwrap();
        let first = fs::read(&device.root).unwrap();
        run(&device).unwrap();
        assert_eq!(fs::read(&device.root).unwrap(), first);
    }

    #[test]
    fn a_write_cut_short_is_finished_by_the_next_attempt() {
        let device = device();
        // As if the power went halfway through: the first chunk only.
        let (image, _) = crate::testing::disk();
        let chunk = &device.manifest.target.chunks[0];
        let start = (device.manifest.target.start + chunk.offset) as usize;
        let target = OpenOptions::new().write(true).open(&device.root).unwrap();
        target
            .write_all_at(&image[start..start + chunk.len as usize], chunk.offset)
            .unwrap();
        drop(target);
        assert!(!holds(&device.root, &device.manifest));

        run(&device).unwrap();
        assert!(holds(&device.root, &device.manifest));
    }

    #[test]
    fn a_damaged_upload_is_refused_with_nothing_written() {
        let device = device();
        let path = device.staging.join(UPLOAD);
        let mut upload = fs::read(&path).unwrap();
        let middle = upload.len() / 2;
        upload[middle] ^= 0xff;
        fs::write(&path, upload).unwrap();
        let before = fs::read(&device.root).unwrap();

        let failure = run(&device).unwrap_err();
        assert!(matches!(failure, Failure::Untouched(_)), "{failure:?}");
        assert!(failure.message().contains("damaged"));
        assert_eq!(fs::read(&device.root).unwrap(), before);
        assert_eq!(
            fs::read(device.esp.join("bzImage")).unwrap(),
            b"the old kernel"
        );
    }

    #[test]
    fn a_damaged_kernel_is_refused_with_nothing_written() {
        let device = device();
        fs::write(device.staging.join(KERNEL), b"a kernal").unwrap();
        let before = fs::read(&device.root).unwrap();

        assert!(matches!(run(&device), Err(Failure::Untouched(_))));
        assert_eq!(fs::read(&device.root).unwrap(), before);
    }

    #[test]
    fn a_partition_too_small_is_refused() {
        let device = device();
        fs::write(&device.root, vec![0u8; MIB as usize]).unwrap();
        let failure = run(&device).unwrap_err();
        assert!(matches!(failure, Failure::Untouched(_)), "{failure:?}");
        assert!(failure.message().contains("needs"));
    }

    #[test]
    fn an_unchanged_kernel_is_not_rewritten() {
        let device = device();
        fs::write(device.esp.join("bzImage"), b"a kernel").unwrap();
        let mut console = Vec::new();
        apply(
            &device.staging,
            &device.root,
            &device.esp,
            &mut console,
            &mut || Ok(()),
        )
        .unwrap();
        assert!(String::from_utf8(console).unwrap().contains("unchanged"));
    }

    #[test]
    fn starting_is_called_only_once_the_checks_passed() {
        let device = device();
        let mut calls = 0;
        apply(
            &device.staging,
            &device.root,
            &device.esp,
            &mut io::sink(),
            &mut || {
                calls += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls, 1);

        fs::write(device.staging.join(KERNEL), b"damaged").unwrap();
        let mut called = false;
        let _ = apply(
            &device.staging,
            &device.root,
            &device.esp,
            &mut io::sink(),
            &mut || {
                called = true;
                Ok(())
            },
        );
        assert!(!called);
    }

    #[test]
    fn a_disk_update_writes_the_whole_image_to_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let (image, manifest) = staged_as(dir.path(), Mode::Disk);
        let disk = dir.path().join("sda");
        fs::write(&disk, vec![0x5au8; (DISK_SECTORS * 512) as usize]).unwrap();
        let upload = dir.path().join(UPLOAD);

        check(&manifest, &upload, dir.path(), &disk, None, &mut io::sink()).unwrap();
        write(&manifest, &upload, &disk, &mut io::sink(), &mut || Ok(())).unwrap();

        assert!(holds(&disk, &manifest));
        let written = fs::read(&disk).unwrap();
        // The partition table, the boot partition's mapped blocks, data's.
        assert_eq!(&written[..512 * 34], &image[..512 * 34]);
        assert_eq!(written[MIB as usize], image[MIB as usize]);
        assert_eq!(written[(10 * MIB) as usize], image[(10 * MIB) as usize]);
        // Past the image, the disk is as it was.
        assert_eq!(written[(13 * MIB) as usize], 0x5a);
    }

    #[test]
    fn a_disk_update_is_not_applied_as_a_root_update() {
        let dir = tempfile::tempdir().unwrap();
        staged_as(dir.path(), Mode::Disk);
        let root = dir.path().join("sda2");
        fs::write(&root, vec![0u8; (6 * MIB) as usize]).unwrap();
        let failure = apply(dir.path(), &root, dir.path(), &mut io::sink(), &mut || {
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(failure, Failure::Untouched(_)), "{failure:?}");
    }
}
