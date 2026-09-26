//! A device's journal, followed live on a connection of its own.
//!
//! Following the journal never ends, and the device window's worker has to
//! keep polling beside it, so the Log page opens a second session. The
//! follow asks for the next page every second and stops once nobody listens
//! (the page or the window closed), which ends the thread.

use std::hash::{Hash, Hasher};
use std::time::Duration;

use iced::futures::channel::mpsc as ui;
use iced::Subscription;
use protocol::api::LogsQuery;
use tessaro_client::journal::Entry;
use tessaro_client::nodes::Node;

use crate::worker;

/// How much of the journal comes first, before it is followed.
const BACKLOG: u32 = 300;
const RETRY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub enum Event {
    Connected,
    Entry(Box<Entry>),
    Lost(String),
}

/// A stream is its node, its unit filter and a generation: a new unit, or
/// Clear-and-reload, is a new stream.
#[derive(Debug, Clone)]
struct Spec {
    node: Node,
    unit: Option<String>,
    generation: u64,
}

impl Hash for Spec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.node.id.hash(state);
        self.unit.hash(state);
        self.generation.hash(state);
    }
}

pub fn subscription(node: Node, unit: Option<String>, generation: u64) -> Subscription<Event> {
    Subscription::run_with(
        Spec {
            node,
            unit,
            generation,
        },
        start,
    )
}

fn start(spec: &Spec) -> ui::UnboundedReceiver<Event> {
    let (out, receive) = ui::unbounded();
    let spec = spec.clone();
    std::thread::spawn(move || follow(spec, out));
    receive
}

fn follow(spec: Spec, out: ui::UnboundedSender<Event>) {
    while !out.is_closed() {
        let lost = match worker::connect(&spec.node) {
            Ok((mut session, _)) => {
                let _ = out.unbounded_send(Event::Connected);
                let query = LogsQuery {
                    unit: spec.unit.clone(),
                    lines: Some(BACKLOG),
                    cursor: None,
                };
                let followed = session.logs(query, true, &|| out.is_closed(), |event| {
                    let _ = out.unbounded_send(Event::Entry(Box::new(Entry::parse(&event))));
                });
                match followed {
                    Ok(()) => "the log stream ended".to_string(),
                    Err(why) => why,
                }
            }
            Err(why) => why,
        };
        if out.unbounded_send(Event::Lost(lost)).is_err() {
            return;
        }
        std::thread::sleep(RETRY);
    }
}
