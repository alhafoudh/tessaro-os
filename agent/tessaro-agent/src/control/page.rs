//! What the control plane does to the page on screen, over the DevTools
//! session: reload it, empty the cache, run JavaScript in it, and show or
//! hide the on-screen keyboard for it. Each is a `tessaro-ctl` command and a
//! page bridge action alike (docs/bridge.md).

use std::time::Duration;

use protocol::{Done, EvalException, EvalResult};
use serde_json::{json, Value};

use super::{Caller, Control, CDP_LIMIT};
use crate::deadline::{blocking, within};
use crate::watchdog::Heartbeat;

/// The longest CSS selector `keyboard show` takes.
const SELECTOR_MAX: usize = 1024;

impl Control {
    /// A TV remote's key into the page as a key press (`screen.cec.keys`):
    /// down, or up when `down` is false. A key that types (digits, Enter)
    /// carries its text, so a focused field gets it.
    pub(super) async fn dispatch_key(
        &self,
        press: &protocol::cec::Press,
        down: bool,
        repeat: bool,
    ) -> Result<(), String> {
        let kind = match (down, press.text) {
            (false, _) => "keyUp",
            (true, Some(_)) => "keyDown",
            (true, None) => "rawKeyDown",
        };
        let mut params = json!({
            "type": kind,
            "key": press.key,
            "code": press.code,
            "windowsVirtualKeyCode": press.virtual_key,
            "nativeVirtualKeyCode": press.virtual_key,
            "autoRepeat": repeat,
        });
        if let (true, Some(text)) = (down, press.text) {
            params["text"] = json!(text);
            params["unmodifiedText"] = json!(text);
        }
        self.session
            .call(
                &Heartbeat::detached(),
                "Input.dispatchKeyEvent",
                params,
                CDP_LIMIT,
            )
            .await // naked: SessionHandle::call bounds itself with within()
            .map(drop)
    }

    pub(super) async fn reload(&self) -> Result<Done, String> {
        self.session
            .call(
                &Heartbeat::detached(),
                "Page.reload",
                json!({ "ignoreCache": true }),
                CDP_LIMIT,
            )
            .await?; // naked: SessionHandle::call bounds itself with within()
        Ok(Done::new("reloaded the page"))
    }

    pub(super) async fn clear_cache(&self) -> Result<Done, String> {
        self.session
            .call(
                &Heartbeat::detached(),
                "Network.clearBrowserCache",
                json!({}),
                CDP_LIMIT,
            )
            .await?; // naked: SessionHandle::call bounds itself with within()
        Ok(Done::new("emptied the browser's cache"))
    }

    /// Run `code` in the page. The journal gets its size and hash, never the
    /// code: it may carry a secret. Past the deadline the script is stopped
    /// with `Runtime.terminateExecution`, so a `while (true)` cannot hold the
    /// page.
    pub(super) async fn eval(
        &self,
        caller: &Caller,
        code: &str,
        timeout_ms: Option<u64>,
        await_promise: bool,
        user_gesture: bool,
    ) -> Result<EvalResult, String> {
        if code.len() > protocol::EVAL_MAX {
            return Err(format!(
                "the script is {} bytes; at most {} are taken",
                code.len(),
                protocol::EVAL_MAX
            ));
        }
        let millis = timeout_ms
            .unwrap_or(protocol::EVAL_TIMEOUT_MS)
            .clamp(1, protocol::EVAL_TIMEOUT_MAX_MS);
        let limit = Duration::from_millis(millis);

        let digest = openssl::sha::sha256(code.as_bytes());
        let hash: String = digest[..6]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.log.info(format!(
            "eval of {} bytes (sha256 {hash}...) by {}",
            code.len(),
            caller.describe()
        ));

        let params = json!({
            "expression": code,
            "returnByValue": true,
            "awaitPromise": await_promise,
            "userGesture": user_gesture,
            // Chromium's own stop for a script that never yields; the
            // deadline below covers a Promise that never settles.
            "timeout": millis,
        });
        let heartbeat = Heartbeat::detached();
        let evaluate = self
            .session
            .call(&heartbeat, "Runtime.evaluate", params, limit + CDP_LIMIT);
        match within("the script", limit, evaluate).await {
            Ok(reply) => Ok(eval_result(&reply?)),
            Err(expired) => {
                let stopped = self
                    .session
                    .call(
                        &heartbeat,
                        "Runtime.terminateExecution",
                        json!({}),
                        CDP_LIMIT,
                    )
                    .await; // naked: SessionHandle::call bounds itself with within()
                self.log.info(format!(
                    "eval by {}: {expired}; {}",
                    caller.describe(),
                    if stopped.is_ok() {
                        "stopped it"
                    } else {
                        "could not stop it"
                    }
                ));
                Err(format!("{expired}; the script was stopped"))
            }
        }
    }

    /// Hide: blur whatever has the focus, which is what makes Chromium hide
    /// the keyboard. Show: focus `selector`, or what already has the focus,
    /// as if touched - the keyboard only follows a focused field.
    pub(super) async fn keyboard(
        &self,
        show: bool,
        selector: Option<&str>,
    ) -> Result<Done, String> {
        if let Some(selector) = selector {
            if selector.len() > SELECTOR_MAX {
                return Err(format!("the selector is longer than {SELECTOR_MAX} bytes"));
            }
        }
        if show {
            let ini = self.paths.weston_config.clone();
            let running = blocking("reading the Weston config", move || {
                Ok(std::fs::read_to_string(&ini)
                    .map(|text| osk_running(&text))
                    .unwrap_or(true))
            })
            .await?;
            if !running {
                return Err(
                    "the on-screen keyboard is not running: screen.osk is never, or auto with a \
                     hardware keyboard plugged in; `tessaro-ctl config set screen.osk=always`"
                        .to_string(),
                );
            }
        }

        let selector = serde_json::to_string(&selector).expect("a string serializes");
        let expression = if show {
            format!(
                r#"(() => {{
  const selector = {selector};
  const field = selector ? document.querySelector(selector) : document.activeElement;
  if (selector && !field) return "nothing on the page matches " + selector;
  if (!field || field === document.body) return "nothing has the focus; name a field to type into";
  field.focus();
  if (navigator.virtualKeyboard && navigator.virtualKeyboard.show) navigator.virtualKeyboard.show();
  return "";
}})()"#
            )
        } else {
            r#"(() => {
  const field = document.activeElement;
  if (field && field !== document.body) field.blur();
  if (navigator.virtualKeyboard && navigator.virtualKeyboard.hide) navigator.virtualKeyboard.hide();
  return "";
})()"#
                .to_string()
        };

        let reply = self
            .session
            .call(
                &Heartbeat::detached(),
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "userGesture": show }),
                CDP_LIMIT,
            )
            .await?; // naked: SessionHandle::call bounds itself with within()
        match reply["result"]["value"].as_str() {
            Some("") => Ok(Done::new(if show {
                "showed the on-screen keyboard"
            } else {
                "hid the on-screen keyboard"
            })),
            Some(problem) => Err(problem.to_string()),
            None => Err(eval_result(&reply)
                .exception
                .map(|exception| exception.text)
                .unwrap_or_else(|| "the page did not answer".to_string())),
        }
    }
}

/// `Runtime.evaluate`'s answer as `eval` reports it.
fn eval_result(reply: &Value) -> EvalResult {
    let object = &reply["result"];
    let kind = object["subtype"]
        .as_str()
        .or_else(|| object["type"].as_str())
        .unwrap_or("undefined")
        .to_string();
    let exception = reply.get("exceptionDetails").map(|details| EvalException {
        text: details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("an exception")
            .to_string(),
        line: details["lineNumber"].as_u64().unwrap_or(0) + 1,
        column: details["columnNumber"].as_u64().unwrap_or(0) + 1,
    });
    let description = object["unserializableValue"]
        .as_str()
        .or_else(|| object["description"].as_str())
        .map(str::to_string);
    EvalResult {
        value: object.get("value").cloned(),
        kind,
        description: if object.get("value").is_some() {
            None
        } else {
            description
        },
        exception,
    }
}

/// Is weston-keyboard running, by the config Weston started with? An
/// `[input-method]` section with an empty `path=` is how
/// `tessaro-weston-config` turns it off.
fn osk_running(ini: &str) -> bool {
    let mut in_section = false;
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == "[input-method]";
        } else if in_section {
            if let Some(path) = line.strip_prefix("path=") {
                return !path.trim().is_empty();
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_input_method_path_is_no_keyboard() {
        assert!(!osk_running("[core]\nidle-time=0\n[input-method]\npath=\n"));
        assert!(osk_running(
            "[input-method]\npath=/usr/libexec/weston-keyboard\n"
        ));
        assert!(osk_running("[core]\nidle-time=0\n"));
        assert!(osk_running("[input-method]\n[output]\npath=\n"));
    }

    #[test]
    fn eval_reports_values_types_and_exceptions() {
        let number = eval_result(&json!({ "result": { "type": "number", "value": 2 } }));
        assert_eq!(number.value, Some(json!(2)));
        assert_eq!(number.kind, "number");
        assert_eq!(number.exception, None);

        let nothing = eval_result(&json!({ "result": { "type": "undefined" } }));
        assert_eq!((nothing.value, nothing.kind.as_str()), (None, "undefined"));

        let node = eval_result(&json!({
            "result": { "type": "object", "subtype": "node", "description": "div#app" }
        }));
        assert_eq!(node.kind, "node");
        assert_eq!(node.description.as_deref(), Some("div#app"));

        let thrown = eval_result(&json!({
            "result": { "type": "object", "subtype": "error" },
            "exceptionDetails": {
                "text": "Uncaught",
                "lineNumber": 0,
                "columnNumber": 6,
                "exception": { "description": "ReferenceError: nope is not defined" }
            }
        }));
        let exception = thrown.exception.expect("an exception");
        assert_eq!(exception.text, "ReferenceError: nope is not defined");
        assert_eq!((exception.line, exception.column), (1, 7));
    }
}
