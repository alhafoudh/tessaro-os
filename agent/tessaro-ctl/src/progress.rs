//! Progress lines for anything that moves many bytes: an image upload and
//! its preparation, files sent or received.

use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use crate::style::{self, pad, paint};

/// A progress line: the step's verb in its own column, then the details.
pub(crate) fn step_line(
    verb_style: anstyle::Style,
    verb: &str,
    rest: impl std::fmt::Display,
) -> String {
    format!("{} {rest}", pad(verb_style, verb, 10))
}

/// Progress on stderr. On a terminal one line redraws itself; otherwise,
/// and with `--json`, a line per tenth, so a log stays readable.
///
/// The text goes through anstream, which drops its colors when they are
/// off; the `\r` and clear-to-end-of-line around it go straight to stderr,
/// since the redraw needs them even under `--color never`.
pub(crate) struct Progress {
    redraw: bool,
    shown: Option<u64>,
}

impl Progress {
    pub(crate) fn new(json: bool) -> Self {
        Self {
            redraw: !json && std::io::stderr().is_terminal(),
            shown: None,
        }
    }

    pub(crate) fn show(&mut self, line: &str, done: u64, total: u64) {
        if self.redraw {
            Self::redraw(line, "");
            return;
        }
        let tenth = done * 10 / total.max(1);
        if self.shown != Some(tenth) {
            self.shown = Some(tenth);
            let _ = writeln!(anstream::stderr(), "{line}");
        }
    }

    /// A step is over: its last line stays.
    pub(crate) fn done(&mut self, line: &str) {
        if self.redraw {
            Self::redraw(line, "\n");
        } else {
            let _ = writeln!(anstream::stderr(), "{line}");
        }
        self.shown = None;
    }

    /// Overwrite the current terminal line with `line`, then `end`.
    fn redraw(line: &str, end: &str) {
        let mut raw = std::io::stderr();
        let _ = write!(raw, "\r");
        let _ = write!(anstream::stderr(), "{line}");
        let _ = write!(raw, "\x1b[K{end}");
        let _ = raw.flush();
    }
}

/// Upload speed over the last few seconds, and what it means for the rest.
pub(crate) struct Rate {
    pub(crate) started: Instant,
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    const WINDOW: Duration = Duration::from_secs(5);

    pub(crate) fn new(from: u64) -> Self {
        let now = Instant::now();
        Self {
            started: now,
            samples: VecDeque::from([(now, from)]),
        }
    }

    /// Bytes per second, and the time left as text.
    pub(crate) fn update(&mut self, done: u64, total: u64) -> (f64, String) {
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
    pub(crate) fn line(&mut self, done: u64, total: u64) -> String {
        let (speed, eta) = self.update(done, total);
        format!(
            "{}/{}  {:>3}%  {}/s  {} {eta}",
            mb(done),
            mb(total),
            percent(done, total),
            mb(speed as u64),
            paint(style::LABEL, "ETA"),
        )
    }
}

pub(crate) fn clock(duration: Duration) -> String {
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

pub(crate) fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

pub(crate) fn percent(done: u64, total: u64) -> u64 {
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
        let plain = anstream::adapter::strip_str(&line).to_string();
        assert_eq!(plain, "0.0 MB/10.0 MB    0%  0.0 MB/s  ETA --:--");
    }
}
