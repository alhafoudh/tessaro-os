//! The HDMI-CEC message log of a device, followed for its CEC console on a
//! connection of its own.
//!
//! As the Log page's journal (`logs.rs`): the device window's worker keeps
//! polling beside it, so the console opens a second session. It asks for
//! what is new every `cec::MESSAGES_POLL` and stops once nobody listens (the
//! console closed), which ends the thread. Each answer goes up as one page,
//! so a long backlog is one update of the console, not one per message. A
//! lost connection, or a refusal such as CEC being off, is asked again from
//! the last message shown, so nothing is shown twice.

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc as ui;
use iced::Subscription;
use protocol::api::{self, CecMessagesQuery};
use protocol::CecMessages;
use tessaro_client::cec::MESSAGES_POLL;
use tessaro_client::nodes::Node;

use crate::worker;

const RETRY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub enum Event {
    Connected,
    /// What the log gained since the last page.
    Page(CecMessages),
    Lost(String),
}

/// A stream is its node and a generation: each console opened is a new one.
#[derive(Debug, Clone)]
struct Spec {
    node: Node,
    generation: u64,
}

impl Hash for Spec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.node.id.hash(state);
        self.generation.hash(state);
    }
}

pub fn subscription(node: Node, generation: u64) -> Subscription<Event> {
    Subscription::run_with(Spec { node, generation }, start)
}

fn start(spec: &Spec) -> ui::UnboundedReceiver<Event> {
    let (out, receive) = ui::unbounded();
    let spec = spec.clone();
    std::thread::spawn(move || follow(spec, out));
    receive
}

/// Waits `pause`, or less once nobody listens; whether anybody still does.
fn wait(out: &ui::UnboundedSender<Event>, pause: Duration) -> bool {
    let waited = Instant::now();
    while waited.elapsed() < pause {
        if out.is_closed() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !out.is_closed()
}

/// Everything the device kept first, then every message after the last one
/// shown.
fn follow(spec: Spec, out: ui::UnboundedSender<Event>) {
    let mut after = 0;
    while !out.is_closed() {
        let lost = match worker::connect(&spec.node) {
            Ok((mut session, _)) => {
                let _ = out.unbounded_send(Event::Connected);
                loop {
                    let page = match session
                        .call::<api::screen::CecMessages>(CecMessagesQuery { after }, ())
                    {
                        Ok(page) => page,
                        Err(why) => break why,
                    };
                    after = page.next;
                    if !page.messages.is_empty() && out.unbounded_send(Event::Page(page)).is_err() {
                        return;
                    }
                    if !wait(&out, MESSAGES_POLL) {
                        return;
                    }
                }
            }
            Err(why) => why,
        };
        if out.unbounded_send(Event::Lost(lost)).is_err() || !wait(&out, RETRY) {
            return;
        }
    }
}
