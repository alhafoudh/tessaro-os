//! The words for schedules (`protocol::ScheduleInfo`), shared by
//! `tessaro-ctl schedule` and the GUI's Schedules page so they say it the
//! same way, and reading a timeout the way both accept it.

use protocol::ScheduleRun;

/// A timeout as typed: `90`, `90s`, `10m`, `2h`, `1h30m`; `0` or `none`
/// for no timeout, as `0`.
pub fn parse_timeout(text: &str) -> Result<u64, String> {
    let text = text.trim();
    if text == "none" {
        return Ok(0);
    }
    let refuse = || format!("{text:?} is not a timeout: seconds, or like 90s, 10m, 2h, 1h30m");
    if text.is_empty() {
        return Err(refuse());
    }
    let mut total: u64 = 0;
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        let unit = match ch {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            _ => return Err(refuse()),
        };
        let value: u64 = digits.parse().map_err(|_| refuse())?;
        total = value
            .checked_mul(unit)
            .and_then(|seconds| total.checked_add(seconds))
            .ok_or_else(refuse)?;
        digits.clear();
    }
    if !digits.is_empty() {
        let value: u64 = digits.parse().map_err(|_| refuse())?;
        total = total.checked_add(value).ok_or_else(refuse)?;
    }
    Ok(total)
}

/// Seconds as `45s`, `10min`, `1h 30min`, `2d 3h`.
pub fn duration(seconds: u64) -> String {
    let (days, hours, minutes, rest) = (
        seconds / 86_400,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    );
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{rest}s"),
        (0, 0, _) if rest == 0 => format!("{minutes}min"),
        (0, 0, _) => format!("{minutes}min {rest}s"),
        (0, _, 0) => format!("{hours}h"),
        (0, _, _) => format!("{hours}h {minutes}min"),
        (_, 0, _) => format!("{days}d"),
        _ => format!("{days}d {hours}h"),
    }
}

/// `unix` from `now`, both seconds since the epoch: `in 1h 20min`,
/// `3min ago`, `now`.
pub fn relative(unix: i64, now: i64) -> String {
    let difference = unix - now;
    // Minutes are enough past an hour; seconds would only jitter.
    let rounded = |seconds: u64| {
        if seconds >= 3600 {
            duration(seconds / 60 * 60)
        } else {
            duration(seconds)
        }
    };
    match difference {
        0 => "now".to_string(),
        1.. => format!("in {}", rounded(difference.unsigned_abs())),
        _ => format!("{} ago", rounded(difference.unsigned_abs())),
    }
}

/// How a run ended in a few words: `success`, or systemd's result with the
/// exit status when there is one (`exit-code 1`, `timeout`).
pub fn outcome(run: &ScheduleRun) -> String {
    if run.succeeded() || run.status.is_empty() || run.status == "0" {
        run.result.clone()
    } else {
        format!("{} {}", run.result, run.status)
    }
}

/// Now, as seconds since the epoch, by this computer's clock.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_read_like_they_are_typed() {
        assert_eq!(parse_timeout("90"), Ok(90));
        assert_eq!(parse_timeout("90s"), Ok(90));
        assert_eq!(parse_timeout("10m"), Ok(600));
        assert_eq!(parse_timeout("1h30m"), Ok(5400));
        assert_eq!(parse_timeout(" 2h "), Ok(7200));
        assert_eq!(parse_timeout("none"), Ok(0));
        assert_eq!(parse_timeout("0"), Ok(0));
        assert!(parse_timeout("").is_err());
        assert!(parse_timeout("10x").is_err());
        assert!(parse_timeout("m").is_err());
    }

    #[test]
    fn durations_and_relative_times() {
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(600), "10min");
        assert_eq!(duration(605), "10min 5s");
        assert_eq!(duration(5400), "1h 30min");
        assert_eq!(duration(7200), "2h");
        assert_eq!(duration(183_600), "2d 3h");
        assert_eq!(relative(1_000 + 4_830, 1_000), "in 1h 20min");
        assert_eq!(relative(1_000 - 180, 1_000), "3min ago");
        assert_eq!(relative(1_000, 1_000), "now");
    }

    #[test]
    fn outcomes_name_the_status_only_when_it_says_something() {
        let run = |result: &str, status: &str| ScheduleRun {
            started: protocol::Moment {
                unix: 0,
                local: String::new(),
            },
            finished: protocol::Moment {
                unix: 0,
                local: String::new(),
            },
            result: result.to_string(),
            status: status.to_string(),
        };
        assert_eq!(outcome(&run("success", "0")), "success");
        assert_eq!(outcome(&run("exit-code", "1")), "exit-code 1");
        assert_eq!(outcome(&run("timeout", "")), "timeout");
    }
}
