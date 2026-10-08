//! What happened to the scanners: each one plugged in, out or failing, and
//! each scan's length and duration, never what it said, for `scanner logs`
//! and the consoles. The last `KEPT` entries, with a `seq` that only grows,
//! as in the CEC message log (`cec::log`).

use std::collections::VecDeque;

use protocol::scanner::{ScanLog, ScanLogEntry};

pub const KEPT: usize = 500;

#[derive(Debug, Default)]
pub struct Log {
    next: u64,
    kept: VecDeque<ScanLogEntry>,
}

/// One entry before it is kept: what happened, to which scanner.
#[derive(Debug, Default)]
pub struct Entry<'a> {
    pub scanner: &'a str,
    pub event: &'a str,
    pub length: Option<u32>,
    pub ms: Option<u64>,
    pub symbology: Option<String>,
    pub message: Option<String>,
}

impl Log {
    pub fn push(&mut self, entry: Entry<'_>, at_ms: i64) {
        self.next += 1;
        if self.kept.len() == KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back(ScanLogEntry {
            seq: self.next,
            at_ms,
            time: String::new(),
            scanner: entry.scanner.to_string(),
            event: entry.event.to_string(),
            length: entry.length,
            ms: entry.ms,
            symbology: entry.symbology,
            message: entry.message,
        });
    }

    /// Every kept entry after `after`, and the `seq` to ask after next.
    pub fn page(&self, after: u64) -> ScanLog {
        ScanLog {
            entries: self
                .kept
                .iter()
                .filter(|entry| entry.seq > after)
                .cloned()
                .collect(),
            next: self.next,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_everything_after_a_seq_that_only_grows() {
        let mut log = Log::default();
        for at in 0..KEPT as i64 + 3 {
            log.push(
                Entry {
                    scanner: "front",
                    event: "scan",
                    length: Some(13),
                    ms: Some(40),
                    ..Default::default()
                },
                at,
            );
        }
        let page = log.page(0);
        assert_eq!(page.entries.len(), KEPT);
        assert_eq!(page.entries[0].seq, 4);
        assert_eq!(page.next, KEPT as u64 + 3);
        assert!(log.page(page.next).entries.is_empty());
    }
}
