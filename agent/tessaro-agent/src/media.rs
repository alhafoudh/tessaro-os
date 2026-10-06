//! The media cache: copies of the playlists' images and videos that do not
//! come from the device itself, so the player plays them with the network
//! down and never waits on a slow server between two items.
//!
//! Every image or video item of a playlist the player can play - the
//! default, and every one in the timetable - whose source is not
//! `http://127.0.0.1/...` (`PlaylistItem::cached`) is fetched into
//! `/data/tessaro/media-cache/<sha256 of the URL>.<ext>` and served by nginx
//! at `http://127.0.0.1/media-cache/`. A fetch goes to a `.part` file and is
//! renamed in, so nginx never serves half a file. A copy is asked again
//! with `If-None-Match`/`If-Modified-Since` every `REVALIDATE`; a failure
//! keeps the copy it has. Copies nothing wants any more are removed, and
//! `/data` keeps `files::RESERVE` free. Until a source has a copy the
//! player plays it from where it is (`playlists::player_doc`).
//!
//! The `media_cache` table of `tessaro.db` says what each copy is; the
//! fetching is driven by `control/playlists.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use protocol::playlist::{cache_name, CacheStatus};
use tessaro_db::rusqlite::{self, params, Connection};
use update::fsutil;

use crate::db::{Db, Stored};
use crate::deadline::blocking;
use crate::http::{HttpError, HyperHttp};
use crate::paths::Paths;

/// How often a copy is asked again whether its source changed.
pub const REVALIDATE: i64 = 15 * 60;
/// How soon a source whose fetch failed is tried again.
pub const RETRY: i64 = 2 * 60;
/// The largest copy: a long video at a high bitrate, not a disk.
pub const MAX_FILE: u64 = 4 << 30;
/// Redirects followed before a source counts as broken.
const REDIRECTS: usize = 5;
/// A stalled connection, per phase: connect and each frame of the body.
pub const TIMEOUT_S: i64 = 30;
/// Where nginx serves the copies.
pub const SERVED_AT: &str = "/media-cache/";

#[derive(Debug, Default)]
pub struct MediaCache {
    pub entries: Vec<Cached>,
}

/// What the cache knows of one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    pub url: String,
    /// The copy's name in the cache directory.
    pub file: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub size: Option<u64>,
    /// When the copy was last fetched or confirmed, seconds since the epoch.
    pub fetched_at: Option<i64>,
    /// Why the last try failed; cleared by the next that works.
    pub error: Option<String>,
}

impl Stored for MediaCache {
    const WHAT: &'static str = "the media cache";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows = db.prepare(
            "SELECT url, file, etag, last_modified, size, fetched_at, error \
             FROM media_cache ORDER BY url",
        )?;
        let entries = rows
            .query_map([], |row| {
                Ok(Cached {
                    url: row.get(0)?,
                    file: row.get(1)?,
                    etag: row.get(2)?,
                    last_modified: row.get(3)?,
                    size: row.get::<_, Option<i64>>(4)?.map(|size| size.max(0) as u64),
                    fetched_at: row.get(5)?,
                    error: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { entries })
    }

    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM media_cache", [])?;
        let mut insert = db.prepare(
            "INSERT INTO media_cache (url, file, etag, last_modified, size, fetched_at, error) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for entry in &self.entries {
            insert.execute(params![
                entry.url,
                entry.file,
                entry.etag,
                entry.last_modified,
                entry.size.map(|size| size as i64),
                entry.fetched_at,
                entry.error,
            ])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM media_cache", []).map(drop)
    }
}

impl MediaCache {
    pub fn get(&self, url: &str) -> Option<&Cached> {
        self.entries.iter().find(|entry| entry.url == url)
    }

    /// Record what a try at `url` came to.
    pub fn record(&mut self, url: &str, now: i64, outcome: &Result<Fetched, String>) {
        let at = match self.entries.iter().position(|entry| entry.url == url) {
            Some(at) => at,
            None => {
                self.entries.push(Cached {
                    url: url.to_string(),
                    file: cache_name(url),
                    etag: None,
                    last_modified: None,
                    size: None,
                    fetched_at: None,
                    error: None,
                });
                self.entries.len() - 1
            }
        };
        let entry = &mut self.entries[at];
        match outcome {
            Ok(Fetched::Fresh {
                etag,
                last_modified,
                size,
            }) => {
                entry.etag = etag.clone();
                entry.last_modified = last_modified.clone();
                entry.size = Some(*size);
                entry.fetched_at = Some(now);
                entry.error = None;
            }
            Ok(Fetched::NotModified) => {
                entry.fetched_at = Some(now);
                entry.error = None;
            }
            Err(err) => {
                entry.fetched_at = Some(now);
                entry.error = Some(err.clone());
            }
        }
    }
}

/// What a fetch came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// A new copy is in place.
    Fresh {
        etag: Option<String>,
        last_modified: Option<String>,
        size: u64,
    },
    /// The copy is still what the source has.
    NotModified,
}

/// The sources of `wanted` to fetch now: no copy yet, a copy older than
/// `REVALIDATE`, a failure older than `RETRY`. `present` says whether a
/// copy's file is there.
pub fn due(
    cache: &MediaCache,
    wanted: &BTreeSet<String>,
    now: i64,
    present: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    wanted
        .iter()
        .filter(|url| match cache.get(url) {
            None => true,
            Some(entry) => {
                let age = now - entry.fetched_at.unwrap_or(0);
                if !present(&entry.file) {
                    entry.error.is_none() || age >= RETRY
                } else if entry.error.is_some() {
                    age >= RETRY
                } else {
                    age >= REVALIDATE
                }
            }
        })
        .cloned()
        .collect()
}

/// Where the player loads each source of `cache` that has a copy from.
pub fn local(cache: &MediaCache, present: &dyn Fn(&str) -> bool) -> BTreeMap<String, String> {
    cache
        .entries
        .iter()
        .filter(|entry| present(&entry.file))
        .map(|entry| (entry.url.clone(), format!("{SERVED_AT}{}", entry.file)))
        .collect()
}

/// How far the copies of `wanted` are.
pub fn status(
    cache: &MediaCache,
    wanted: &BTreeSet<String>,
    present: &dyn Fn(&str) -> bool,
) -> CacheStatus {
    let mut status = CacheStatus::default();
    for url in wanted {
        match cache.get(url) {
            Some(entry) if present(&entry.file) => status.ready += 1,
            Some(entry) if entry.error.is_some() => status.failed += 1,
            _ => status.pending += 1,
        }
    }
    status
}

/// Fetch `url` into `dir`, asking conditionally when `known` has a copy
/// there. Redirects are followed; anything but a 200 or a 304 at the end is
/// an error.
pub async fn fetch(
    http: &HyperHttp,
    dir: &Path,
    url: &str,
    known: Option<&Cached>,
    copy_present: bool,
) -> Result<Fetched, String> {
    let file = cache_name(url);
    let part = dir.join(format!(".{file}.part"));
    let mut headers: Vec<(&'static str, String)> = Vec::new();
    if let Some(known) = known.filter(|_| copy_present) {
        if let Some(etag) = &known.etag {
            headers.push(("if-none-match", etag.clone()));
        }
        if let Some(modified) = &known.last_modified {
            headers.push(("if-modified-since", modified.clone()));
        }
    }

    let mut current = url.to_string();
    for _ in 0..=REDIRECTS {
        // naked: every phase of download() is under its own within()
        let answer = http.download(&current, &headers, &part, MAX_FILE).await;
        let answer = match answer {
            Ok(answer) => answer,
            Err(err) => {
                let leftover = part.clone();
                // A .part left by a failure is evicted on the next tick anyway.
                let _ = blocking("removing a media download", move || {
                    fsutil::remove_if_exists(&leftover).map_err(|err| err.to_string())
                })
                .await;
                return Err(describe(&err));
            }
        };
        match answer.status {
            200 => {
                let dir = dir.to_path_buf();
                let file = file.clone();
                let part = part.clone();
                blocking("putting a media copy in place", move || {
                    place(&dir, &part, &file)
                })
                .await?;
                return Ok(Fetched::Fresh {
                    etag: answer.etag,
                    last_modified: answer.last_modified,
                    size: answer.size,
                });
            }
            304 if copy_present => return Ok(Fetched::NotModified),
            301 | 302 | 303 | 307 | 308 => {
                let location = answer
                    .location
                    .ok_or_else(|| format!("HTTP {} without a Location", answer.status))?;
                current = follow(&current, &location)
                    .ok_or_else(|| format!("a redirect to {location} cannot be followed"))?;
                // A conditional request only means something for the URL
                // that answered it before.
                headers.clear();
            }
            status => return Err(format!("the server answered HTTP {status}")),
        }
    }
    Err(format!("more than {REDIRECTS} redirects"))
}

/// The finished `part` renamed to `file`, unless that leaves `/data` with
/// less than the reserve, in which case the copy goes again.
fn place(dir: &Path, part: &Path, file: &str) -> Result<(), String> {
    let target = dir.join(file);
    std::fs::rename(part, &target).map_err(|err| format!("{}: {err}", target.display()))?;
    let free = fsutil::available(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    if free < crate::files::RESERVE {
        let _ = fsutil::remove_if_exists(&target);
        return Err(format!(
            "/data has {} free, less than the {} the kiosk keeps",
            update::megabytes(free),
            update::megabytes(crate::files::RESERVE)
        ));
    }
    Ok(())
}

fn describe(err: &HttpError) -> String {
    err.to_string()
}

/// Where a redirect from `from` to `location` goes: an absolute URL, or a
/// path on the same server.
pub fn follow(from: &str, location: &str) -> Option<String> {
    if location.starts_with("http://") || location.starts_with("https://") {
        return Some(location.to_string());
    }
    let (scheme, rest) = from.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if let Some(path) = location.strip_prefix("//") {
        return Some(format!("{scheme}://{path}"));
    }
    if location.starts_with('/') {
        return Some(format!("{scheme}://{authority}{location}"));
    }
    let path = &rest[authority.len()..];
    let path = path.split(['?', '#']).next().unwrap_or("");
    let base = path.rsplit_once('/').map_or("", |(base, _)| base);
    Some(format!("{scheme}://{authority}{base}/{location}"))
}

/// Remove the copies, the rows and the leftover files nothing in `wanted`
/// needs: a source no playlist has any more, a `.part` an interrupted fetch
/// left. Returns how many copies went.
pub fn evict(dir: &Path, cache: &mut MediaCache, wanted: &BTreeSet<String>) -> usize {
    let before = cache.entries.len();
    cache.entries.retain(|entry| wanted.contains(&entry.url));
    let kept: BTreeSet<&str> = cache
        .entries
        .iter()
        .map(|entry| entry.file.as_str())
        .collect();
    let mut removed = before - cache.entries.len();
    if let Ok(listing) = std::fs::read_dir(dir) {
        for entry in listing.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let leftover = name.starts_with('.') || !kept.contains(name.as_str());
            if leftover && fsutil::remove_if_exists(&entry.path()).is_ok() && !name.starts_with('.')
            {
                removed += 1;
            }
        }
    }
    removed.min(before)
}

/// Whether the copy `file` is in `dir`.
pub fn present(dir: &Path, file: &str) -> bool {
    std::fs::symlink_metadata(dir.join(file)).is_ok_and(|meta| meta.is_file())
}

/// The cache directory, made if it is not there.
pub fn make_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))
}

/// Every copy and the table emptied: a factory reset.
pub fn wipe(paths: &Paths, db: &Db) -> Result<(), String> {
    let dir = paths.media_cache_dir();
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("{}: {err}", dir.display())),
    }
    db.transaction(|tx| MediaCache::clear(tx).map_err(|err| err.to_string()))
}

/// The client the cache fetches with: through the local proxy when there
/// is one, the device's certificate authorities and the extra ones.
pub fn client(proxy: Option<std::net::SocketAddr>) -> HyperHttp {
    HyperHttp::new(
        TIMEOUT_S,
        TIMEOUT_S,
        0,
        crate::watchdog::Heartbeat::detached(),
    )
    .with_proxy(proxy)
}

/// One minute, the cache's tick.
pub const TICK: Duration = Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(url: &str, fetched_at: i64, error: Option<&str>) -> Cached {
        Cached {
            url: url.to_string(),
            file: cache_name(url),
            etag: Some("\"1\"".to_string()),
            last_modified: None,
            size: Some(10),
            fetched_at: Some(fetched_at),
            error: error.map(str::to_string),
        }
    }

    fn wanted(urls: &[&str]) -> BTreeSet<String> {
        urls.iter().map(|url| url.to_string()).collect()
    }

    #[test]
    fn a_source_is_due_when_new_stale_or_failed_long_enough() {
        let cache = MediaCache {
            entries: vec![
                cached("https://a.test/fresh.png", 1000, None),
                cached("https://a.test/stale.png", 1000 - REVALIDATE, None),
                cached("https://a.test/failed.png", 1000 - RETRY, Some("HTTP 500")),
                cached("https://a.test/just-failed.png", 990, Some("HTTP 500")),
            ],
        };
        let all = wanted(&[
            "https://a.test/fresh.png",
            "https://a.test/stale.png",
            "https://a.test/failed.png",
            "https://a.test/just-failed.png",
            "https://a.test/new.png",
        ]);
        let due = due(&cache, &all, 1000, &|_| true);
        assert_eq!(
            due,
            vec![
                "https://a.test/failed.png".to_string(),
                "https://a.test/new.png".to_string(),
                "https://a.test/stale.png".to_string(),
            ]
        );
    }

    #[test]
    fn a_missing_copy_is_fetched_again_at_once() {
        let cache = MediaCache {
            entries: vec![cached("https://a.test/gone.png", 1000, None)],
        };
        let due = due(&cache, &wanted(&["https://a.test/gone.png"]), 1000, &|_| {
            false
        });
        assert_eq!(due.len(), 1);
    }

    #[test]
    fn status_counts_copies_failures_and_the_rest() {
        let cache = MediaCache {
            entries: vec![
                cached("https://a.test/ok.png", 1, None),
                cached("https://a.test/bad.png", 1, Some("refused")),
            ],
        };
        let all = wanted(&[
            "https://a.test/ok.png",
            "https://a.test/bad.png",
            "https://a.test/new.png",
        ]);
        let ok = cache_name("https://a.test/ok.png");
        let status = status(&cache, &all, &|file| file == ok);
        assert_eq!(
            status,
            CacheStatus {
                ready: 1,
                pending: 1,
                failed: 1
            }
        );
    }

    #[test]
    fn a_failure_keeps_the_copy_and_the_etag() {
        let mut cache = MediaCache {
            entries: vec![cached("https://a.test/x.png", 1, None)],
        };
        cache.record("https://a.test/x.png", 50, &Err("timed out".to_string()));
        let entry = cache.get("https://a.test/x.png").unwrap();
        assert_eq!(entry.etag.as_deref(), Some("\"1\""));
        assert_eq!(entry.error.as_deref(), Some("timed out"));
        let local = local(&cache, &|_| true);
        assert!(local["https://a.test/x.png"].starts_with(SERVED_AT));
    }

    #[test]
    fn redirects_resolve_absolute_rooted_and_relative() {
        assert_eq!(
            follow("https://a.test/x/y.mp4", "https://b.test/z.mp4").as_deref(),
            Some("https://b.test/z.mp4")
        );
        assert_eq!(
            follow("https://a.test/x/y.mp4?v=1", "/z.mp4").as_deref(),
            Some("https://a.test/z.mp4")
        );
        assert_eq!(
            follow("https://a.test/x/y.mp4", "z.mp4").as_deref(),
            Some("https://a.test/x/z.mp4")
        );
        assert_eq!(
            follow("https://a.test/x", "//cdn.test/z").as_deref(),
            Some("https://cdn.test/z")
        );
    }

    #[test]
    fn eviction_removes_what_nothing_wants_and_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let keep = "https://a.test/keep.png";
        let drop = "https://a.test/drop.png";
        let mut cache = MediaCache {
            entries: vec![cached(keep, 1, None), cached(drop, 1, None)],
        };
        for name in [
            cache_name(keep),
            cache_name(drop),
            ".x.part".to_string(),
            "stray".to_string(),
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        evict(dir.path(), &mut cache, &wanted(&[keep]));
        assert_eq!(cache.entries.len(), 1);
        let left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec![cache_name(keep)]);
    }
}
