//! Minimal client for Chromium's DevTools protocol.
//!
//! One WebSocket per command, request and response matched by id, no
//! persistent listener. A crash or a navigation between cycles is noticed by
//! the next command failing, which the caller counts as a ping failure - the
//! same cadence-based model the shell agent this descends from used, and the
//! reason there is no event loop here.
//!
//! This replaced a D-Bus `Ping` against cog. It is a strictly better health
//! signal: `Runtime.evaluate` proves the *renderer* is turning, not just that
//! the UI process is alive.

use std::cell::Cell;
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::Message;

use crate::error::{Error, Result};
use crate::http::HttpGet;
use crate::log::Log;
use crate::ports::Cdp;

pub struct CdpClient<'a, H: HttpGet> {
    log: &'a Log,
    base_url: String,
    timeout: Duration,
    http: H,
    next_id: Cell<u64>,
}

impl<'a, H: HttpGet> CdpClient<'a, H> {
    /// `timeout` is seconds, and covers the HTTP round trip, the WebSocket
    /// handshake and the wait for a reply alike.
    pub fn new(log: &'a Log, base_url: &str, http: H, timeout: i64) -> Self {
        Self {
            log,
            base_url: base_url.trim_end_matches('/').to_string(),
            timeout: Duration::from_secs(timeout.max(1) as u64),
            http,
            next_id: Cell::new(0),
        }
    }

    /// Evaluate an expression in the page target and return its value.
    pub fn evaluate(&self, expression: &str) -> Result<Value> {
        let response = self.command(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
        )?;

        Ok(response["result"]["result"]["value"].clone())
    }

    /// The WebSocket URL of the first page target.
    fn page_ws_url(&self) -> Result<String> {
        let page = self.page_target()?;

        match page["webSocketDebuggerUrl"].as_str() {
            Some(url) if !url.is_empty() => Ok(url.to_string()),
            _ => Err(Error::Cdp(
                "no webSocketDebuggerUrl for the page target".to_string(),
            )),
        }
    }

    /// The first page target, as `/json/list` describes it.
    fn page_target(&self) -> Result<Value> {
        let response = self
            .http
            .get(&format!("{}/json/list", self.base_url))
            .map_err(|err| Error::Cdp(format!("/json/list failed: {err}")))?;

        if !(200..=299).contains(&response.status) {
            return Err(Error::Cdp(format!(
                "/json/list answered HTTP {}",
                response.status
            )));
        }

        let targets: Vec<Value> = serde_json::from_str(&response.body)
            .map_err(|err| Error::Cdp(format!("unparseable /json/list: {err}")))?;

        targets
            .into_iter()
            .find(|target| target["type"] == "page")
            .ok_or_else(|| Error::Cdp("no page target".to_string()))
    }

    fn command(&self, method: &str, params: Value) -> Result<Value> {
        let ws_url = self.page_ws_url()?;

        let id = self.next_id.get() + 1;
        self.next_id.set(id);

        let mut socket = self.connect(&ws_url)?;
        let payload = json!({ "id": id, "method": method, "params": params }).to_string();

        let outcome = self.exchange(&mut socket, payload, id, method);

        // Nothing useful to do about a failed close - the far side is
        // Chromium and the socket is about to be dropped either way.
        let _ = socket.close(None);

        outcome
    }

    fn connect(&self, ws_url: &str) -> Result<tungstenite::WebSocket<TcpStream>> {
        // Connect by hand rather than through tungstenite::connect, so the
        // connect and read timeouts are ours. A wedged browser that accepts
        // the TCP connection and then says nothing is exactly the failure
        // this program exists to notice.
        let address = authority(ws_url)?
            .to_socket_addrs()
            .map_err(|err| Error::Cdp(format!("{ws_url} does not resolve: {err}")))?
            .next()
            .ok_or_else(|| Error::Cdp(format!("{ws_url} does not resolve")))?;

        let stream = TcpStream::connect_timeout(&address, self.timeout)
            .map_err(|err| Error::Cdp(format!("connect to {address} failed: {err}")))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|err| Error::Cdp(format!("could not set socket timeouts: {err}")))?;

        let (socket, _response) = tungstenite::client(ws_url, stream)
            .map_err(|err| Error::Cdp(format!("websocket handshake failed: {err}")))?;

        Ok(socket)
    }

    /// Send the command and read frames until the matching reply arrives.
    /// Anything with another id is a CDP event for a listener we do not have.
    fn exchange(
        &self,
        socket: &mut tungstenite::WebSocket<TcpStream>,
        payload: String,
        id: u64,
        method: &str,
    ) -> Result<Value> {
        socket
            .send(Message::Text(payload.into()))
            .map_err(|err| Error::Cdp(format!("send failed: {err}")))?;

        let deadline = Instant::now() + self.timeout;

        loop {
            if Instant::now() >= deadline {
                return Err(Error::Cdp(format!("no reply within {:?}", self.timeout)));
            }

            let message = match socket.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(err))
                    if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    return Err(Error::Cdp(format!("no reply within {:?}", self.timeout)))
                }
                Err(err) => return Err(Error::Cdp(format!("read failed: {err}"))),
            };

            let text = match message {
                Message::Text(text) => text,
                Message::Close(frame) => {
                    return Err(Error::Cdp(format!("websocket closed: {frame:?}")))
                }
                _ => continue,
            };

            let reply: Value = serde_json::from_str(&text)
                .map_err(|err| Error::Cdp(format!("unparseable message: {err}")))?;

            if reply["id"].as_u64() != Some(id) {
                continue;
            }

            if !reply["error"].is_null() {
                return Err(Error::Cdp(format!("{method}: {}", reply["error"])));
            }

            return Ok(reply);
        }
    }
}

impl<H: HttpGet> Cdp for CdpClient<'_, H> {
    fn alive(&self) -> bool {
        // One round trip, not two: `evaluate` resolves the page target
        // itself, so a browser with no page target fails here just as it
        // would have on a separate lookup.
        match self.evaluate("1 + 1") {
            Ok(_) => true,
            Err(err) => {
                self.log.debug(format!("cdp alive? failed: {err}"));
                false
            }
        }
    }

    fn current_url(&self) -> Option<String> {
        // Straight off /json/list - one cheap HTTP round trip, no websocket.
        match self.page_target() {
            Ok(page) => page["url"].as_str().map(str::to_string),
            Err(err) => {
                self.log.debug(format!("cdp current_url failed: {err}"));
                None
            }
        }
    }

    fn navigate(&self, url: &str) -> Result<()> {
        let response = self.command("Page.navigate", json!({ "url": url }))?;

        match response["result"]["errorText"].as_str() {
            Some(text) if !text.is_empty() => Err(Error::Cdp(format!("Page.navigate: {text}"))),
            _ => Ok(()),
        }
    }
}

/// `host:port` out of a `ws://host:port/path` URL.
fn authority(ws_url: &str) -> Result<String> {
    let rest = ws_url
        .strip_prefix("ws://")
        .or_else(|| ws_url.strip_prefix("wss://"))
        .ok_or_else(|| Error::Cdp(format!("not a websocket URL: {ws_url}")))?;

    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host_port.is_empty() {
        return Err(Error::Cdp(format!("no host in {ws_url}")));
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
}
