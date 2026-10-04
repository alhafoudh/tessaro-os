//! Browser policies: named Chromium policy documents the device merges into
//! the one policy file it renders (docs/kiosk-browser.md, **Policies**).
//!
//! A document is kept as it was typed, comments and trailing commas
//! included, so an editor gets the user's own text back. The rules are here
//! so both clients refuse what the device would before anything is sent,
//! and Webconfig ports them (`webconfig/src/flows/policies.ts`).

use data_encoding::HEXLOWER;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Largest document, in bytes of its text.
pub const POLICY_TEXT_MAX: usize = 64 * 1024;

/// Most documents the device holds. Chromium reads the merged file whole on
/// every change.
pub const POLICIES_MAX: usize = 32;

/// Longest document name.
pub const POLICY_NAME_MAX: usize = 32;

/// The grants that follow the kiosk origin: the device APIs, the microphone,
/// the camera and reaching `http://127.0.0.1` without a Local Network Access
/// prompt.
pub const ORIGIN_POLICIES: &[&str] = &[
    "SerialAllowAllPortsForUrls",
    "WebHidAllowAllDevicesForUrls",
    "AudioCaptureAllowedUrls",
    "VideoCaptureAllowedUrls",
    "LocalNetworkAccessAllowedForUrls",
];

/// The entries that point Chromium at the local proxy.
pub const PROXY_POLICIES: &[&str] = &["ProxyMode", "ProxyServer", "ProxyBypassList"];

/// The extra certificate authorities, as base64 DER.
pub const CA_POLICY: &str = "CACertificates";

/// printer.enable: whether the page prints, and that print preview starts on
/// the CUPS default printer, which is where `--kiosk-printing` prints.
pub const PRINT_POLICIES: &[&str] = &["PrintingEnabled", "PrintPreviewUseSystemDefaultPrinter"];

/// The URL filter lists. A document may set them; browser.block and
/// browser.allow are appended to what it sets, not put in its place.
pub const BLOCK_POLICY: &str = "URLBlocklist";
pub const ALLOW_POLICY: &str = "URLAllowlist";

/// What "new policy" starts from in every client.
pub const TEMPLATE: &str = include_str!("policy-template.jsonc");

/// The command that sets a key the device renders itself, or `None` for a
/// key a document may carry.
pub fn managed(key: &str) -> Option<&'static str> {
    if ORIGIN_POLICIES.contains(&key) {
        Some("tessaro-ctl config set browser.device_origins=ORIGIN")
    } else if PROXY_POLICIES.contains(&key) {
        Some("tessaro-ctl network proxy set URL")
    } else if key == CA_POLICY {
        Some("tessaro-ctl network certs add FILE")
    } else if PRINT_POLICIES.contains(&key) {
        Some("tessaro-ctl config set printer.enable=1")
    } else {
        None
    }
}

/// A name the device stores a document under: lower-case letters, digits,
/// `-` and `_`, starting with a letter or digit.
pub fn check_name(name: &str) -> Result<(), String> {
    let first_ok = name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit());
    let rest_ok = name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
    if name.is_empty() {
        Err("a policy needs a name".to_string())
    } else if name.len() > POLICY_NAME_MAX {
        Err(format!(
            "{name:?} is too long; a policy name is at most {POLICY_NAME_MAX} characters"
        ))
    } else if !first_ok || !rest_ok {
        Err(format!(
            "{name:?} is not a policy name: use lower-case letters, digits, - and _, \
             starting with a letter or digit"
        ))
    } else {
        Ok(())
    }
}

/// A document's revision: the SHA-256 of its text, lower-case hex. A save
/// that names the revision it started from is refused once the stored text
/// has moved on.
pub fn revision(text: &str) -> String {
    HEXLOWER.encode(&Sha256::digest(text.as_bytes()))
}

/// Why a document is refused. `line` and `column` count from 1 into the
/// text as typed; 0 when the problem has no one place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl PolicyError {
    fn whole(message: impl Into<String>) -> Self {
        Self {
            line: 0,
            column: 0,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(
                f,
                "line {} column {}: {}",
                self.line, self.column, self.message
            )
        } else {
            f.write_str(&self.message)
        }
    }
}

/// The document's entries, or why the device would refuse it: too large,
/// not JSON, not an object, a key that is no policy name, or a key the
/// device sets itself.
pub fn check(text: &str) -> Result<Map<String, Value>, PolicyError> {
    if text.len() > POLICY_TEXT_MAX {
        return Err(PolicyError::whole(format!(
            "that is {} bytes; a policy is at most {POLICY_TEXT_MAX}",
            text.len()
        )));
    }
    let value: Value = serde_json::from_str(&strict_json(text)).map_err(|err| {
        let message = err.to_string();
        let suffix = format!(" at line {} column {}", err.line(), err.column());
        PolicyError {
            line: err.line(),
            column: err.column(),
            message: message
                .strip_suffix(&suffix)
                .unwrap_or(&message)
                .to_string(),
        }
    })?;
    let Value::Object(entries) = value else {
        return Err(PolicyError::whole(
            "a policy is one JSON object: { \"PolicyName\": value, ... }",
        ));
    };
    // In sorted order, so every client names the same key first.
    for key in &keys(&entries) {
        let named = key
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
            && key.chars().all(|ch| ch.is_ascii_alphanumeric());
        if !named {
            return Err(PolicyError::whole(format!(
                "{key:?} is not a Chromium policy name (letters and digits, like DownloadRestrictions)"
            )));
        }
        if let Some(command) = managed(key) {
            return Err(PolicyError::whole(format!(
                "{key} is set by the device itself; use `{command}`"
            )));
        }
    }
    Ok(entries)
}

/// The policies a document sets, sorted. serde_json's map keeps the typed
/// order where a crate in the build turns on `preserve_order` and sorts
/// where none does, so anything shown goes through this.
pub fn keys(entries: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = entries.keys().cloned().collect();
    keys.sort();
    keys
}

/// Chromium's policy loader accepts `//` and `/* */` comments and trailing
/// commas (`JSON_PARSE_CHROMIUM_EXTENSIONS`); serde_json accepts none of
/// them. Each is blanked with spaces, newlines kept, so a parse error's line
/// and column still point into the text as it was typed.
pub fn strict_json(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    let mut in_string = false;
    let blank = |out: &mut String, ch: char| out.push(if ch == '\n' { '\n' } else { ' ' });

    while at < chars.len() {
        let ch = chars[at];
        if in_string {
            out.push(ch);
            if ch == '\\' && at + 1 < chars.len() {
                out.push(chars[at + 1]);
                at += 1;
            } else if ch == '"' {
                in_string = false;
            }
            at += 1;
            continue;
        }

        match (ch, chars.get(at + 1)) {
            ('"', _) => {
                in_string = true;
                out.push(ch);
                at += 1;
            }
            ('/', Some('/')) => {
                while at < chars.len() && chars[at] != '\n' {
                    blank(&mut out, chars[at]);
                    at += 1;
                }
            }
            ('/', Some('*')) => {
                let start = at;
                at += 2;
                while at < chars.len() && !(chars[at] == '*' && chars.get(at + 1) == Some(&'/')) {
                    at += 1;
                }
                at = (at + 2).min(chars.len());
                for &skipped in &chars[start..at] {
                    blank(&mut out, skipped);
                }
            }
            (',', _) => {
                let next = chars[at + 1..]
                    .iter()
                    .enumerate()
                    .find(|(_, ch)| !ch.is_whitespace());
                // A comma followed, past whitespace and comments, by a closer.
                let trailing = match next {
                    Some((_, '}')) | Some((_, ']')) => true,
                    Some((offset, '/')) => {
                        let rest: String = chars[at + 1 + offset..].iter().collect();
                        strict_json(&rest).trim_start().starts_with(['}', ']'])
                    }
                    _ => false,
                };
                out.push(if trailing { ' ' } else { ',' });
                at += 1;
            }
            _ => {
                out.push(ch);
                at += 1;
            }
        }
    }
    out
}

// --- on the wire -----------------------------------------------------------

/// One stored document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyInfo {
    pub name: String,
    /// From 1, the highest priority: it wins a policy others set too.
    pub position: u32,
    pub revision: String,
    /// The policies it sets, sorted.
    pub keys: Vec<String>,
    /// Why the stored text no longer passes the check, when it does not: the
    /// device leaves such a document out of the policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// One stored document with its text, as typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyDoc {
    pub name: String,
    pub text: String,
    pub revision: String,
}

/// A key set in more than one place, and the document on the other side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyOverlap {
    pub key: String,
    pub policy: String,
}

/// What saving a document did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicySaved {
    pub name: String,
    pub position: u32,
    pub revision: String,
    pub keys: Vec<String>,
    /// The stored text and position were already these; nothing was written.
    pub unchanged: bool,
    /// Keys it sets that the image sets too: its value wins.
    pub overrides_image: Vec<String>,
    /// Keys a document lower in priority sets too, which this one wins.
    pub shadows: Vec<PolicyOverlap>,
    /// Keys a document higher in priority sets too, which win over this one.
    pub shadowed_by: Vec<PolicyOverlap>,
    /// The browser was restarted to read the new policy.
    pub restarted: bool,
}

/// What moving a document did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyMoved {
    pub name: String,
    /// Where it is now: what was asked, held to 1 and the number of
    /// documents.
    pub position: u32,
    pub restarted: bool,
}

/// What removing a document did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyRemoved {
    pub name: String,
    pub restarted: bool,
}

/// Where one entry of the merged policy comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PolicySource {
    /// The image's own policy.
    Image,
    /// A stored document.
    Policy { name: String },
    /// The device, from its settings (`managed`).
    Device,
}

/// One entry of the policy Chromium reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EffectiveEntry {
    pub key: String,
    pub value: Value,
    pub source: PolicySource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas_keep_their_place() {
        let text = "{\n  /* one\n     two */\n  \"A\": 1, // x\n  \"B\": [1, 2,],\n}";
        let strict = strict_json(text);
        assert_eq!(strict.len(), text.len());
        assert_eq!(strict.lines().count(), text.lines().count());
        let value: Value = serde_json::from_str(&strict).unwrap();
        assert_eq!(value, serde_json::json!({"A": 1, "B": [1, 2]}));
    }

    #[test]
    fn strings_are_left_alone() {
        let text = r#"{"Url": "http://a.test/*,", "Esc": "a\"//b"}"#;
        let value: Value = serde_json::from_str(&strict_json(text)).unwrap();
        assert_eq!(value["Url"], "http://a.test/*,");
        assert_eq!(value["Esc"], "a\"//b");
    }

    #[test]
    fn a_syntax_error_points_into_the_text_as_typed() {
        let text = "{\n  /* a\n     comment */\n  \"A\": 1\n  \"B\": 2\n}";
        let err = check(text).unwrap_err();
        assert_eq!((err.line, err.column), (5, 3));
        assert!(!err.message.contains(" at line"), "{}", err.message);
        assert!(err.to_string().starts_with("line 5 column 3: "), "{err}");
    }

    #[test]
    fn the_device_keys_are_refused_with_their_command() {
        let err = check(r#"{"CACertificates": []}"#).unwrap_err();
        assert!(
            err.message.contains("tessaro-ctl network certs add"),
            "{err}"
        );
        let err = check(r#"{"ProxyMode": "direct"}"#).unwrap_err();
        assert!(
            err.message.contains("tessaro-ctl network proxy set"),
            "{err}"
        );
        let err = check(r#"{"SerialAllowAllPortsForUrls": []}"#).unwrap_err();
        assert!(err.message.contains("browser.device_origins"), "{err}");
    }

    #[test]
    fn only_an_object_of_policy_names_passes() {
        assert!(check("[]").is_err());
        assert!(check(r#"{"URL Blocklist": []}"#).is_err());
        assert!(check(r#"{"1Bad": []}"#).is_err());
        assert_eq!(check("{}").unwrap().len(), 0);
        let big = format!("{{\"A\": \"{}\"}}", "x".repeat(POLICY_TEXT_MAX));
        assert_eq!(check(&big).unwrap_err().line, 0);
    }

    #[test]
    fn the_template_passes() {
        let entries = check(TEMPLATE).unwrap();
        assert!(entries.contains_key("DownloadRestrictions"));
        assert!(!entries.contains_key("URLBlocklist"));
    }

    #[test]
    fn names() {
        assert!(check_name("lockdown").is_ok());
        assert!(check_name("corp-urls_2").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("-x").is_err());
        assert!(check_name("Upper").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name(&"a".repeat(POLICY_NAME_MAX + 1)).is_err());
    }

    #[test]
    fn a_revision_is_the_texts_hash() {
        assert_eq!(revision(""), HEXLOWER.encode(&Sha256::digest(b"")));
        assert_ne!(revision("{}"), revision("{ }"));
    }
}
