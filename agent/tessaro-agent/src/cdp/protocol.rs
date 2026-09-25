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

/// The page's window as it is with no zoom: its device scale factor (the
/// Weston output scale, since Chromium cannot scale itself on this stack)
/// and its size in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub ratio: f64,
    pub width: f64,
    pub height: f64,
}

/// `Runtime.evaluate` params asking the page for its `Window`.
pub fn window_query() -> Value {
    json!({
        "expression": "[window.devicePixelRatio, window.innerWidth, window.innerHeight]",
        "returnByValue": true,
    })
}

/// The `Window` out of that query's result, if the page gave a usable one.
pub fn window(result: &Value) -> Option<Window> {
    let values = result["result"]["value"].as_array()?;
    let number = |index: usize| {
        values
            .get(index)?
            .as_f64()
            .filter(|value| value.is_finite() && *value > 0.0)
    };
    Some(Window {
        ratio: number(0)?,
        width: number(1)?,
        height: number(2)?,
    })
}

/// `Emulation.setDeviceMetricsOverride` params for a page zoom of `percent`:
/// a viewport that many times smaller in CSS pixels, drawn at a scale
/// factor that many times larger, so it still fills the window pixel for
/// pixel and the page reflows the way Ctrl+/- makes it. The scale factor
/// alone (width and height 0) is not a zoom: the viewport keeps its CSS
/// size and is only drawn at a higher resolution. The scale is worked out
/// from the rounded width, so the page ends exactly at the window's edge.
pub fn zoom_override(window: &Window, percent: u16) -> Value {
    let zoom = f64::from(percent) / 100.0;
    let width = (window.width / zoom).round().max(1.0);
    let height = (window.height / zoom).round().max(1.0);
    json!({
        "width": width as u64,
        "height": height as u64,
        "deviceScaleFactor": window.ratio * window.width / width,
        "mobile": false,
    })
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

    #[test]
    fn zoom_shrinks_the_viewport_and_fills_the_window_with_it() {
        let result = json!({ "result": { "type": "object", "value": [2, 1280, 720] } });
        let window = window(&result).unwrap();
        assert_eq!(
            window,
            Window {
                ratio: 2.0,
                width: 1280.0,
                height: 720.0
            }
        );

        assert_eq!(
            zoom_override(&window, 200),
            json!({ "width": 640, "height": 360, "deviceScaleFactor": 4.0, "mobile": false })
        );
        assert_eq!(
            zoom_override(&window, 25),
            json!({ "width": 5120, "height": 2880, "deviceScaleFactor": 0.5, "mobile": false })
        );
    }

    #[test]
    fn a_rounded_viewport_still_ends_at_the_windows_edge() {
        let window = Window {
            ratio: 1.0,
            width: 800.0,
            height: 600.0,
        };
        let params = zoom_override(&window, 150);

        assert_eq!(params["width"], 533);
        assert_eq!(params["height"], 400);
        let drawn = 533.0 * params["deviceScaleFactor"].as_f64().unwrap();
        assert!((drawn - 800.0).abs() < 1e-9, "{drawn}");
    }

    #[test]
    fn a_page_without_a_usable_window_gives_none() {
        assert_eq!(window(&json!({})), None);
        assert_eq!(
            window(&json!({ "result": { "type": "object", "value": [0, 800, 600] } })),
            None
        );
        assert_eq!(
            window(&json!({ "result": { "type": "object", "value": [1, 800] } })),
            None
        );
        assert_eq!(
            window(&json!({ "result": { "type": "number", "value": 2 } })),
            None
        );
    }
}
