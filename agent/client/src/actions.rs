//! What a command sends, worked out the same way in both clients: the
//! settings a `time ntp` changes, the body of a `time set`, and the like.
//! Each takes what the user typed or picked and gives back the request, or
//! why it cannot be sent.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::api::TimeSetBody;
use protocol::keys;

/// The settings `time ntp on|off` changes: `time.ntp.enable`, and the
/// servers when some are given. Servers go with NTP on only; none leaves
/// `time.ntp.servers` as it is.
pub fn ntp_change(on: bool, servers: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::from([(
        keys::NTP_ENABLE.to_string(),
        if on { "1" } else { "0" }.to_string(),
    )]);
    let servers: Vec<&str> = servers
        .iter()
        .map(|server| server.trim())
        .filter(|server| !server.is_empty())
        .collect();
    if !servers.is_empty() {
        if !on {
            return Err("NTP servers go with NTP on".to_string());
        }
        values.insert(keys::NTP_SERVERS.to_string(), servers.join(","));
    }
    Ok(values)
}

/// The body of `time set`: `local` as the device's local time
/// (`YYYY-MM-DD HH:MM[:SS]`), or without it this computer's clock.
pub fn set_clock(local: Option<&str>) -> Result<TimeSetBody, String> {
    match local.map(str::trim).filter(|local| !local.is_empty()) {
        Some(local) => {
            protocol::parse_local_time(local)?;
            Ok(TimeSetBody {
                usec: None,
                local: Some(local.to_string()),
            })
        }
        None => Ok(TimeSetBody {
            usec: Some(now_usec()?),
            local: None,
        }),
    }
}

/// The root password to set from a password typed twice: `None` when both
/// are empty, for a random one the device shows once.
pub fn root_password(typed: &str, again: &str) -> Result<Option<String>, String> {
    if typed != again {
        return Err("the passwords do not match".to_string());
    }
    if typed.is_empty() {
        return Ok(None);
    }
    protocol::check_password(typed)?;
    Ok(Some(typed.to_string()))
}

/// This computer's clock.
fn now_usec() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_micros() as u64)
        .map_err(|_| "this computer's clock is before 1970".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_go_with_ntp_on_only() {
        let on = ntp_change(true, &["a".into(), " b ".into()]).unwrap();
        assert_eq!(on[keys::NTP_SERVERS], "a,b");
        assert!(ntp_change(false, &["a".into()]).is_err());
        let off = ntp_change(false, &[" ".into()]).unwrap();
        assert!(!off.contains_key(keys::NTP_SERVERS));
        assert_eq!(off[keys::NTP_ENABLE], "0");
    }

    #[test]
    fn a_root_password_is_typed_twice() {
        assert_eq!(root_password("", ""), Ok(None));
        assert!(root_password("a", "b").is_err());
    }

    #[test]
    fn a_clock_is_this_computers_without_a_time() {
        assert!(set_clock(None).unwrap().usec.is_some());
        assert!(set_clock(Some("nonsense")).is_err());
    }
}
