//! `update fetch`: the device downloads an image itself instead of being
//! sent one, and prepares it as it would an upload (`updates.rs`).
//!
//! The bmap first, from beside the image unless named, then the image,
//! straight into the update's `upload.part` through the same appends a
//! client's chunks go through, so a restart or a dropped link resumes it
//! with a `Range` request. Nothing vouches for the image but the URL: its
//! SHA-256 is worked out as it arrives and goes into the manifest before the
//! last bytes do, so the initramfs still checks the file it writes is the
//! one that was prepared. Through network.proxy.url, like everything else
//! the device fetches.

use std::sync::Arc;
use std::time::Duration;

use protocol::{FetchEvent, FetchSpec, ImageUpload, UpdatePhase};
use tokio::sync::mpsc;

use super::{After, Caller, Control, Stream};
use crate::deadline::blocking;
use crate::http::{HttpError, HyperHttp};

/// The bmap is text, a few hundred kilobytes for the largest image.
const BMAP_MAX: usize = 16 << 20;
/// What is written to the upload at once: synced each time, so a power cut
/// costs at most this much.
const PIECE: usize = 4 << 20;
/// Redirects followed, as for the media cache.
const REDIRECTS: usize = 5;
/// How often the checks after the download are looked in on.
const LOOK: Duration = Duration::from_secs(1);

impl Control {
    /// Tell the device's events where an update goes from here - received,
    /// verifying, preparing, ready, failed, cancelled - until it gets
    /// somewhere it stays. One follower at a time.
    pub(super) fn follow_update(self: &Arc<Self>) {
        if self
            .update_followed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut last: Option<UpdatePhase> = None;
            loop {
                // naked: Updates waits only through blocking(), under within()
                let Ok(status) = this.updates.status().await else {
                    break;
                };
                if last != Some(status.phase) {
                    last = Some(status.phase);
                    let phase = serde_json::to_value(status.phase).unwrap_or_default();
                    let name = phase.as_str().unwrap_or("unknown").to_string();
                    this.emit(
                        "update",
                        &name,
                        serde_json::json!({
                            "name": status.name,
                            "size": status.size,
                            "received": status.received,
                            "error": status.error,
                        }),
                    );
                }
                if matches!(
                    status.phase,
                    UpdatePhase::Ready
                        | UpdatePhase::Failed
                        | UpdatePhase::Idle
                        | UpdatePhase::Pending
                ) {
                    break;
                }
                // naked: a pause between looks, not a wait on anything
                tokio::time::sleep(LOOK * 2).await;
            }
            this.update_followed
                .store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// Start fetching `spec.url`; its steps are the job's.
    pub(super) fn update_fetch(
        self: &Arc<Self>,
        caller: &Caller,
        spec: FetchSpec,
    ) -> Result<Stream, String> {
        if !(spec.url.starts_with("http://") || spec.url.starts_with("https://")) {
            return Err("the image's URL is http:// or https://".to_string());
        }
        let name = image_name(&spec.url)?;
        let bmap = match &spec.bmap {
            Some(bmap) => bmap.clone(),
            None => bmap_url(&spec.url)?,
        };
        if (spec.wipe_data || spec.reboot) && !spec.commit {
            return Err("--wipe-data and --reboot go with --commit".to_string());
        }
        self.log.info(format!(
            "update: fetching {} for {}",
            spec.url,
            caller.describe()
        ));
        let (send, steps) = mpsc::channel(16);
        let this = Arc::clone(self);
        let who = caller.describe();
        self.follow_update();
        tokio::spawn(async move {
            // naked: every wait in fetch is under within() - HTTP's, blocking()'s - or a step to the job
            let outcome = this.fetch(&who, &spec, &name, &bmap, &send).await;
            if let Err(err) = &outcome {
                this.log
                    .info(format!("update: fetching {}: {err}", spec.url));
            }
            // naked: the job's channel, which drain() empties under its own within()
            let _ = send
                .send(outcome.map(|_| step("done", 0, None, None)))
                .await;
        });
        Ok(Stream::Fetch(steps))
    }

    async fn fetch(
        self: &Arc<Self>,
        who: &str,
        spec: &FetchSpec,
        name: &str,
        bmap_url: &str,
        send: &mpsc::Sender<Result<FetchEvent, String>>,
    ) -> Result<(), String> {
        // Every `tell` is a step into the job's channel, which drain()
        // empties under its own within(); every HTTP wait is HyperHttp's,
        // each under within().
        let tell = |event: FetchEvent| async move {
            // A cancelled job has dropped its end: stop.
            // naked: the job's channel
            send.send(Ok(event))
                .await
                .map_err(|_| "cancelled".to_string())
        };
        let client = crate::media::client(self.proxy);

        // naked: the job's channel
        tell(step("bmap", 0, None, Some(format!("fetching {bmap_url}")))).await?;
        // naked: HyperHttp bounds every step with within()
        let bmap = fetch_text(&client, bmap_url).await?;

        // naked: HyperHttp bounds every step with within()
        let (mut body, url) = open(&client, &spec.url, &[]).await?;
        let size = body
            .header(hyper::header::CONTENT_LENGTH)
            .and_then(|length| length.parse::<u64>().ok())
            .ok_or("the server does not say how large the image is (no Content-Length)")?;
        let etag = body.header(hyper::header::ETAG).unwrap_or_default();
        // The image is only known by where it came from until it is all
        // here: that is what a resume matches on.
        let identity = update::sha256(format!("{url}\n{size}\n{etag}").as_bytes());
        let begun = self
            .updates
            .begin(
                who,
                ImageUpload {
                    name: name.to_string(),
                    size,
                    sha256: identity,
                    bmap,
                    verify: false,
                    repartition: spec.repartition,
                },
            )
            .await?; // naked: Updates waits only through blocking(), under within()

        let mut offset = begun.offset;
        let mut hasher = openssl::sha::Sha256::new();
        if begun.phase == UpdatePhase::Receiving && offset > 0 {
            // naked: the job's channel
            tell(step(
                "download",
                offset,
                Some(size),
                Some("resuming".to_string()),
            ))
            .await?;
            let path = self.paths.update_dir().join(update::UPLOAD);
            let mut prefix = hasher;
            hasher = blocking("reading what was fetched", move || {
                hash_prefix(&path, offset, &mut prefix).map(|()| prefix)
            })
            .await?;
            drop(body);
            // naked: HyperHttp bounds every step with within()
            let (resumed, _) =
                open(&client, &url, &[("range", format!("bytes={offset}-"))]).await?;
            body = resumed;
            if body.status == 200 {
                // No ranges here: the whole file again, and what is already
                // in skipped.
                // naked: HyperHttp bounds every frame with within()
                skip(&client, &mut body, offset).await?;
            }
        }

        if begun.phase == UpdatePhase::Receiving {
            // naked: the job's channel
            tell(step("download", offset, Some(size), Some(name.to_string()))).await?;
            let mut piece: Vec<u8> = Vec::with_capacity(PIECE);
            let mut told = offset;
            loop {
                // naked: HyperHttp bounds every frame with within()
                let data = client
                    .frame(&mut body)
                    .await
                    .map_err(|err| err.to_string())?;
                let end = data.is_none();
                if let Some(data) = &data {
                    if offset + (piece.len() + data.len()) as u64 > size {
                        return Err("the server sent more than it said the image is".to_string());
                    }
                    hasher.update(data);
                    piece.extend_from_slice(data);
                }
                let last = offset + piece.len() as u64 == size;
                if end && !last {
                    return Err(format!(
                        "the download stopped at {} of {}; `tessaro-ctl update fetch` again resumes it",
                        update::megabytes(offset + piece.len() as u64),
                        update::megabytes(size)
                    ));
                }
                if piece.len() >= PIECE || last {
                    if last {
                        let sha256 = protocol::hex(
                            &std::mem::replace(&mut hasher, openssl::sha::Sha256::new()).finish(),
                        );
                        self.updates.set_sha256(sha256).await?;
                    }
                    let length = piece.len() as u64;
                    let bytes = std::mem::take(&mut piece);
                    self.updates.append(offset, bytes).await?;
                    offset += length;
                    if offset == size || offset - told >= size / 50 {
                        told = offset;
                        // naked: the job's channel
                        tell(step("download", offset, Some(size), None)).await?;
                    }
                }
                if last {
                    break;
                }
            }
        }

        // The checks the upload goes through after its last byte.
        let mut last_phase = None;
        let status = loop {
            let status = self.updates.status().await?;
            match status.phase {
                UpdatePhase::Ready | UpdatePhase::Pending => break status,
                UpdatePhase::Failed => {
                    return Err(status
                        .error
                        .unwrap_or_else(|| "the update failed".to_string()))
                }
                UpdatePhase::Idle => return Err("the update was cancelled".to_string()),
                phase => {
                    if last_phase != Some(phase) {
                        last_phase = Some(phase);
                        let what = match phase {
                            UpdatePhase::Verifying => "verifying",
                            _ => "preparing",
                        };
                        // naked: the job's channel
                        tell(step(what, status.received, Some(status.size), None)).await?;
                    }
                }
            }
            // naked: a pause between looks, not a wait on anything
            tokio::time::sleep(LOOK).await;
            if send.is_closed() {
                return Err("cancelled".to_string());
            }
        };
        // naked: the job's channel
        tell(step(
            "ready",
            status.received,
            Some(status.size),
            Some(format!("{name} is ready")),
        ))
        .await?;

        if spec.commit {
            if self.network.busy() {
                return Err(
                    "a network change is in progress; commit the update once it is done"
                        .to_string(),
                );
            }
            let done = self.updates.commit(who, spec.wipe_data).await?;
            // naked: the job's channel
            tell(step(
                "committed",
                status.received,
                Some(status.size),
                Some(done.message.clone()),
            ))
            .await?;
            if spec.reboot {
                // After the job's last step has gone out to whoever follows it.
                let this = Arc::clone(self);
                tokio::spawn(async move {
                    // naked: a pause, not a wait on anything
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    // naked: run_after is under within() inside Bus
                    this.run_after(After::Reboot).await;
                });
            }
        }
        Ok(())
    }
}

fn step(phase: &str, received: u64, size: Option<u64>, message: Option<String>) -> FetchEvent {
    FetchEvent {
        phase: phase.to_string(),
        received,
        size,
        message,
    }
}

/// A GET, following redirects: its answer once it is a 200 or 206, and the
/// URL that gave it.
async fn open(
    client: &HyperHttp,
    url: &str,
    headers: &[(&'static str, String)],
) -> Result<(crate::http::Streaming, String), String> {
    let mut current = url.to_string();
    for _ in 0..=REDIRECTS {
        // naked: HyperHttp bounds every step with within()
        let answer = client
            .get(&current, headers)
            .await
            .map_err(|err| fetching(&current, &err))?;
        match answer.status {
            200 | 206 => return Ok((answer, current)),
            301 | 302 | 303 | 307 | 308 => {
                let location = answer
                    .header(hyper::header::LOCATION)
                    .ok_or_else(|| format!("{current}: a redirect without a Location"))?;
                current = crate::media::follow(&current, &location)
                    .ok_or_else(|| format!("a redirect to {location} cannot be followed"))?;
            }
            status => return Err(format!("{current}: the server answered {status}")),
        }
    }
    Err(format!("more than {REDIRECTS} redirects"))
}

fn fetching(url: &str, err: &HttpError) -> String {
    format!("{url}: {err}")
}

/// A small text file, the bmap.
async fn fetch_text(client: &HyperHttp, url: &str) -> Result<String, String> {
    // naked: HyperHttp bounds every step with within()
    let (mut body, _) = open(client, url, &[]).await?;
    let mut text = Vec::new();
    // naked: HyperHttp bounds every frame with within()
    while let Some(data) = client
        .frame(&mut body)
        .await
        .map_err(|err| fetching(url, &err))?
    {
        text.extend_from_slice(&data);
        if text.len() > BMAP_MAX {
            return Err(format!("{url} is larger than a bmap is"));
        }
    }
    String::from_utf8(text).map_err(|_| format!("{url} is not text, so not a bmap"))
}

/// Read past what a whole-file answer repeats of what is already in.
async fn skip(
    client: &HyperHttp,
    body: &mut crate::http::Streaming,
    mut left: u64,
) -> Result<(), String> {
    while left > 0 {
        // naked: HyperHttp bounds every frame with within()
        let data = client
            .frame(body)
            .await
            .map_err(|err| err.to_string())?
            .ok_or("the download ended before what was already fetched")?;
        if (data.len() as u64) > left {
            return Err(
                "the server cannot resume a download; `tessaro-ctl update cancel` and fetch again"
                    .to_string(),
            );
        }
        left -= data.len() as u64;
    }
    Ok(())
}

/// The first `length` bytes of the upload into `hasher`.
fn hash_prefix(
    path: &std::path::Path,
    length: u64,
    hasher: &mut openssl::sha::Sha256,
) -> Result<(), String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut file = file.take(length);
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        if read == 0 {
            return Ok(());
        }
        hasher.update(&buffer[..read]);
    }
}

/// The file name an image is staged under: the URL's last segment.
fn image_name(url: &str) -> Result<String, String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or("");
    if name.is_empty() || !name.contains(".wic") {
        return Err(format!("{url} does not name a .wic image"));
    }
    Ok(name.to_string())
}

/// Where the bmap is when not named: beside the image, `.wic.bmap` for
/// `.wic.zst`, `.wic.bz2` or `.wic.gz`.
fn bmap_url(url: &str) -> Result<String, String> {
    let (path, rest) = match url.find(['?', '#']) {
        Some(at) => url.split_at(at),
        None => (url, ""),
    };
    let at = path
        .rfind(".wic")
        .ok_or_else(|| format!("{url} does not name a .wic image"))?;
    Ok(format!("{}.wic.bmap{rest}", &path[..at]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bmap_is_beside_the_image() {
        assert_eq!(
            bmap_url("https://ex.test/img/tessaro-os-raspberrypi5.rootfs.wic.zst").unwrap(),
            "https://ex.test/img/tessaro-os-raspberrypi5.rootfs.wic.bmap"
        );
        assert_eq!(
            bmap_url("https://ex.test/a.rootfs.wic.bz2?sig=1").unwrap(),
            "https://ex.test/a.rootfs.wic.bmap?sig=1"
        );
        assert!(bmap_url("https://ex.test/a.iso").is_err());
    }

    #[test]
    fn an_image_is_named_after_its_file() {
        assert_eq!(
            image_name("https://ex.test/x/a.rootfs.wic.zst?t=1").unwrap(),
            "a.rootfs.wic.zst"
        );
        assert!(image_name("https://ex.test/").is_err());
        assert!(image_name("https://ex.test/readme.txt").is_err());
    }

    #[test]
    fn a_prefix_hashes_like_the_whole_file_does() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("upload.part");
        std::fs::write(&path, b"hello world").unwrap();
        let mut hasher = openssl::sha::Sha256::new();
        hash_prefix(&path, 5, &mut hasher).unwrap();
        hasher.update(b" world");
        assert_eq!(
            protocol::hex(&hasher.finish()),
            update::sha256(b"hello world")
        );
    }
}
