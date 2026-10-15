//! One MQTT v5 client connection as a state machine, over
//! mqtt-protocol-core. Nothing here touches a socket or a clock source of
//! its own: the caller feeds it what the stream read (`received`), writes
//! what it asks to send (`output`), fires its timers when they are due
//! (`next_timer`, `fire_timers`) and reads what happened (`event`). The
//! agent drives it on tokio under `deadline::within`, the clients on a
//! blocking socket.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use mqtt_protocol_core::mqtt;
use mqtt_protocol_core::mqtt::connection::{Event as Raw, TimerKind};
use mqtt_protocol_core::mqtt::packet::{Packet, Property, Qos as RawQos, SubEntry, SubOpts};
use mqtt_protocol_core::mqtt::prelude::*;

/// How a message is delivered. QoS 2 is never asked for: nothing here
/// needs exactly-once, and every handler on the device is idempotent or
/// says it ran twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qos {
    AtMostOnce,
    AtLeastOnce,
}

impl Qos {
    fn raw(self) -> RawQos {
        match self {
            Qos::AtMostOnce => RawQos::AtMostOnce,
            Qos::AtLeastOnce => RawQos::AtLeastOnce,
        }
    }
}

/// A message, either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: Qos,
    pub retain: bool,
    pub content_type: Option<String>,
    pub response_topic: Option<String>,
    pub correlation: Option<Vec<u8>>,
    pub user: Vec<(String, String)>,
    /// Seconds the broker keeps it for a subscriber that is away.
    pub expiry: Option<u32>,
}

impl Message {
    pub fn new(topic: impl Into<String>, payload: impl Into<Vec<u8>>) -> Message {
        Message {
            topic: topic.into(),
            payload: payload.into(),
            qos: Qos::AtLeastOnce,
            retain: false,
            content_type: None,
            response_topic: None,
            correlation: None,
            user: Vec::new(),
            expiry: None,
        }
    }

    pub fn qos(mut self, qos: Qos) -> Message {
        self.qos = qos;
        self
    }

    pub fn retain(mut self, retain: bool) -> Message {
        self.retain = retain;
        self
    }

    pub fn content_type(mut self, content_type: &str) -> Message {
        self.content_type = Some(content_type.to_string());
        self
    }

    pub fn user(mut self, key: &str, value: &str) -> Message {
        self.user.push((key.to_string(), value.to_string()));
        self
    }

    fn props(&self) -> Result<Vec<Property>, String> {
        let mut props: Vec<Property> = Vec::new();
        if let Some(content_type) = &self.content_type {
            props.push(
                mqtt::packet::ContentType::new(content_type.as_str())
                    .map_err(err)?
                    .into(),
            );
        }
        if let Some(topic) = &self.response_topic {
            props.push(
                mqtt::packet::ResponseTopic::new(topic.as_str())
                    .map_err(err)?
                    .into(),
            );
        }
        if let Some(data) = &self.correlation {
            props.push(
                mqtt::packet::CorrelationData::new(data)
                    .map_err(err)?
                    .into(),
            );
        }
        if let Some(seconds) = self.expiry {
            props.push(
                mqtt::packet::MessageExpiryInterval::new(seconds)
                    .map_err(err)?
                    .into(),
            );
        }
        for (key, value) in &self.user {
            props.push(
                mqtt::packet::UserProperty::new(key.as_str(), value.as_str())
                    .map_err(err)?
                    .into(),
            );
        }
        Ok(props)
    }
}

/// What the CONNECT says.
#[derive(Debug, Clone)]
pub struct Options {
    pub client_id: String,
    pub username: Option<String>,
    pub password: Option<Vec<u8>>,
    /// Seconds; 0 turns keep-alive off.
    pub keep_alive: u16,
    /// Published by the broker when the connection drops without a
    /// DISCONNECT.
    pub will: Option<Message>,
}

/// What happened on the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The broker took the CONNECT.
    Connected,
    /// The broker refused the CONNECT; the reason, in words.
    Refused(String),
    Message(Message),
    /// A QoS 1 publish of ours was acknowledged.
    Acked(u16),
    /// A SUBSCRIBE was answered; the filters the broker refused, in words.
    Subscribed {
        id: u16,
        refused: Vec<String>,
    },
    /// The connection is over and the stream should be closed; why.
    Closed(String),
}

pub struct Session {
    conn: mqtt::Connection<mqtt::role::Client>,
    out: Vec<u8>,
    events: VecDeque<Event>,
    timers: Vec<(TimerKind, Instant)>,
    /// The filters of each SUBSCRIBE in flight, to name the refused ones.
    pending: Vec<(u16, Vec<String>)>,
    connected: bool,
    closed: bool,
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

impl Session {
    pub fn new() -> Session {
        let mut conn = mqtt::Connection::<mqtt::role::Client>::new(mqtt::Version::V5_0);
        conn.set_auto_pub_response(true);
        conn.set_auto_ping_response(true);
        Session {
            conn,
            out: Vec::new(),
            events: VecDeque::new(),
            timers: Vec::new(),
            pending: Vec::new(),
            connected: false,
            closed: false,
        }
    }

    pub fn connected(&self) -> bool {
        self.connected && !self.closed
    }

    /// Sends the CONNECT; `Event::Connected` or `Refused` follows.
    pub fn connect(&mut self, options: &Options, now: Instant) -> Result<(), String> {
        let mut builder = mqtt::packet::v5_0::Connect::builder()
            .client_id(options.client_id.as_str())
            .map_err(err)?
            .clean_start(true)
            .keep_alive(options.keep_alive);
        if let Some(name) = &options.username {
            builder = builder.user_name(name.as_str()).map_err(err)?;
        }
        if let Some(password) = &options.password {
            if options.username.is_none() {
                builder = builder.user_name("").map_err(err)?;
            }
            builder = builder.password(password).map_err(err)?;
        }
        if let Some(will) = &options.will {
            builder = builder
                .will_message(
                    will.topic.as_str(),
                    &will.payload,
                    will.qos.raw(),
                    will.retain,
                )
                .map_err(err)?;
            let props = will.props()?;
            if !props.is_empty() {
                builder = builder.will_props(props);
            }
        }
        let packet = builder.build().map_err(err)?;
        // A ping unanswered for a keep-alive period ends the connection: a
        // half-open socket would otherwise look connected until TCP gives up.
        if options.keep_alive > 0 {
            self.conn
                .set_pingresp_recv_timeout(u64::from(options.keep_alive) * 1000);
        }
        let events = self.conn.checked_send(packet);
        self.handle(events, now);
        Ok(())
    }

    /// Publishes a message; the packet id of a QoS 1 one, which an
    /// `Event::Acked` names when the broker has it.
    pub fn publish(&mut self, message: &Message, now: Instant) -> Result<Option<u16>, String> {
        if !self.connected() {
            return Err("not connected".into());
        }
        let mut builder = mqtt::packet::v5_0::Publish::builder()
            .topic_name(message.topic.as_str())
            .map_err(err)?
            .qos(message.qos.raw())
            .retain(message.retain)
            .payload(message.payload.as_slice());
        let props = message.props()?;
        if !props.is_empty() {
            builder = builder.props(props);
        }
        let id = match message.qos {
            Qos::AtMostOnce => None,
            Qos::AtLeastOnce => Some(self.conn.acquire_packet_id().map_err(err)?),
        };
        if let Some(id) = id {
            builder = builder.packet_id(id);
        }
        let packet = builder.build().map_err(err)?;
        let events = self.conn.checked_send(packet);
        self.handle(events, now);
        Ok(id)
    }

    /// Subscribes to filters; `Event::Subscribed` with the same id answers.
    /// No-local: what this client publishes never comes back to it, so a
    /// filter that also matches its own topics cannot feed on itself.
    pub fn subscribe(&mut self, filters: &[(String, Qos)], now: Instant) -> Result<u16, String> {
        if !self.connected() {
            return Err("not connected".into());
        }
        let mut entries = Vec::new();
        for (filter, qos) in filters {
            let options = SubOpts::new().set_qos(qos.raw()).set_nl(true);
            entries.push(SubEntry::new(filter.as_str(), options).map_err(err)?);
        }
        let id = self.conn.acquire_packet_id().map_err(err)?;
        let packet = mqtt::packet::v5_0::Subscribe::builder()
            .entries(entries)
            .packet_id(id)
            .build()
            .map_err(err)?;
        self.pending
            .push((id, filters.iter().map(|(f, _)| f.clone()).collect()));
        let events = self.conn.checked_send(packet);
        self.handle(events, now);
        Ok(id)
    }

    pub fn unsubscribe(&mut self, filters: &[String], now: Instant) -> Result<(), String> {
        if !self.connected() || filters.is_empty() {
            return Ok(());
        }
        let id = self.conn.acquire_packet_id().map_err(err)?;
        let packet = mqtt::packet::v5_0::Unsubscribe::builder()
            .entries(filters.iter().map(String::as_str))
            .map_err(err)?
            .packet_id(id)
            .build()
            .map_err(err)?;
        let events = self.conn.checked_send(packet);
        self.handle(events, now);
        Ok(())
    }

    /// A clean goodbye: the broker drops the will.
    pub fn disconnect(&mut self, now: Instant) {
        if !self.connected() {
            return;
        }
        if let Ok(packet) = mqtt::packet::v5_0::Disconnect::builder().build() {
            let events = self.conn.checked_send(packet);
            self.handle(events, now);
        }
    }

    /// Bytes the stream read.
    pub fn received(&mut self, bytes: &[u8], now: Instant) {
        let mut cursor = mqtt::common::Cursor::new(bytes);
        while (cursor.position() as usize) < bytes.len() && !self.closed {
            let events = self.conn.recv(&mut cursor);
            self.handle(events, now);
        }
    }

    /// The stream ended or failed.
    pub fn lost(&mut self, why: &str, now: Instant) {
        if self.closed {
            return;
        }
        let events = self.conn.notify_closed();
        self.handle(events, now);
        self.close(why);
    }

    /// When the next timer is due, if any.
    pub fn next_timer(&self) -> Option<Instant> {
        self.timers.iter().map(|(_, at)| *at).min()
    }

    pub fn fire_timers(&mut self, now: Instant) {
        let due: Vec<TimerKind> = self
            .timers
            .iter()
            .filter(|(_, at)| *at <= now)
            .map(|(kind, _)| *kind)
            .collect();
        self.timers.retain(|(_, at)| *at > now);
        for kind in due {
            if kind == TimerKind::PingrespRecv {
                self.close("the broker did not answer a ping");
            }
            let events = self.conn.notify_timer_fired(kind);
            self.handle(events, now);
        }
    }

    /// What to write to the stream now.
    pub fn output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    pub fn has_output(&self) -> bool {
        !self.out.is_empty()
    }

    pub fn event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    fn close(&mut self, why: &str) {
        if !self.closed {
            self.closed = true;
            self.connected = false;
            self.timers.clear();
            self.events.push_back(Event::Closed(why.to_string()));
        }
    }

    fn handle(&mut self, events: Vec<Raw>, now: Instant) {
        for event in events {
            match event {
                Raw::RequestSendPacket { packet, .. } => {
                    self.out.extend_from_slice(&packet.to_continuous_buffer())
                }
                Raw::NotifyPacketReceived(packet) => self.packet(packet),
                Raw::NotifyPacketIdReleased(_) => {}
                Raw::RequestTimerReset { kind, duration_ms } => {
                    self.timers.retain(|(k, _)| *k != kind);
                    self.timers
                        .push((kind, now + Duration::from_millis(duration_ms)));
                }
                Raw::RequestTimerCancel(kind) => self.timers.retain(|(k, _)| *k != kind),
                Raw::NotifyError(error) => {
                    if !self.connected {
                        self.close(&format!("{error:?}"));
                    }
                }
                Raw::RequestClose => self.close("the connection was closed by protocol"),
            }
        }
    }

    fn packet(&mut self, packet: Packet) {
        match packet {
            Packet::V5_0Connack(connack) => {
                let code = connack.reason_code();
                if code == mqtt::result_code::ConnectReasonCode::Success {
                    self.connected = true;
                    self.events.push_back(Event::Connected);
                } else {
                    self.events
                        .push_back(Event::Refused(refusal(&format!("{code:?}"))));
                    self.close("the broker refused the connection");
                }
            }
            Packet::V5_0Publish(publish) => {
                let mut message =
                    Message::new(publish.topic_name(), publish.payload().as_slice().to_vec());
                message.qos = if publish.qos() == RawQos::AtMostOnce {
                    Qos::AtMostOnce
                } else {
                    Qos::AtLeastOnce
                };
                message.retain = publish.retain();
                for prop in publish.props() {
                    match prop {
                        Property::ContentType(p) => {
                            message.content_type = Some(p.val().to_string())
                        }
                        Property::ResponseTopic(p) => {
                            message.response_topic = Some(p.val().to_string())
                        }
                        Property::CorrelationData(p) => {
                            message.correlation = Some(p.val().to_vec())
                        }
                        Property::MessageExpiryInterval(p) => message.expiry = Some(p.val()),
                        Property::UserProperty(p) => message
                            .user
                            .push((p.key().to_string(), p.val().to_string())),
                        _ => {}
                    }
                }
                self.events.push_back(Event::Message(message));
            }
            Packet::V5_0Puback(puback) => self.events.push_back(Event::Acked(puback.packet_id())),
            Packet::V5_0Suback(suback) => {
                let id = suback.packet_id();
                let filters = self
                    .pending
                    .iter()
                    .position(|(pending, _)| *pending == id)
                    .map(|at| self.pending.remove(at).1)
                    .unwrap_or_default();
                let refused = suback
                    .reason_codes()
                    .iter()
                    .zip(filters.iter().map(Some).chain(std::iter::repeat(None)))
                    .filter(|(code, _)| (**code as u8) >= 0x80)
                    .map(|(code, filter)| match filter {
                        Some(filter) => format!("{filter}: {}", refusal(&format!("{code:?}"))),
                        None => refusal(&format!("{code:?}")),
                    })
                    .collect();
                self.events.push_back(Event::Subscribed { id, refused });
            }
            Packet::V5_0Disconnect(disconnect) => {
                let why = match disconnect.reason_code() {
                    Some(code) => {
                        format!("the broker disconnected: {}", refusal(&format!("{code:?}")))
                    }
                    None => "the broker disconnected".to_string(),
                };
                self.close(&why);
            }
            _ => {}
        }
    }
}

/// A reason code's name in words: `NotAuthorized` is "not authorized".
fn refusal(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

fn err(error: impl std::fmt::Debug) -> String {
    refusal(&format!("{error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options {
            client_id: "ab12".into(),
            username: Some("user".into()),
            password: Some(b"p$ss".to_vec()),
            keep_alive: 30,
            will: Some(
                Message::new("tessaro/ab12/status", br#"{"online":false}"#.to_vec()).retain(true),
            ),
        }
    }

    // A broker's side of the conversation, from the same library.
    struct Broker {
        conn: mqtt::Connection<mqtt::role::Server>,
    }

    impl Broker {
        fn new() -> Broker {
            Broker {
                conn: mqtt::Connection::<mqtt::role::Server>::new(mqtt::Version::V5_0),
            }
        }

        // Feeds the client's bytes; returns the packets the broker got.
        fn take(&mut self, bytes: &[u8]) -> Vec<Packet> {
            let mut cursor = mqtt::common::Cursor::new(bytes);
            let mut got = Vec::new();
            while (cursor.position() as usize) < bytes.len() {
                for event in self.conn.recv(&mut cursor) {
                    if let Raw::NotifyPacketReceived(packet) = event {
                        got.push(packet);
                    }
                }
            }
            got
        }

        fn send(&mut self, packet: impl Into<Packet>) -> Vec<u8> {
            let mut out = Vec::new();
            for event in self.conn.send(packet.into()) {
                if let Raw::RequestSendPacket { packet, .. } = event {
                    out.extend(packet.to_continuous_buffer());
                }
            }
            out
        }

        fn accept(&mut self) -> Vec<u8> {
            self.send(
                mqtt::packet::v5_0::Connack::builder()
                    .session_present(false)
                    .reason_code(mqtt::result_code::ConnectReasonCode::Success)
                    .build()
                    .unwrap(),
            )
        }
    }

    fn connected(now: Instant) -> (Session, Broker) {
        let mut session = Session::new();
        let mut broker = Broker::new();
        session.connect(&options(), now).unwrap();
        let got = broker.take(&session.output());
        match &got[..] {
            [Packet::V5_0Connect(connect)] => {
                assert_eq!(connect.client_id(), "ab12");
                assert_eq!(connect.user_name(), Some("user"));
                assert_eq!(connect.password(), Some(&b"p$ss"[..]));
                assert_eq!(connect.keep_alive(), 30);
                assert_eq!(connect.will_topic(), Some("tessaro/ab12/status"));
                assert!(connect.will_retain());
            }
            other => panic!("expected a CONNECT, got {other:?}"),
        }
        session.received(&broker.accept(), now);
        assert_eq!(session.event(), Some(Event::Connected));
        (session, broker)
    }

    #[test]
    fn it_connects_with_credentials_and_a_will() {
        let now = Instant::now();
        let (session, _) = connected(now);
        assert!(session.connected());
        assert!(
            session.next_timer().is_some(),
            "keep-alive starts a ping timer"
        );
    }

    #[test]
    fn a_refused_connect_says_why_and_closes() {
        let now = Instant::now();
        let mut session = Session::new();
        let mut broker = Broker::new();
        session.connect(&options(), now).unwrap();
        broker.take(&session.output());
        let refuse = broker.send(
            mqtt::packet::v5_0::Connack::builder()
                .session_present(false)
                .reason_code(mqtt::result_code::ConnectReasonCode::NotAuthorized)
                .build()
                .unwrap(),
        );
        session.received(&refuse, now);
        assert_eq!(
            session.event(),
            Some(Event::Refused("not authorized".into()))
        );
        assert!(matches!(session.event(), Some(Event::Closed(_))));
        assert!(!session.connected());
    }

    #[test]
    fn a_publish_carries_its_properties_and_is_acked() {
        let now = Instant::now();
        let (mut session, mut broker) = connected(now);
        let mut message = Message::new("tessaro/ab12/state/device", b"{}".to_vec())
            .retain(true)
            .content_type("application/json")
            .user("device", "ab12");
        message.response_topic = Some("tessaro/client/x/reply".into());
        message.correlation = Some(vec![1, 2, 3]);
        let id = session
            .publish(&message, now)
            .unwrap()
            .expect("QoS 1 has an id");
        let got = broker.take(&session.output());
        let [Packet::V5_0Publish(publish)] = &got[..] else {
            panic!("expected a PUBLISH, got {got:?}")
        };
        assert_eq!(publish.topic_name(), "tessaro/ab12/state/device");
        assert!(publish.retain());
        let ack = broker.send(
            mqtt::packet::v5_0::Puback::builder()
                .packet_id(id)
                .build()
                .unwrap(),
        );
        session.received(&ack, now);
        assert_eq!(session.event(), Some(Event::Acked(id)));
    }

    #[test]
    fn an_incoming_message_keeps_its_properties() {
        let now = Instant::now();
        let (mut client, mut broker) = connected(now);
        let id = broker.conn.acquire_packet_id().unwrap();
        let props: Vec<Property> = vec![
            mqtt::packet::ResponseTopic::new("tessaro/client/x/reply")
                .unwrap()
                .into(),
            mqtt::packet::CorrelationData::new(vec![9u8, 9])
                .unwrap()
                .into(),
            mqtt::packet::UserProperty::new("params", "{}")
                .unwrap()
                .into(),
        ];
        let publish = mqtt::packet::v5_0::Publish::builder()
            .topic_name("tessaro/ab12/cmd/device/Status")
            .unwrap()
            .qos(RawQos::AtLeastOnce)
            .packet_id(id)
            .props(props)
            .payload(&b"{}"[..])
            .build()
            .unwrap();
        let bytes = broker.send(publish);
        // Arrives in two pieces, as a stream may deliver it.
        let (head, tail) = bytes.split_at(5);
        client.received(head, now);
        assert_eq!(client.event(), None);
        client.received(tail, now);
        let Some(Event::Message(message)) = client.event() else {
            panic!("expected a message")
        };
        assert_eq!(message.topic, "tessaro/ab12/cmd/device/Status");
        assert_eq!(
            message.response_topic.as_deref(),
            Some("tessaro/client/x/reply")
        );
        assert_eq!(message.correlation, Some(vec![9, 9]));
        assert_eq!(message.user, vec![("params".to_string(), "{}".to_string())]);
        let acked = broker.take(&client.output());
        assert!(
            matches!(&acked[..], [Packet::V5_0Puback(_)]),
            "QoS 1 is acknowledged: {acked:?}"
        );
    }

    #[test]
    fn a_subscribe_names_the_filters_the_broker_refused() {
        let now = Instant::now();
        let (mut session, mut broker) = connected(now);
        let filters = vec![
            ("a/#".to_string(), Qos::AtLeastOnce),
            ("b/#".to_string(), Qos::AtLeastOnce),
        ];
        let id = session.subscribe(&filters, now).unwrap();
        broker.take(&session.output());
        let suback = broker.send(
            mqtt::packet::v5_0::Suback::builder()
                .packet_id(id)
                .reason_codes(vec![
                    mqtt::result_code::SubackReasonCode::GrantedQos1,
                    mqtt::result_code::SubackReasonCode::NotAuthorized,
                ])
                .build()
                .unwrap(),
        );
        session.received(&suback, now);
        assert_eq!(
            session.event(),
            Some(Event::Subscribed {
                id,
                refused: vec!["b/#: not authorized".into()]
            })
        );
    }

    #[test]
    fn an_unanswered_ping_closes_the_session() {
        let now = Instant::now();
        let (mut session, mut broker) = connected(now);
        let ping_at = session.next_timer().unwrap();
        session.fire_timers(ping_at);
        let got = broker.take(&session.output());
        assert!(
            matches!(&got[..], [Packet::V5_0Pingreq(_)]),
            "a ping is sent: {got:?}"
        );
        let give_up = session.next_timer().expect("a ping waits for its answer");
        session.fire_timers(give_up);
        assert_eq!(
            session.event(),
            Some(Event::Closed("the broker did not answer a ping".into()))
        );
        assert!(!session.connected());
    }

    #[test]
    fn publishing_while_away_is_refused() {
        let mut session = Session::new();
        assert!(session
            .publish(&Message::new("a", b"x".to_vec()), Instant::now())
            .is_err());
    }

    #[test]
    fn reason_codes_read_as_words() {
        assert_eq!(refusal("NotAuthorized"), "not authorized");
        assert_eq!(
            refusal("BadUserNameOrPassword"),
            "bad user name or password"
        );
    }
}
