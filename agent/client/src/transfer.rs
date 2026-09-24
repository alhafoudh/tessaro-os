//! Moving bytes to and from a device, in `UPDATE_CHUNK` pieces each
//! acknowledged before the next: one file into or out of the store, and an
//! image for an update. A dropped link resumes from what the device says it
//! has. Progress goes to a callback, so the terminal and the GUI each show
//! it their own way.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use protocol::files::{FileBegun, FileData, FileEntry};
use protocol::{Command, Received};
use sha2::{Digest, Sha256};

use crate::connect::Session;

/// `bytes` in MB, one decimal, for progress lines and errors.
pub fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// One local file to `path` in the store, from where the device says it
/// has got to: nowhere, part of it, or all of it already. The bytes sent,
/// or `None` when the device had it. `on` gets the offset after each chunk.
pub fn send_file(
    session: &mut Session,
    local: &Path,
    path: &str,
    size: u64,
    mtime: i64,
    mut on: impl FnMut(u64),
) -> Result<Option<u64>, String> {
    let begun: FileBegun = session.call(Command::FilesBegin {
        path: path.to_string(),
        size,
        mtime,
    })?;
    if begun.offset >= size && size > 0 {
        return Ok(None);
    }

    let mut file = File::open(local).map_err(|err| format!("{}: {err}", local.display()))?;
    file.seek(SeekFrom::Start(begun.offset))
        .map_err(|err| format!("{}: {err}", local.display()))?;
    let mut buffer = vec![0u8; protocol::UPDATE_CHUNK];
    let mut offset = begun.offset;
    while offset < size {
        let want = ((size - offset) as usize).min(buffer.len());
        file.read_exact(&mut buffer[..want]).map_err(|err| {
            format!(
                "{}: {err} (did it change while it was sent?)",
                local.display()
            )
        })?;
        let data = data_encoding::BASE64.encode(&buffer[..want]);
        let received: Received = session
            .call(Command::FilesChunk {
                path: path.to_string(),
                offset,
                data,
            })
            .map_err(|err| {
                format!(
                    "{path} stopped at {}: {err}; send it again to resume",
                    mb(offset)
                )
            })?;
        offset = received.received;
        on(offset);
    }
    Ok(Some(size - begun.offset))
}

/// One stored file to `target`, through a temporary file beside it, stamped
/// with the device's mtime. `false` when `target` already has its size and
/// mtime. `on` gets the bytes received and the size after each chunk.
pub fn fetch_file(
    session: &mut Session,
    entry: &FileEntry,
    target: &Path,
    mut on: impl FnMut(u64, u64),
) -> Result<bool, String> {
    if fs::metadata(target).is_ok_and(|meta| {
        meta.is_file() && meta.len() == entry.size && mtime_of(&meta) == entry.mtime
    }) {
        return Ok(false);
    }
    let dir = target
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary: PathBuf = dir.join(format!(
        ".{}.part",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download")
    ));
    let fail = |err: std::io::Error| format!("{}: {err}", temporary.display());
    let mut file = File::create(&temporary).map_err(fail)?;
    let mut offset = 0u64;
    let mut mtime = entry.mtime;
    let mut size = entry.size;
    while offset < size {
        let data: FileData = session.call(Command::FilesRead {
            path: entry.path.clone(),
            offset,
            len: protocol::UPDATE_CHUNK as u64,
        })?;
        (size, mtime) = (data.size, data.mtime);
        if data.data.is_empty() {
            break;
        }
        let bytes = data_encoding::BASE64
            .decode(data.data.as_bytes())
            .map_err(|_| "the device sent a chunk that is not base64".to_string())?;
        file.write_all(&bytes).map_err(fail)?;
        offset += bytes.len() as u64;
        on(offset, size);
    }
    if offset != size {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "{} changed on the device while it was read; run it again",
            entry.path
        ));
    }
    file.set_modified(time_of(mtime)).map_err(fail)?;
    file.sync_all().map_err(fail)?;
    drop(file);
    fs::rename(&temporary, target).map_err(|err| format!("{}: {err}", target.display()))?;
    Ok(true)
}

/// `x.rootfs.wic.bz2` -> `x.rootfs.wic.bmap`, the same rule as `image:flash`.
pub fn bmap_for(image: &Path) -> PathBuf {
    let text = image.to_string_lossy();
    let base = text.strip_suffix(".bz2").unwrap_or(&text);
    PathBuf::from(format!("{base}.bmap"))
}

/// The image's SHA-256, hex, for `update-begin`. `on` gets the bytes read.
pub fn hash(path: &Path, mut on: impl FnMut(u64)) -> Result<String, String> {
    let mut file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        done += read as u64;
        on(done);
    }
    Ok(protocol::hex(&hasher.finalize()))
}

/// The image from `from` on, after `update-begin`. `on` gets the offset the
/// device acknowledged after each chunk.
pub fn upload_image(
    session: &mut Session,
    path: &Path,
    size: u64,
    from: u64,
    mut on: impl FnMut(u64),
) -> Result<(), String> {
    let mut file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    file.seek(SeekFrom::Start(from))
        .map_err(|err| format!("{}: {err}", path.display()))?;
    let mut buffer = vec![0u8; protocol::UPDATE_CHUNK];
    let mut offset = from;
    while offset < size {
        let want = ((size - offset) as usize).min(buffer.len());
        file.read_exact(&mut buffer[..want])
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let data = data_encoding::BASE64.encode(&buffer[..want]);
        let received: Received = session
            .call(Command::UpdateChunk { offset, data })
            .map_err(|err| {
                format!(
                    "the upload stopped at {}: {err}; send it again to resume",
                    mb(offset)
                )
            })?;
        offset = received.received;
        on(offset);
    }
    Ok(())
}

/// Whole seconds since the epoch, rounded down, as the device keeps them.
pub fn mtime_of(meta: &fs::Metadata) -> i64 {
    match meta.modified() {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(after) => after.as_secs() as i64,
            Err(before) => -(before.duration().as_secs_f64().ceil() as i64),
        },
        Err(_) => 0,
    }
}

pub fn time_of(mtime: i64) -> SystemTime {
    if mtime >= 0 {
        UNIX_EPOCH + Duration::from_secs(mtime as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(mtime.unsigned_abs())
    }
}

/// `YYYY-MM-DD HH:MM`, UTC: the device's clock may not be the local one.
pub fn date(mtime: i64) -> String {
    let days = mtime.div_euclid(86_400);
    let seconds = mtime.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bmap_is_found_next_to_the_image() {
        assert_eq!(
            bmap_for(Path::new("out/tessaro-os-qemux86-64.rootfs.wic.bz2")),
            PathBuf::from("out/tessaro-os-qemux86-64.rootfs.wic.bmap")
        );
        assert_eq!(bmap_for(Path::new("x.wic")), PathBuf::from("x.wic.bmap"));
    }

    #[test]
    fn dates_are_utc() {
        assert_eq!(date(0), "1970-01-01 00:00");
        assert_eq!(date(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(date(951_782_400), "2000-02-29 00:00");
        assert_eq!(date(-60), "1969-12-31 23:59");
    }
}
