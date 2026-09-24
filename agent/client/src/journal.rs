//! One entry of the `logs` stream: `journalctl --output=json`, read the same
//! way by every client.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Microseconds since the Unix epoch, when the entry was written.
    pub time: Option<u64>,
    /// `SYSLOG_IDENTIFIER`, else the unit, else `?`.
    pub source: String,
    /// syslog priority: 0 is emerg, 3 err, 4 warning, 7 debug.
    pub priority: Option<u8>,
    pub message: String,
}

impl Entry {
    pub fn parse(event: &Value) -> Self {
        let field = |name: &str| event.get(name).and_then(Value::as_str);
        let message = match event.get("MESSAGE") {
            Some(Value::String(text)) => text.clone(),
            // Non-UTF-8 messages come as a byte array.
            Some(Value::Array(bytes)) => {
                let bytes: Vec<u8> = bytes
                    .iter()
                    .filter_map(|b| b.as_u64().map(|b| b as u8))
                    .collect();
                String::from_utf8_lossy(&bytes).into_owned()
            }
            Some(other) => other.to_string(),
            None => event.to_string(),
        };
        Self {
            time: field("__REALTIME_TIMESTAMP").and_then(|time| time.parse().ok()),
            source: field("SYSLOG_IDENTIFIER")
                .or_else(|| field("_SYSTEMD_UNIT"))
                .unwrap_or("?")
                .to_string(),
            priority: field("PRIORITY").and_then(|priority| priority.parse().ok()),
            message,
        }
    }

    /// `HH:MM:SS`, UTC: the device and the client need not share a zone.
    pub fn clock(&self) -> String {
        let Some(time) = self.time else {
            return String::new();
        };
        let seconds = time / 1_000_000 % 86_400;
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_entry_reads_its_fields() {
        let entry = Entry::parse(&json!({
            "__REALTIME_TIMESTAMP": "1758729600123456",
            "SYSLOG_IDENTIFIER": "tessaro-agent",
            "_SYSTEMD_UNIT": "tessaro-agent.service",
            "PRIORITY": "4",
            "MESSAGE": "the browser stopped answering",
        }));
        assert_eq!(entry.source, "tessaro-agent");
        assert_eq!(entry.priority, Some(4));
        assert_eq!(entry.message, "the browser stopped answering");
        assert_eq!(entry.clock(), "16:00:00");
    }

    #[test]
    fn a_byte_array_message_is_read_as_text() {
        let entry = Entry::parse(&json!({ "MESSAGE": [104, 105] }));
        assert_eq!(entry.message, "hi");
    }

    #[test]
    fn missing_fields_leave_it_readable() {
        let entry = Entry::parse(&json!({ "_SYSTEMD_UNIT": "weston.service" }));
        assert_eq!(entry.source, "weston.service");
        assert_eq!(entry.priority, None);
        assert_eq!(entry.clock(), "");
        assert!(entry.message.contains("weston.service"));
    }
}
