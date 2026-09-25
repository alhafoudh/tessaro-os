//! The DevTools wire format: what goes out, and what comes back.
//!
//! Page-level session, so every command is flat - no `sessionId` - which is
//! also what `DeviceAccess` (TODO item 5) needs, since it is page-scoped.

use serde_json::{json, Value};

pub fn request(id: u64, method: &str, params: &Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// The answer to a command we sent: its `result`, or the protocol's error.
    Reply {
        id: u64,
        result: Result<Value, String>,
    },
    /// Something the browser volunteered.
    Event {
        method: String,
        params: Value,
    },
    Other,
}

pub fn parse(text: &str) -> Incoming {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Incoming::Other;
    };

    if let Some(id) = value["id"].as_u64() {
        let result = match value.get("error") {
            Some(error) if !error.is_null() => Err(error.to_string()),
            _ => Ok(value["result"].clone()),
        };
        return Incoming::Reply { id, result };
    }

    match value["method"].as_str() {
        Some(method) => Incoming::Event {
            method: method.to_string(),
            params: value["params"].clone(),
        },
        None => Incoming::Other,
    }
}

/// The URL a `Frame` object is showing, fragment included - CDP reports the
/// fragment separately and the page would otherwise look like it never moved.
pub fn frame_url(frame: &Value) -> Option<String> {
    let url = frame["url"].as_str()?;
    Some(format!(
        "{url}{}",
        frame["urlFragment"].as_str().unwrap_or("")
    ))
}

/// A frame with no parent is the page itself, not an iframe on it.
pub fn is_main_frame(frame: &Value) -> bool {
    frame.get("parentId").is_none_or(Value::is_null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_one_flat_object() {
        let text = request(7, "Page.navigate", &json!({ "url": "http://kiosk.test/" }));
        let value: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(
            value,
            json!({ "id": 7, "method": "Page.navigate", "params": { "url": "http://kiosk.test/" } })
        );
    }

    #[test]
    fn a_reply_carries_its_result_or_its_error() {
        assert_eq!(
            parse(r#"{"id": 3, "result": {"frameId": "F"}}"#),
            Incoming::Reply {
                id: 3,
                result: Ok(json!({ "frameId": "F" }))
            }
        );

        let Incoming::Reply {
            id: 4,
            result: Err(error),
        } = parse(r#"{"id": 4, "error": {"code": -32601, "message": "not found"}}"#)
        else {
            panic!("not an error reply");
        };
        assert!(error.contains("not found"));
    }

    #[test]
    fn anything_without_an_id_is_an_event() {
        assert_eq!(
            parse(r#"{"method": "Page.frameNavigated", "params": {"frame": {"id": "F"}}}"#),
            Incoming::Event {
                method: "Page.frameNavigated".to_string(),
                params: json!({ "frame": { "id": "F" } }),
            }
        );
        assert_eq!(parse("garbage"), Incoming::Other);
    }

    #[test]
    fn the_fragment_is_part_of_the_url() {
        let frame = json!({ "url": "http://kiosk.test/app", "urlFragment": "#menu" });

        assert_eq!(
            frame_url(&frame).as_deref(),
            Some("http://kiosk.test/app#menu")
        );
        assert!(is_main_frame(&frame));
        assert!(!is_main_frame(&json!({ "parentId": "TOP" })));
    }
}
