//! The device's disk: the bundled image unpacked once into a base that is
//! never written, and a qcow2 overlay on it that takes every write. Reset to
//! factory is deleting the overlay, instant and with nothing to unpack again
//! (docs/try-tessaro.md, "The disk").

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ruzstd::decoding::StreamingDecoder;
use serde::{Deserialize, Serialize};

const OVERLAY: &str = "disk.qcow2";
const STATE: &str = "state.json";
/// Runs of zeros this long are seeked over, not written, so the base stays
/// sparse: most of the image's partitions are empty.
const BLOCK: usize = 64 * 1024;

/// What the data directory knows about the device across launches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The base file the overlay was made on.
    pub base: Option<String>,
    /// The device's node id once it answered, so Reset can forget it in the
    /// client store.
    pub node: Option<String>,
}

impl State {
    pub fn load(dir: &Path) -> Self {
        std::fs::read(dir.join(STATE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join(STATE);
        let body = serde_json::to_vec_pretty(self).map_err(|err| err.to_string())?;
        std::fs::write(&path, body).map_err(|err| format!("{}: {err}", path.display()))
    }
}

pub fn overlay(dir: &Path) -> PathBuf {
    dir.join(OVERLAY)
}

/// The base's file name for a bundled image: its name without `.zst`.
pub fn base_name(image: &Path) -> String {
    let name = image
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.strip_suffix(".zst").unwrap_or(&name).to_string()
}

/// The disk ready to boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub overlay: PathBuf,
    /// The device runs an older image than the bundle carries; Reset moves
    /// it to the bundled one.
    pub newer_bundled: bool,
}

/// Unpack the base if there is none and make the overlay if there is none.
/// `read` counts the compressed bytes read, for a progress bar.
pub fn prepare(
    image: &Path,
    qemu_img: &Path,
    dir: &Path,
    read: Arc<AtomicU64>,
) -> Result<Prepared, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let bundled = base_name(image);
    let mut state = State::load(dir);
    let overlay = overlay(dir);
    if overlay.exists() {
        let base = state.base.clone().unwrap_or_default();
        if base.is_empty() || !dir.join(&base).exists() {
            return Err(
                "the device's disk lost the image it was made on; Reset to factory starts over"
                    .to_string(),
            );
        }
        return Ok(Prepared {
            overlay,
            newer_bundled: base != bundled,
        });
    }
    let base = dir.join(&bundled);
    if !base.exists() {
        unpack(image, &base, read)?;
    }
    create_overlay(qemu_img, &base, &overlay)?;
    state.base = Some(bundled);
    state.save(dir)?;
    Ok(Prepared {
        overlay,
        newer_bundled: false,
    })
}

/// The image's disk bytes into `to`, sparse. Written next to it first, so
/// a cut-off unpack is never taken for a whole one. Every zstd frame in the
/// file is read, one after another: `zstd -T` writes one, pzstd one per
/// chunk.
pub fn unpack(image: &Path, to: &Path, read: Arc<AtomicU64>) -> Result<(), String> {
    let fail = |err: io::Error| format!("unpacking {}: {err}", image.display());
    let file = File::open(image).map_err(fail)?;
    let mut source = BufReader::with_capacity(1 << 20, Counted { inner: file, read });
    let part = to.with_extension("part");
    let mut out = File::create(&part).map_err(fail)?;
    let mut buffer = vec![0u8; 16 * BLOCK];
    let mut length = 0u64;
    while !source.fill_buf().map_err(fail)?.is_empty() {
        let mut frame = StreamingDecoder::new(&mut source)
            .map_err(|err| format!("unpacking {}: {err}", image.display()))?;
        loop {
            let filled = fill(&mut frame, &mut buffer).map_err(fail)?;
            if filled == 0 {
                break;
            }
            write_sparse(&mut out, &buffer[..filled]).map_err(fail)?;
            length += filled as u64;
        }
    }
    // A trailing run of zeros was seeked over, not written.
    out.set_len(length).map_err(fail)?;
    out.sync_all().map_err(fail)?;
    std::fs::rename(&part, to).map_err(fail)
}

/// `bytes` at the file's position, all-zero blocks seeked over.
fn write_sparse(out: &mut File, bytes: &[u8]) -> io::Result<()> {
    for block in bytes.chunks(BLOCK) {
        if block.iter().all(|byte| *byte == 0) {
            out.seek(SeekFrom::Current(block.len() as i64))?;
        } else {
            out.write_all(block)?;
        }
    }
    Ok(())
}

/// Read until `buffer` is full or the image ends.
fn fill(source: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match source.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

/// The compressed file, its bytes counted as they are read.
struct Counted {
    inner: File,
    read: Arc<AtomicU64>,
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.read.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

fn create_overlay(qemu_img: &Path, base: &Path, overlay: &Path) -> Result<(), String> {
    let output = Command::new(qemu_img)
        .args(["create", "-q", "-f", "qcow2", "-F", "raw", "-b"])
        .arg(base)
        .arg(overlay)
        .output()
        .map_err(|err| format!("{}: {err}", qemu_img.display()))?;
    if !output.status.success() {
        return Err(format!(
            "making the device's disk: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Back to factory: the overlay gone, and every base but the bundled one,
/// so the next start makes a fresh device from the image the app carries.
/// The node id the device had, for the caller to forget.
pub fn reset(dir: &Path, bundled: &str) -> Result<Option<String>, String> {
    let state = State::load(dir);
    let remove = |path: PathBuf| match std::fs::remove_file(&path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            Err(format!("{}: {err}", path.display()))
        }
        _ => Ok(()),
    };
    remove(overlay(dir))?;
    remove(dir.join(STATE))?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stale = (name.ends_with(".wic") || name.ends_with(".part")) && name != bundled;
            if stale {
                remove(entry.path())?;
            }
        }
    }
    Ok(state.node)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bytes` as zstd in two frames, as pzstd writes a large file.
    fn compress(bytes: &[u8]) -> Vec<u8> {
        use ruzstd::encoding::{compress_to_vec, CompressionLevel};
        bytes
            .chunks(bytes.len().div_ceil(2))
            .flat_map(|half| compress_to_vec(half, CompressionLevel::Fastest))
            .collect()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("try-tessaro-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_image_unpacks_to_the_same_bytes_with_its_zeros_left_sparse() {
        let dir = temp("unpack");
        let mut data = vec![0u8; 3 * BLOCK + 100];
        data[10] = 1;
        data[2 * BLOCK + 5] = 7;
        let image = dir.join("tessaro.wic.zst");
        std::fs::write(&image, compress(&data)).unwrap();
        let read = Arc::new(AtomicU64::new(0));
        let out = dir.join("tessaro.wic");
        unpack(&image, &out, read.clone()).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), data);
        assert_eq!(
            read.load(Ordering::Relaxed),
            std::fs::metadata(&image).unwrap().len()
        );
        assert!(!dir.join("tessaro.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_image_ending_in_zeros_keeps_its_length() {
        let dir = temp("tail");
        let mut data = vec![0u8; 4 * BLOCK];
        data[0] = 9;
        let image = dir.join("t.wic.zst");
        std::fs::write(&image, compress(&data)).unwrap();
        let out = dir.join("t.wic");
        unpack(&image, &out, Arc::default()).unwrap();
        assert_eq!(std::fs::metadata(&out).unwrap().len(), data.len() as u64);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_base_is_named_after_the_image() {
        assert_eq!(
            base_name(Path::new("/x/tessaro-os-genericarm64-1.2.wic.zst")),
            "tessaro-os-genericarm64-1.2.wic"
        );
    }

    #[test]
    fn reset_keeps_only_the_bundled_base_and_hands_back_the_node() {
        let dir = temp("reset");
        for name in ["old.wic", "new.wic", "disk.qcow2", "settings.json"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        State {
            base: Some("old.wic".to_string()),
            node: Some("n-1".to_string()),
        }
        .save(&dir)
        .unwrap();
        assert_eq!(reset(&dir, "new.wic").unwrap(), Some("n-1".to_string()));
        assert!(!dir.join("old.wic").exists());
        assert!(!dir.join("disk.qcow2").exists());
        assert!(!dir.join(STATE).exists());
        assert!(dir.join("new.wic").exists());
        assert!(dir.join("settings.json").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_overlay_without_its_base_asks_for_a_reset() {
        let dir = temp("orphan");
        std::fs::write(dir.join(OVERLAY), b"x").unwrap();
        let err = prepare(
            Path::new("/nonexistent/t.wic.zst"),
            Path::new("/nonexistent/qemu-img"),
            &dir,
            Arc::default(),
        )
        .unwrap_err();
        assert!(err.contains("Reset to factory"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
