//! The setup portal's API: HTTP on a unix socket, which nginx proxies
//! `/api/` to for the phones on the hotspot (docs/setup-portal.md).
//!
//! Every route is a control-plane command, run through `Control::handle` as
//! `Caller::Portal`, so the portal can do nothing `tessaro-ctl` could not and
//! the journal names it. What it may do is narrow on purpose: read the
//! device's state, list WiFi networks, join one, and set the keys in `KEYS`.
//! Nothing under `access` - the portal never claims a device - and no other
//! setting.
//!
//! It answers only while the device is unclaimed. A claim closes it: from
//! then on the device is managed by whoever claimed it, with tessaro-ctl or
//! tessaro-gui, and a phone that knows the hotspot password gets a 403.
//!
//! Who reaches the socket is settled outside this file. Its directory is
//! root:www 0750 (tmpfiles), so only nginx's workers and root can connect,
//! and nginx answers the portal only on the hotspot's subnet.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use protocol::{keys, Command, Secret, Verify, WifiSecurity};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::UnixListener;
use tokio::sync::watch;

use crate::control::{Caller, Control, Reply};
use crate::deadline::within;
use crate::log::Log;

/// The settings the portal may change: what setting a device up takes, the
/// screens a technician flips on site, and the portal's own sign-in sheet.
pub const KEYS: &[&str] = &[
    keys::WIFI_CAPTIVE,
    keys::URL,
    keys::NAME,
    keys::TIMEZONE,
    "network.ethernet.mode",
    "network.ethernet.address",
    "network.ethernet.gateway",
    "network.ethernet.dns",
    keys::MAINTENANCE_ENABLE,
    keys::DEBUG_ENABLE,
];

/// The largest request body: a handful of settings, or a WiFi password.
const BODY_MAX: usize = 64 * 1024;
/// Reading a request body. A phone on the hotspot is one hop away.
const BODY_LIMIT: Duration = Duration::from_secs(10);
/// One connection, all its requests included. nginx opens one per request.
const CONNECTION_LIMIT: Duration = Duration::from_secs(90);
/// How long a `set` is waited for before the answer is "still applying". A
/// network change that renames the hotspot takes it down under the phone,
/// and the page must not hang on an answer that cannot arrive.
const SET_WAIT: Duration = Duration::from_secs(20);
/// Between the answer and a restart the change asked for: the agent restarts
/// itself for most of `KEYS`, and nginx must have the reply first.
const AFTER_GAP: Duration = Duration::from_secs(1);

pub fn spawn(
    control: Arc<Control>,
    path: PathBuf,
    log: Arc<Log>,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    // The directory is tmpfiles' and gives the socket its only protection;
    // without it there is no portal rather than a socket anyone can reach.
    let Some(dir) = path.parent().filter(|dir| dir.is_dir()) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is missing", path.parent().unwrap_or(&path).display()),
        ));
    };
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if metadata.file_type().is_socket() {
            std::fs::remove_file(&path)?;
        }
    }
    let listener = UnixListener::bind(&path)?;
    // Open to all: the root:www 0750 directory is what keeps others out.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
    log.info(format!(
        "setup portal: listening on {} (in {})",
        path.display(),
        dir.display()
    ));

    tokio::spawn(async move {
        let mut stop = shutdown.clone();
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted, // naked: an accept loop waits for as long as the agent runs
                _ = stop.changed() => break,
            };
            let stream = match accepted {
                Ok((stream, _)) => stream,
                Err(err) => {
                    log.info(format!("setup portal: accept failed: {err}"));
                    continue;
                }
            };
            let control = Arc::clone(&control);
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |request| {
                    let control = Arc::clone(&control);
                    async move {
                        // naked: every route waits only through Control, which bounds its calls, or within()
                        Ok::<_, Infallible>(respond(&control, request).await)
                    }
                });
                let connection = hyper::server::conn::http1::Builder::new()
                    .keep_alive(false)
                    .serve_connection(TokioIo::new(stream), service);
                if let Err(err) =
                    within("a setup portal connection", CONNECTION_LIMIT, connection).await
                {
                    log.debug(format!("setup portal: {err}"));
                }
            });
        }
        let _ = std::fs::remove_file(&path);
    });
    Ok(())
}

/// One request, answered with JSON.
async fn respond(control: &Arc<Control>, request: Request<Incoming>) -> Response<Full<Bytes>> {
    // nginx sets it; the address only goes into the journal.
    let peer = request
        .headers()
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let caller = Caller::Portal { peer };
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    let (status, body) = match (method, path.as_str()) {
        _ if control.claimed() => (
            StatusCode::FORBIDDEN,
            json!({
                "error": "this device is claimed, so the setup portal is closed; \
                          manage it with tessaro-gui or tessaro-ctl",
                "claimed": true,
            }),
        ),
        (Method::GET, "/api/state") => {
            // naked: welcome() and handle() wait only through blocking(), Network and Control
            state(control, &caller).await
        }
        (Method::GET, "/api/zones") => {
            // naked: Control bounds every call it makes
            answer(control.handle(&caller, Command::TimeZones).await)
        }
        (Method::GET, "/api/wifi") => {
            let scan = Command::WifiScan {
                interface: None,
                rescan: true,
            };
            // naked: Control bounds every call it makes
            answer(control.handle(&caller, scan).await)
        }
        (Method::POST, "/api/set") => {
            // naked: read_json is under within()
            match read_json::<SetBody>(request).await {
                Ok(body) => {
                    // naked: set waits for its answer under within()
                    set(control, caller, body.values).await
                }
                Err(err) => (StatusCode::BAD_REQUEST, json!({ "error": err })),
            }
        }
        (Method::POST, "/api/wifi/join") => {
            // naked: read_json is under within()
            match read_json::<JoinBody>(request).await {
                Ok(body) => join(control, caller, body),
                Err(err) => (StatusCode::BAD_REQUEST, json!({ "error": err })),
            }
        }
        _ => (StatusCode::NOT_FOUND, json!({ "error": "no such route" })),
    };
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    response
}

/// What the page shows: the welcome page's facts, what the screen shows, and
/// the current value of every key in `KEYS`, with the revision they were read
/// at. Reading it keeps the online check running.
async fn state(control: &Arc<Control>, caller: &Caller) -> (StatusCode, Value) {
    control.portal_seen();
    // naked: blocking() and Network, each under within()
    let welcome = match control.welcome().await {
        Ok(welcome) => welcome,
        Err(err) => return failed(err),
    };
    // naked: Control bounds every call it makes
    let status = match control.handle(caller, Command::Status).await.result {
        Ok(status) => status,
        Err(err) => return failed(err),
    };
    // naked: Control bounds every call it makes
    let settings = match control
        .handle(caller, Command::Get { key: None })
        .await
        .result
    {
        Ok(settings) => settings,
        Err(err) => return failed(err),
    };
    (
        StatusCode::OK,
        json!({
            "welcome": welcome,
            "screen": {
                "url": status["kiosk_url"],
                "showing": status["current_url"],
                "answering": status["browser_answering"],
                "maintenance": status["maintenance"],
                "debug": status["debug_screen"],
            },
            "revision": settings["revision"],
            "settings": portal_settings(&settings),
        }),
    )
}

/// `KEYS` out of a `config get`, as key to value.
fn portal_settings(settings: &Value) -> BTreeMap<String, Value> {
    settings["settings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|setting| {
            let key = setting["key"].as_str()?;
            KEYS.contains(&key)
                .then(|| (key.to_string(), setting["value"].clone()))
        })
        .collect()
}

#[derive(Deserialize)]
struct SetBody {
    values: BTreeMap<String, String>,
}

/// The keys of a `set` the portal refuses, if any.
fn refused(values: &BTreeMap<String, String>) -> Vec<&str> {
    values
        .keys()
        .map(String::as_str)
        .filter(|key| !KEYS.contains(key))
        .collect()
}

/// The first change saved through the portal ends the sign-in sheet: whoever
/// made it found the portal, and a phone that joins the hotspot later should
/// not be sent there uninvited. It rides in the same `set`, so it is saved
/// only if the change is. A `set` of the flag itself is left as asked.
fn with_captive_off(mut values: BTreeMap<String, String>) -> BTreeMap<String, String> {
    values
        .entry(keys::WIFI_CAPTIVE.to_string())
        .or_insert_with(|| "0".to_string());
    values
}

/// `config set`, waited for up to `SET_WAIT`. A change that takes longer -
/// a network transaction, or one that renamed the hotspot under the phone -
/// goes on, and the page is told it is still applying.
async fn set(
    control: &Arc<Control>,
    caller: Caller,
    values: BTreeMap<String, String>,
) -> (StatusCode, Value) {
    let refused = refused(&values);
    if !refused.is_empty() {
        return (
            StatusCode::FORBIDDEN,
            json!({ "error": format!("the setup portal cannot set {}", refused.join(", ")) }),
        );
    }
    if values.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            json!({ "error": "nothing to set" }),
        );
    }
    let command = Command::Set {
        values: with_captive_off(values),
        if_revision: None,
        apply: true,
        verify: Verify::default(),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let task = Arc::clone(control);
    tokio::spawn(async move {
        // naked: Control bounds every call it makes
        let reply = task.handle(&caller, command).await;
        // The captive flag follows network.wifi.captive at once.
        task.nudge_welcome();
        let after = reply.after.clone();
        let _ = sender.send(reply);
        if let Some(after) = after {
            // naked: a fixed pause, so nginx has the answer before a restart
            tokio::time::sleep(AFTER_GAP).await;
            // naked: run_after waits only through Bus and Network, whose calls are within()
            task.run_after(after).await;
        }
    });
    match within("a setup portal change", SET_WAIT, receiver).await {
        Ok(Ok(reply)) => answer(reply),
        Ok(Err(_)) => failed("the change stopped without an answer".to_string()),
        Err(_) => (StatusCode::ACCEPTED, json!({ "pending": true })),
    }
}

#[derive(Deserialize)]
struct JoinBody {
    ssid: String,
    #[serde(default)]
    psk: Option<String>,
    #[serde(default)]
    security: Option<WifiSecurity>,
    #[serde(default)]
    hidden: bool,
}

/// `network wifi join`, started and not waited for: it takes the hotspot
/// down, and with it the phone that asked. The device rolls back to the
/// hotspot if the join fails; the journal and the screen tell which.
fn join(control: &Arc<Control>, caller: Caller, body: JoinBody) -> (StatusCode, Value) {
    if body.ssid.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            json!({ "error": "no network named" }),
        );
    }
    let command = Command::WifiJoin {
        ssid: body.ssid,
        psk: body.psk.filter(|psk| !psk.is_empty()).map(Secret),
        security: body.security,
        hidden: body.hidden,
        verify: Verify::default(),
    };
    let task = Arc::clone(control);
    tokio::spawn(async move {
        // naked: Control bounds every call it makes
        let reply = task.handle(&caller, command).await;
        if let Some(after) = reply.after {
            // naked: run_after waits only through Bus and Network, whose calls are within()
            task.run_after(after).await;
        }
    });
    (StatusCode::ACCEPTED, json!({ "pending": true }))
}

async fn read_json<T: for<'de> Deserialize<'de>>(request: Request<Incoming>) -> Result<T, String> {
    let body = Limited::new(request.into_body(), BODY_MAX);
    let bytes = match within("reading a setup portal request", BODY_LIMIT, body.collect()).await {
        Ok(Ok(collected)) => collected.to_bytes(),
        Ok(Err(err)) => return Err(format!("reading the request: {err}")),
        Err(expired) => return Err(expired.to_string()),
    };
    serde_json::from_slice(&bytes).map_err(|err| format!("not a valid request: {err}"))
}

fn answer(reply: Reply) -> (StatusCode, Value) {
    match reply.result {
        Ok(value) => (StatusCode::OK, value),
        Err(err) => (StatusCode::UNPROCESSABLE_ENTITY, json!({ "error": err })),
    }
}

fn failed(err: String) -> (StatusCode, Value) {
    (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": err }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_portal_keys_may_be_set() {
        let values: BTreeMap<_, _> = [
            (keys::URL.to_string(), "https://example.com/".to_string()),
            ("access.listen".to_string(), "0.0.0.0:1".to_string()),
            ("data.x".to_string(), "1".to_string()),
        ]
        .into();
        assert_eq!(refused(&values), vec!["access.listen", "data.x"]);

        let fine: BTreeMap<_, _> = KEYS
            .iter()
            .map(|key| (key.to_string(), String::new()))
            .collect();
        assert!(refused(&fine).is_empty());
    }

    #[test]
    fn every_portal_key_is_in_the_registry_and_none_is_access() {
        for key in KEYS {
            assert!(keys::find(key).is_some(), "{key} is not a registry key");
            assert!(!key.starts_with("access."), "{key}");
        }
    }

    #[test]
    fn the_portal_settings_are_picked_out_of_a_config_get() {
        let settings = json!({
            "revision": 7,
            "settings": [
                { "key": "browser.url", "env": "KIOSK_URL", "value": "https://a/", "source": "set" },
                { "key": "access.listen", "env": "KIOSK_API_LISTEN", "value": "0.0.0.0:7400", "source": "default" },
                { "key": "network.ethernet.mode", "env": "KIOSK_ETHERNET_MODE", "value": null, "source": "default" },
            ],
        });
        let picked = portal_settings(&settings);
        assert_eq!(picked.get("browser.url"), Some(&json!("https://a/")));
        assert_eq!(picked.get("network.ethernet.mode"), Some(&Value::Null));
        assert!(!picked.contains_key("access.listen"));
    }

    #[test]
    fn the_first_saved_change_turns_the_sign_in_sheet_off_unless_it_sets_it() {
        let url: BTreeMap<_, _> = [(keys::URL.to_string(), "https://a/".to_string())].into();
        let sent = with_captive_off(url);
        assert_eq!(sent.get(keys::WIFI_CAPTIVE).map(String::as_str), Some("0"));
        assert_eq!(sent.get(keys::URL).map(String::as_str), Some("https://a/"));

        let back_on: BTreeMap<_, _> = [(keys::WIFI_CAPTIVE.to_string(), "1".to_string())].into();
        assert_eq!(
            with_captive_off(back_on)
                .get(keys::WIFI_CAPTIVE)
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn a_join_body_takes_the_security_by_its_wire_name() {
        let body: JoinBody =
            serde_json::from_str(r#"{"ssid":"office","psk":"secret123","security":"psk"}"#)
                .unwrap();
        assert_eq!(body.security, Some(WifiSecurity::Psk));
        assert!(!body.hidden);
    }
}
