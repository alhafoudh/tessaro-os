//! One thread per open device, owning its blocking `Session`.
//!
//! The UI never waits on a socket. The worker polls `Status` on its own and
//! fetches the settings again only when their revision moved, so the tables
//! stay live without sending every setting every tick. Requests from the
//! device window come in over a channel the worker hands out first
//! (`Event::Ready`); every answer goes back as an `Event`. A lost connection
//! is retried with a growing pause until the window closes, which drops the
//! subscription and with it the thread.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc as ui;
use iced::Subscription;
use protocol::keys::{self, Consumer};
use protocol::{
    Applied, Command, Done, KeyInfo, NodeInfo, RestartTarget, Screenshot, Settings, Status, Verify,
};
use tessaro_client::connect::{self, Answer, Session, Target, Trust};
use tessaro_client::nodes::{Node, Nodes};

use crate::CLIENT;

/// How often `Status` is asked for.
const POLL: Duration = Duration::from_secs(2);
/// While a guarded change waits for its confirmation: the countdown moves.
const POLL_PENDING: Duration = Duration::from_secs(1);
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MOST: Duration = Duration::from_secs(15);
/// Longer than the device takes to apply, check and roll back a network
/// change, as `tessaro-ctl` waits (`net.rs`).
const NETWORK_CHANGE: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub enum Request {
    /// Fetch the settings again, whatever the revision says.
    Refresh,
    Set {
        values: BTreeMap<String, String>,
        if_revision: u64,
    },
    Unset {
        keys: Vec<String>,
        if_revision: u64,
    },
    Confirm,
    Restart(RestartTarget),
    Reboot,
    /// One screenshot now.
    Screenshot,
    /// Any command, answered as it came (`Event::Answer`) for the page that
    /// asked, which `tag` names. `long` waits as for a network change.
    Call {
        tag: &'static str,
        command: Command,
        long: bool,
    },
    /// A screenshot every `LIVE_SHOT` from now on, or no more.
    Live(bool),
}

/// How often a live screenshot is taken.
pub const LIVE_SHOT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub enum Event {
    /// Where requests go, for as long as this worker lives.
    Ready(mpsc::Sender<Request>),
    Connecting,
    Connected(NodeInfo),
    Lost(String),
    Status(Box<Status>),
    Settings(Settings),
    Keys(Vec<KeyInfo>),
    Applied(Result<Box<Applied>, String>),
    Done(Result<String, String>),
    /// The screen, as an encoded image (JPEG from the browser).
    Screenshot(Result<Vec<u8>, String>),
    /// What a `Request::Call` got back.
    Answer(&'static str, Result<serde_json::Value, String>),
    /// Worth telling the user: the device moved, a warning from the client.
    Note(String),
}

/// A session with a known node, by name: its last address first, then
/// mDNS, and held to its pin either way. nodes.json is read again each time,
/// so a login from the main window counts at the next try, and a device
/// found elsewhere is remembered there. Also what is worth telling the user.
///
/// A node nobody pinned - an unclaimed device opened from the list - is
/// reached at the address it was seen at, and only while it stays unclaimed.
pub fn connect(node: &Node) -> Result<(Session, Vec<String>), String> {
    let mut nodes = Nodes::load()?;
    let target = match nodes.by_id(&node.id).cloned() {
        Some(known) => Target::Named {
            name: known.name.clone(),
            port: None,
            known: Some(known),
        },
        None => Target::Remote {
            address: node
                .address
                .parse()
                .map_err(|_| format!("{} is no longer a known node", node.name))?,
            expected: Some(node.id.clone()),
            label: node.name.clone(),
        },
    };
    let session = connect::open(&target, &nodes, &mut Trust::KnownOnly, CLIENT)?;
    if session.node.id != node.id {
        return Err(format!(
            "{}: a different device answers at {} (node {} {})",
            node.name, node.address, session.node.name, session.node.id
        ));
    }
    let mut notes = session.notes.clone();
    if let (Some(was), Some((address, _))) = (nodes.refresh(&session)?, &session.remote) {
        notes.push(format!(
            "{}: now at {address}, was {was}",
            session.node.name
        ));
    }
    Ok((session, notes))
}

/// A worker is its node: the subscription is keyed by the node id alone, so
/// a renamed or moved device keeps the thread it has.
#[derive(Debug, Clone)]
struct Spec(Node);

impl Hash for Spec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}

pub fn subscription(node: Node) -> Subscription<Event> {
    Subscription::run_with(Spec(node), start)
}

fn start(spec: &Spec) -> ui::UnboundedReceiver<Event> {
    let (out, receive) = ui::unbounded();
    let worker = Worker {
        node: spec.0.clone(),
        out,
        live: Cell::new(false),
    };
    std::thread::spawn(move || worker.run());
    receive
}

/// Why a session ended.
enum Stop {
    /// Nobody listens any more: the window closed.
    Closed,
    Lost(String),
}

struct Worker {
    node: Node,
    out: ui::UnboundedSender<Event>,
    /// Live screenshots are on; kept across reconnections.
    live: Cell<bool>,
}

impl Worker {
    fn send(&self, event: Event) -> Result<(), Stop> {
        self.out.unbounded_send(event).map_err(|_| Stop::Closed)
    }

    fn run(self) {
        let (requests_to, requests) = mpsc::channel();
        if self.send(Event::Ready(requests_to)).is_err() {
            return;
        }
        let mut pause = RETRY_FIRST;
        loop {
            if self.send(Event::Connecting).is_err() {
                return;
            }
            let lost = match self.open() {
                Ok(session) => {
                    pause = RETRY_FIRST;
                    match self.serve(session, &requests) {
                        Stop::Closed => return,
                        Stop::Lost(why) => why,
                    }
                }
                Err(why) => why,
            };
            if self.send(Event::Lost(lost)).is_err() {
                return;
            }
            // Wait before the next try; Refresh tries at once. Anything
            // else cannot be done without a connection.
            match requests.recv_timeout(pause) {
                Ok(Request::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Ok(Request::Live(on)) => self.live.set(on),
                // Answered by its tag, so the page stops waiting for it.
                Ok(Request::Call { tag, .. }) => {
                    let _ = self.send(Event::Answer(tag, Err("not connected".to_string())));
                }
                Ok(_) => {
                    let _ = self.send(Event::Done(Err("not connected".to_string())));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            pause = (pause * 2).min(RETRY_MOST);
        }
    }

    fn open(&self) -> Result<Session, String> {
        let (session, notes) = connect(&self.node)?;
        for note in notes {
            let _ = self.send(Event::Note(note));
        }
        Ok(session)
    }

    fn serve(&self, mut session: Session, requests: &mpsc::Receiver<Request>) -> Stop {
        match self.serving(&mut session, requests) {
            Ok(never) => match never {},
            Err(stop) => stop,
        }
    }

    fn serving(
        &self,
        session: &mut Session,
        requests: &mpsc::Receiver<Request>,
    ) -> Result<std::convert::Infallible, Stop> {
        self.send(Event::Connected(session.node.clone()))?;
        let keys: Vec<KeyInfo> = ask(session, Command::Keys)?;
        self.send(Event::Keys(keys))?;

        let mut revision = None;
        let mut next_shot = Instant::now();
        loop {
            let status: Status = ask(session, Command::Status)?;
            if revision != Some(status.revision) {
                let settings: Settings = ask(session, Command::Get { key: None })?;
                revision = Some(settings.revision);
                self.send(Event::Settings(settings))?;
            }
            let mut wait = if status.pending.is_some() {
                POLL_PENDING
            } else {
                POLL
            };
            self.send(Event::Status(Box::new(status)))?;

            if self.live.get() {
                if Instant::now() >= next_shot {
                    self.screenshot(session)?;
                    next_shot = Instant::now() + LIVE_SHOT;
                }
                wait = wait.min(next_shot.saturating_duration_since(Instant::now()));
            }

            // Until the next poll, or right after a request: a change shows
            // in the status at once.
            match requests.recv_timeout(wait) {
                Ok(Request::Refresh) => revision = None,
                Ok(Request::Live(on)) => {
                    self.live.set(on);
                    next_shot = Instant::now();
                }
                Ok(request) => self.handle(session, request)?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Stop::Closed),
            }
        }
    }

    fn screenshot(&self, session: &mut Session) -> Result<(), Stop> {
        let shot = match session.request::<Screenshot>(Command::Screenshot) {
            Answer::Ok(shot) => data_encoding::BASE64
                .decode(shot.data.as_bytes())
                .map_err(|err| format!("the image is not base64: {err}")),
            Answer::Refused(error) => Err(error),
            Answer::Lost(why) => {
                self.send(Event::Screenshot(Err(format!(
                    "lost the connection: {why}"
                ))))?;
                return Err(Stop::Lost(why));
            }
        };
        self.send(Event::Screenshot(shot))
    }

    fn handle(&self, session: &mut Session, request: Request) -> Result<(), Stop> {
        match request {
            Request::Set {
                values,
                if_revision,
            } => {
                let network = values.keys().any(|key| is_network_key(key));
                let command = Command::Set {
                    values,
                    if_revision: Some(if_revision),
                    apply: true,
                    verify: Verify::default(),
                };
                self.apply(session, network, command)
            }
            Request::Unset { keys, if_revision } => {
                let network = keys.iter().any(|key| is_network_key(key));
                let command = Command::Unset {
                    keys,
                    if_revision: Some(if_revision),
                    apply: true,
                    verify: Verify::default(),
                };
                self.apply(session, network, command)
            }
            Request::Confirm => self.done(session, Command::Confirm),
            Request::Restart(what) => self.done(session, Command::Restart { what }),
            Request::Reboot => self.done(session, Command::Reboot),
            Request::Screenshot => self.screenshot(session),
            Request::Call { tag, command, long } => {
                if long {
                    session.set_read_timeout(Some(NETWORK_CHANGE));
                }
                let answer = session.request::<serde_json::Value>(command);
                session.restore_read_timeout();
                match answer {
                    Answer::Ok(value) => self.send(Event::Answer(tag, Ok(value))),
                    Answer::Refused(error) => self.send(Event::Answer(tag, Err(error))),
                    Answer::Lost(why) => {
                        self.send(Event::Answer(
                            tag,
                            Err(format!("lost the connection: {why}")),
                        ))?;
                        Err(Stop::Lost(why))
                    }
                }
            }
            Request::Refresh | Request::Live(_) => Ok(()),
        }
    }

    /// A set or unset. A network change waits for the device's own verdict,
    /// and may lose the connection it came in on - the device keeps or rolls
    /// back the change by itself.
    fn apply(&self, session: &mut Session, network: bool, command: Command) -> Result<(), Stop> {
        if network {
            session.set_read_timeout(Some(NETWORK_CHANGE));
        }
        let answer = session.request::<Applied>(command);
        session.restore_read_timeout();
        match answer {
            Answer::Ok(applied) => self.send(Event::Applied(Ok(Box::new(applied)))),
            Answer::Refused(error) => self.send(Event::Applied(Err(error))),
            Answer::Lost(why) => {
                let error = if network {
                    format!(
                        "lost the connection while the device applied the change ({why}). \
                         That is expected when it moved the link this connection came in on: \
                         the device keeps the change or rolls it back on its own."
                    )
                } else {
                    format!("lost the connection: {why}")
                };
                self.send(Event::Applied(Err(error)))?;
                Err(Stop::Lost(why))
            }
        }
    }

    fn done(&self, session: &mut Session, command: Command) -> Result<(), Stop> {
        match session.request::<Done>(command) {
            Answer::Ok(done) => self.send(Event::Done(Ok(done.message))),
            Answer::Refused(error) => self.send(Event::Done(Err(error))),
            Answer::Lost(why) => {
                self.send(Event::Done(Err(format!("lost the connection: {why}"))))?;
                Err(Stop::Lost(why))
            }
        }
    }
}

/// One command whose refusal is as fatal as a lost connection: the polls.
fn ask<T: serde::de::DeserializeOwned>(session: &mut Session, command: Command) -> Result<T, Stop> {
    match session.request(command) {
        Answer::Ok(value) => Ok(value),
        Answer::Refused(error) | Answer::Lost(error) => Err(Stop::Lost(error)),
    }
}

/// Whether a change to `key` is a network change the device verifies.
pub fn is_network_key(key: &str) -> bool {
    keys::find(key).is_some_and(|key| key.consumers.contains(&Consumer::Network))
}
