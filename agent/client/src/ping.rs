//! Pings, for `tessaro-ctl device ping`, `tessaro-ctl network ping` and the
//! GUI's pages alike: the round trips over the API connection itself, and
//! the lines a device's `network ping` job reads as.

use std::time::{Duration, Instant};

use protocol::api;
use protocol::{PingEvent, PING_MAX_COUNT};

use crate::connect::Session;
use crate::report::Report;
use crate::text::{Line, Tone};

/// The width of a ping line's label: `reply`, `timeout`, `result`.
const LABEL: usize = 9;

/// What `device` measured.
pub struct Summary {
    pub sent: u32,
    pub rtts: Vec<Duration>,
}

impl Summary {
    pub fn received(&self) -> u32 {
        self.rtts.len() as u32
    }

    fn millis(&self) -> impl Iterator<Item = f64> + '_ {
        self.rtts.iter().map(|rtt| rtt.as_secs_f64() * 1000.0)
    }

    pub fn rtt_ms(&self) -> Vec<f64> {
        self.millis().collect()
    }

    pub fn min_ms(&self) -> Option<f64> {
        self.millis().reduce(f64::min)
    }

    pub fn max_ms(&self) -> Option<f64> {
        self.millis().reduce(f64::max)
    }

    pub fn avg_ms(&self) -> Option<f64> {
        (!self.rtts.is_empty()).then(|| self.millis().sum::<f64>() / self.rtts.len() as f64)
    }

    /// `result    3/4 answered, 25% lost (min 1.0, avg 1.2, max 1.5 ms)`.
    pub fn line(&self) -> Line {
        summary_line(
            self.sent,
            self.received(),
            self.min_ms(),
            self.avg_ms(),
            self.max_ms(),
            "answered",
        )
    }
}

/// What `device` pings, and how long opening the session took: the TCP
/// connect, and the TLS handshake with asking the device who it is.
pub fn intro(session: &Session) -> Vec<Line> {
    let mut lines = vec![Line::of(
        Tone::Muted,
        match session.remote.as_ref() {
            Some((address, _)) => format!("{} ({address}): the API connection", session.node.name),
            None => format!("{}: the local socket", session.node.name),
        },
    )];
    if let Some(timing) = session.timing {
        lines.push(label("connect").add(Tone::Heading, ms(timing.connect)));
        lines.push(
            label("tls")
                .add(Tone::Heading, ms(timing.handshake))
                .text(" ")
                .add(Tone::Muted, "(handshake and device id)"),
        );
    }
    lines
}

/// `count` round trips on the session, `interval` apart, each reported as a
/// line. A lost one is reported and the pinging goes on, like `ping`'s.
pub fn device(
    session: &mut Session,
    count: u32,
    interval: Duration,
    report: &mut dyn Report,
) -> Result<Summary, String> {
    check_count(count)?;
    let mut summary = Summary {
        sent: 0,
        rtts: Vec::new(),
    };
    for seq in 1..=count {
        if seq > 1 {
            let until = Instant::now() + interval.max(Duration::from_millis(50));
            while Instant::now() < until && !report.stopped() {
                std::thread::sleep(Duration::from_millis(50).min(until - Instant::now()));
            }
        }
        if report.stopped() {
            break;
        }
        let started = Instant::now();
        summary.sent = seq;
        match session.fetch::<api::device::Ping>() {
            Ok(_) => {
                let rtt = started.elapsed();
                summary.rtts.push(rtt);
                report.line(
                    label("reply")
                        .add(Tone::Heading, ms(rtt))
                        .text(" ")
                        .add(Tone::Muted, format!("seq={seq}")),
                );
            }
            Err(error) => report.line(
                label("lost")
                    .add(Tone::Bad, error)
                    .text(" ")
                    .add(Tone::Muted, format!("seq={seq}")),
            ),
        }
        report.progress(
            Line::of(Tone::Label, "pinging"),
            u64::from(seq),
            u64::from(count),
        );
    }
    Ok(summary)
}

/// A ping count both clients accept: at least one, at most what the
/// device's own `network ping` takes.
pub fn check_count(count: u32) -> Result<(), String> {
    if count == 0 {
        return Err("the count must be at least 1".to_string());
    }
    if count > PING_MAX_COUNT {
        return Err(format!("the count can be at most {PING_MAX_COUNT}"));
    }
    Ok(())
}

/// One step of a device's `network ping` job.
pub fn event_line(step: &PingEvent) -> Line {
    match step {
        PingEvent::Start { host, address } => {
            let line = label("ping").add(Tone::Heading, host);
            if host == address {
                line
            } else {
                line.text(format!(" ({address})"))
            }
        }
        PingEvent::Reply { seq, bytes, rtt_ms } => label("reply")
            .add(Tone::Heading, format!("{rtt_ms:.1} ms"))
            .text(" ")
            .add(Tone::Muted, format!("seq={seq} {bytes} bytes")),
        PingEvent::Timeout { seq } => label("timeout")
            .add(Tone::Warn, "no reply")
            .text(" ")
            .add(Tone::Muted, format!("seq={seq}")),
        PingEvent::Summary {
            sent,
            received,
            min_ms,
            avg_ms,
            max_ms,
        } => summary_line(*sent, *received, *min_ms, *avg_ms, *max_ms, "received"),
    }
}

/// `result    3/4 VERB, 25% lost (min 1.0, avg 1.2, max 1.5 ms)`.
pub fn summary_line(
    sent: u32,
    received: u32,
    min: Option<f64>,
    avg: Option<f64>,
    max: Option<f64>,
    verb: &str,
) -> Line {
    let loss = ((sent - received.min(sent)) * 100)
        .checked_div(sent)
        .unwrap_or(0);
    let loss_tone = match loss {
        0 => Tone::Ok,
        100 => Tone::Bad,
        _ => Tone::Warn,
    };
    let number = |v: Option<f64>| match v {
        Some(v) => format!("{v:.1}"),
        None => "n/a".to_string(),
    };
    let line = Line::new()
        .pad(Tone::Heading, "result", LABEL)
        .text(format!(" {received}/{sent} {verb}, "))
        .add(loss_tone, format!("{loss}% lost"));
    match (min, avg, max) {
        (None, None, None) => line,
        _ => line.text(" ").add(
            Tone::Muted,
            format!(
                "(min {}, avg {}, max {} ms)",
                number(min),
                number(avg),
                number(max)
            ),
        ),
    }
}

/// A line's label in its column, and the space after it.
fn label(text: &str) -> Line {
    Line::new().pad(Tone::Label, text, LABEL).text(" ")
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_counts_what_was_lost() {
        let summary = Summary {
            sent: 4,
            rtts: vec![
                Duration::from_millis(1),
                Duration::from_millis(2),
                Duration::from_millis(3),
            ],
        };
        assert_eq!(
            summary.line().to_string(),
            "result    3/4 answered, 25% lost (min 1.0, avg 2.0, max 3.0 ms)"
        );
        assert_eq!(summary.line().tone(), Tone::Warn);
    }

    #[test]
    fn no_replies_have_no_spread() {
        assert_eq!(
            summary_line(2, 0, None, None, None, "received").to_string(),
            "result    0/2 received, 100% lost"
        );
    }

    #[test]
    fn counts_are_bounded() {
        assert!(check_count(0).is_err());
        assert!(check_count(1).is_ok());
        assert!(check_count(PING_MAX_COUNT + 1).is_err());
    }

    #[test]
    fn job_steps_read_as_ping_prints_them() {
        let reply = PingEvent::Reply {
            seq: 2,
            bytes: 64,
            rtt_ms: 0.84,
        };
        assert_eq!(
            event_line(&reply).to_string(),
            "reply     0.8 ms seq=2 64 bytes"
        );
        let summary = PingEvent::Summary {
            sent: 4,
            received: 3,
            min_ms: Some(0.5),
            avg_ms: Some(1.0),
            max_ms: Some(2.0),
        };
        assert_eq!(
            event_line(&summary).to_string(),
            "result    3/4 received, 25% lost (min 0.5, avg 1.0, max 2.0 ms)"
        );
    }

    #[test]
    fn a_start_names_the_address_when_it_differs() {
        let step = PingEvent::Start {
            host: "example.com".into(),
            address: "93.184.216.34".into(),
        };
        assert_eq!(
            event_line(&step).to_string(),
            "ping      example.com (93.184.216.34)"
        );
    }
}
