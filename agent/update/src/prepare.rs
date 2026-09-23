//! Turn an uploaded `.wic.bz2` into staging the initramfs can apply blindly.
//!
//! One sequential pass over the decompressed image: the bmap's ranges come
//! in disk order, so each is read, hashed and - where it overlaps the boot or
//! the root partition - written into `boot.img` / `root.img` at the
//! partition-relative offset. Both are sparse files the size of their
//! partition, so only the mapped bytes take space on `/data`. The pass stops
//! after the root partition: `/data` and swap in the image are never needed.
//!
//! The bmap's checksum covers a whole range and is only known at its end,
//! so a mismatch fails the whole preparation rather than one piece of it.
//! That is the right answer anyway - it means the image and the bmap are from
//! different builds.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read};
use std::os::unix::fs::FileExt;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::bmap::Bmap;
use crate::manifest::{self, Boot, Chunk, Manifest, Root, Source};
use crate::ptable::{self, Partition};
use crate::{
    fsutil, BOOT_IMAGE, BOOT_PARTITION, CHUNK, KERNEL, MANIFEST, ROOT_IMAGE, ROOT_PARTITION,
};

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

/// Reads the image, bz2-compressed or not: pbzip2 output starts `BZh`.
pub fn open_image(path: &Path) -> io::Result<Box<dyn Read>> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 3];
    let read = file.read(&mut magic)?;
    let file = File::open(path)?;
    if read == 3 && &magic == b"BZh" {
        Ok(Box::new(bzip2::read::MultiBzDecoder::new(
            io::BufReader::new(file),
        )))
    } else {
        Ok(Box::new(io::BufReader::new(file)))
    }
}

/// Stage `image` into `dir`.
///
/// * `check` sees the image's partition table before anything is written,
///   and refuses an image that is not for this disk.
/// * `kernel` copies the kernel out of the staged boot partition (a vfat
///   image) into the path it is given, and returns its name on the ESP.
pub fn prepare(
    image: impl Read,
    bmap: &Bmap,
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
    let limit = boot.end().max(root.end());
    if limit > bmap.image_size {
        return Err("the image's partitions extend past its end".to_string());
    }

    let root_path = dir.join(ROOT_IMAGE);
    let boot_path = dir.join(BOOT_IMAGE);
    let root_file = sparse(&root_path, root.size)?;
    let boot_file = sparse(&boot_path, boot.size)?;

    let total = bmap.mapped_within(0, limit);
    let mut done = 0u64;
    let mut chunks = Chunker::default();
    let mut image = Cursor::new(head).chain(image);
    let mut position = 0u64;
    let mut buffer = vec![0u8; BUFFER];

    for range in &bmap.ranges {
        let start = bmap.start(range);
        let end = bmap.end(range);
        if start >= limit {
            break;
        }
        skip(&mut image, start - position)?;
        position = start;

        let mut hasher = Sha256::new();
        while position < end {
            if observer.cancelled() {
                return Err("cancelled".to_string());
            }
            let len = (end - position).min(BUFFER as u64) as usize;
            let data = &mut buffer[..len];
            image.read_exact(data).map_err(short)?;
            hasher.update(&*data);

            for (partition, file, is_root) in
                [(&root, &root_file, true), (&boot, &boot_file, false)]
            {
                let from = position.max(partition.start);
                let to = (position + len as u64).min(partition.end());
                if from >= to {
                    continue;
                }
                let piece = &data[(from - position) as usize..(to - position) as usize];
                let offset = from - partition.start;
                file.write_at(piece, offset)
                    .map_err(|err| format!("writing the staging on /data: {err}"))?;
                if is_root {
                    chunks.feed(offset, piece);
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

    let chunks = chunks.finish();
    for file in [&root_file, &boot_file] {
        file.sync_all()
            .map_err(|err| format!("syncing the staging on /data: {err}"))?;
    }
    drop((root_file, boot_file));

    let kernel_path = dir.join(KERNEL);
    let name = kernel(&boot_path, &kernel_path)?;
    let (size, sha256) = crate::sha256_file(&kernel_path)
        .map_err(|err| format!("reading the staged kernel: {err}"))?;
    fs::remove_file(&boot_path).map_err(|err| format!("{}: {err}", boot_path.display()))?;

    let manifest = Manifest {
        format: manifest::FORMAT,
        source,
        root: Root {
            partuuid: root.partuuid,
            start: root.start,
            size: root.size,
            chunks,
        },
        boot: Boot {
            partuuid: boot.partuuid,
            kernel: manifest::File { name, size, sha256 },
        },
    };
    fsutil::write_json(&dir.join(MANIFEST), &manifest)
        .map_err(|err| format!("writing the manifest: {err}"))?;
    Ok(manifest)
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

fn skip(image: &mut impl Read, bytes: u64) -> Result<(), String> {
    let copied = io::copy(&mut image.take(bytes), &mut io::sink()).map_err(short)?;
    if copied < bytes {
        return Err("the image ends before its bmap does".to_string());
    }
    Ok(())
}

fn short(err: io::Error) -> String {
    if err.kind() == io::ErrorKind::UnexpectedEof {
        "the image ends before its bmap does".to_string()
    } else {
        format!("reading the image: {err}")
    }
}

/// Cuts what is written to the root partition into chunks: contiguous, at
/// most `CHUNK` long, each with its SHA-256.
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
    use crate::testing::{disk, fake_kernel, source, staged, Quiet, BLOCK, MIB};

    #[test]
    fn only_the_mapped_root_and_boot_bytes_are_staged() {
        let dir = tempfile::tempdir().unwrap();
        let (image, manifest) = staged(dir.path());

        let root_start = (2 * MIB) as usize;
        let root = fs::read(dir.path().join(ROOT_IMAGE)).unwrap();
        assert_eq!(root.len() as u64, 6 * MIB);
        // Mapped: the first 1101 blocks (more than one chunk), and the last
        // MiB (the range that runs on into data is cut at the partition's end).
        let first = (1101 * BLOCK) as usize;
        assert_eq!(&root[..first], &image[root_start..root_start + first]);
        assert!(root[first..(5 * MIB) as usize].iter().all(|b| *b == 0));
        assert_eq!(
            &root[(5 * MIB) as usize..],
            &image[(7 * MIB) as usize..(8 * MIB) as usize]
        );

        assert_eq!(manifest.root.start, 2 * MIB);
        assert_eq!(manifest.root.size, 6 * MIB);
        assert_eq!(
            manifest.root.partuuid,
            "02020202-0202-0202-0202-020202020202"
        );
        assert_eq!(
            manifest.boot.partuuid,
            "01010101-0101-0101-0101-010101010101"
        );
        assert_eq!(manifest.root.mapped(), 1101 * BLOCK + MIB);
        assert_eq!(manifest.boot.kernel.name, "bzImage");
        assert_eq!(manifest.boot.kernel.size, 8);
        assert!(!dir.path().join(BOOT_IMAGE).exists());

        // Chunks are contiguous, capped, and hash what is in root.img.
        let chunks = &manifest.root.chunks;
        assert!(chunks.iter().all(|chunk| chunk.len <= CHUNK));
        assert_eq!(chunks[0].offset, 0);
        assert_eq!(chunks[1].offset, CHUNK);
        assert_eq!(chunks[2].offset, 5 * MIB);
        for chunk in chunks {
            let bytes = &root[chunk.offset as usize..(chunk.offset + chunk.len) as usize];
            assert_eq!(crate::sha256(bytes), chunk.sha256);
        }
        let on_disk: Manifest = fsutil::read_json(&dir.path().join(MANIFEST)).unwrap();
        assert_eq!(on_disk, manifest);
    }

    #[test]
    fn a_bz2_image_stages_the_same() {
        use bzip2::write::BzEncoder;
        use std::io::Write;

        let (image, text) = disk();
        // Two concatenated streams, the way pbzip2 writes them.
        let mut compressed = Vec::new();
        for half in image.chunks(image.len() / 2) {
            let mut encoder = BzEncoder::new(Vec::new(), bzip2::Compression::fast());
            encoder.write_all(half).unwrap();
            compressed.extend(encoder.finish().unwrap());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.wic.bz2");
        fs::write(&path, &compressed).unwrap();

        let staging = dir.path().join("staging");
        fs::create_dir(&staging).unwrap();
        let bmap = bmap::parse(&text).unwrap();
        let manifest = prepare(
            open_image(&path).unwrap(),
            &bmap,
            source(),
            &staging,
            |_| Ok(()),
            fake_kernel,
            &mut Quiet,
        )
        .unwrap();

        let plain = tempfile::tempdir().unwrap();
        let (_, expected) = staged(plain.path());
        assert_eq!(manifest.root.chunks, expected.root.chunks);
    }

    #[test]
    fn a_bmap_from_another_build_is_refused() {
        let (mut image, text) = disk();
        image[(3 * MIB) as usize] ^= 0xff;
        let bmap = bmap::parse(&text).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = prepare(
            &image[..],
            &bmap,
            source(),
            dir.path(),
            |_| Ok(()),
            fake_kernel,
            &mut Quiet,
        )
        .unwrap_err();
        assert!(err.contains("different builds"), "{err}");
        assert!(!dir.path().join(MANIFEST).exists());
    }

    #[test]
    fn the_layout_check_runs_before_anything_is_written() {
        let (image, text) = disk();
        let bmap = bmap::parse(&text).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = prepare(
            &image[..],
            &bmap,
            source(),
            dir.path(),
            |_| Err("not this disk".to_string()),
            fake_kernel,
            &mut Quiet,
        )
        .unwrap_err();
        assert_eq!(err, "not this disk");
        assert!(!dir.path().join(ROOT_IMAGE).exists());
    }

    #[test]
    fn a_truncated_image_is_refused() {
        let (image, text) = disk();
        let bmap = bmap::parse(&text).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = prepare(
            &image[..(5 * MIB) as usize],
            &bmap,
            source(),
            dir.path(),
            |_| Ok(()),
            fake_kernel,
            &mut Quiet,
        )
        .unwrap_err();
        assert!(err.contains("ends before"), "{err}");
    }

    /// Against a real build: `TESSARO_TEST_WIC=path/to/x.rootfs.wic.bz2
    /// cargo test -p update -- --ignored real_image`. The bmap is found
    /// next to it; the kernel is not extracted (that needs a loop mount).
    #[test]
    #[ignore]
    fn a_real_image_stages() {
        let Ok(path) = std::env::var("TESSARO_TEST_WIC") else {
            return;
        };
        let path = Path::new(&path);
        let bmap_path = path.with_extension("bmap");
        let bmap = bmap::parse(&fs::read_to_string(bmap_path).unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let manifest = prepare(
            open_image(path).unwrap(),
            &bmap,
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
            "root {} mapped of {}, {} chunks, in {:?}",
            manifest.root.mapped(),
            manifest.root.size,
            manifest.root.chunks.len(),
            started.elapsed()
        );
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
        let (image, text) = disk();
        let bmap = bmap::parse(&text).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut last = Last(0, 0);
        prepare(
            &image[..],
            &bmap,
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
