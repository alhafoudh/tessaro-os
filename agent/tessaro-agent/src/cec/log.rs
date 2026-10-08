//! The message log: the last `KEPT` messages of every adapter's bus, both
//! ways, for `screen cec messages`, the consoles and the page.
//!
//! Each message gets the next `seq`, which only grows, so a reader asking
//! for what came `after` the last one it saw never misses or repeats one,
//! however many were dropped off the front meanwhile. The wall-clock time a
//! page shows is filled in when it is read (`time`): turning a moment into
//! local time reads `/etc/localtime`, which the bus's loop must not.

use std::collections::VecDeque;

use protocol::{CecDirection, CecMessage, CecMessages};

use super::bus::Msg;

/// The most messages kept. The TV's power is asked every 10s, so this is
/// at least the last half hour of a quiet bus.
pub const KEPT: usize = 500;

#[derive(Debug, Default)]
pub struct Log {
    next: u64,
    kept: VecDeque<CecMessage>,
}

impl Log {
    /// Keep one message, and answer it as it was kept.
    pub fn push(
        &mut self,
        device: &str,
        direction: CecDirection,
        msg: &Msg,
        acked: Option<bool>,
        at_ms: i64,
    ) -> CecMessage {
        self.next += 1;
        let data: Vec<u8> = msg
            .opcode
            .into_iter()
            .chain(msg.args.iter().copied())
            .collect();
        let message = CecMessage {
            seq: self.next,
            at_ms,
            time: String::new(),
            device: device.to_string(),
            direction,
            from: msg.from,
            to: msg.to,
            data: protocol::cec::hex(&data),
            acked,
        };
        if self.kept.len() == KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back(message.clone());
        message
    }

    /// Every kept message after `after`, and the `seq` to ask after next.
    pub fn page(&self, after: u64) -> CecMessages {
        CecMessages {
            messages: self
                .kept
                .iter()
                .filter(|message| message.seq > after)
                .cloned()
                .collect(),
            next: self.next,
        }
    }
}

/// Now, in milliseconds since the epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// `21:04:05.123` on the device's clock. Reads `/etc/localtime`: call it
/// from `blocking`.
pub fn time(at_ms: i64) -> String {
    let Ok(usec) = u64::try_from(at_ms.saturating_mul(1000)) else {
        return String::new();
    };
    match crate::time::local_clock(usec) {
        Some(local) => format!(
            "{}.{:03}",
            local.text.rsplit(' ').next().unwrap_or_default(),
            at_ms.rem_euclid(1000)
        ),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(opcode: u8) -> Msg {
        Msg::new(0, 4, opcode, &[1])
    }

    #[test]
    fn a_page_is_everything_after_a_seq_that_only_grows() {
        let mut log = Log::default();
        log.push("/dev/cec0", CecDirection::In, &msg(0x90), None, 1);
        let out = log.push(
            "/dev/cec0",
            CecDirection::Out,
            &Msg::poll(4, 0),
            Some(true),
            2,
        );
        assert_eq!((out.seq, out.data.as_str(), out.acked), (2, "", Some(true)));

        let all = log.page(0);
        assert_eq!(all.messages.len(), 2);
        assert_eq!(all.messages[0].data, "90 01");
        assert_eq!(all.next, 2);
        let rest = log.page(all.next);
        assert!(rest.messages.is_empty());
        assert_eq!(rest.next, 2);
    }

    #[test]
    fn a_cursor_survives_messages_dropped_off_the_front() {
        let mut log = Log::default();
        for at in 0..KEPT as i64 + 10 {
            log.push("/dev/cec0", CecDirection::In, &msg(0x36), None, at);
        }
        let page = log.page(5);
        assert_eq!(page.messages.len(), KEPT);
        assert_eq!(page.messages[0].seq, 11);
        assert_eq!(page.next, KEPT as u64 + 10);
        // A cursor from before an agent restart is past every seq: nothing,
        // and the reader starts over from the newest.
        assert_eq!(log.page(10_000).next, KEPT as u64 + 10);
    }
}
