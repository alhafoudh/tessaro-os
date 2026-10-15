//! The devices at every broker this machine keeps (`nodes brokers`), for as
//! long as the main window lives: each broker's retained statuses, looked
//! at again every `EVERY`, so a device that connects or goes shows in the
//! list as it would over mDNS (`discovery.rs`).

use std::time::{Duration, Instant};

use iced::futures::channel::mpsc;
use iced::Subscription;
use tessaro_client::brokers::Brokers;
use tessaro_client::mqtt::{Link, Seen, SCAN};

use crate::CLIENT;

/// Between two looks at the brokers.
const EVERY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub enum Event {
    /// What one broker, `@NAME`, keeps a status for; every one it has.
    Scanned(String, Vec<Seen>),
    /// A broker that could not be asked, and why.
    Failed(String, String),
    /// The brokers kept, by `@NAME`: one no longer kept is dropped.
    Kept(Vec<String>),
}

/// A new `generation` starts over, which is what Rescan and a change to
/// the brokers do: the old thread ends as soon as nobody listens to it.
pub fn subscription(generation: u64) -> Subscription<Event> {
    Subscription::run_with(generation, scan)
}

fn scan(_: &u64) -> mpsc::UnboundedReceiver<Event> {
    let (send, receive) = mpsc::unbounded();
    std::thread::spawn(move || {
        while !send.is_closed() {
            let started = Instant::now();
            let kept = Brokers::load().unwrap_or_default();
            let labels: Vec<String> = kept.brokers.iter().map(|broker| broker.label()).collect();
            if send.unbounded_send(Event::Kept(labels)).is_err() {
                return;
            }
            for broker in &kept.brokers {
                let event = match Link::connect(broker, CLIENT).and_then(|mut link| {
                    let seen = link.scan(SCAN);
                    link.close();
                    seen
                }) {
                    Ok(seen) => Event::Scanned(broker.label(), seen),
                    Err(why) => Event::Failed(broker.label(), why),
                };
                if send.unbounded_send(event).is_err() {
                    return;
                }
            }
            // Woken every second to notice that the subscription went away.
            while started.elapsed() < EVERY {
                if send.is_closed() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
    receive
}
