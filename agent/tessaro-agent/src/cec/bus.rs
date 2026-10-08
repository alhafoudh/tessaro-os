//! The CEC bus as the agent follows it, with no I/O: messages in, what to
//! send and what to report out. `cec::io` does the talking, the control
//! plane's worker (`control/cec.rs`) puts the two together.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use protocol::{CecAdapter, CecDevice, CecPower};

/// The TV's logical address, always.
pub const TV: u8 = 0;
/// The unregistered address: the sender of a message from a device that has
/// no logical address, and the destination of a broadcast.
pub const UNREGISTERED: u8 = 15;
pub const BROADCAST: u8 = 15;

pub mod op {
    pub const FEATURE_ABORT: u8 = 0x00;
    pub const IMAGE_VIEW_ON: u8 = 0x04;
    pub const TEXT_VIEW_ON: u8 = 0x0d;
    pub const STANDBY: u8 = 0x36;
    pub const USER_CONTROL_PRESSED: u8 = 0x44;
    pub const USER_CONTROL_RELEASED: u8 = 0x45;
    pub const GIVE_OSD_NAME: u8 = 0x46;
    pub const SET_OSD_NAME: u8 = 0x47;
    pub const ROUTING_CHANGE: u8 = 0x80;
    pub const ROUTING_INFORMATION: u8 = 0x81;
    pub const ACTIVE_SOURCE: u8 = 0x82;
    pub const GIVE_PHYSICAL_ADDR: u8 = 0x83;
    pub const REPORT_PHYSICAL_ADDR: u8 = 0x84;
    pub const REQUEST_ACTIVE_SOURCE: u8 = 0x85;
    pub const SET_STREAM_PATH: u8 = 0x86;
    pub const DEVICE_VENDOR_ID: u8 = 0x87;
    pub const GIVE_DEVICE_VENDOR_ID: u8 = 0x8c;
    pub const MENU_REQUEST: u8 = 0x8d;
    pub const MENU_STATUS: u8 = 0x8e;
    pub const GIVE_DEVICE_POWER_STATUS: u8 = 0x8f;
    pub const REPORT_POWER_STATUS: u8 = 0x90;
    pub const INACTIVE_SOURCE: u8 = 0x9d;
    pub const CEC_VERSION: u8 = 0x9e;
    pub const ABORT: u8 = 0xff;
}

/// Feature Abort's reasons.
const UNRECOGNIZED_OPCODE: u8 = 0;

/// One CEC message: who to whom, the opcode, its operands. No opcode is a
/// poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    pub from: u8,
    pub to: u8,
    pub opcode: Option<u8>,
    pub args: Vec<u8>,
}

impl Msg {
    pub fn new(from: u8, to: u8, opcode: u8, args: &[u8]) -> Self {
        Self {
            from,
            to,
            opcode: Some(opcode),
            args: args.to_vec(),
        }
    }

    pub fn poll(from: u8, to: u8) -> Self {
        Self {
            from,
            to,
            opcode: None,
            args: Vec::new(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![(self.from << 4) | (self.to & 0xf)];
        if let Some(opcode) = self.opcode {
            bytes.push(opcode);
            bytes.extend(&self.args);
        }
        bytes.truncate(crate::cec::uapi::MAX_MSG_SIZE);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let header = *bytes.first()?;
        Some(Self {
            from: header >> 4,
            to: header & 0xf,
            opcode: bytes.get(1).copied(),
            args: bytes.get(2..).unwrap_or_default().to_vec(),
        })
    }

    fn broadcast(&self) -> bool {
        self.to == BROADCAST
    }

    /// The physical address in the operands from `at`.
    fn address(&self, at: usize) -> Option<u16> {
        Some(u16::from_be_bytes([
            *self.args.get(at)?,
            *self.args.get(at + 1)?,
        ]))
    }
}

// The names of addresses and makers are the clients' too.
pub use protocol::cec::{kind, physical, vendor};

fn power(status: u8) -> Option<CecPower> {
    match status {
        0 => Some(CecPower::On),
        1 => Some(CecPower::Standby),
        2 => Some(CecPower::TurningOn),
        3 => Some(CecPower::TurningOff),
        _ => None,
    }
}

/// Something that happened on the bus, for the page, the scripts and the
/// journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// One of `protocol::cec::EVENTS`.
    pub name: &'static str,
    /// For `key`: the key's name and code.
    pub key: Option<(String, u8)>,
    /// For `key`: pressed or released.
    pub pressed: bool,
    /// For `key`: pressed again while held.
    pub repeat: bool,
}

impl Event {
    fn plain(name: &'static str) -> Self {
        Self {
            name,
            key: None,
            pressed: false,
            repeat: false,
        }
    }

    fn key(code: u8, pressed: bool, repeat: bool) -> Self {
        Self {
            name: "key",
            key: Some((protocol::cec::key_name(code), code)),
            pressed,
            repeat,
        }
    }
}

/// What the bus asks of the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Send(Msg),
    Event(Event),
}

/// How the device is set up to behave right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    /// The screen is meant to be on: not switched off with `screen power`.
    pub screen_on: bool,
    /// `screen.cec.source` is `always`.
    pub keep_source: bool,
    pub now: Instant,
}

/// The least time between two inputs taken back with
/// `screen.cec.source=always`, so two devices set that way cannot take it
/// from each other in a loop.
pub const TAKE_BACK_EVERY: Duration = Duration::from_secs(10);

/// How long a remote key counts as held after its last `<User Control
/// Pressed>`: TVs repeat it every 500ms or less while held, and the
/// standard's 550ms is the longest gap before a missing release is assumed.
pub const KEY_HELD: Duration = Duration::from_millis(550);

/// One adapter's view of its bus.
#[derive(Debug, Clone, Default)]
pub struct Bus {
    /// The device's physical address; `None` while the TV gives none.
    pub physical: Option<u16>,
    /// The logical address the device claimed.
    pub logical: Option<u8>,
    /// The TV shows the device's input.
    pub active: bool,
    /// The TV's power, as last reported.
    pub tv: Option<CecPower>,
    /// Every other device that answered, by logical address.
    pub devices: BTreeMap<u8, CecDevice>,
    /// The remote key held down, and when it was last pressed.
    held: Option<(u8, Instant)>,
    /// When the input was last taken back.
    taken: Option<Instant>,
}

impl Bus {
    fn from(&self) -> u8 {
        self.logical.unwrap_or(UNREGISTERED)
    }

    fn device(&mut self, address: u8) -> &mut CecDevice {
        self.devices.entry(address).or_insert_with(|| CecDevice {
            address,
            kind: kind(address).to_string(),
            physical: None,
            name: None,
            vendor: None,
            power: None,
        })
    }

    /// The addresses the adapter has, as the kernel reports them.
    pub fn addresses(&mut self, physical: u16, logical: Option<u8>) {
        self.physical = (physical != crate::cec::uapi::PHYS_ADDR_INVALID).then_some(physical);
        self.logical = logical;
    }

    /// Wake the TV, and with `source` switch it to the device's input.
    pub fn wake(&mut self, source: bool) -> Vec<Out> {
        // Image View On is the one message a device without a logical
        // address may send: a TV that drops hot-plug in standby leaves the
        // device none until it wakes.
        let mut out = vec![Out::Send(Msg::new(self.from(), TV, op::IMAGE_VIEW_ON, &[]))];
        if source {
            out.extend(self.take_source());
        }
        out
    }

    /// Put the TV in standby: the TV only, never a broadcast, which would
    /// switch off a sound bar or a receiver too.
    pub fn standby(&mut self) -> Vec<Out> {
        vec![Out::Send(Msg::new(self.from(), TV, op::STANDBY, &[]))]
    }

    /// Put everything on the bus in standby: `screen cec standby --all`.
    pub fn standby_all(&mut self) -> Vec<Out> {
        vec![Out::Send(Msg::new(
            self.from(),
            BROADCAST,
            op::STANDBY,
            &[],
        ))]
    }

    /// A remote key pressed and let go, as the remote would send it to
    /// `to`: the TV, or an audio system for its volume.
    pub fn key(&self, code: u8, to: u8) -> Vec<Out> {
        vec![
            Out::Send(Msg::new(self.from(), to, op::USER_CONTROL_PRESSED, &[code])),
            Out::Send(Msg::new(self.from(), to, op::USER_CONTROL_RELEASED, &[])),
        ]
    }

    /// Any message: `data` is the opcode and its operands, the header is
    /// the device's address and `to`.
    pub fn raw(&self, to: u8, data: &[u8]) -> Vec<Out> {
        vec![Out::Send(Msg {
            from: self.from(),
            to,
            opcode: data.first().copied(),
            args: data.get(1..).unwrap_or_default().to_vec(),
        })]
    }

    /// Ask the TV for its power.
    pub fn ask_power(&self) -> Vec<Out> {
        match self.logical {
            Some(from) => vec![Out::Send(Msg::new(
                from,
                TV,
                op::GIVE_DEVICE_POWER_STATUS,
                &[],
            ))],
            None => Vec::new(),
        }
    }

    /// Ask a device that answered a poll what it is.
    pub fn ask_about(&self, address: u8) -> Vec<Out> {
        let Some(from) = self.logical else {
            return Vec::new();
        };
        let mut out = vec![
            Out::Send(Msg::new(from, address, op::GIVE_PHYSICAL_ADDR, &[])),
            Out::Send(Msg::new(from, address, op::GIVE_OSD_NAME, &[])),
            Out::Send(Msg::new(from, address, op::GIVE_DEVICE_VENDOR_ID, &[])),
        ];
        if address == TV {
            out.push(Out::Send(Msg::new(
                from,
                TV,
                op::GIVE_DEVICE_POWER_STATUS,
                &[],
            )));
        }
        out
    }

    /// The devices that answered a round of polls; the rest are gone.
    pub fn present(&mut self, answered: &[u8]) {
        self.devices.retain(|address, _| answered.contains(address));
        for address in answered {
            self.device(*address);
        }
    }

    /// Broadcast `<Active Source>`: the TV switches to the device.
    pub fn take_source(&mut self) -> Vec<Out> {
        let (Some(from), Some(physical)) = (self.logical, self.physical) else {
            return Vec::new();
        };
        let mut out = vec![Out::Send(Msg::new(
            from,
            BROADCAST,
            op::ACTIVE_SOURCE,
            &physical.to_be_bytes(),
        ))];
        out.extend(self.set_active(true));
        out
    }

    fn set_active(&mut self, active: bool) -> Vec<Out> {
        if self.active == active {
            return Vec::new();
        }
        self.active = active;
        vec![Out::Event(Event::plain(if active {
            "source-gained"
        } else {
            "source-lost"
        }))]
    }

    fn set_tv(&mut self, power: CecPower) -> Vec<Out> {
        let before = self.tv.replace(power);
        self.device(TV).power = Some(power);
        // Only a change between known states is news: the first answer is
        // where the device starts from.
        match (before, power) {
            (Some(before), CecPower::On) if before != CecPower::On => {
                vec![Out::Event(Event::plain("tv-on"))]
            }
            (Some(before), CecPower::Standby) if before != CecPower::Standby => {
                vec![Out::Event(Event::plain("tv-standby"))]
            }
            _ => Vec::new(),
        }
    }

    /// Someone else's `<Active Source>`, or a switch's route away: the
    /// device lost the TV's input, and with `keep_source` takes it back.
    fn source_moved(&mut self, to: u16, context: Context) -> Vec<Out> {
        if Some(to) == self.physical {
            return self.set_active(true);
        }
        let mut out = self.set_active(false);
        let lost = !out.is_empty();
        let due = self
            .taken
            .is_none_or(|taken| context.now.duration_since(taken) >= TAKE_BACK_EVERY);
        if lost && context.keep_source && context.screen_on && due {
            self.taken = Some(context.now);
            out.extend(self.take_source());
        }
        out
    }

    /// A key held past `KEY_HELD` with no release: released now.
    pub fn release_late(&mut self, now: Instant) -> Vec<Out> {
        match self.held {
            Some((code, at)) if now.duration_since(at) > KEY_HELD => {
                self.held = None;
                vec![Out::Event(Event::key(code, false, false))]
            }
            _ => Vec::new(),
        }
    }

    /// A message from the bus.
    pub fn received(&mut self, msg: &Msg, context: Context) -> Vec<Out> {
        let Some(opcode) = msg.opcode else {
            return Vec::new();
        };
        let ours = self.logical.is_some_and(|ours| msg.to == ours);
        let from = msg.from;
        let mut out = Vec::new();

        // The TV choosing a source or sending keys is a TV that is on.
        if from == TV
            && matches!(
                opcode,
                op::ACTIVE_SOURCE
                    | op::SET_STREAM_PATH
                    | op::ROUTING_CHANGE
                    | op::REQUEST_ACTIVE_SOURCE
                    | op::USER_CONTROL_PRESSED
                    | op::MENU_REQUEST
            )
        {
            out.extend(self.set_tv(CecPower::On));
        }

        match opcode {
            op::STANDBY if from == TV || msg.broadcast() => {
                out.extend(self.set_tv(CecPower::Standby));
                out.extend(self.set_active(false));
            }
            op::REPORT_POWER_STATUS => {
                if let Some(power) = msg.args.first().and_then(|status| power(*status)) {
                    if from == TV {
                        out.extend(self.set_tv(power));
                    } else {
                        self.device(from).power = Some(power);
                    }
                }
            }
            op::ACTIVE_SOURCE => {
                if let Some(address) = msg.address(0) {
                    if from != TV {
                        self.device(from).physical = Some(physical(address));
                    }
                    out.extend(self.source_moved(address, context));
                }
            }
            op::ROUTING_CHANGE => {
                if let Some(address) = msg.address(2) {
                    out.extend(self.routed(address, context));
                }
            }
            op::ROUTING_INFORMATION => {
                if let Some(address) = msg.address(0) {
                    out.extend(self.routed(address, context));
                }
            }
            op::SET_STREAM_PATH => {
                if let Some(address) = msg.address(0) {
                    out.extend(self.routed(address, context));
                }
            }
            op::REQUEST_ACTIVE_SOURCE if self.active => out.extend(self.take_source()),
            op::REQUEST_ACTIVE_SOURCE => {}
            op::INACTIVE_SOURCE => {}
            op::REPORT_PHYSICAL_ADDR => {
                if let Some(address) = msg.address(0) {
                    self.device(from).physical = Some(physical(address));
                }
            }
            op::SET_OSD_NAME => {
                let name: String = msg
                    .args
                    .iter()
                    .filter(|b| b.is_ascii_graphic() || **b == b' ')
                    .map(|b| *b as char)
                    .collect();
                if !name.trim().is_empty() {
                    self.device(from).name = Some(name.trim().to_string());
                }
            }
            op::DEVICE_VENDOR_ID => {
                if let [a, b, c, ..] = msg.args[..] {
                    let oui = u32::from_be_bytes([0, a, b, c]);
                    self.device(from).vendor = Some(vendor(oui));
                }
            }
            op::GIVE_DEVICE_POWER_STATUS if ours => {
                let status = if context.screen_on { 0 } else { 1 };
                out.push(Out::Send(Msg::new(
                    self.from(),
                    from,
                    op::REPORT_POWER_STATUS,
                    &[status],
                )));
            }
            op::MENU_REQUEST if ours => {
                // Activated: what makes a TV send its remote's keys on.
                out.push(Out::Send(Msg::new(
                    self.from(),
                    from,
                    op::MENU_STATUS,
                    &[0],
                )));
            }
            op::USER_CONTROL_PRESSED if ours => {
                if let Some(&code) = msg.args.first() {
                    let repeat = self.held.is_some_and(|(held, _)| held == code);
                    if let Some((held, _)) = self.held.filter(|(held, _)| *held != code) {
                        out.push(Out::Event(Event::key(held, false, false)));
                    }
                    self.held = Some((code, context.now));
                    out.push(Out::Event(Event::key(code, true, repeat)));
                }
            }
            op::USER_CONTROL_RELEASED if ours => {
                if let Some((code, _)) = self.held.take() {
                    out.push(Out::Event(Event::key(code, false, false)));
                }
            }
            // Answers to what the device asked, and what needs none.
            op::FEATURE_ABORT
            | op::STANDBY
            | op::MENU_STATUS
            | op::CEC_VERSION
            | op::IMAGE_VIEW_ON
            | op::TEXT_VIEW_ON => {}
            other if ours => {
                out.push(Out::Send(Msg::new(
                    self.from(),
                    from,
                    op::FEATURE_ABORT,
                    &[
                        if other == op::ABORT { op::ABORT } else { other },
                        UNRECOGNIZED_OPCODE,
                    ],
                )));
            }
            _ => {}
        }
        out
    }

    /// The TV or a switch chose a path: the device's own means it shows
    /// now, and says so as the active source.
    fn routed(&mut self, to: u16, context: Context) -> Vec<Out> {
        if Some(to) == self.physical {
            self.take_source()
        } else {
            self.source_moved(to, context)
        }
    }

    /// The adapter and its bus as `screen show` lists them.
    pub fn snapshot(&self, device: &str, connector: Option<String>, name: &str) -> CecAdapter {
        let mut devices: Vec<CecDevice> = self
            .devices
            .values()
            .filter(|device| Some(device.address) != self.logical)
            .cloned()
            .collect();
        devices.sort_by_key(|device| device.address);
        CecAdapter {
            device: device.to_string(),
            connector,
            address: self.logical,
            physical: self.physical.map(physical),
            name: name.to_string(),
            active: self.active,
            tv: self.tv,
            devices,
            problem: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus() -> Bus {
        let mut bus = Bus::default();
        bus.addresses(0x1000, Some(4));
        bus
    }

    fn at(now: Instant) -> Context {
        Context {
            screen_on: true,
            keep_source: false,
            now,
        }
    }

    fn sent(out: &[Out]) -> Vec<Vec<u8>> {
        out.iter()
            .filter_map(|out| match out {
                Out::Send(msg) => Some(msg.encode()),
                Out::Event(_) => None,
            })
            .collect()
    }

    fn events(out: &[Out]) -> Vec<&'static str> {
        out.iter()
            .filter_map(|out| match out {
                Out::Event(event) => Some(event.name),
                Out::Send(_) => None,
            })
            .collect()
    }

    #[test]
    fn messages_round_trip() {
        let msg = Msg::new(4, 0, op::REPORT_POWER_STATUS, &[1]);
        assert_eq!(msg.encode(), vec![0x40, 0x90, 0x01]);
        assert_eq!(Msg::decode(&[0x40, 0x90, 0x01]), Some(msg));
        assert_eq!(Msg::poll(4, 4).encode(), vec![0x44]);
        assert_eq!(Msg::decode(&[0x0f]).unwrap().opcode, None);
        assert_eq!(physical(0x1200), "1.2.0.0");
    }

    #[test]
    fn waking_switches_the_input_only_when_asked() {
        let mut bus = bus();
        assert_eq!(sent(&bus.wake(false)), vec![vec![0x40, 0x04]]);
        let out = bus.wake(true);
        assert_eq!(
            sent(&out),
            vec![vec![0x40, 0x04], vec![0x4f, 0x82, 0x10, 0x00]]
        );
        assert_eq!(events(&out), vec!["source-gained"]);
        assert!(bus.active);
    }

    #[test]
    fn without_an_address_only_the_wake_goes_out() {
        let mut bus = Bus::default();
        bus.addresses(crate::cec::uapi::PHYS_ADDR_INVALID, None);
        assert_eq!(sent(&bus.wake(true)), vec![vec![0xf0, 0x04]]);
        assert_eq!(sent(&bus.standby()), vec![vec![0xf0, 0x36]]);
        assert!(bus.ask_power().is_empty());
    }

    #[test]
    fn keys_standby_for_all_and_raw_messages_are_what_is_asked() {
        let mut bus = bus();
        assert_eq!(sent(&bus.standby_all()), vec![vec![0x4f, 0x36]]);
        assert_eq!(
            sent(&bus.key(0x41, protocol::cec::AUDIO)),
            vec![vec![0x45, 0x44, 0x41], vec![0x45, 0x45]]
        );
        assert_eq!(sent(&bus.raw(0, &[0x8f])), vec![vec![0x40, 0x8f]]);
        assert_eq!(
            sent(&bus.raw(15, &[0x89, 1, 2])),
            vec![vec![0x4f, 0x89, 1, 2]]
        );
    }

    #[test]
    fn standby_goes_to_the_tv_alone() {
        assert_eq!(sent(&bus().standby()), vec![vec![0x40, 0x36]]);
    }

    #[test]
    fn the_tv_changing_power_is_an_event_after_the_first_answer() {
        let mut bus = bus();
        let now = Instant::now();
        let report = |status| Msg::new(TV, 4, op::REPORT_POWER_STATUS, &[status]);
        assert!(events(&bus.received(&report(0), at(now))).is_empty());
        assert_eq!(
            events(&bus.received(&Msg::new(TV, BROADCAST, op::STANDBY, &[]), at(now))),
            vec!["tv-standby"]
        );
        assert_eq!(
            events(&bus.received(&report(2), at(now))),
            Vec::<&str>::new()
        );
        assert_eq!(events(&bus.received(&report(0), at(now))), vec!["tv-on"]);
        assert_eq!(bus.tv, Some(CecPower::On));
    }

    #[test]
    fn the_input_is_followed_and_taken_back_with_always() {
        let mut bus = bus();
        let now = Instant::now();
        bus.wake(true);
        let other = Msg::new(8, BROADCAST, op::ACTIVE_SOURCE, &[0x20, 0x00]);

        let out = bus.received(&other, at(now));
        assert_eq!(events(&out), vec!["source-lost"]);
        assert!(sent(&out).is_empty());

        let keep = Context {
            keep_source: true,
            ..at(now)
        };
        bus.active = true;
        let out = bus.received(&other, keep);
        assert_eq!(events(&out), vec!["source-lost", "source-gained"]);
        assert_eq!(sent(&out), vec![vec![0x4f, 0x82, 0x10, 0x00]]);

        // Not again within TAKE_BACK_EVERY.
        let out = bus.received(&other, keep);
        assert_eq!(events(&out), vec!["source-lost"]);
        let later = Context {
            now: now + TAKE_BACK_EVERY,
            ..keep
        };
        bus.active = true;
        assert_eq!(
            events(&bus.received(&other, later)),
            vec!["source-lost", "source-gained"]
        );

        // Never while the screen is meant to be off.
        let off = Context {
            screen_on: false,
            now: now + TAKE_BACK_EVERY * 2,
            ..keep
        };
        bus.active = true;
        assert_eq!(events(&bus.received(&other, off)), vec!["source-lost"]);
    }

    #[test]
    fn a_stream_path_to_the_device_makes_it_the_source() {
        let mut bus = bus();
        let out = bus.received(
            &Msg::new(TV, BROADCAST, op::SET_STREAM_PATH, &[0x10, 0x00]),
            at(Instant::now()),
        );
        assert_eq!(sent(&out), vec![vec![0x4f, 0x82, 0x10, 0x00]]);
        assert_eq!(events(&out), vec!["source-gained"]);
        let out = bus.received(
            &Msg::new(TV, BROADCAST, op::REQUEST_ACTIVE_SOURCE, &[]),
            at(Instant::now()),
        );
        assert_eq!(sent(&out), vec![vec![0x4f, 0x82, 0x10, 0x00]]);
    }

    #[test]
    fn keys_are_pressed_repeated_and_released() {
        let mut bus = bus();
        let now = Instant::now();
        let press = |code| Msg::new(TV, 4, op::USER_CONTROL_PRESSED, &[code]);
        let key = |out: &[Out]| match out.last() {
            Some(Out::Event(event)) => event.clone(),
            other => panic!("{other:?}"),
        };

        let first = key(&bus.received(&press(0x01), at(now)));
        assert_eq!(
            (first.key.unwrap().0.as_str(), first.pressed, first.repeat),
            ("up", true, false)
        );
        assert!(key(&bus.received(&press(0x01), at(now))).repeat);

        // Another key releases the first.
        let out = bus.received(&press(0x00), at(now));
        assert_eq!(out.len(), 2);
        let released =
            key(&bus.received(&Msg::new(TV, 4, op::USER_CONTROL_RELEASED, &[]), at(now)));
        assert_eq!(
            (released.key.unwrap().0.as_str(), released.pressed),
            ("select", false)
        );

        // A lost release is assumed after KEY_HELD.
        bus.received(&press(0x72), at(now));
        assert!(bus.release_late(now).is_empty());
        let late = bus.release_late(now + KEY_HELD + Duration::from_millis(1));
        assert_eq!(key(&late).key.unwrap().0, "red");
    }

    #[test]
    fn the_device_answers_for_itself_and_refuses_the_rest() {
        let mut bus = bus();
        let off = Context {
            screen_on: false,
            ..at(Instant::now())
        };
        assert_eq!(
            sent(&bus.received(&Msg::new(TV, 4, op::GIVE_DEVICE_POWER_STATUS, &[]), off)),
            vec![vec![0x40, 0x90, 0x01]]
        );
        assert_eq!(
            sent(&bus.received(&Msg::new(TV, 4, op::MENU_REQUEST, &[0]), off)),
            vec![vec![0x40, 0x8e, 0x00]]
        );
        assert_eq!(
            sent(&bus.received(&Msg::new(TV, 4, 0x1a, &[1]), off)),
            vec![vec![0x40, 0x00, 0x1a, 0x00]]
        );
        // Not to the device, and broadcasts, get nothing.
        assert!(sent(&bus.received(&Msg::new(TV, 8, 0x1a, &[1]), off)).is_empty());
        assert!(sent(&bus.received(&Msg::new(TV, BROADCAST, 0x1a, &[1]), off)).is_empty());
    }

    #[test]
    fn what_devices_say_about_themselves_is_kept() {
        let mut bus = bus();
        let now = at(Instant::now());
        bus.present(&[0, 5]);
        bus.received(&Msg::new(TV, 4, op::SET_OSD_NAME, b"TV"), now);
        bus.received(
            &Msg::new(TV, BROADCAST, op::DEVICE_VENDOR_ID, &[0x00, 0x00, 0xf0]),
            now,
        );
        bus.received(
            &Msg::new(5, BROADCAST, op::REPORT_PHYSICAL_ADDR, &[0x20, 0x00, 5]),
            now,
        );
        bus.received(
            &Msg::new(5, BROADCAST, op::DEVICE_VENDOR_ID, &[0x12, 0x34, 0x56]),
            now,
        );

        let snapshot = bus.snapshot("/dev/cec0", Some("HDMI-A-1".into()), "lobby");
        assert_eq!(snapshot.physical.as_deref(), Some("1.0.0.0"));
        assert_eq!(snapshot.devices.len(), 2);
        assert_eq!(snapshot.devices[0].name.as_deref(), Some("TV"));
        assert_eq!(snapshot.devices[0].vendor.as_deref(), Some("Samsung"));
        assert_eq!(snapshot.devices[1].kind, "audio");
        assert_eq!(snapshot.devices[1].physical.as_deref(), Some("2.0.0.0"));
        assert_eq!(snapshot.devices[1].vendor.as_deref(), Some("123456"));

        bus.present(&[0]);
        assert_eq!(bus.snapshot("/dev/cec0", None, "x").devices.len(), 1);
    }
}
