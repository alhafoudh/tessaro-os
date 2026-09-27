//! Progress of a flow that takes a while, told to whoever runs it.
//!
//! A shared flow (`update::send`, `files::upload_tree`, `ping::device`)
//! never prints: it reports to a `Report`. The ctl's is its progress line on
//! stderr, the GUI's sends the job's events to its page. A flow checks
//! `stopped` between steps, so the GUI's Cancel ends it there.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::text::{Line, Tone};
use crate::transfer::mb;

pub trait Report {
    /// Where a step is: `line` says what, `done` of `total` how far.
    fn progress(&mut self, line: Line, done: u64, total: u64);
    /// A line that stays: a step is over, or something worth telling.
    fn line(&mut self, line: Line);
    /// The user gave up; the flow ends at the next step.
    fn stopped(&self) -> bool {
        false
    }
}

/// A progress line: the step's verb in its own column, then the details.
pub fn step_line(tone: Tone, verb: &str, rest: impl Into<Line>) -> Line {
    Line::new().pad(tone, verb, 10).text(" ").join(rest.into())
}

/// Speed over the last few seconds, and what it means for the rest.
pub struct Rate {
    pub started: Instant,
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    const WINDOW: Duration = Duration::from_secs(5);

    pub fn new(from: u64) -> Self {
        let now = Instant::now();
        Self {
            started: now,
            samples: VecDeque::from([(now, from)]),
        }
    }

    /// Bytes per second, and the time left as text.
    pub fn update(&mut self, done: u64, total: u64) -> (f64, String) {
        let now = Instant::now();
        self.samples.push_back((now, done));
        while self.samples.len() > 2 && now.duration_since(self.samples[0].0) > Self::WINDOW {
            self.samples.pop_front();
        }
        let (then, before) = self.samples[0];
        let seconds = now.duration_since(then).as_secs_f64();
        if seconds <= 0.0 || done <= before {
            return (0.0, "--:--".to_string());
        }
        let speed = (done - before) as f64 / seconds;
        let left = Duration::from_secs_f64(total.saturating_sub(done) as f64 / speed);
        (speed, clock(left))
    }

    /// `done` of `total` with the rate folded in:
    /// `12.0 MB/40.0 MB   30%  4.1 MB/s  ETA 0:07`.
    pub fn line(&mut self, done: u64, total: u64) -> Line {
        let (speed, eta) = self.update(done, total);
        Line::plain(format!(
            "{}/{}  {:>3}%  {}/s  ",
            mb(done),
            mb(total),
            percent(done, total),
            mb(speed as u64),
        ))
        .add(Tone::Label, "ETA")
        .text(format!(" {eta}"))
    }
}

/// A duration as a stopwatch shows it: `1:15`, `1:02:05`.
pub fn clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

pub fn percent(done: u64, total: u64) -> u64 {
    done * 100 / total.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn the_rate_needs_two_samples() {
        let mut rate = Rate::new(0);
        rate.samples[0].0 -= Duration::from_secs(2);
        let (speed, eta) = rate.update(2_000_000, 10_000_000);
        assert!((900_000.0..1_100_000.0).contains(&speed), "{speed}");
        assert_eq!(eta, "0:08");
    }

    #[test]
    fn a_line_before_any_progress_has_no_eta() {
        let line = Rate::new(0).line(0, 10_000_000);
        assert_eq!(
            line.to_string(),
            "0.0 MB/10.0 MB    0%  0.0 MB/s  ETA --:--"
        );
    }
}
