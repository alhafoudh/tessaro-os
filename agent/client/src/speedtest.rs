//! The device's speed test, for `tessaro-ctl network speedtest` and the
//! GUI's Network page alike: the sizes it offers and how its steps read.

use protocol::{speedtest_size_label, Direction, SpeedtestEvent, SPEEDTEST_SIZES};

use crate::text::{Line, Tone};

/// The width of a line's label: `server`, `latency`, `download`.
const LABEL: usize = 9;

/// A size the device offers, as `100k`, `1m`, ...
pub fn parse_size(text: &str) -> Result<u64, String> {
    SPEEDTEST_SIZES
        .into_iter()
        .find(|size| speedtest_size_label(*size) == text.to_ascii_lowercase())
        .ok_or_else(|| format!("one of {}", size_labels().join(", ")))
}

/// Every size the device offers, as `parse_size` reads them.
pub fn size_labels() -> Vec<String> {
    SPEEDTEST_SIZES.map(speedtest_size_label).into()
}

/// One step of the job.
pub fn event_line(step: &SpeedtestEvent) -> Line {
    // The headline number in its tone, a missing one muted; the spread and
    // the sample counts are background.
    let value = |tone: Tone, v: Option<f64>, unit: &str| match v {
        Some(v) => Line::of(tone, format!("{v:.1} {unit}")),
        None => Line::of(Tone::Muted, "n/a"),
    };
    let number = |v: Option<f64>, unit: &str| match v {
        Some(v) => format!("{v:.1} {unit}"),
        None => "n/a".to_string(),
    };
    let label = |text: &str| Line::new().pad(Tone::Label, text, LABEL).text(" ");
    match step {
        SpeedtestEvent::Server { ip, colo, country } => label("server")
            .text("Cloudflare ")
            .add(Tone::Heading, colo)
            .text(format!(", seen from {ip} ({country})")),
        SpeedtestEvent::Latency {
            samples,
            avg_ms,
            min_ms,
            max_ms,
        } => label("latency")
            .join(value(Tone::Heading, *avg_ms, "ms"))
            .text(" ")
            .add(
                Tone::Muted,
                format!(
                    "(min {}, max {}, {samples} samples)",
                    number(*min_ms, "ms"),
                    number(*max_ms, "ms")
                ),
            ),
        SpeedtestEvent::Transfer {
            direction,
            size,
            samples,
            attempts,
            median_mbit,
            min_mbit,
            max_mbit,
        } => {
            let direction = match direction {
                Direction::Download => "download",
                Direction::Upload => "upload",
            };
            // Samples short of the attempts means retries: worth noticing.
            let counted = if samples < attempts {
                Tone::Warn
            } else {
                Tone::Muted
            };
            label(direction)
                .pad(Tone::Heading, speedtest_size_label(*size), 5)
                .text(" ")
                .join(value(Tone::Heading, *median_mbit, "Mbit/s"))
                .text(" ")
                .add(
                    Tone::Muted,
                    format!(
                        "(min {}, max {},",
                        number(*min_mbit, "Mbit/s"),
                        number(*max_mbit, "Mbit/s")
                    ),
                )
                .text(" ")
                .add(counted, format!("{samples}/{attempts} samples)"))
        }
        SpeedtestEvent::Result {
            download_mbit,
            upload_mbit,
            latency_ms,
        } => Line::new()
            .pad(Tone::Heading, "result", LABEL)
            .text(" download ")
            .join(value(Tone::Ok, *download_mbit, "Mbit/s"))
            .text(", upload ")
            .join(value(Tone::Ok, *upload_mbit, "Mbit/s"))
            .text(", latency ")
            .join(value(Tone::Ok, *latency_ms, "ms")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_back() {
        for label in size_labels() {
            assert_eq!(speedtest_size_label(parse_size(&label).unwrap()), label);
        }
        assert!(parse_size("5m").is_err());
    }

    #[test]
    fn max_size_takes_the_offered_sizes_only() {
        assert_eq!(parse_size("25M"), Ok(25_000_000));
        assert_eq!(parse_size("100k"), Ok(100_000));
    }

    #[test]
    fn steps_read_as_aligned_lines() {
        let transfer = event_line(&SpeedtestEvent::Transfer {
            direction: Direction::Upload,
            size: 1_000_000,
            samples: 3,
            attempts: 4,
            median_mbit: Some(42.5),
            min_mbit: Some(40.0),
            max_mbit: None,
        });
        assert_eq!(
            transfer.to_string(),
            "upload    1m    42.5 Mbit/s (min 40.0 Mbit/s, max n/a, 3/4 samples)"
        );
        assert_eq!(transfer.tone(), Tone::Warn);
        assert_eq!(
            event_line(&SpeedtestEvent::Result {
                download_mbit: Some(93.14),
                upload_mbit: None,
                latency_ms: Some(12.0),
            })
            .to_string(),
            "result    download 93.1 Mbit/s, upload n/a, latency 12.0 ms"
        );
    }
}
