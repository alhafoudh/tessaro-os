//! The local page Chromium shows while the site is unreachable.
//!
//! Precedence, first match wins:
//!   1. `KIOSK_OFFLINE_URL` - navigated to verbatim (handled by the caller)
//!   2. `KIOSK_OFFLINE_PAGE` (default `/data/kiosk/offline.html`)
//!   3. `KIOSK_OFFLINE_PAGE_DEFAULT` (the one shipped in the image)
//!
//! The chosen file is staged into `KIOSK_OFFLINE_DIR` through a temporary
//! file and an atomic rename, so the browser can never be served a
//! half-written page. Staging happens on every use, so dropping a file onto
//! /data takes effect without restarting anything.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::config::Config;
use crate::log::Log;
use crate::ports::OfflinePage;

pub struct Offline<'a> {
    log: &'a Log,
    config: &'a Config,
}

impl<'a> Offline<'a> {
    pub fn new(log: &'a Log, config: &'a Config) -> Self {
        Self { log, config }
    }

    fn source(&self) -> &str {
        let page = Path::new(&self.config.offline_page);

        match fs::metadata(page) {
            // Missing or unreadable: fall through to the shipped default
            // silently. An operator page is optional by design.
            Err(_) => &self.config.offline_page_default,
            Ok(metadata) => {
                let size = metadata.len() as i64;
                if size > 0 && size <= self.config.offline_max_bytes {
                    &self.config.offline_page
                } else {
                    self.log.info(format!(
                        "ignoring {} (size {size}, limit {})",
                        self.config.offline_page, self.config.offline_max_bytes
                    ));
                    &self.config.offline_page_default
                }
            }
        }
    }
}

/// Staging is a few hundred bytes of local file I/O, done synchronously on
/// purpose: `tokio::fs` would only wrap the same calls in `spawn_blocking`.
/// If the disk ever wedges here the runtime stalls with it, the watchdog
/// keepalive goes quiet, and systemd restarts us - which is the right outcome
/// for a dying `/data`.
#[async_trait(?Send)]
impl OfflinePage for Offline<'_> {
    async fn stage(&self) -> Option<String> {
        Offline::stage(self)
    }
}

impl Offline<'_> {
    fn stage(&self) -> Option<String> {
        let source = self.source();
        let contents = match fs::read(source) {
            Ok(contents) => contents,
            Err(_) => {
                self.log
                    .info(format!("no readable offline page ({source})"));
                return None;
            }
        };

        let dir = Path::new(&self.config.offline_dir);
        // Single level, not the whole chain: the directory is a tmpfiles entry
        // owned by the image, and quietly creating a parent here would paper
        // over a broken /run or a mistyped KIOSK_OFFLINE_DIR.
        if !dir.is_dir() {
            if let Err(err) = fs::create_dir(dir) {
                self.log.info(format!(
                    "staging {source} failed: {err}; keeping the previous page"
                ));
                return None;
            }
        }

        let temp = dir.join(temp_name());
        if let Err(err) = write_then_rename(&temp, &dir.join("index.html"), &contents) {
            // Best-effort: if the rename is what failed, the copy is still
            // sitting there, and the directory is small and root-owned.
            let _ = fs::remove_file(&temp);
            self.log.info(format!(
                "staging {source} failed: {err}; keeping the previous page"
            ));
            return None;
        }

        Some(format!("file://{}/index.html", self.config.offline_dir))
    }
}

fn write_then_rename(temp: &Path, target: &Path, contents: &[u8]) -> std::io::Result<()> {
    fs::write(temp, contents)?;
    set_mode(temp, 0o644)?;
    // Same directory, so same filesystem, so the rename is atomic and the
    // browser never sees a partial page.
    fs::rename(temp, target)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Unique within the directory without pulling in an RNG: this process is the
/// only writer, and the clock moves between staging attempts.
fn temp_name() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);

    PathBuf::from(format!(".index.html.{}.{nanos}", process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_support::config_with;

    struct Fixture {
        _root: tempfile::TempDir,
        page: PathBuf,
        shipped: PathBuf,
        dir: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("tempdir");
            let page = root.path().join("operator.html");
            let shipped = root.path().join("shipped.html");
            let dir = root.path().join("stage");

            fs::write(&shipped, b"<h1>shipped</h1>").expect("write shipped");

            Self {
                _root: root,
                page,
                shipped,
                dir,
            }
        }

        fn config(&self, overrides: &[(&str, &str)]) -> Config {
            let mut settings = vec![
                ("KIOSK_OFFLINE_PAGE", self.page.to_str().unwrap()),
                ("KIOSK_OFFLINE_PAGE_DEFAULT", self.shipped.to_str().unwrap()),
                ("KIOSK_OFFLINE_DIR", self.dir.to_str().unwrap()),
            ];
            settings.extend_from_slice(overrides);
            config_with(&settings)
        }

        fn staged(&self) -> Option<String> {
            fs::read_to_string(self.dir.join("index.html")).ok()
        }

        fn leftovers(&self) -> Vec<String> {
            fs::read_dir(&self.dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .filter(|name| name.starts_with(".index.html."))
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    #[test]
    fn stages_the_operator_page_and_leaves_no_temp_file() {
        let fixture = Fixture::new();
        fs::write(&fixture.page, b"<h1>operator</h1>").unwrap();
        let config = fixture.config(&[]);
        let log = Log::buffered(true);

        let uri = Offline::new(&log, &config).stage();

        assert_eq!(
            uri,
            Some(format!("file://{}/index.html", fixture.dir.display()))
        );
        assert_eq!(fixture.staged().as_deref(), Some("<h1>operator</h1>"));
        assert!(fixture.leftovers().is_empty());
    }

    #[test]
    fn an_oversize_page_falls_back_to_the_shipped_one() {
        let fixture = Fixture::new();
        fs::write(&fixture.page, vec![b'x'; 1000]).unwrap();
        let config = fixture.config(&[("KIOSK_OFFLINE_MAX_BYTES", "100")]);
        let log = Log::buffered(true);

        assert!(Offline::new(&log, &config).stage().is_some());

        assert_eq!(fixture.staged().as_deref(), Some("<h1>shipped</h1>"));
        assert!(log.lines().iter().any(|line| line.contains("ignoring")));
    }

    #[test]
    fn an_empty_page_falls_back_to_the_shipped_one() {
        let fixture = Fixture::new();
        fs::write(&fixture.page, b"").unwrap();
        let config = fixture.config(&[]);
        let log = Log::buffered(true);

        assert!(Offline::new(&log, &config).stage().is_some());

        assert_eq!(fixture.staged().as_deref(), Some("<h1>shipped</h1>"));
    }

    #[test]
    fn a_missing_page_falls_back_silently() {
        let fixture = Fixture::new();
        let config = fixture.config(&[]);
        let log = Log::buffered(true);

        assert!(Offline::new(&log, &config).stage().is_some());

        assert_eq!(fixture.staged().as_deref(), Some("<h1>shipped</h1>"));
        assert!(!log.lines().iter().any(|line| line.contains("ignoring")));
    }

    #[test]
    fn nothing_readable_stages_nothing() {
        let fixture = Fixture::new();
        fs::remove_file(&fixture.shipped).unwrap();
        let config = fixture.config(&[]);
        let log = Log::buffered(true);

        assert_eq!(Offline::new(&log, &config).stage(), None);
        assert_eq!(fixture.staged(), None);
    }

    #[test]
    fn an_uncreatable_stage_directory_gives_up_cleanly() {
        // Regression: the cleanup path used to run before the temp path was
        // known and blew up inside the handler that existed to swallow this.
        let fixture = Fixture::new();
        fs::write(&fixture.page, b"<h1>operator</h1>").unwrap();
        // The parent does not exist either, so a single-level create_dir
        // cannot work - which is the point: a mistyped KIOSK_OFFLINE_DIR must
        // surface as a log line, not as a directory tree conjured under /run.
        let blocker = fixture.dir.join("missing-parent").join("stage");
        let config = fixture.config(&[("KIOSK_OFFLINE_DIR", blocker.to_str().unwrap())]);
        let log = Log::buffered(true);

        assert_eq!(Offline::new(&log, &config).stage(), None);
        assert!(log.lines().iter().any(|line| line.contains("staging")));
    }
}
