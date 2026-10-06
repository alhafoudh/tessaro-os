//! Progress lines for anything that moves many bytes: an image upload and
//! its preparation, files sent or received. The flows themselves are in
//! `tessaro_client` and report here through `Report`.

use std::io::{IsTerminal, Write};

use tessaro_client::report::Report;
use tessaro_client::text::Line;

use crate::out::{self, eprint, eprintln};
use crate::style;

/// Progress on stderr. On a terminal one line redraws itself; otherwise,
/// and with `--json`, a line per tenth, so a log stays readable. For one of
/// several devices only the end of each step is kept: the output is printed
/// whole afterwards, when the progress is over (`out::capture`).
///
/// The text goes through anstream, which drops its colors when they are
/// off; the `\r` and clear-to-end-of-line around it go straight to stderr,
/// since the redraw needs them even under `--color never`.
pub(crate) struct Progress {
    redraw: bool,
    kept: bool,
    shown: Option<u64>,
}

impl Progress {
    pub(crate) fn new(json: bool) -> Self {
        let kept = out::capturing();
        Self {
            redraw: !json && !kept && std::io::stderr().is_terminal(),
            kept,
            shown: None,
        }
    }

    pub(crate) fn show(&mut self, line: &str, done: u64, total: u64) {
        if self.kept {
            return;
        }
        if self.redraw {
            Self::redraw(line, "");
            return;
        }
        let tenth = done * 10 / total.max(1);
        if self.shown != Some(tenth) {
            self.shown = Some(tenth);
            eprintln!("{line}");
        }
    }

    /// A step is over: its last line stays.
    pub(crate) fn done(&mut self, line: &str) {
        if self.redraw {
            Self::redraw(line, "\n");
        } else {
            eprintln!("{line}");
        }
        self.shown = None;
    }

    /// Overwrite the current terminal line with `line`, then `end`.
    fn redraw(line: &str, end: &str) {
        let mut raw = std::io::stderr();
        let _ = write!(raw, "\r");
        eprint!("{line}");
        let _ = write!(raw, "\x1b[K{end}");
        let _ = raw.flush();
    }
}

impl Report for Progress {
    fn progress(&mut self, line: Line, done: u64, total: u64) {
        self.show(&style::line(&line), done, total);
    }

    fn line(&mut self, line: Line) {
        self.done(&style::line(&line));
    }
}
