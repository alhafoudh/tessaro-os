//! Keeping the link: connect, subscribe, publish what the device is and
//! does, take what arrives, and when anything goes wrong, connect again a
//! little later. One task, which only moves bytes and messages: whatever
//! takes longer - a command, a status, a screenshot - runs on a task of its
//! own and hands its message back through the outbox.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::mqtt::topic::Layout;
use protocol::mqtt::MqttState;
use serde_json::{json, Value};
use tessaro_mqtt::{Event, Message, Options, Qos, Session};
use tokio::sync::{broadcast, mpsc, watch, Notify, Semaphore};
use tokio::time::Instant;

use super::link::{self, Failure, Transport};
use super::{commands, Out};
use crate::api::Dispatch;
use crate::config::{self, MqttCommands};
use crate::control::{Control, Plan};
use crate::deadline::within;
use crate::sync::lock;

/// The broker's answer to the CONNECT.
const CONNACK: Duration = Duration::from_secs(15);
/// One write to the broker. A broker that takes no bytes for this long is
/// as good as gone.
const WRITE: Duration = Duration::from_secs(30);
/// Waiting before the next attempt: doubling from the first to the most.
const BACKOFF_FIRST: Duration = Duration::from_secs(2);
const BACKOFF_MOST: Duration = Duration::from_secs(300);
/// A connection that lasted this long was a good one: the next failure
/// starts the backoff over.
const STEADY: Duration = Duration::from_secs(60);
/// Events kept for the broker while the link is down; the oldest go first.
const QUEUE: usize = 1000;
/// Commands running at once; more wait their turn.
const COMMANDS: usize = 8;
/// What the outbox holds before whoever adds to it waits.
const OUTBOX: usize = 256;
/// How often the journal is looked at while mqtt.logs is on.
const LOGS: Duration = Duration::from_secs(5);
/// Lines of the journal one look publishes at most.
const LOG_LINES: u32 = 500;
/// How long a broker keeps an answer for a client that went away.
const ANSWER_EXPIRY: u32 = 300;
/// How long a link about to go for a change lets out what is still on its
/// way.
const LINGER: Duration = Duration::from_secs(2);

/// Start keeping the link, for as long as the agent runs.
pub fn spawn(control: Arc<Control>, dispatch: Dispatch, shutdown: watch::Receiver<bool>) {
    let queue = Arc::new(Queue::default());
    queue.feed(control.events(), Arc::clone(&control));
    tokio::spawn(supervise(control, dispatch, queue, shutdown));
}

/// The events waiting for the broker.
#[derive(Default)]
struct Queue {
    events: Mutex<VecDeque<protocol::mqtt::Event>>,
    ready: Notify,
}

impl Queue {
    /// Every event from now on into the queue, whether the link is up or
    /// not, so the broker gets what happened while it was away.
    fn feed(
        self: &Arc<Self>,
        mut events: broadcast::Receiver<protocol::mqtt::Event>,
        control: Arc<Control>,
    ) {
        let queue = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // naked: an internal channel, open as long as the agent runs
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if control.current_mqtt().url.is_empty() {
                    continue;
                }
                let queued = {
                    let mut waiting = lock(&queue.events);
                    if waiting.len() >= QUEUE {
                        waiting.pop_front();
                    }
                    waiting.push_back(event);
                    waiting.len() as u64
                };
                control.mqtt().update(|link| link.queued = queued);
                queue.ready.notify_one();
            }
        });
    }

    fn take(&self) -> Vec<protocol::mqtt::Event> {
        lock(&self.events).drain(..).collect()
    }
}

/// How a connection ended.
enum Ended {
    /// The agent is stopping.
    Shutdown,
    /// The settings or the secrets changed: connect again now.
    Changed,
    /// It failed or dropped, after `lasted`.
    Failed { error: String, lasted: Duration },
}

async fn supervise(
    control: Arc<Control>,
    dispatch: Dispatch,
    queue: Arc<Queue>,
    mut shutdown: watch::Receiver<bool>,
) {
    let shared = control.mqtt();
    let mut follow = control.follow();
    let mut backoff = BACKOFF_FIRST;
    loop {
        if *shutdown.borrow() {
            return;
        }
        let config = follow.borrow_and_update().config.mqtt.clone();
        if config.url.is_empty() {
            shared.enter(MqttState::Off, None);
            shared.update(|link| link.broker = None);
            lock(&queue.events).clear();
            shared.update(|link| link.queued = 0);
            tokio::select! {
                // naked: the configuration's channel, open as long as the agent runs
                _ = follow.changed() => continue,
                // naked: the shutdown channel
                _ = shutdown.changed() => return,
            }
        }
        shared.enter(MqttState::Connecting, None);
        // naked: mqtt_plan reads through blocking() under within()
        let plan = match control.mqtt_plan(&config, None).await {
            Ok(plan) => plan,
            Err(err) => {
                shared.enter(MqttState::Failed, Some(err.clone()));
                control.log_mqtt(&format!("mqtt: {err}"));
                tokio::select! {
                    // naked: the configuration's channel
                    _ = follow.changed() => continue,
                    // naked: the shutdown channel
                    _ = shutdown.changed() => return,
                }
            }
        };
        shared.update(|link| link.broker = Some(plan.target.url.to_string()));
        // naked: a connection's every wait on the broker is under within() or its keep-alive
        let ended = Connection::run(
            &control,
            &dispatch,
            &queue,
            &plan,
            &config,
            &mut follow,
            &mut shutdown,
        )
        .await;
        match ended {
            Ended::Shutdown => return,
            Ended::Changed => backoff = BACKOFF_FIRST,
            Ended::Failed { error, lasted } => {
                if lasted >= STEADY {
                    backoff = BACKOFF_FIRST;
                }
                let first = shared.link().state != MqttState::Failed
                    || shared.link().last_error.as_deref() != Some(error.as_str());
                shared.enter(MqttState::Failed, Some(error.clone()));
                // Once per new reason, so a broker that is down does not
                // fill the journal.
                if first {
                    control.log_mqtt(&format!(
                        "mqtt: {}: {error}; trying again in {}s",
                        plan.target.url,
                        backoff.as_secs()
                    ));
                }
                tokio::select! {
                    // naked: a pause before the next attempt
                    _ = tokio::time::sleep(backoff) => {}
                    // naked: the configuration's channel
                    _ = follow.changed() => {}
                    // naked: an internal notification
                    _ = shared.reconnect.notified() => {}
                    // naked: the shutdown channel
                    _ = shutdown.changed() => return,
                }
                backoff = (backoff * 2).min(BACKOFF_MOST);
            }
        }
    }
}

/// Try the link once, as far as the broker taking the CONNECT, and leave:
/// `tessaro-ctl mqtt test`. The broker certificate's fingerprint, with the
/// failure too when TLS got that far.
pub async fn probe(plan: &Plan) -> Result<Option<String>, (Failure, Option<String>)> {
    // naked: open bounds every step with within()
    let opened = link::open(&plan.target)
        .await
        .map_err(|failure| (failure, None))?;
    let fingerprint = opened.fingerprint.clone();
    let mut transport = opened.transport;
    // A client id of its own, so the test does not push the device's link
    // off the broker.
    let mut options = options(plan, &plan.layout, MqttCommands::Off);
    options.client_id = format!("{}-test", plan.layout.id());
    options.will = None;
    let mut session = Session::new();
    // naked: handshake bounds itself with within()
    let outcome = handshake(&mut session, &mut transport, &options).await;
    if outcome.is_ok() {
        session.disconnect(std::time::Instant::now());
        let _ = within(
            "saying goodbye to the broker",
            Duration::from_secs(2),
            transport.send(session.output()),
        )
        .await;
    }
    // naked: close bounds itself with within()
    transport.close().await;
    outcome.map(|()| fingerprint.clone()).map_err(|error| {
        (
            Failure {
                stage: "mqtt",
                error,
            },
            fingerprint,
        )
    })
}

fn options(plan: &Plan, layout: &Layout, commands: MqttCommands) -> Options {
    let mut offline = serde_json::to_value(protocol::mqtt::Presence {
        online: false,
        id: Some(layout.id().to_string()),
        name: None,
        tags: Vec::new(),
        version: None,
        machine: None,
        fingerprint: None,
        claimed: None,
        commands: Some(commands.name().to_string()),
    })
    .unwrap_or(Value::Null);
    if let Value::Object(fields) = &mut offline {
        fields.retain(|_, value| !value.is_null());
    }
    Options {
        client_id: layout.id().to_string(),
        username: plan.username.clone(),
        password: plan.password.clone(),
        keep_alive: plan.keepalive,
        will: Some(
            Message::new(layout.status(), offline.to_string().into_bytes())
                .retain(true)
                .content_type(protocol::mqtt::envelope::JSON),
        ),
    }
}

/// CONNECT and wait for the broker's answer.
async fn handshake(
    session: &mut Session,
    transport: &mut Transport,
    options: &Options,
) -> Result<(), String> {
    session.connect(options, std::time::Instant::now())?;
    let answer = async {
        loop {
            if session.has_output() {
                // naked: bounded by the CONNACK within() below
                transport
                    .send(session.output())
                    .await
                    .map_err(|err| err.to_string())?;
            }
            while let Some(event) = session.event() {
                match event {
                    Event::Connected => return Ok(()),
                    Event::Refused(why) => {
                        return Err(format!("the broker refused the connection: {why}"))
                    }
                    Event::Closed(why) => return Err(why),
                    _ => {}
                }
            }
            // naked: bounded by the CONNACK within() below
            match transport.recv().await {
                Ok(Some(bytes)) => session.received(&bytes, std::time::Instant::now()),
                Ok(None) => return Err("the broker closed the connection".to_string()),
                Err(err) => return Err(err.to_string()),
            }
        }
    };
    match within("the broker's answer to CONNECT", CONNACK, answer).await {
        Ok(outcome) => outcome,
        Err(expired) => Err(expired.to_string()),
    }
}

/// One connection, from CONNECT to whatever ends it.
struct Connection<'a> {
    control: &'a Arc<Control>,
    dispatch: &'a Dispatch,
    config: config::Mqtt,
    session: Session,
    transport: Transport,
    layout: Layout,
    subscribed: BTreeSet<String>,
    script_filters: Vec<String>,
    tags: Vec<String>,
    outbox: mpsc::Sender<Out>,
    /// Who hears that a message is written: the answers whose `After`
    /// waits for it.
    sent: Vec<tokio::sync::oneshot::Sender<()>>,
    commands: Arc<Semaphore>,
    /// The device's state is being worked out for `state/`; the next tick
    /// waits for it.
    stating: Arc<std::sync::atomic::AtomicBool>,
    capturing: Arc<std::sync::atomic::AtomicBool>,
    reading_logs: Arc<std::sync::atomic::AtomicBool>,
    log_cursor: Arc<Mutex<Option<String>>>,
    last_frame: Arc<Mutex<Option<String>>>,
}

impl<'a> Connection<'a> {
    async fn run(
        control: &'a Arc<Control>,
        dispatch: &'a Dispatch,
        queue: &Queue,
        plan: &Plan,
        config: &config::Mqtt,
        follow: &mut config::Follow,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Ended {
        let started = Instant::now();
        let failed = |error: String| Ended::Failed {
            error,
            lasted: started.elapsed(),
        };
        // naked: open bounds every step with within()
        let opened = match link::open(&plan.target).await {
            Ok(opened) => opened,
            Err(failure) => return failed(failure.error),
        };
        let mut transport = opened.transport;
        let mut session = Session::new();
        let options = options(plan, &plan.layout, config.commands);
        // naked: handshake bounds itself with within()
        if let Err(err) = handshake(&mut session, &mut transport, &options).await {
            // naked: close bounds itself with within()
            transport.close().await;
            return failed(err);
        }

        let shared = control.mqtt();
        shared.update(|link| link.connects += 1);
        shared.enter(MqttState::Connected, None);
        control.log_mqtt(&format!(
            "mqtt: connected to {} as {}, topics under {}/{}",
            plan.target.url,
            plan.layout.id(),
            plan.layout.prefix(),
            plan.layout.id()
        ));
        let (outbox, mut taken) = mpsc::channel(OUTBOX);
        shared.attach(Some(outbox.clone()));
        // naked: mqtt_script_filters reads through blocking() under within()
        let script_filters = control.mqtt_script_filters().await;
        let mut this = Connection {
            control,
            dispatch,
            config: config.clone(),
            session,
            transport,
            layout: plan.layout.clone(),
            subscribed: BTreeSet::new(),
            script_filters,
            tags: control.tags(),
            outbox,
            sent: Vec::new(),
            commands: Arc::new(Semaphore::new(COMMANDS)),
            stating: Arc::default(),
            capturing: Arc::default(),
            reading_logs: Arc::default(),
            log_cursor: Arc::default(),
            last_frame: Arc::default(),
        };
        // naked: serve waits on the broker under within() or the session's keep-alive
        let ended = this
            .serve(queue, follow, shutdown, &mut taken, started)
            .await;
        shared.attach(None);
        // naked: close bounds itself with within()
        this.transport.close().await;
        ended
    }

    async fn serve(
        &mut self,
        queue: &Queue,
        follow: &mut config::Follow,
        shutdown: &mut watch::Receiver<bool>,
        taken: &mut mpsc::Receiver<Out>,
        started: Instant,
    ) -> Ended {
        let failed = |error: String| Ended::Failed {
            error,
            lasted: started.elapsed(),
        };
        if let Err(err) = self.subscribe().await {
            return failed(err);
        }
        self.publish_presence();
        self.publish_state();
        self.publish_queue(queue);
        let mut news = self.dispatch.jobs().news();
        let mut state_tick = tokio::time::interval(seconds(self.config.state_interval));
        state_tick.reset();
        let mut frame_tick = tokio::time::interval(seconds(self.config.screenshot_interval.max(1)));
        let mut logs_tick = tokio::time::interval(LOGS);
        if let Err(err) = self.flush().await {
            return failed(err);
        }

        loop {
            let timer = self.session.next_timer();
            let wait = async {
                match timer {
                    // naked: the session's own timer
                    Some(at) => tokio::time::sleep_until(Instant::from_std(at)).await,
                    // naked: no timer, nothing to wait for
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                // naked: the session's keep-alive bounds a quiet link
                read = self.transport.recv() => match read {
                    Ok(Some(bytes)) => self.session.received(&bytes, std::time::Instant::now()),
                    Ok(None) => return failed("the broker closed the connection".to_string()),
                    Err(err) => return failed(err.to_string()),
                },
                // naked: the session's own timers
                () = wait => self.session.fire_timers(std::time::Instant::now()),
                // naked: an internal channel
                out = taken.recv() => match out {
                    Some(Out::Publish { message, sent }) => {
                        self.publish(&message);
                        self.sent.extend(sent);
                    }
                    Some(Out::Retire { done }) => {
                        self.retire();
                        let _ = self.flush().await;
                        let _ = done.send(());
                    }
                    None => {}
                },
                // naked: an internal notification
                () = queue.ready.notified() => self.publish_queue(queue),
                // naked: an internal channel
                job = news.recv() => if let Ok(job) = job {
                    self.publish_job(job);
                },
                // naked: a timer
                _ = state_tick.tick() => self.publish_state(),
                // naked: a timer
                _ = frame_tick.tick(), if self.config.screenshot_interval > 0 => self.capture(),
                // naked: a timer
                _ = logs_tick.tick(), if self.config.logs != "off" => self.read_logs(),
                // naked: the configuration's channel
                _ = follow.changed() => {
                    let next = follow.borrow_and_update().config.mqtt.clone();
                    if link_changed(&self.config, &next) {
                        // The answer to the very command that made this
                        // change is on its way: it goes out first.
                        self.linger(taken).await;
                        // Topics the device leaves for good would be stale
                        // there forever: emptied instead of marked offline.
                        let leaving = next.url.is_empty() || next.prefix != self.config.prefix || next.url != self.config.url;
                        if leaving {
                            self.retire();
                        }
                        self.goodbye(!leaving).await;
                        return Ended::Changed;
                    }
                    let timers = next.state_interval != self.config.state_interval
                        || next.screenshot_interval != self.config.screenshot_interval;
                    self.config = next;
                    if timers {
                        state_tick = tokio::time::interval(seconds(self.config.state_interval));
                        state_tick.reset();
                        frame_tick = tokio::time::interval(seconds(self.config.screenshot_interval.max(1)));
                    }
                    self.tags = self.control.tags();
                    if let Err(err) = self.subscribe().await {
                        return failed(err);
                    }
                    // A name, tags or the settings may have moved.
                    self.publish_presence();
                    self.publish_state();
                }
                // naked: an internal notification
                () = self.control.mqtt().reconnect.notified() => {
                    self.goodbye(true).await;
                    return Ended::Changed;
                }
                // naked: an internal notification
                () = self.control.mqtt().resubscribe.notified() => {
                    // naked: mqtt_script_filters reads through blocking() under within()
                    self.script_filters = self.control.mqtt_script_filters().await;
                    if let Err(err) = self.subscribe().await {
                        return failed(err);
                    }
                }
                // naked: the shutdown channel
                _ = shutdown.changed() => {
                    self.goodbye(true).await;
                    return Ended::Shutdown;
                }
            }
            while let Some(event) = self.session.event() {
                match event {
                    Event::Message(message) => self.take(message),
                    Event::Closed(why) => return failed(why),
                    Event::Subscribed { refused, .. } => {
                        for refusal in refused {
                            self.control.log_mqtt(&format!(
                                "mqtt: the broker refused a subscription to {refusal}"
                            ));
                        }
                    }
                    Event::Connected | Event::Refused(_) | Event::Acked(_) => {}
                }
            }
            if let Err(err) = self.flush().await {
                return failed(err);
            }
        }
    }

    /// For `LINGER`, publish what is handed over, as before: the answers
    /// and states already on their way when the link is about to go.
    async fn linger(&mut self, taken: &mut mpsc::Receiver<Out>) {
        let until = Instant::now() + LINGER;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match within("the last messages before reconnecting", left, taken.recv()).await {
                Ok(Some(Out::Publish { message, sent })) => {
                    self.publish(&message);
                    self.sent.extend(sent);
                    if self.flush().await.is_err() {
                        return;
                    }
                }
                Ok(Some(Out::Retire { done })) => {
                    self.retire();
                    let _ = self.flush().await;
                    let _ = done.send(());
                }
                Ok(None) | Err(_) => return,
            }
        }
    }

    /// Write what the session has to send.
    async fn flush(&mut self) -> Result<(), String> {
        if self.session.has_output() {
            let bytes = self.session.output();
            match within("writing to the broker", WRITE, self.transport.send(bytes)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Err(format!("writing to the broker: {err}")),
                Err(expired) => return Err(expired.to_string()),
            }
        }
        for sent in self.sent.drain(..) {
            let _ = sent.send(());
        }
        Ok(())
    }

    /// The device is going: offline in its status unless `offline` is false
    /// (its topics were just emptied), and a DISCONNECT, which tells the
    /// broker not to publish the will on top.
    async fn goodbye(&mut self, offline: bool) {
        if offline {
            let status = json!({ "online": false, "id": self.layout.id() });
            self.publish(
                &Message::new(self.layout.status(), status.to_string().into_bytes())
                    .retain(true)
                    .content_type(protocol::mqtt::envelope::JSON),
            );
        }
        self.session.disconnect(std::time::Instant::now());
        let _ = self.flush().await;
    }

    /// Empty every retained topic of the device.
    fn retire(&mut self) {
        for topic in [
            self.layout.status(),
            self.layout.state("device"),
            self.layout.state("config"),
            self.layout.state("mqtt"),
        ] {
            self.publish(&Message::new(topic, Vec::new()).retain(true));
        }
    }

    fn publish(&mut self, message: &Message) {
        match self.session.publish(message, std::time::Instant::now()) {
            Ok(_) => self.control.mqtt().update(|link| link.published += 1),
            Err(err) => self
                .control
                .log_mqtt(&format!("mqtt: publishing to {}: {err}", message.topic)),
        }
    }

    fn publish_json(&mut self, topic: String, value: &Value, retain: bool, qos: Qos) {
        let message = Message::new(topic, value.to_string().into_bytes())
            .retain(retain)
            .qos(qos)
            .content_type(protocol::mqtt::envelope::JSON);
        self.publish(&message);
    }

    fn publish_presence(&mut self) {
        let presence = self.control.presence_message(self.config.commands);
        let value = serde_json::to_value(presence).unwrap_or(Value::Null);
        self.publish_json(self.layout.status(), &value, true, Qos::AtLeastOnce);
    }

    /// `state/device`, `state/config` and `state/mqtt`, worked out on a task
    /// of their own: the status asks the system bus and the browser.
    fn publish_state(&mut self) {
        if self.stating.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let control = Arc::clone(self.control);
        let outbox = self.outbox.clone();
        let layout = self.layout.clone();
        let stating = Arc::clone(&self.stating);
        tokio::spawn(async move {
            let mut parts: Vec<(String, Value)> = Vec::new();
            // naked: Control bounds every call it makes
            match control
                .handle(&crate::control::Caller::Local, protocol::Command::Status)
                .await
                .result
            {
                Ok(status) => parts.push((layout.state("device"), status)),
                Err(err) => control.log_mqtt(&format!("mqtt: the device's status: {err}")),
            }
            // naked: a read through blocking() under within()
            if let Ok(config) = control.mqtt_config_state().await {
                parts.push((layout.state("config"), config));
            }
            // naked: a read through blocking() under within()
            if let Ok(status) = serde_json::to_value(control.mqtt_status().await) {
                parts.push((layout.state("mqtt"), status));
            }
            for (topic, value) in parts {
                let message = Message::new(topic, value.to_string().into_bytes())
                    .retain(true)
                    .content_type(protocol::mqtt::envelope::JSON);
                let out = Out::Publish {
                    message,
                    sent: None,
                };
                // naked: the outbox, which the link empties as fast as it writes
                if outbox.send(out).await.is_err() {
                    break;
                }
            }
            stating.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    fn publish_queue(&mut self, queue: &Queue) {
        for event in queue.take() {
            let topic = self.layout.event(&event.source, &event.name);
            let value = serde_json::to_value(&event).unwrap_or(Value::Null);
            self.publish_json(topic, &value, false, Qos::AtLeastOnce);
        }
        self.control.mqtt().update(|link| link.queued = 0);
    }

    fn publish_job(&mut self, news: crate::api::jobs::News) {
        let value = match news.step {
            Some(step) => json!({ "index": news.index, "step": step }),
            None => json!({ "done": news.done, "error": news.error }),
        };
        self.publish_json(self.layout.job(&news.job), &value, false, Qos::AtLeastOnce);
    }

    /// A screenshot for `screen/frame`, sent only when it differs from the
    /// last one.
    fn capture(&mut self) {
        if self
            .capturing
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let control = Arc::clone(self.control);
        let outbox = self.outbox.clone();
        let topic = self.layout.frame();
        let width = u32::try_from(self.config.screenshot_width).unwrap_or(0);
        let quality = u8::try_from(self.config.screenshot_quality).unwrap_or(60);
        let capturing = Arc::clone(&self.capturing);
        let last = Arc::clone(&self.last_frame);
        tokio::spawn(async move {
            // naked: screenshot bounds the capture with within()
            let shot = control
                .screenshot(Some(width).filter(|w| *w > 0), Some(quality))
                .await;
            if let Ok(shot) = shot {
                let digest = protocol::hex(&openssl::sha::sha256(shot.data.as_bytes()));
                let fresh = lock(&last).as_deref() != Some(digest.as_str());
                if let (true, Ok(jpeg)) = (fresh, openssl::base64::decode_block(&shot.data)) {
                    *lock(&last) = Some(digest);
                    let message = Message::new(topic, jpeg)
                        .qos(Qos::AtMostOnce)
                        .content_type("image/jpeg");
                    let out = Out::Publish {
                        message,
                        sent: None,
                    };
                    // naked: the outbox, which the link empties as fast as it writes
                    let _ = outbox.send(out).await;
                }
            }
            capturing.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// The journal since the last look, at mqtt.logs' priority and up.
    fn read_logs(&mut self) {
        if self
            .reading_logs
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let control = Arc::clone(self.control);
        let outbox = self.outbox.clone();
        let topic = self.layout.logs();
        let most = priority(&self.config.logs);
        let reading = Arc::clone(&self.reading_logs);
        let cursor = Arc::clone(&self.log_cursor);
        tokio::spawn(async move {
            let from = lock(&cursor).clone();
            // The first look only finds where the journal is now.
            let lines = if from.is_none() {
                Some(1)
            } else {
                Some(LOG_LINES)
            };
            // naked: journalctl runs under within() in Control::logs
            if let Ok(page) = control.journal(lines, from.clone()).await {
                if from.is_some() {
                    for entry in page.entries {
                        let level = entry["PRIORITY"]
                            .as_str()
                            .and_then(|p| p.parse::<u8>().ok())
                            .unwrap_or(6);
                        if level > most {
                            continue;
                        }
                        let message = Message::new(topic.clone(), entry.to_string().into_bytes())
                            .qos(Qos::AtMostOnce)
                            .content_type(protocol::mqtt::envelope::JSON);
                        let out = Out::Publish {
                            message,
                            sent: None,
                        };
                        // naked: the outbox, which the link empties as fast as it writes
                        if outbox.send(out).await.is_err() {
                            break;
                        }
                    }
                }
                if page.cursor.is_some() {
                    *lock(&cursor) = page.cursor;
                }
            }
            reading.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// What the device subscribes to now: its commands and its tags'
    /// unless mqtt.commands is off, the page's subtree while mqtt.page is
    /// on, and every script's topic filters while mqtt.scripts is.
    fn wanted(&self) -> BTreeSet<String> {
        let mut wanted = BTreeSet::new();
        if self.config.commands != MqttCommands::Off {
            wanted.insert(self.layout.commands());
            for tag in &self.tags {
                wanted.insert(self.layout.tag_commands(tag));
            }
        }
        if self.config.page {
            wanted.insert(self.layout.page_in());
        }
        if self.config.scripts {
            wanted.extend(self.script_filters.iter().cloned());
        }
        wanted
    }

    async fn subscribe(&mut self) -> Result<(), String> {
        let wanted = self.wanted();
        let gone: Vec<String> = self.subscribed.difference(&wanted).cloned().collect();
        let new: Vec<(String, Qos)> = wanted
            .difference(&self.subscribed)
            .map(|filter| (filter.clone(), Qos::AtLeastOnce))
            .collect();
        let now = std::time::Instant::now();
        self.session.unsubscribe(&gone, now)?;
        if !new.is_empty() {
            self.session.subscribe(&new, now)?;
        }
        self.subscribed = wanted;
        let list: Vec<String> = self.subscribed.iter().cloned().collect();
        self.control.mqtt().update(|link| link.subscriptions = list);
        self.flush().await
    }

    /// A message from the broker: a command, something for the page, or
    /// one a script runs on - or several of these at once.
    fn take(&mut self, message: Message) {
        self.control.mqtt().update(|link| link.received += 1);
        if let Some(command) = self.layout.command(&message.topic) {
            let ours = command
                .tag
                .is_none_or(|tag| self.tags.iter().any(|known| known == tag));
            if self.config.commands != MqttCommands::Off && ours {
                let (group, name) = (command.group.to_string(), command.name.to_string());
                let control = Arc::clone(self.control);
                let dispatch = self.dispatch.clone();
                let outbox = self.outbox.clone();
                let permits = Arc::clone(&self.commands);
                let mode = self.config.commands;
                let id = self.layout.id().to_string();
                let message = message.clone();
                tokio::spawn(async move {
                    // naked: a semaphore of this process, released as commands end
                    let Ok(_permit) = permits.acquire_owned().await else {
                        return;
                    };
                    // naked: commands::handle bounds every call through Control
                    commands::handle(
                        &control, &dispatch, &outbox, mode, &id, &group, &name, message,
                    )
                    .await;
                });
            }
        }
        if self.config.page {
            if let Some(sub) = self.layout.page_in_sub(&message.topic) {
                let control = Arc::clone(self.control);
                let sub = sub.to_string();
                let message = message.clone();
                tokio::spawn(async move {
                    // naked: the page bridge bounds its calls with within()
                    control.mqtt_page_in(&sub, &message).await;
                });
            }
        }
        if self.config.scripts && protocol::mqtt::runs_on(&self.script_filters, &message.topic) {
            let control = Arc::clone(self.control);
            tokio::spawn(async move {
                // naked: starting the runs goes through the bus, under within()
                control.mqtt_scripts(&message.topic, &message.payload).await;
            });
        }
    }
}

/// Whether a change of the settings needs a new connection.
fn link_changed(old: &config::Mqtt, new: &config::Mqtt) -> bool {
    old.url != new.url
        || old.username != new.username
        || old.prefix != new.prefix
        || old.fingerprint != new.fingerprint
        || old.keepalive != new.keepalive
        // The will says which mqtt.commands the device runs with.
        || old.commands != new.commands
}

/// A journal priority's number: `err` is 3, `debug` 7.
fn priority(level: &str) -> u8 {
    match level {
        "err" => 3,
        "warning" => 4,
        "notice" => 5,
        "info" => 6,
        _ => 7,
    }
}

fn seconds(value: i64) -> Duration {
    Duration::from_secs(value.max(1) as u64)
}

/// The answer to a command, to the request's response topic.
pub fn answer(
    request: &Message,
    id: &str,
    status: &str,
    payload: Vec<u8>,
    content_type: &str,
    headers: &[(String, String)],
) -> Option<Message> {
    let topic = request.response_topic.clone()?;
    let mut message = Message::new(topic, payload)
        .content_type(content_type)
        .user(protocol::mqtt::envelope::STATUS, status)
        .user(protocol::mqtt::envelope::DEVICE, id);
    message.correlation = request.correlation.clone();
    message.expiry = Some(ANSWER_EXPIRY);
    for (name, value) in headers {
        message = message.user(name, value);
    }
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the development broker of `mise run mqtt:run`, which has to
    /// be running: every kind of listener takes the device, the TLS ones
    /// with the user and pinned to the dev certificate. Ignored without one:
    /// `TESSARO_TEST_MQTT=1 cargo test -p tessaro-agent dev_broker -- --ignored`,
    /// with `TESSARO_TEST_MQTT_PORTS=plain,tls,ws,wss` when they are not the
    /// defaults.
    #[tokio::test]
    #[ignore]
    async fn every_listener_of_the_dev_broker_takes_the_device() {
        let ports = std::env::var("TESSARO_TEST_MQTT_PORTS")
            .unwrap_or_else(|_| "1883,8883,9001,9443".into());
        let ports: Vec<u16> = ports.split(',').map(|port| port.parse().unwrap()).collect();
        let pem = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../build/mqtt/server.pem"
        ))
        .expect("the dev broker's certificate: run `mise run mqtt:run` first");
        let der = openssl::x509::X509::from_pem(&pem)
            .unwrap()
            .to_der()
            .unwrap();
        let fingerprint = protocol::hex(&openssl::sha::sha256(&der));
        let cases = [
            (format!("mqtt://127.0.0.1:{}", ports[0]), false),
            (format!("mqtts://localhost:{}", ports[1]), true),
            (format!("ws://127.0.0.1:{}", ports[2]), false),
            (format!("wss://localhost:{}", ports[3]), true),
        ];
        for (url, tls) in cases {
            let plan = Plan {
                target: link::Target {
                    url: protocol::mqtt::parse_url(&url).unwrap(),
                    fingerprint: tls.then(|| fingerprint.clone()),
                    identity: None,
                    proxy: None,
                },
                layout: Layout::new("tessaro-test", "0123abcd"),
                username: tls.then(|| "tessaro".to_string()),
                password: tls.then(|| b"tessaro".to_vec()),
                keepalive: 30,
            };
            match probe(&plan).await {
                Ok(seen) => assert_eq!(seen.is_some(), tls, "{url}"),
                Err((failure, _)) => panic!("{url}: {}: {}", failure.stage, failure.error),
            }
            if tls {
                let mut wrong = plan.clone();
                wrong.password = Some(b"nope".to_vec());
                let (failure, _) = probe(&wrong)
                    .await
                    .expect_err("a wrong password is refused");
                assert_eq!(failure.stage, "mqtt", "{url}: {}", failure.error);
            }
        }
    }

    fn mqtt(url: &str) -> config::Mqtt {
        config::Config::load(&std::collections::HashMap::from([(
            "KIOSK_MQTT_URL".to_string(),
            url.to_string(),
        )]))
        .mqtt
    }

    #[test]
    fn only_what_the_connection_is_made_with_reconnects() {
        let old = mqtt("mqtt://b");
        let mut new = old.clone();
        new.logs = "info".into();
        new.screenshot_interval = 30;
        new.page = false;
        assert!(!link_changed(&old, &new));
        new.username = "kiosk".into();
        assert!(link_changed(&old, &new));
        let mut new = old.clone();
        new.commands = MqttCommands::Read;
        assert!(link_changed(&old, &new));
    }

    #[test]
    fn journal_priorities_count_down_to_err() {
        assert_eq!(priority("err"), 3);
        assert_eq!(priority("info"), 6);
        assert_eq!(priority("debug"), 7);
    }

    #[test]
    fn an_answer_goes_to_the_response_topic_with_the_correlation() {
        let mut request = Message::new("tessaro/ab/cmd/device/Status", b"{}".to_vec());
        assert!(answer(&request, "ab", "ok", vec![], "application/json", &[]).is_none());
        request.response_topic = Some("tessaro/client/x/reply".into());
        request.correlation = Some(vec![7]);
        let reply = answer(
            &request,
            "ab",
            "ok",
            b"{}".to_vec(),
            "application/json",
            &[("x-tessaro-size".into(), "3".into())],
        )
        .unwrap();
        assert_eq!(reply.topic, "tessaro/client/x/reply");
        assert_eq!(reply.correlation, Some(vec![7]));
        assert!(reply.user.contains(&("status".into(), "ok".into())));
        assert!(reply.user.contains(&("device".into(), "ab".into())));
        assert!(reply.user.contains(&("x-tessaro-size".into(), "3".into())));
    }
}
