//! The words for what a device's clock reports (`protocol::TimeStatus`),
//! shared by `tessaro-ctl time show` and the GUI's Time page so they say
//! it the same way. Formatting only: every number is the device's.

/// A duration in microseconds, in the largest unit that keeps it readable:
/// `850µs`, `12.3ms`, `4.51s`, `34min 8s`.
pub fn span(usec: u64) -> String {
    match usec {
        0..1_000 => format!("{usec}µs"),
        1_000..1_000_000 => format!("{:.1}ms", usec as f64 / 1_000.0),
        1_000_000..60_000_000 => format!("{:.2}s", usec as f64 / 1_000_000.0),
        _ => {
            let seconds = usec / 1_000_000;
            let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
            match (hours, seconds) {
                (0, 0) => format!("{minutes}min"),
                (0, _) => format!("{minutes}min {seconds}s"),
                _ => format!("{hours}h {minutes}min"),
            }
        }
    }
}

/// A clock's offset from its NTP server, signed: `+1.2ms` is behind it.
pub fn offset(usec: i64) -> String {
    let sign = if usec < 0 { '-' } else { '+' };
    format!("{sign}{}", span(usec.unsigned_abs()))
}

/// How far the hardware clock is from the system clock, each as reported.
pub fn offset_between(rtc: u64, now: u64) -> String {
    if rtc >= now {
        format!("{} ahead", span(rtc - now))
    } else {
        format!("{} behind", span(now - rtc))
    }
}

/// Seconds east of UTC as `UTC+02:00`.
pub fn utc_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// NTP's precision, a power of two seconds: `-25` as `2^-25, 30ns`.
pub fn precision(log2: i32) -> String {
    let nanos = 2f64.powi(log2) * 1e9;
    let readable = if nanos < 1_000.0 {
        format!("{nanos:.0}ns")
    } else {
        span((nanos / 1_000.0).round() as u64)
    };
    format!("2^{log2}, {readable}")
}

/// NTP's leap indicator, for a value other than 0 (no warning).
pub fn leap(code: u32) -> &'static str {
    match code {
        0 => "none",
        1 => "a leap second is added at the end of the day",
        2 => "a leap second is removed at the end of the day",
        _ => "the server says it is not synchronized",
    }
}

/// timesyncd's frequency correction as drift: `-12.345 ppm`.
pub fn drift(ppm: f64) -> String {
    format!("{ppm:+.3} ppm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_pick_a_readable_unit() {
        assert_eq!(span(850), "850µs");
        assert_eq!(span(12_345), "12.3ms");
        assert_eq!(span(4_512_000), "4.51s");
        assert_eq!(span(2_048_000_000), "34min 8s");
        assert_eq!(span(1_920_000_000), "32min");
        assert_eq!(span(7_200_000_000), "2h 0min");
        assert_eq!(offset(-1_234), "-1.2ms");
        assert_eq!(offset(500), "+500µs");
    }

    #[test]
    fn utc_offsets_precision_and_drift() {
        assert_eq!(utc_offset(7200), "UTC+02:00");
        assert_eq!(utc_offset(-12_600), "UTC-03:30");
        assert_eq!(precision(-25), "2^-25, 30ns");
        assert_eq!(precision(-10), "2^-10, 977µs");
        assert_eq!(drift(-12.3454), "-12.345 ppm");
    }

    #[test]
    fn the_hardware_clock_is_compared_to_the_system_clock() {
        assert_eq!(offset_between(1_000_300, 1_000_000), "300µs ahead");
        assert_eq!(offset_between(1_000_000, 3_000_000), "2.00s behind");
    }
}
