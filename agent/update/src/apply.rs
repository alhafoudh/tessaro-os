//! Write a prepared update to the disk. Runs in the initramfs, before the
//! root filesystem is mounted, so nothing has the blocks it overwrites open.
//!
//! The order is what makes a power cut survivable:
//!
//! 1. Verify every staged chunk and the staged kernel. A failure here is
//!    `Untouched`: nothing was written, the old system boots.
//! 2. Write the chunks, sync, drop the device's page cache, read them back.
//! 3. Swap the kernel on the ESP: a `.new` file, fsync, rename.
//!
//! Anything that goes wrong in 2 or 3 is `Partial`, and the caller keeps the
//! marker so the next boot does all of it again. Every step is idempotent.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::manifest::{self, Chunk, Manifest};
use crate::{fsutil, megabytes, KERNEL, MANIFEST, ROOT_IMAGE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Refused before the first write; the old system is intact.
    Untouched(String),
    /// Refused after writing began; the old root is gone. Retry.
    Partial(String),
}

impl Failure {
    pub fn message(&self) -> &str {
        match self {
            Failure::Untouched(message) | Failure::Partial(message) => message,
        }
    }
}

/// `dir` is the staging directory, `root` the root partition's device node,
/// `esp` where the boot partition is mounted. `starting` runs once, just
/// before the first write, and must make that fact durable.
pub fn apply(
    dir: &Path,
    root: &Path,
    esp: &Path,
    console: &mut dyn Write,
    starting: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Manifest, Failure> {
    use Failure::{Partial, Untouched};

    let manifest: Manifest = fsutil::read_json(&dir.join(MANIFEST))
        .map_err(|err| Untouched(format!("the staged manifest is unreadable: {err}")))?;
    if manifest.format != manifest::FORMAT {
        return Err(Untouched(format!(
            "the update was prepared in format {}, this initramfs applies {}",
            manifest.format,
            manifest::FORMAT
        )));
    }

    // 1. Nothing touches the disk until everything staged is known good.
    say(console, "checking the staged update");
    let staged = File::open(dir.join(ROOT_IMAGE))
        .map_err(|err| Untouched(format!("the staged root image is missing: {err}")))?;
    let mut buffer = Vec::new();
    for chunk in &manifest.root.chunks {
        read_chunk(&staged, chunk, &mut buffer)
            .map_err(|err| Untouched(format!("reading the staged root image: {err}")))?;
        if crate::sha256(&buffer) != chunk.sha256 {
            return Err(Untouched(format!(
                "the staged root image is damaged at offset {}; nothing was written",
                chunk.offset
            )));
        }
    }
    let kernel = &manifest.boot.kernel;
    let staged_kernel = dir.join(KERNEL);
    match crate::sha256_file(&staged_kernel) {
        Ok((size, sha256)) if size == kernel.size && sha256 == kernel.sha256 => {}
        Ok(_) => {
            return Err(Untouched(
                "the staged kernel is damaged; nothing was written".to_string(),
            ))
        }
        Err(err) => return Err(Untouched(format!("the staged kernel is unreadable: {err}"))),
    }

    let mut target = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root)
        .map_err(|err| Untouched(format!("{}: {err}", root.display())))?;
    let capacity = target
        .seek(SeekFrom::End(0))
        .map_err(|err| Untouched(format!("{}: {err}", root.display())))?;
    if capacity < manifest.root.size {
        return Err(Untouched(format!(
            "{} holds {}, the new root filesystem needs {}",
            root.display(),
            megabytes(capacity),
            megabytes(manifest.root.size)
        )));
    }

    let esp_kernel = esp.join(&kernel.name);
    let kernel_current = crate::sha256_file(&esp_kernel)
        .is_ok_and(|(size, sha256)| size == kernel.size && sha256 == kernel.sha256);
    if !kernel_current {
        // The .new copy sits next to the old kernel until the rename.
        let free = fsutil::available(esp).unwrap_or(0);
        if free < kernel.size {
            return Err(Untouched(format!(
                "the boot partition has {} free, the new kernel needs {}",
                megabytes(free),
                megabytes(kernel.size)
            )));
        }
    }

    // 2. The root partition.
    starting().map_err(Untouched)?;
    let total = manifest.root.mapped();
    let mut written = 0u64;
    let mut shown = 0u64;
    say(
        console,
        &format!(
            "writing the root filesystem, {} - do not power off",
            megabytes(total)
        ),
    );
    for chunk in &manifest.root.chunks {
        read_chunk(&staged, chunk, &mut buffer)
            .map_err(|err| Partial(format!("reading the staged root image: {err}")))?;
        target
            .write_all_at(&buffer, chunk.offset)
            .map_err(|err| Partial(format!("writing {}: {err}", root.display())))?;
        written += chunk.len;
        let percent = written * 100 / total.max(1);
        if percent >= shown + 10 {
            shown = percent - percent % 10;
            say(console, &format!("writing root {shown}%"));
        }
    }
    target
        .sync_all()
        .map_err(|err| Partial(format!("syncing {}: {err}", root.display())))?;
    fsutil::drop_cache(&target);
    drop(target);

    say(console, "reading the root filesystem back");
    let target = File::open(root).map_err(|err| Partial(format!("{}: {err}", root.display())))?;
    for chunk in &manifest.root.chunks {
        read_chunk(&target, chunk, &mut buffer)
            .map_err(|err| Partial(format!("reading {} back: {err}", root.display())))?;
        if crate::sha256(&buffer) != chunk.sha256 {
            return Err(Partial(format!(
                "{} reads back differently at offset {}",
                root.display(),
                chunk.offset
            )));
        }
    }

    // 3. The kernel, last: the new root's modules are already in place when
    //    the new kernel first boots.
    if kernel_current {
        say(console, "the kernel is unchanged");
    } else {
        say(
            console,
            &format!("installing the new kernel as {}", kernel.name),
        );
        install(&staged_kernel, esp, &kernel.name, &kernel.sha256).map_err(Partial)?;
    }

    say(console, "update written");
    Ok(manifest)
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

/// Only for tests: hash of what `apply` would leave at `chunk` in `file`.
#[cfg(test)]
fn digest_at(file: &Path, chunk: &Chunk) -> String {
    let file = File::open(file).unwrap();
    let mut buffer = Vec::new();
    read_chunk(&file, chunk, &mut buffer).unwrap();
    crate::sha256(&buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{staged, MIB};

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

    fn matches(device: &Device) -> bool {
        device
            .manifest
            .root
            .chunks
            .iter()
            .all(|chunk| digest_at(&device.root, chunk) == chunk.sha256)
    }

    #[test]
    fn the_mapped_blocks_and_the_kernel_are_written() {
        let device = device();
        run(&device).unwrap();

        assert!(matches(&device));
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
        // As if the power went halfway through the first chunk.
        let chunk = &device.manifest.root.chunks[0];
        let staged = fs::read(device.staging.join(ROOT_IMAGE)).unwrap();
        let half = (chunk.len / 2) as usize;
        let target = OpenOptions::new().write(true).open(&device.root).unwrap();
        target.write_all_at(&staged[..half], 0).unwrap();
        drop(target);
        assert!(!matches(&device));

        run(&device).unwrap();
        assert!(matches(&device));
    }

    #[test]
    fn damaged_staging_is_refused_with_nothing_written() {
        let device = device();
        let path = device.staging.join(ROOT_IMAGE);
        let mut staged = fs::read(&path).unwrap();
        let last = device.manifest.root.chunks.last().unwrap();
        staged[(last.offset + 10) as usize] ^= 0xff;
        fs::write(&path, staged).unwrap();
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
    fn starting_is_called_only_once_staging_verified() {
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
}
