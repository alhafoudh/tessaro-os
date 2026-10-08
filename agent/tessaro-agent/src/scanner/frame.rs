//! Where a scan begins and ends. Pure: the worker hands it what came from
//! the scanner and the time, and asks it when the quiet ends a scan.
//!
//! A scan begins with its first byte. It ends with its terminator (Enter, a
//! CR, a byte the scanner is set to send) or with `gap_ms` of quiet, which is
//! all a scanner set to send no terminator has. A HID POS scanner says
//! itself that a scan is complete (`finish`). The terminator is not part of
//! the scan; the prefix and suffix the scanner is set to add are taken off.

use std::time::{Duration, Instant};

use protocol::scanner::{Terminator, Transport, SCAN_MAX};

/// What came from the scanner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Byte(u8),
    /// A keyboard scanner's Enter or Tab key.
    Enter,
    Tab,
}

/// A scan that ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub bytes: Vec<u8>,
    /// From its first byte to its last.
    pub ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Begin,
    End(Done),
}

#[derive(Debug)]
pub struct Assembler {
    transport: Transport,
    terminator: Terminator,
    gap: Duration,
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    bytes: Vec<u8>,
    first: Option<Instant>,
    last: Option<Instant>,
    /// The last scan ended with a CR, so an LF right after it is that
    /// scan's CR LF, not a scan of its own.
    after_cr: bool,
}

impl Assembler {
    pub fn new(spec: &protocol::scanner::ScannerSpec) -> Self {
        Self {
            transport: spec.transport,
            terminator: spec.terminator(),
            gap: Duration::from_millis(u64::from(spec.gap_ms())),
            prefix: spec.strip_prefix.clone().unwrap_or_default().into_bytes(),
            suffix: spec.strip_suffix.clone().unwrap_or_default().into_bytes(),
            bytes: Vec::new(),
            first: None,
            last: None,
            after_cr: false,
        }
    }

    /// One piece of a scan, and what it begins or ends.
    pub fn push(&mut self, input: Input, now: Instant) -> Vec<Event> {
        let mut events = Vec::new();
        // A gap the worker has not looked at yet ends the scan before.
        if let Some(done) = self.expire(now) {
            events.push(Event::End(done));
        }
        let after_cr = std::mem::take(&mut self.after_cr);
        match (self.transport, input) {
            (Transport::Keyboard, Input::Enter) => match self.terminator {
                Terminator::Auto | Terminator::Enter => events.extend(self.finish(now)),
                _ => events.extend(self.add(b'\r', now)),
            },
            (Transport::Keyboard, Input::Tab) => match self.terminator {
                Terminator::Tab => events.extend(self.finish(now)),
                _ => events.extend(self.add(b'\t', now)),
            },
            (_, Input::Enter) => events.extend(self.add(b'\r', now)),
            (_, Input::Tab) => events.extend(self.add(b'\t', now)),
            (Transport::Serial, Input::Byte(byte)) => match (self.terminator, byte) {
                (Terminator::Auto, b'\n') if after_cr => {}
                (Terminator::Auto, b'\r') => {
                    events.extend(self.finish(now));
                    self.after_cr = true;
                }
                (Terminator::Auto, b'\n') | (Terminator::Cr, b'\r') | (Terminator::Lf, b'\n') => {
                    events.extend(self.finish(now))
                }
                (Terminator::CrLf, b'\n') if self.bytes.last() == Some(&b'\r') => {
                    self.bytes.pop();
                    events.extend(self.finish(now));
                }
                (Terminator::Byte(end), byte) if byte == end => events.extend(self.finish(now)),
                _ => events.extend(self.add(byte, now)),
            },
            (_, Input::Byte(byte)) => events.extend(self.add(byte, now)),
        }
        events
    }

    /// Several bytes at once, as one report or one read brings them.
    pub fn push_all(&mut self, bytes: &[u8], now: Instant) -> Vec<Event> {
        bytes
            .iter()
            .flat_map(|byte| self.push(Input::Byte(*byte), now))
            .collect()
    }

    /// The scanner says the scan is complete.
    pub fn finish(&mut self, now: Instant) -> Option<Event> {
        self.end(now).map(Event::End)
    }

    /// When the quiet ends the scan in progress, if one is.
    pub fn deadline(&self) -> Option<Instant> {
        self.last.map(|last| last + self.gap)
    }

    /// The scan in progress, ended if the quiet since its last piece is
    /// `gap_ms` or longer.
    pub fn expire(&mut self, now: Instant) -> Option<Done> {
        match self.deadline() {
            Some(deadline) if now >= deadline => self.end(now),
            _ => None,
        }
    }

    fn add(&mut self, byte: u8, now: Instant) -> Option<Event> {
        let begins = self.first.is_none();
        if begins {
            self.first = Some(now);
        }
        self.last = Some(now);
        if self.bytes.len() < SCAN_MAX {
            self.bytes.push(byte);
        }
        begins.then_some(Event::Begin)
    }

    /// End the scan in progress; nothing for a terminator alone.
    fn end(&mut self, _now: Instant) -> Option<Done> {
        let first = self.first.take()?;
        let last = self.last.take().unwrap_or(first);
        let mut bytes = std::mem::take(&mut self.bytes);
        if !self.prefix.is_empty() && bytes.starts_with(&self.prefix) {
            bytes.drain(..self.prefix.len());
        }
        if !self.suffix.is_empty() && bytes.ends_with(&self.suffix) {
            bytes.truncate(bytes.len() - self.suffix.len());
        }
        Some(Done {
            bytes,
            ms: last.duration_since(first).as_millis() as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::scanner::ScannerSpec;

    fn spec(transport: Transport, terminator: &str) -> ScannerSpec {
        ScannerSpec {
            name: "front".into(),
            transport,
            vendor: "0c2e".into(),
            product: "0b61".into(),
            serial: None,
            port: None,
            layout: None,
            terminator: Some(terminator.into()),
            gap_ms: Some(50),
            baud: None,
            strip_prefix: None,
            strip_suffix: None,
            enabled: true,
        }
    }

    fn ends(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::End(done) => Some(String::from_utf8_lossy(&done.bytes).to_string()),
                Event::Begin => None,
            })
            .collect()
    }

    fn feed(assembler: &mut Assembler, bytes: &[u8]) -> Vec<Event> {
        assembler.push_all(bytes, Instant::now())
    }

    #[test]
    fn a_keyboard_scan_begins_with_its_first_key_and_ends_with_enter() {
        let mut keys = Assembler::new(&spec(Transport::Keyboard, "auto"));
        let t0 = Instant::now();
        assert_eq!(keys.push(Input::Byte(b'4'), t0), vec![Event::Begin]);
        assert!(keys
            .push(Input::Byte(b'2'), t0 + Duration::from_millis(8))
            .is_empty());
        let events = keys.push(Input::Enter, t0 + Duration::from_millis(16));
        assert_eq!(
            events,
            vec![Event::End(Done {
                bytes: b"42".to_vec(),
                ms: 8
            })]
        );
        // Enter alone is no scan.
        assert!(keys
            .push(Input::Enter, t0 + Duration::from_millis(20))
            .is_empty());
    }

    #[test]
    fn a_keyboard_scanner_ending_with_tab_types_its_enter() {
        let mut keys = Assembler::new(&spec(Transport::Keyboard, "tab"));
        let now = Instant::now();
        keys.push(Input::Byte(b'a'), now);
        keys.push(Input::Enter, now);
        keys.push(Input::Byte(b'b'), now);
        assert_eq!(ends(&keys.push(Input::Tab, now)), vec!["a\rb"]);
    }

    #[test]
    fn serial_auto_ends_on_cr_or_lf_and_crlf_is_one_end() {
        let mut serial = Assembler::new(&spec(Transport::Serial, "auto"));
        assert_eq!(
            ends(&feed(&mut serial, b"123\r\n456\n789\r")),
            vec!["123", "456", "789"]
        );
    }

    #[test]
    fn serial_terminators_are_taken_literally() {
        let mut cr = Assembler::new(&spec(Transport::Serial, "cr"));
        assert_eq!(ends(&feed(&mut cr, b"a\nb\r")), vec!["a\nb"]);
        let mut crlf = Assembler::new(&spec(Transport::Serial, "crlf"));
        assert_eq!(ends(&feed(&mut crlf, b"a\rb\r\n")), vec!["a\rb"]);
        let mut etx = Assembler::new(&spec(Transport::Serial, "0x03"));
        assert_eq!(ends(&feed(&mut etx, b"\x02abc\x03")), vec!["\x02abc"]);
        let mut none = Assembler::new(&spec(Transport::Serial, "none"));
        assert_eq!(ends(&feed(&mut none, b"a\rb\n")), Vec::<String>::new());
    }

    #[test]
    fn the_quiet_ends_a_scan_without_a_terminator() {
        let mut none = Assembler::new(&spec(Transport::Serial, "none"));
        let t0 = Instant::now();
        none.push_all(b"abc", t0);
        assert_eq!(none.deadline(), Some(t0 + Duration::from_millis(50)));
        assert_eq!(none.expire(t0 + Duration::from_millis(49)), None);
        let done = none.expire(t0 + Duration::from_millis(50)).unwrap();
        assert_eq!(done.bytes, b"abc");
        assert_eq!(none.deadline(), None);
        // A piece after the gap is a new scan, the old one ended first.
        none.push_all(b"x", t0 + Duration::from_millis(60));
        let events = none.push(Input::Byte(b'y'), t0 + Duration::from_millis(200));
        assert_eq!(ends(&events), vec!["x"]);
        assert_eq!(events.last(), Some(&Event::Begin));
    }

    #[test]
    fn the_prefix_and_suffix_are_taken_off() {
        let mut typed = spec(Transport::Serial, "cr");
        typed.strip_prefix = Some("]C1".into());
        typed.strip_suffix = Some("!".into());
        let mut serial = Assembler::new(&typed);
        assert_eq!(
            ends(&feed(&mut serial, b"]C1(01)0950\r(01)!\r")),
            vec!["(01)0950", "(01)"]
        );
    }

    #[test]
    fn a_scan_is_kept_up_to_its_limit() {
        let mut serial = Assembler::new(&spec(Transport::Serial, "cr"));
        let mut bytes = vec![b'x'; SCAN_MAX + 10];
        bytes.push(b'\r');
        let events = feed(&mut serial, &bytes);
        match events.last() {
            Some(Event::End(done)) => assert_eq!(done.bytes.len(), SCAN_MAX),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_hid_pos_scan_ends_when_the_scanner_says() {
        let mut hidpos = Assembler::new(&spec(Transport::Hidpos, "auto"));
        let now = Instant::now();
        hidpos.push_all(b"\rab", now);
        match hidpos.finish(now) {
            Some(Event::End(done)) => assert_eq!(done.bytes, b"\rab"),
            other => panic!("{other:?}"),
        }
        assert_eq!(hidpos.finish(now), None);
    }
}
