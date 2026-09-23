//! Finding the page to talk to: `/json/list` and the WebSocket URL in it.
//!
//! Once per connection attempt, not once per cycle - the session holds the
//! socket open, and asks again only when it has to reconnect.

use serde_json::Value;

use crate::http::HyperHttp;

/// The WebSocket URL of the first page target.
pub async fn page_ws_url(http: &HyperHttp, base_url: &str) -> Result<String, String> {
    let response = http
        .fetch(&format!("{base_url}/json/list"))
        .await // naked: every phase inside fetch() has its own within()
        .map_err(|err| format!("/json/list failed: {err}"))?;

    if !(200..=299).contains(&response.status) {
        return Err(format!("/json/list answered HTTP {}", response.status));
    }

    ws_url_from_list(&response.body)
}

/// The page target out of a `/json/list` body. Chromium drives the first
/// target of type "page"; anything else (service workers, extensions) is not
/// what is on screen.
pub fn ws_url_from_list(body: &str) -> Result<String, String> {
    let targets: Vec<Value> =
        serde_json::from_str(body).map_err(|err| format!("unparseable /json/list: {err}"))?;

    let page = targets
        .into_iter()
        .find(|target| target["type"] == "page")
        .ok_or_else(|| "no page target".to_string())?;

    match page["webSocketDebuggerUrl"].as_str() {
        Some(url) if !url.is_empty() => Ok(url.to_string()),
        _ => Err("no webSocketDebuggerUrl for the page target".to_string()),
    }
}

/// `host:port` out of a `ws://host:port/path` URL.
pub fn authority(ws_url: &str) -> Result<String, String> {
    let rest = ws_url
        .strip_prefix("ws://")
        .or_else(|| ws_url.strip_prefix("wss://"))
        .ok_or_else(|| format!("not a websocket URL: {ws_url}"))?;

    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host_port.is_empty() {
        return Err(format!("no host in {ws_url}"));
    }

    if host_port.contains(':') {
        Ok(host_port.to_string())
    } else {
        let port = if ws_url.starts_with("wss://") {
            443
        } else {
            80
        };
        Ok(format!("{host_port}:{port}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_takes_host_and_port_off_a_devtools_url() {
        assert_eq!(
            authority("ws://127.0.0.1:9222/devtools/page/ABC").unwrap(),
            "127.0.0.1:9222"
        );
    }

    #[test]
    fn authority_defaults_the_port() {
        assert_eq!(authority("ws://localhost/x").unwrap(), "localhost:80");
    }

    #[test]
    fn authority_rejects_a_non_websocket_url() {
        assert!(authority("http://127.0.0.1:9222/").is_err());
    }

    #[test]
    fn the_first_page_target_is_the_one() {
        let body = r#"[
            {"type": "service_worker", "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/SW"},
            {"type": "page", "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/P"}
        ]"#;

        assert_eq!(
            ws_url_from_list(body).unwrap(),
            "ws://127.0.0.1:9222/devtools/page/P"
        );
    }

    #[test]
    fn no_page_target_says_so() {
        assert_eq!(ws_url_from_list("[]"), Err("no page target".to_string()));
        assert!(ws_url_from_list("not json")
            .unwrap_err()
            .starts_with("unparseable"));
    }
}
