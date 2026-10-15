//! The device's link to its MQTT broker (docs/mqtt.md).
//!
//! With mqtt.url set, one task keeps a connection to the broker, client id
//! the device id, and over it:
//!
//! * publishes what the device is - `<prefix>/<id>/status`, retained, with a
//!   will that says it went away - and what it is doing, retained under
//!   `state/`, every event (`crate::events`) under `event/`, every step of a
//!   job under `jobs/`, and on request screenshots and the journal;
//! * takes commands on `cmd/<group>/<Name>` and on the same under each of
//!   its tags, runs them exactly as the HTTP API does (`api::Dispatch`) and
//!   answers to the request's response topic (`commands`);
//! * hands the page what arrives below `page/in/`, and starts the scripts
//!   whose topic filters a message matches.
//!
//! The broker is trusted like the local socket: whoever may publish to the
//! device's command topics has as much of the device as mqtt.commands
//! gives. `link` opens the bytes under the session, `run` keeps the
//! session, and `Shared` is how the control plane sees and nudges it.

mod commands;
pub mod link;
mod run;

use std::sync::Mutex;

use protocol::mqtt::MqttState;
use tessaro_mqtt::Message;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::Instant;

use crate::deadline::within;
use crate::sync::lock;

pub use run::{probe, spawn};

/// How long a factory reset waits for the device's retained topics to be
/// emptied before it goes on without.
const RETIRE_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long `publish` waits for a full outbox to take a message.
const TAKE_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// What the control plane hands the running link.
pub enum Out {
    /// A message to publish; `sent` hears when it is written to the broker.
    Publish {
        message: Message,
        sent: Option<oneshot::Sender<()>>,
    },
    /// Empty every retained topic the device keeps, before it goes away
    /// for good: a factory reset.
    Retire { done: oneshot::Sender<()> },
}

/// The link as `tessaro-ctl mqtt status` reports it.
#[derive(Debug, Clone)]
pub struct Link {
    pub state: MqttState,
    pub since: Instant,
    pub broker: Option<String>,
    pub last_error: Option<String>,
    pub subscriptions: Vec<String>,
    pub queued: u64,
    pub published: u64,
    pub received: u64,
    /// Connections the broker took since the agent started.
    pub connects: u64,
}

impl Default for Link {
    fn default() -> Self {
        Link {
            state: MqttState::Off,
            since: Instant::now(),
            broker: None,
            last_error: None,
            subscriptions: Vec::new(),
            queued: 0,
            published: 0,
            received: 0,
            connects: 0,
        }
    }
}

/// Held by `Control`: how the link stands, and what the control plane may
/// ask of it.
#[derive(Default)]
pub struct Shared {
    link: Mutex<Link>,
    /// Connect again at once: the password or the certificate changed.
    reconnect: Notify,
    /// Work out the subscriptions again: a script's topic filters changed.
    resubscribe: Notify,
    /// Where the running session takes what to publish; `None` while there
    /// is none.
    out: Mutex<Option<mpsc::Sender<Out>>>,
}

impl Shared {
    pub fn link(&self) -> Link {
        lock(&self.link).clone()
    }

    pub(crate) fn update(&self, change: impl FnOnce(&mut Link)) {
        change(&mut lock(&self.link));
    }

    /// Move to `state`, keeping the counters; `error` is the reason for a
    /// failure, and a connection forgets the last one.
    pub(crate) fn enter(&self, state: MqttState, error: Option<String>) {
        let mut link = lock(&self.link);
        if link.state != state {
            link.since = Instant::now();
        }
        link.state = state;
        match state {
            MqttState::Failed => link.last_error = error,
            MqttState::Connected => link.last_error = None,
            MqttState::Off => {
                link.last_error = None;
                link.subscriptions.clear();
            }
            MqttState::Connecting => {}
        }
        if state != MqttState::Connected {
            link.subscriptions.clear();
        }
    }

    pub fn reconnect(&self) {
        self.reconnect.notify_one();
    }

    pub fn resubscribe(&self) {
        self.resubscribe.notify_one();
    }

    pub(crate) fn attach(&self, out: Option<mpsc::Sender<Out>>) {
        *lock(&self.out) = out;
    }

    fn outbox(&self) -> Option<mpsc::Sender<Out>> {
        lock(&self.out).clone()
    }

    /// Publish now; an error while the link is down, nothing is kept for
    /// later. What the page publishes goes this way.
    pub async fn publish(&self, message: Message) -> Result<(), String> {
        let out = self
            .outbox()
            .ok_or("the device is not connected to its MQTT broker")?;
        let sending = out.send(Out::Publish {
            message,
            sent: None,
        });
        let taken = within("the MQTT link taking a message", TAKE_LIMIT, sending).await;
        match taken {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err("the device is not connected to its MQTT broker".to_string()),
            Err(expired) => Err(expired.to_string()),
        }
    }

    /// Empty the device's retained topics on the broker, if it is
    /// connected, and wait a little for it: a factory reset, after which
    /// nothing would ever empty them.
    pub async fn retire(&self) {
        let Some(out) = self.outbox() else {
            return;
        };
        let (done, finished) = oneshot::channel();
        let retired = async {
            // naked: bounded by the within() below
            if out.send(Out::Retire { done }).await.is_ok() {
                // naked: bounded by the within() below
                let _ = finished.await;
            }
        };
        let _ = within(
            "emptying the device's topics on the broker",
            RETIRE_LIMIT,
            retired,
        )
        .await;
    }
}
