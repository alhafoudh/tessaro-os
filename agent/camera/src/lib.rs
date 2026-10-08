//! The camera mirror's pieces, as a library: `tessaro-camera` (main.rs) is
//! built from them, and `tessaro-vision` reads its hidden mirror with the
//! same V4L2 capture and turns its MJPEG frames into pictures with the same
//! JPEG fix-up.

pub mod choice;
pub mod loopback;
pub mod mirrors;
pub mod settings;
pub mod snapshot;
pub mod v4l2;

use std::fs;
use std::path::Path;

/// Whole or not at all: a temporary file in the same directory, then a
/// rename over the real one, so a reader never sees half of it. A failure is
/// logged, not fatal: the mirror is what matters, the files only report on it.
pub fn write_whole(path: &Path, body: &[u8]) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("camera");
    let temporary = path.with_file_name(format!(".{name}.tmp"));
    let result = fs::write(&temporary, body).and_then(|()| fs::rename(&temporary, path));
    if let Err(err) = result {
        eprintln!("{}: {err}", path.display());
        let _ = fs::remove_file(&temporary);
    }
}
