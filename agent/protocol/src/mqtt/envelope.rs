//! How a request and its answer travel. A command topic names the endpoint
//! (`.../cmd/<group>/<Name>`); the message carries what an HTTP request
//! carries in its query and body, and the answer what the HTTP answer
//! carries in its status, headers and body. MQTT v5's response topic and
//! correlation data pair the two.
//!
//! * A JSON endpoint's request is `{"params": {...}, "body": ...}`, both
//!   optional: an empty payload is no params and no body.
//! * A raw endpoint's request (an upload, an update chunk, a print) is the
//!   bytes themselves, with its params as JSON in the `params` user
//!   property.
//! * An answer has the `status` user property, `ok` or the error code, and
//!   the `device` one naming who answered (a tag reaches several). An `ok`
//!   answer is the endpoint's JSON, or the raw bytes of a raw one with its
//!   headers as user properties; a refusal is `{"error", "code"}`, as over
//!   HTTP.

use serde_json::{Map, Value};

/// User property: a raw request's params, as a JSON object.
pub const PARAMS: &str = "params";
/// User property: `ok`, or the code of the refusal.
pub const STATUS: &str = "status";
/// User property: the id of the device that answered.
pub const DEVICE: &str = "device";
/// `STATUS` of an answer that is not a refusal.
pub const OK: &str = "ok";

pub const JSON: &str = "application/json";

/// A request as the device takes it: the params as the pairs an HTTP query
/// would give, and the body as HTTP would carry it - JSON text, or the raw
/// bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub params: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Reads a request. `raw` is whether the endpoint takes raw bytes.
pub fn read_request(
    payload: &[u8],
    user: &[(String, String)],
    raw: bool,
) -> Result<Request, String> {
    if raw {
        let params = match user.iter().find(|(key, _)| key == PARAMS) {
            Some((_, text)) => {
                let value: Value = serde_json::from_str(text)
                    .map_err(|e| format!("the {PARAMS} property: {e}"))?;
                pairs(&value)?
            }
            None => Vec::new(),
        };
        return Ok(Request {
            params,
            body: payload.to_vec(),
        });
    }
    if payload.iter().all(u8::is_ascii_whitespace) {
        return Ok(Request {
            params: Vec::new(),
            body: Vec::new(),
        });
    }
    let value: Value =
        serde_json::from_slice(payload).map_err(|e| format!("the request is not JSON: {e}"))?;
    let Value::Object(mut fields) = value else {
        return Err("the request is a JSON object: {\"params\": {...}, \"body\": ...}".into());
    };
    if let Some(unknown) = fields.keys().find(|key| *key != "params" && *key != "body") {
        return Err(format!(
            "{unknown:?} is not part of a request; it has params and body"
        ));
    }
    let params = match fields.remove("params") {
        Some(value) => pairs(&value)?,
        None => Vec::new(),
    };
    let body = match fields.remove("body") {
        None | Some(Value::Null) => Vec::new(),
        Some(body) => serde_json::to_vec(&body).map_err(|e| e.to_string())?,
    };
    Ok(Request { params, body })
}

/// The params of a request: a flat object, its values written as a query
/// string would write them.
fn pairs(value: &Value) -> Result<Vec<(String, String)>, String> {
    let fields = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Object(fields) => fields,
        _ => return Err("params is a JSON object".into()),
    };
    let mut out = Vec::new();
    for (key, value) in fields {
        let text = match value {
            Value::Null => continue,
            Value::String(text) => text.clone(),
            Value::Bool(_) | Value::Number(_) => value.to_string(),
            Value::Array(_) | Value::Object(_) => {
                return Err(format!(
                    "params.{key}: a param is a string, a number or a boolean"
                ))
            }
        };
        out.push((key.clone(), text));
    }
    Ok(out)
}

/// A JSON request's payload, from the query pairs a client would send over
/// HTTP and its JSON body (empty for none).
pub fn write_request(params: &[(String, String)], body: &[u8]) -> Result<Vec<u8>, String> {
    let mut fields = Map::new();
    if !params.is_empty() {
        fields.insert("params".into(), params_object(params));
    }
    if !body.is_empty() {
        let body: Value =
            serde_json::from_slice(body).map_err(|e| format!("the body is not JSON: {e}"))?;
        fields.insert("body".into(), body);
    }
    serde_json::to_vec(&Value::Object(fields)).map_err(|e| e.to_string())
}

/// The `params` user property of a raw request.
pub fn params_property(params: &[(String, String)]) -> String {
    params_object(params).to_string()
}

fn params_object(params: &[(String, String)]) -> Value {
    Value::Object(
        params
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// The value of a user property.
pub fn property<'a>(user: &'a [(String, String)], key: &str) -> Option<&'a str> {
    user.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_json_request_carries_params_and_a_body() {
        let request = read_request(
            br#"{"params":{"device":"video0","width":640,"all":true},"body":{"a":1}}"#,
            &[],
            false,
        )
        .unwrap();
        // The order follows serde_json's features, which the workspace decides.
        let mut params = request.params;
        params.sort();
        assert_eq!(
            params,
            props(&[("all", "true"), ("device", "video0"), ("width", "640")])
        );
        assert_eq!(request.body, br#"{"a":1}"#);
    }

    #[test]
    fn an_empty_payload_is_no_params_and_no_body() {
        assert_eq!(
            read_request(b"", &[], false).unwrap(),
            Request {
                params: vec![],
                body: vec![]
            }
        );
        assert_eq!(
            read_request(b" \n", &[], false).unwrap(),
            Request {
                params: vec![],
                body: vec![]
            }
        );
        assert_eq!(
            read_request(b"{}", &[], false).unwrap(),
            Request {
                params: vec![],
                body: vec![]
            }
        );
        assert_eq!(
            read_request(br#"{"body":null}"#, &[], false).unwrap().body,
            b""
        );
    }

    #[test]
    fn a_malformed_request_says_what_is_wrong() {
        assert!(read_request(b"nope", &[], false)
            .unwrap_err()
            .contains("not JSON"));
        assert!(read_request(b"[1]", &[], false)
            .unwrap_err()
            .contains("JSON object"));
        assert!(read_request(br#"{"parms":{}}"#, &[], false)
            .unwrap_err()
            .contains("\"parms\""));
        assert!(read_request(br#"{"params":{"a":[1]}}"#, &[], false)
            .unwrap_err()
            .contains("params.a"));
        assert!(read_request(br#"{"params":3}"#, &[], false)
            .unwrap_err()
            .contains("JSON object"));
    }

    #[test]
    fn a_raw_request_is_its_bytes_with_params_beside_them() {
        let request = read_request(
            &[0, 1, 2],
            &props(&[("params", r#"{"offset":4096}"#)]),
            true,
        )
        .unwrap();
        assert_eq!(request.params, props(&[("offset", "4096")]));
        assert_eq!(request.body, vec![0, 1, 2]);
        assert_eq!(read_request(b"x", &[], true).unwrap().params, vec![]);
        assert!(read_request(b"x", &props(&[("params", "{")]), true).is_err());
    }

    #[test]
    fn what_a_client_writes_the_device_reads_back() {
        let params = props(&[("device", "video0"), ("width", "640")]);
        let payload = write_request(&params, br#"{"a":[1,2]}"#).unwrap();
        let request = read_request(&payload, &[], false).unwrap();
        assert_eq!(request.params, params);
        assert_eq!(request.body, br#"{"a":[1,2]}"#);
        assert_eq!(write_request(&[], b"").unwrap(), b"{}");
        let raw = read_request(
            b"bytes",
            &props(&[("params", &params_property(&params))]),
            true,
        )
        .unwrap();
        assert_eq!(raw.params, params);
    }
}
