//! What the device did or saw, as one stream: a CEC event, someone arriving,
//! a scan, a setting changed, the browser restarted. Whoever produces one
//! emits it here next to whatever else it does with it (the page's event,
//! the scripts that run on it, the journal), and whoever wants every one
//! subscribes: the MQTT link publishes them (`crate::mqtt`).
//!
//! A broadcast channel, so an emitter never waits: a subscriber that falls
//! behind loses the oldest, and the gap shows in `seq`.

use std::sync::atomic::{AtomicU64, Ordering};

use protocol::mqtt::Event;
use serde_json::Value;
use tokio::sync::broadcast;

/// Events a subscriber may fall behind by before it loses some.
const BACKLOG: usize = 512;

pub struct Events {
    sender: broadcast::Sender<Event>,
    seq: AtomicU64,
}

impl Default for Events {
    fn default() -> Self {
        Events::new()
    }
}

impl Events {
    pub fn new() -> Events {
        let (sender, _) = broadcast::channel(BACKLOG);
        Events {
            sender,
            seq: AtomicU64::new(0),
        }
    }

    /// One event; nobody listening is fine.
    pub fn emit(&self, source: &str, name: &str, detail: Value) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.sender.send(Event {
            seq,
            source: source.to_string(),
            name: name.to_string(),
            detail,
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_numbered_in_order_for_every_subscriber() {
        let events = Events::new();
        events.emit("nobody", "listens", json!(null));
        let mut first = events.subscribe();
        let mut second = events.subscribe();
        events.emit("cec", "tv-on", json!({"event": "tv-on"}));
        events.emit("config", "changed", json!({"keys": ["browser.url"]}));
        for receiver in [&mut first, &mut second] {
            let one = receiver.try_recv().unwrap();
            assert_eq!(
                (one.seq, one.source.as_str(), one.name.as_str()),
                (2, "cec", "tv-on")
            );
            let two = receiver.try_recv().unwrap();
            assert_eq!((two.seq, two.source.as_str()), (3, "config"));
            assert!(receiver.try_recv().is_err());
        }
    }
}
