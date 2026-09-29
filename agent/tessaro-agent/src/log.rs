//! Diagnostics are journal-only by design; nothing technical ever reaches the
//! screen. stderr *is* the journal - systemd captures it under
//! `SyslogIdentifier=tessaro-agent`.
//!
//! Deliberately no timestamps and no level prefixes: journald already stamps
//! every line with a clock, a unit and a priority, and duplicating that just
//! makes `journalctl` output twice as wide. The message is the whole line.

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::Mutex;

enum Sink {
    Stderr,
    /// Used by tests, so a case can assert on what was logged without the
    /// suite spraying the terminal. Compiled out of the shipped binary, which
    /// only ever writes to the journal.
    #[cfg(test)]
    Buffer(Mutex<Vec<String>>),
}

pub struct Log {
    /// `agent.debug`, which the control plane switches on a running agent.
    debug: AtomicBool,
    sink: Sink,
}

impl Log {
    pub fn new(debug: bool) -> Self {
        Self {
            debug: AtomicBool::new(debug),
            sink: Sink::Stderr,
        }
    }

    #[cfg(test)]
    pub fn buffered(debug: bool) -> Self {
        Self {
            debug: AtomicBool::new(debug),
            sink: Sink::Buffer(Mutex::new(Vec::new())),
        }
    }

    pub fn set_debug(&self, debug: bool) {
        self.debug.store(debug, Ordering::Relaxed);
    }

    pub fn info(&self, message: impl Display) {
        self.write(message);
    }

    pub fn debug(&self, message: impl Display) {
        if self.debug.load(Ordering::Relaxed) {
            self.write(message);
        }
    }

    fn write(&self, message: impl Display) {
        match &self.sink {
            Sink::Stderr => eprintln!("{message}"),
            #[cfg(test)]
            Sink::Buffer(lines) => lines
                .lock()
                .expect("log buffer poisoned")
                .push(message.to_string()),
        }
    }

    /// Everything logged so far. Only meaningful for a buffered log.
    #[cfg(test)]
    pub fn lines(&self) -> Vec<String> {
        match &self.sink {
            Sink::Stderr => Vec::new(),
            Sink::Buffer(lines) => lines.lock().expect("log buffer poisoned").clone(),
        }
    }
}
