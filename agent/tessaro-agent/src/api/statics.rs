//! The files the API server serves besides the API: the setup page at `/`
//! and Swagger UI at `/api/docs/`. Both are read-only trees in the image.

use std::path::{Path, PathBuf};

use crate::deadline::blocking;

/// The largest file served; the page, its fonts and Swagger UI's bundle all
/// fit with room to spare.
const MAX: u64 = 8 * 1024 * 1024;

pub struct File {
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

/// `request` (the URL path below `root`'s mount) as a file under `root`: a
/// directory is its `index.html`. With `fallback`, a path that is no file
/// is `index.html` itself, the way the setup page wants every path it does
/// not know answered. `None` when there is nothing to serve.
pub async fn read(root: &Path, request: &str, fallback: bool) -> Option<File> {
    let path = resolve(root, request)?;
    let root = root.to_path_buf();
    // naked: blocking() is under within()
    blocking("reading a static file", move || {
        Ok(load(&path).or_else(|| fallback.then(|| load(&root.join("index.html"))).flatten()))
    })
    .await
    .ok()
    .flatten()
}

/// Below `root`, and nowhere else: no `..`, no hidden names.
fn resolve(root: &Path, request: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for name in request.split('/').filter(|name| !name.is_empty()) {
        if name.starts_with('.') || name.contains('\\') || name.contains('%') {
            return None;
        }
        path.push(name);
    }
    Some(path)
}

fn load(path: &Path) -> Option<File> {
    let path = if path.is_dir() {
        path.join("index.html")
    } else {
        path.to_path_buf()
    };
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX {
        return None;
    }
    Some(File {
        content_type: content_type(&path),
        body: std::fs::read(&path).ok()?,
    })
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn files_come_from_below_the_root_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<p>setup</p>").unwrap();
        std::fs::create_dir(dir.path().join("fonts")).unwrap();
        std::fs::write(dir.path().join("fonts/a.woff2"), "font").unwrap();
        std::fs::write(dir.path().join(".hidden"), "no").unwrap();

        let root = dir.path();
        let page = read(root, "/", false).await.unwrap();
        assert_eq!(page.body, b"<p>setup</p>");
        assert!(page.content_type.starts_with("text/html"));
        let font = read(root, "/fonts/a.woff2", false).await.unwrap();
        assert_eq!(font.content_type, "font/woff2");

        assert!(read(root, "/nothing", false).await.is_none());
        assert_eq!(
            read(root, "/nothing/here", true).await.unwrap().body,
            b"<p>setup</p>"
        );
        assert!(read(root, "/../etc/passwd", true).await.is_none());
        assert!(read(root, "/.hidden", false).await.is_none());
        assert!(read(&root.join("missing"), "/", true).await.is_none());
    }
}
