//! The dry run: prove an uploaded `.wic.zst` is what its bmap says, so the
//! initramfs can write it without second-guessing.
//!
//! One sequential pass over the decompressed image. The bmap's ranges come in
//! disk order, so each is read and hashed against the bmap, and the parts
//! inside the target - the root partition, or the whole disk - are cut into
//! chunks with checksums of their own, which is what the initramfs checks
//! each piece against as it writes it. Nothing of the image is kept except,
//! for a root update, the image's boot partition, sparse in `boot.img`, long
//! enough to copy the kernel out of it. The pass stops at the end of what is
//! needed: a root update never decompresses `/data` and swap.
//!
//! The bmap's checksum covers a whole range and is only known at its end,
//! so a mismatch fails the whole preparation rather than one piece of it.
//! That is the right answer anyway - it means the image and the bmap are from
//! different builds.

use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read};
use std::os::unix::fs::FileExt;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::bmap::Bmap;
use crate::image::{short, skip, Image};
use crate::manifest::{self, Chunk, Manifest, Mode, Source, Target};
use crate::ptable::{self, Partition};
use crate::{fsutil, BOOT_IMAGE, BOOT_PARTITION, CHUNK, KERNEL, MANIFEST, ROOT_PARTITION};

/// How much of the image is read before anything else, for the partition
/// table. GPT needs 17 KiB of it.
const HEAD: u64 = 1 << 20;

const BUFFER: usize = 1 << 20;

pub trait Observer {
    /// `done` of `total` mapped bytes read and checked.
    fn progress(&mut self, done: u64, total: u64);
    fn cancelled(&self) -> bool {
        false
    }
}

/// Check `image` and describe it in `dir`'s manifest.
///
/// * `check` sees the image's partition table before anything is written,
///   and refuses an image that is not for this disk.
/// * `kernel`, for a root update, copies the kernel out of the image's boot
///   partition (a vfat image) into the path it is given, and returns its name
///   on the ESP.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    image: Image,
    bmap: &Bmap,
    mode: Mode,
    source: Source,
    dir: &Path,
    check: impl FnOnce(&[Partition]) -> Result<(), String>,
    kernel: impl FnOnce(&Path, &Path) -> Result<String, String>,
    observer: &mut dyn Observer,
) -> Result<Manifest, String> {
    let mut image = image;
    let head_len = HEAD.min(bmap.image_size) as usize;
    let mut head = vec![0u8; head_len];
    image.read_exact(&mut head).map_err(short)?;
    let partitions = ptable::parse(&head)?;
    let boot = ptable::find(&partitions, BOOT_PARTITION)
        .ok_or("the image has no boot partition")?
        .clone();
    let root = ptable::find(&partitions, ROOT_PARTITION)
        .ok_or("the image has no root partition")?
        .clone();
    check(&partitions)?;
    if partitions.iter().any(|part| part.end() > bmap.image_size) {
        return Err("the image's partitions extend past its end".to_string());
    }

    // Where the chunks come from, and how far the pass has to read.
    let (target, limit) = match mode {
        Mode::Root => ((root.start, root.size), boot.end().max(root.end())),
        Mode::Disk => ((0, bmap.image_size), bmap.image_size),
    };
    let (target_start, target_size) = target;
    let target_end = target_start + target_size;

    let boot_path = dir.join(BOOT_IMAGE);
    let boot_file = match mode {
        Mode::Root => Some(sparse(&boot_path, boot.size)?),
        Mode::Disk => None,
    };

    let total = bmap.mapped_within(0, limit);
    let mut done = 0u64;
    let mut chunks = Chunker::default();
    let mut position = 0u64;
    let mut buffer = vec![0u8; BUFFER];
    {
        let mut stream = Cursor::new(head).chain(&mut image);
        for range in &bmap.ranges {
            let start = bmap.start(range);
            let end = bmap.end(range);
            if start >= limit {
                break;
            }
            skip(&mut stream, start - position)?;
            position = start;

            let mut hasher = Sha256::new();
            while position < end {
                if observer.cancelled() {
                    return Err("cancelled".to_string());
                }
                let len = (end - position).min(BUFFER as u64) as usize;
                let data = &mut buffer[..len];
                stream.read_exact(data).map_err(short)?;
                hasher.update(&*data);

                if let Some((from, to)) = overlap(position, len, target_start, target_end) {
                    let piece = &data[(from - position) as usize..(to - position) as usize];
                    chunks.feed(from - target_start, piece);
                }
                if let Some(file) = &boot_file {
                    if let Some((from, to)) = overlap(position, len, boot.start, boot.end()) {
                        let piece = &data[(from - position) as usize..(to - position) as usize];
                        file.write_at(piece, from - boot.start)
                            .map_err(|err| format!("writing the staging on /data: {err}"))?;
                    }
                }

                position += len as u64;
                done += (position.min(limit)).saturating_sub(position - len as u64);
                observer.progress(done.min(total), total);
            }

            if crate::hex(&hasher.finalize()) != range.sha256 {
                return Err(format!(
                    "blocks {}-{} do not match the bmap's checksum: the image and the bmap \
                     are from different builds, or the upload is damaged",
                    range.first, range.last
                ));
            }
        }
    }
    let chunks = chunks.finish();

    let kernel = match boot_file {
        Some(file) => {
            file.sync_all()
                .map_err(|err| format!("syncing the staging on /data: {err}"))?;
            drop(file);
            let kernel_path = dir.join(KERNEL);
            let name = kernel(&boot_path, &kernel_path)?;
            let (size, sha256) = crate::sha256_file(&kernel_path)
                .map_err(|err| format!("reading the staged kernel: {err}"))?;
            fs::remove_file(&boot_path).map_err(|err| format!("{}: {err}", boot_path.display()))?;
            Some(manifest::File { name, size, sha256 })
        }
        None => None,
    };

    let (size, sha256) = image
        .finish()
        .map_err(|err| format!("reading the upload: {err}"))?;
    let manifest = Manifest {
        format: manifest::FORMAT,
        source,
        upload: manifest::Digest { size, sha256 },
        mode,
        target: Target {
            start: target_start,
            size: target_size,
            partuuid: root.partuuid,
            chunks,
        },
        kernel,
    };
    fsutil::write_json(&dir.join(MANIFEST), &manifest)
        .map_err(|err| format!("writing the manifest: {err}"))?;
    Ok(manifest)
}

/// The part of `position..position+len` inside `start..end`, if any.
fn overlap(position: u64, len: usize, start: u64, end: u64) -> Option<(u64, u64)> {
    let from = position.max(start);
    let to = (position + len as u64).min(end);
    (from < to).then_some((from, to))
}

fn sparse(path: &Path, size: u64) -> Result<File, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.set_len(size).map(|()| file))
        .map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(file)
}

/// Cuts what is written to the target into chunks: contiguous, at most
/// `CHUNK` long, each with its SHA-256.
#[derive(Default)]
struct Chunker {
    done: Vec<Chunk>,
    open: Option<(u64, u64, Sha256)>,
}

impl Chunker {
    fn feed(&mut self, mut offset: u64, mut data: &[u8]) {
        while !data.is_empty() {
            if let Some((start, len, _)) = &self.open {
                if start + len != offset || *len == CHUNK {
                    self.close();
                }
            }
            let (_, len, hasher) = self.open.get_or_insert_with(|| (offset, 0, Sha256::new()));
            let take = ((CHUNK - *len) as usize).min(data.len());
            hasher.update(&data[..take]);
            *len += take as u64;
            offset += take as u64;
            data = &data[take..];
        }
    }

    fn close(&mut self) {
        if let Some((offset, len, hasher)) = self.open.take() {
            self.done.push(Chunk {
                offset,
                len,
                sha256: crate::hex(&hasher.finalize()),
            });
        }
    }

    fn finish(mut self) -> Vec<Chunk> {
        self.close();
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bmap;
    use crate::testing::{disk, fake_kernel, image, source, staged, Quiet, BLOCK, MIB};

    fn run(image_bytes: Vec<u8>, text: &str, mode: Mode, dir: &Path) -> Result<Manifest, String> {
        prepare(
            image(image_bytes),
            &bmap::parse(text).unwrap(),
            mode,
            source(),
            dir,
            |_| Ok(()),
            fake_kernel,
            &mut Quiet,
        )
    }

    #[test]
    fn a_root_update_chunks_the_mapped_root_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let (image, manifest) = staged(dir.path());

        assert_eq!(manifest.mode, Mode::Root);
        assert_eq!(manifest.target.start, 2 * MIB);
        assert_eq!(manifest.target.size, 6 * MIB);
        assert_eq!(
            manifest.target.partuuid,
            "02020202-0202-0202-0202-020202020202"
        );
        // Mapped: the first 1101 blocks (more than one chunk), and the last
        // MiB (the range that runs on into data is cut at the partition's end).
        assert_eq!(manifest.target.mapped(), 1101 * BLOCK + MIB);
        let chunks = &manifest.target.chunks;
        assert!(chunks.iter().all(|chunk| chunk.len <= CHUNK));
        assert_eq!(chunks[0].offset, 0);
        assert_eq!(chunks[1].offset, CHUNK);
        assert_eq!(chunks[2].offset, 5 * MIB);
        let root = &image[(2 * MIB) as usize..(8 * MIB) as usize];
        for chunk in chunks {
            let bytes = &root[chunk.offset as usize..(chunk.offset + chunk.len) as usize];
            assert_eq!(crate::sha256(bytes), chunk.sha256);
        }

        let kernel = manifest.kernel.as_ref().unwrap();
        assert_eq!(kernel.name, "bzImage");
        assert_eq!(kernel.size, 8);
        assert!(dir.path().join(KERNEL).exists());
        assert!(!dir.path().join(BOOT_IMAGE).exists());

        // The whole upload is hashed, though the pass stopped at root's end.
        let upload = fs::read(dir.path().join(crate::UPLOAD)).unwrap();
        assert_eq!(manifest.upload.size, upload.len() as u64);
        assert_eq!(manifest.upload.sha256, crate::sha256(&upload));
        let on_disk: Manifest = fsutil::read_json(&dir.path().join(MANIFEST)).unwrap();
        assert_eq!(on_disk, manifest);
    }

    #[test]
    fn a_disk_update_chunks_every_mapped_byte_and_needs_no_kernel() {
        let (image, text) = disk();
        let dir = tempfile::tempdir().unwrap();
        let manifest = prepare(
            crate::testing::image(image.clone()),
            &bmap::parse(&text).unwrap(),
            Mode::Disk,
            source(),
            dir.path(),
            |_| Ok(()),
            |_, _| panic!("a disk update writes the whole ESP; no kernel is copied"),
            &mut Quiet,
        )
        .unwrap();

        assert_eq!(manifest.mode, Mode::Disk);
        assert_eq!(manifest.target.start, 0);
        assert_eq!(manifest.target.size, 12 * MIB);
        assert!(manifest.kernel.is_none());
        let bmap = bmap::parse(&text).unwrap();
        assert_eq!(manifest.target.mapped(), bmap.mapped_within(0, 12 * MIB));
        // The partition table, first.
        assert_eq!(manifest.target.chunks[0].offset, 0);
        for chunk in &manifest.target.chunks {
            let bytes = &image[chunk.offset as usize..(chunk.offset + chunk.len) as usize];
            assert_eq!(crate::sha256(bytes), chunk.sha256);
        }
        assert!(!dir.path().join(BOOT_IMAGE).exists());
    }

    #[test]
    fn a_zst_image_prepares_the_same() {
        let (image, text) = disk();
        let compressed = crate::testing::compress(&image);
        let dir = tempfile::tempdir().unwrap();
        let manifest = run(compressed.clone(), &text, Mode::Root, dir.path()).unwrap();

        let plain = tempfile::tempdir().unwrap();
        let (_, expected) = staged(plain.path());
        assert_eq!(manifest.target.chunks, expected.target.chunks);
        // What the initramfs checks is the file as uploaded, compressed.
        assert_eq!(manifest.upload.sha256, crate::sha256(&compressed));
    }

    #[test]
    fn a_bz2_image_prepares_the_same() {
        let (image, text) = disk();
        let compressed = crate::testing::compress_bz2(&image);
        let dir = tempfile::tempdir().unwrap();
        let manifest = run(compressed.clone(), &text, Mode::Root, dir.path()).unwrap();

        let plain = tempfile::tempdir().unwrap();
        let (_, expected) = staged(plain.path());
        assert_eq!(manifest.target.chunks, expected.target.chunks);
        // What the initramfs checks is the file as uploaded, compressed.
        assert_eq!(manifest.upload.sha256, crate::sha256(&compressed));
    }

    #[test]
    fn a_bmap_from_another_build_is_refused() {
        let (mut image, text) = disk();
        image[(3 * MIB) as usize] ^= 0xff;
        let dir = tempfile::tempdir().unwrap();
        let err = run(image, &text, Mode::Root, dir.path()).unwrap_err();
        assert!(err.contains("different builds"), "{err}");
        assert!(!dir.path().join(MANIFEST).exists());
    }

    #[test]
    fn a_disk_update_checks_data_too() {
        // Past the root partition: a root update never reads it.
        let (mut image, text) = disk();
        image[(10 * MIB) as usize] ^= 0xff;
        let dir = tempfile::tempdir().unwrap();
        run(image.clone(), &text, Mode::Root, dir.path()).unwrap();
        let err = run(image, &text, Mode::Disk, dir.path()).unwrap_err();
        assert!(err.contains("different builds"), "{err}");
    }

    #[test]
    fn the_layout_check_runs_before_anything_is_written() {
        let (image_bytes, text) = disk();
        let dir = tempfile::tempdir().unwrap();
        let err = prepare(
            image(image_bytes),
            &bmap::parse(&text).unwrap(),
            Mode::Root,
            source(),
            dir.path(),
            |_| Err("not this disk".to_string()),
            fake_kernel,
            &mut Quiet,
        )
        .unwrap_err();
        assert_eq!(err, "not this disk");
        assert!(!dir.path().join(BOOT_IMAGE).exists());
    }

    #[test]
    fn a_truncated_image_is_refused() {
        let (image, text) = disk();
        let dir = tempfile::tempdir().unwrap();
        let err = run(
            image[..(5 * MIB) as usize].to_vec(),
            &text,
            Mode::Root,
            dir.path(),
        )
        .unwrap_err();
        assert!(err.contains("ends before"), "{err}");
    }

    #[test]
    fn a_truncated_zst_image_is_refused() {
        let (image, text) = disk();
        let compressed = crate::testing::compress(&image);
        let dir = tempfile::tempdir().unwrap();
        let err = run(
            compressed[..compressed.len() / 4].to_vec(),
            &text,
            Mode::Root,
            dir.path(),
        )
        .unwrap_err();
        assert!(err.contains("ends before"), "{err}");
    }

    /// Against a real build: `TESSARO_TEST_WIC=path/to/x.rootfs.wic.zst
    /// cargo test -p update -- --ignored real_image`, or an older `.wic.bz2`.
    /// The bmap is found next to it; the kernel is not extracted (that needs
    /// a loop mount).
    #[test]
    #[ignore]
    fn a_real_image_prepares() {
        let Ok(path) = std::env::var("TESSARO_TEST_WIC") else {
            return;
        };
        let path = Path::new(&path);
        let bmap_path = path.with_extension("bmap");
        let bmap = bmap::parse(&fs::read_to_string(bmap_path).unwrap()).unwrap();
        for mode in [Mode::Root, Mode::Disk] {
            let dir = tempfile::tempdir().unwrap();
            let started = std::time::Instant::now();
            let manifest = prepare(
                Image::open(path).unwrap(),
                &bmap,
                mode,
                source(),
                dir.path(),
                |partitions| {
                    eprintln!("{partitions:#?}");
                    Ok(())
                },
                fake_kernel,
                &mut Quiet,
            )
            .unwrap();
            eprintln!(
                "{mode:?}: {} mapped of {}, {} chunks, in {:?}",
                manifest.target.mapped(),
                manifest.target.size,
                manifest.target.chunks.len(),
                started.elapsed()
            );
        }
    }

    #[test]
    fn progress_reaches_the_total() {
        struct Last(u64, u64);
        impl Observer for Last {
            fn progress(&mut self, done: u64, total: u64) {
                assert!(done >= self.0, "progress went backwards");
                *self = Last(done, total);
            }
        }
        let (image_bytes, text) = disk();
        let bmap = bmap::parse(&text).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut last = Last(0, 0);
        prepare(
            image(image_bytes),
            &bmap,
            Mode::Root,
            source(),
            dir.path(),
            |_| Ok(()),
            fake_kernel,
            &mut last,
        )
        .unwrap();
        assert_eq!(last.0, last.1);
        assert_eq!(last.1, bmap.mapped_within(0, 8 * MIB));
    }
}
