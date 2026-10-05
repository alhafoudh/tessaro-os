//! The main window: every device this machine knows (`nodes.rs`) and
//! every one answering on the network (mDNS), in one list keyed by node id.
//!
//! A device is opened only once it is pinned and has a token. Getting there
//! is the same as with `tessaro-ctl access login|claim`: the device is
//! peeked at first (nothing secret is sent), the dialog shows its
//! certificate's fingerprint, and pinning accepts exactly that fingerprint -
//! if another one answers by the time the token goes out, nothing is sent.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use iced::widget::{column, container, text, text_input};
use iced::{Element, Length, Task};
use protocol::{Claimed, NodeInfo};
use tessaro_client::connect::{self, Found, PinAsk, Target, Trust};
use tessaro_client::nodes::{Node, Nodes};

use crate::dialog::{self, field};
use crate::grid::{cell, col, grid, Col};
use crate::section::{self, action};
use crate::{blocking, discovery, theme, CLIENT};

/// Whether this machine's pin and what the device presents agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pin {
    Known,
    /// Not among the known nodes: login or claim pins it.
    New,
    /// Presents another certificate than the one pinned: reinstalled, or
    /// someone in the middle. Forget it to pin it again.
    Mismatch,
}

/// A line in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The node id, or `name@address` for an announcement without one.
    pub key: String,
    pub id: Option<String>,
    pub name: String,
    pub address: String,
    pub claimed: Option<bool>,
    pub pin: Pin,
    pub token: bool,
    /// Announced by mDNS, or answered a peek this session. False only means
    /// nobody has heard from it, not that it is down.
    pub online: bool,
}

/// The known nodes, overlaid with what is seen on the network: a known
/// device that answers takes its live address and state, and one nobody
/// knows gets a row of its own.
pub fn merge<'a>(known: &Nodes, found: impl IntoIterator<Item = &'a Found>) -> Vec<Row> {
    let mut rows: Vec<Row> = known
        .nodes
        .iter()
        .map(|node| Row {
            key: node.id.clone(),
            id: Some(node.id.clone()),
            name: node.name.clone(),
            address: node.address.clone(),
            claimed: None,
            pin: Pin::Known,
            token: node.token.is_some(),
            online: false,
        })
        .collect();

    for found in found {
        let same = |row: &Row| match (&found.id, &row.id) {
            (Some(found), Some(row)) => found == row,
            _ => row.key == format!("{}@{}", found.name, found.address),
        };
        let at = match rows.iter().position(same) {
            Some(at) => at,
            None => {
                rows.push(Row {
                    key: found
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("{}@{}", found.name, found.address)),
                    id: found.id.clone(),
                    name: found.name.clone(),
                    address: String::new(),
                    claimed: None,
                    pin: Pin::New,
                    token: false,
                    online: false,
                });
                rows.len() - 1
            }
        };
        let row = &mut rows[at];
        row.online = true;
        row.name.clone_from(&found.name);
        row.address = found.address.to_string();
        row.claimed = found.claimed.or(row.claimed);
        let pinned = row.id.as_deref().and_then(|id| known.by_id(id));
        if let (Some(node), Some(fingerprint)) = (pinned, &found.fingerprint) {
            if &node.fingerprint != fingerprint {
                row.pin = Pin::Mismatch;
            }
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name).then(a.key.cmp(&b.key)));
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Login,
    Claim,
}

/// Where a login or claim goes.
#[derive(Debug, Clone)]
pub struct Aim {
    address: SocketAddr,
    expected: Option<String>,
    label: String,
}

/// A device looked at without pinning it.
#[derive(Debug, Clone)]
pub struct Peek {
    node: NodeInfo,
    address: SocketAddr,
    fingerprint: String,
    /// Already among the known nodes (a login to a known device).
    pinned: bool,
}

#[derive(Debug, Clone)]
pub enum Outcome {
    LoggedIn(String),
    Claimed(String, Box<Claimed>),
}

enum Dialog {
    Address {
        input: String,
        error: Option<String>,
        busy: bool,
    },
    Access {
        mode: Mode,
        aim: Aim,
        peek: Option<Box<Peek>>,
        token: String,
        name: String,
        error: Option<String>,
        busy: bool,
    },
    Forget {
        id: String,
        name: String,
    },
    /// What a claim returns once: shown until the user closes it.
    Secrets {
        id: String,
        name: String,
        claimed: Box<Claimed>,
    },
}

#[derive(Debug, Clone)]
pub enum Message {
    Table(crate::grid::Event),
    Select(String),
    Activate(String),
    Filter(String),
    Open,
    Login,
    Claim,
    Forget,
    AddAddress,
    Rescan,
    AddressInput(String),
    AddressSubmit,
    Added(Result<Peek, String>),
    Peeked(Result<Peek, String>),
    TokenInput(String),
    NameInput(String),
    Commit,
    Committed(Result<Outcome, String>),
    ForgetConfirmed,
    SecretsDone,
    Copy(String),
    Cancel,
    /// Up (-1) or Down (1) in the table.
    Step(i32),
    Enter,
}

pub struct NodesView {
    pub(super) tables: crate::grid::Tables,
    known: Nodes,
    /// Announced on the network, by mDNS name.
    seen: BTreeMap<String, Found>,
    /// Answered a peek (added by address, or a login or claim dialog): its
    /// live address and claimed state, kept for the session.
    added: Vec<Found>,
    selected: Option<String>,
    filter: String,
    dialog: Option<Dialog>,
    /// Each dialog here has one field to type in, at 0.
    fields: dialog::Fields,
    discovery: Option<String>,
    message: Option<Result<String, String>>,
    /// Bumped by Rescan: a new browse.
    pub generation: u64,
}

impl NodesView {
    pub fn new() -> Self {
        let (known, message) = match Nodes::load() {
            Ok(known) => (known, None),
            Err(error) => (Nodes::default(), Some(Err(error))),
        };
        Self {
            tables: crate::grid::Tables::default(),
            known,
            seen: BTreeMap::new(),
            added: Vec::new(),
            selected: None,
            filter: String::new(),
            dialog: None,
            fields: dialog::Fields::default(),
            discovery: None,
            message,
            generation: 0,
        }
    }

    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    pub fn rows(&self) -> Vec<Row> {
        merge(&self.known, self.seen.values().chain(&self.added))
    }

    fn selected_row(&self) -> Option<Row> {
        let key = self.selected.as_ref()?;
        self.rows().into_iter().find(|row| &row.key == key)
    }

    /// The node behind a row, if it can be opened: pinned, the pin holding,
    /// and a token to send - or unclaimed, which needs neither.
    fn openable(&self, row: &Row) -> Option<Node> {
        if row.pin == Pin::Mismatch {
            return None;
        }
        let id = row.id.as_deref()?;
        let unclaimed = row.claimed == Some(false);
        match self.known.by_id(id) {
            Some(node) if row.token || unclaimed => Some(node.clone()),
            None if unclaimed && row.online => Some(Node {
                id: id.to_string(),
                name: row.name.clone(),
                address: row.address.clone(),
                fingerprint: String::new(),
                token: None,
            }),
            _ => None,
        }
    }

    pub fn discovered(&mut self, event: discovery::Event) {
        match event {
            discovery::Event::Seen(found) => {
                self.seen.insert(found.name.clone(), found);
            }
            discovery::Event::Gone(name) => {
                self.seen.remove(&name);
            }
            discovery::Event::Failed(why) => self.discovery = Some(why),
        }
    }

    /// Read the known nodes again: a device window may have moved a node.
    pub fn reload(&mut self) {
        if let Ok(known) = Nodes::load() {
            self.known = known;
        }
    }

    /// The task to run, and a node to open a window for.
    pub fn update(&mut self, message: Message) -> (Task<Message>, Option<Node>) {
        let none = (Task::none(), None);
        match message {
            Message::Table(event) => (self.tables.update("nodes", event), None),
            Message::Select(key) => {
                self.selected = Some(key);
                none
            }
            Message::Activate(key) => {
                self.selected = Some(key);
                let Some(row) = self.selected_row() else {
                    return none;
                };
                match self.openable(&row) {
                    Some(node) => (Task::none(), Some(node)),
                    None if row.pin == Pin::Mismatch => {
                        self.message = Some(Err(mismatch(&row)));
                        none
                    }
                    None => self.access(Mode::Login),
                }
            }
            Message::Filter(filter) => {
                self.filter = filter;
                none
            }
            Message::Open => {
                let node = self.selected_row().and_then(|row| self.openable(&row));
                (Task::none(), node)
            }
            Message::Login => self.access(Mode::Login),
            Message::Claim => self.access(Mode::Claim),
            Message::Forget => {
                if let Some(row) = self.selected_row() {
                    if let Some(id) = row.id.filter(|id| self.known.by_id(id).is_some()) {
                        self.dialog = Some(Dialog::Forget { id, name: row.name });
                    }
                }
                none
            }
            Message::ForgetConfirmed => {
                if let Some(Dialog::Forget { id, name }) = self.dialog.take() {
                    // Only its row goes: what a device window stored since
                    // this copy was read stays.
                    self.message = Some(
                        self.known
                            .forget(&id)
                            .map(|_| format!("forgot {name} on this machine")),
                    );
                    self.reload();
                }
                none
            }
            Message::AddAddress => {
                self.dialog = Some(Dialog::Address {
                    input: String::new(),
                    error: None,
                    busy: false,
                });
                (self.fields.focus(0), None)
            }
            Message::AddressInput(value) => {
                if let Some(Dialog::Address { input, error, .. }) = &mut self.dialog {
                    *input = value;
                    *error = None;
                }
                none
            }
            Message::AddressSubmit => {
                let Some(Dialog::Address { input, error, busy }) = &mut self.dialog else {
                    return none;
                };
                match aim(input.trim(), &self.known) {
                    Ok(aim) => {
                        *busy = true;
                        (
                            Task::perform(blocking::run(move || peek(aim)), Message::Added),
                            None,
                        )
                    }
                    Err(why) => {
                        *error = Some(why);
                        none
                    }
                }
            }
            Message::Added(result) => {
                let Some(Dialog::Address { error, busy, .. }) = &mut self.dialog else {
                    return none;
                };
                *busy = false;
                match result {
                    Ok(peek) => {
                        self.dialog = None;
                        self.answered(&peek);
                        if let Err(why) = self.keep(&peek) {
                            self.message = Some(Err(why));
                        }
                        self.selected = Some(peek.node.id);
                    }
                    Err(why) => *error = Some(why),
                }
                none
            }
            Message::Rescan => {
                self.seen.clear();
                self.discovery = None;
                self.generation += 1;
                self.reload();
                none
            }
            Message::Peeked(result) => {
                let Some(Dialog::Access {
                    mode,
                    peek,
                    error,
                    busy,
                    ..
                }) = &mut self.dialog
                else {
                    return none;
                };
                *busy = false;
                let peeked = match result {
                    Ok(peeked) => peeked,
                    Err(why) => {
                        *error = Some(why);
                        return none;
                    }
                };
                *peek = Some(Box::new(peeked.clone()));
                let login = *mode == Mode::Login;
                self.answered(&peeked);
                // A known device that turns out unclaimed has nothing to log
                // in to: open it, as a double click on its row would.
                if login && !peeked.node.claimed {
                    let node = self.selected_row().and_then(|row| self.openable(&row));
                    if node.is_some() {
                        self.dialog = None;
                        return (Task::none(), node);
                    }
                }
                // The token or name field shows only now, with the answer.
                (self.fields.focus(0), None)
            }
            Message::TokenInput(value) => {
                if let Some(Dialog::Access { token, error, .. }) = &mut self.dialog {
                    *token = value;
                    *error = None;
                }
                none
            }
            Message::NameInput(value) => {
                if let Some(Dialog::Access { name, error, .. }) = &mut self.dialog {
                    *name = value;
                    *error = None;
                }
                none
            }
            Message::Commit => {
                let Some(Dialog::Access {
                    mode,
                    peek: Some(peek),
                    token,
                    name,
                    busy,
                    ..
                }) = &mut self.dialog
                else {
                    return none;
                };
                if *busy || (*mode == Mode::Login && token.trim().is_empty()) {
                    return none;
                }
                *busy = true;
                let (mode, peek, token, name) = (
                    *mode,
                    Peek::clone(peek),
                    token.trim().to_string(),
                    name.clone(),
                );
                (
                    Task::perform(
                        blocking::run(move || commit(mode, peek, token, name)),
                        Message::Committed,
                    ),
                    None,
                )
            }
            Message::Committed(result) => {
                let Some(Dialog::Access {
                    error, busy, peek, ..
                }) = &mut self.dialog
                else {
                    return none;
                };
                *busy = false;
                let id = peek.as_ref().map(|peek| peek.node.id.clone());
                match result {
                    Err(why) => {
                        *error = Some(why);
                        none
                    }
                    Ok(Outcome::LoggedIn(name)) => {
                        self.dialog = None;
                        self.reload();
                        self.message = Some(Ok(format!("logged in to {name}")));
                        let node = id.and_then(|id| self.known.by_id(&id).cloned());
                        (Task::none(), node)
                    }
                    Ok(Outcome::Claimed(name, claimed)) => {
                        self.reload();
                        self.message = Some(Ok(format!("claimed {name}")));
                        self.dialog = id.map(|id| Dialog::Secrets { id, name, claimed });
                        none
                    }
                }
            }
            Message::SecretsDone => {
                let node = match self.dialog.take() {
                    Some(Dialog::Secrets { id, .. }) => self.known.by_id(&id).cloned(),
                    _ => None,
                };
                (Task::none(), node)
            }
            Message::Copy(value) => (iced::clipboard::write(value), None),
            Message::Step(by) => {
                if self.dialog.is_none() {
                    let keys: Vec<String> =
                        self.visible_rows().into_iter().map(|row| row.key).collect();
                    let keys = self.tables.ordered("nodes", &keys);
                    self.selected = section::step(&keys, self.selected.as_ref(), by);
                }
                none
            }
            Message::Enter => self.enter(),
            Message::Cancel => {
                // A claim's secrets are never shown again: only Done closes them.
                if !matches!(self.dialog, Some(Dialog::Secrets { .. })) {
                    self.dialog = None;
                }
                none
            }
        }
    }

    /// Open the login or claim dialog for the selected row, and peek.
    /// A peek is the device answering: list it at that address with its
    /// claimed state, as mDNS would, for this session.
    fn answered(&mut self, peek: &Peek) {
        let found = Found {
            name: peek.node.name.clone(),
            address: peek.address,
            id: Some(peek.node.id.clone()),
            fingerprint: Some(peek.fingerprint.clone()),
            claimed: Some(peek.node.claimed),
        };
        self.added.retain(|added| added.id != found.id);
        self.added.push(found);
    }

    /// A device added by address goes into the known nodes, claimed or not, so it
    /// is listed again after a restart even though mDNS cannot see it. A new
    /// one is pinned to the certificate it just presented, without a token;
    /// a known one only takes the new address, and one presenting another
    /// certificate than its pin is left alone for Forget.
    fn keep(&mut self, peek: &Peek) -> Result<(), String> {
        self.reload();
        let address = peek.address.to_string();
        let node = match self.known.by_id(&peek.node.id) {
            Some(known) if known.fingerprint != peek.fingerprint || known.address == address => {
                return Ok(());
            }
            Some(known) => Node {
                address,
                ..known.clone()
            },
            None => Node {
                id: peek.node.id.clone(),
                name: peek.node.name.clone(),
                address,
                fingerprint: peek.fingerprint.clone(),
                token: None,
            },
        };
        self.known.keep(node)
    }

    fn access(&mut self, mode: Mode) -> (Task<Message>, Option<Node>) {
        let Some(row) = self.selected_row() else {
            return (Task::none(), None);
        };
        if row.pin == Pin::Mismatch {
            self.message = Some(Err(mismatch(&row)));
            return (Task::none(), None);
        }
        let Ok(address) = row.address.parse::<SocketAddr>() else {
            self.message = Some(Err(format!("{}: no address to reach it at", row.name)));
            return (Task::none(), None);
        };
        let aim = Aim {
            address,
            expected: row.id.clone(),
            label: row.name.clone(),
        };
        self.dialog = Some(Dialog::Access {
            mode,
            aim: aim.clone(),
            peek: None,
            token: String::new(),
            name: tessaro_client::client_name(),
            error: None,
            busy: true,
        });
        (
            Task::perform(blocking::run(move || peek(aim)), Message::Peeked),
            None,
        )
    }

    /// The rows the filter lets through, as the table shows them.
    fn visible_rows(&self) -> Vec<Row> {
        self.rows()
            .into_iter()
            .filter(|row| {
                section::matches(
                    &self.filter,
                    &[&row.name, &row.address, row.id.as_deref().unwrap_or("")],
                )
            })
            .collect()
    }

    /// Enter: the open dialog's default button, else open the selected row.
    fn enter(&mut self) -> (Task<Message>, Option<Node>) {
        let message = match &self.dialog {
            Some(Dialog::Address { .. }) => Message::AddressSubmit,
            Some(Dialog::Access { .. }) => Message::Commit,
            Some(Dialog::Forget { .. }) => Message::ForgetConfirmed,
            Some(Dialog::Secrets { .. }) => Message::SecretsDone,
            None => match self.selected.clone() {
                Some(key) => Message::Activate(key),
                None => return (Task::none(), None),
            },
        };
        self.update(message)
    }

    pub fn view(&self) -> Element<'_, Message> {
        let rows = self.visible_rows();
        let selected = self.selected_row();
        let selected_at = selected
            .as_ref()
            .and_then(|chosen| rows.iter().position(|row| row.key == chosen.key));

        const COLUMNS: &[Col] = &[
            col("Name", Length::Fixed(180.0)),
            col("Address", Length::Fixed(150.0)),
            col("Node id", Length::Fixed(150.0)),
            col("Claimed", Length::Fixed(90.0)),
            col("Pin", Length::Fixed(80.0)),
            col("Token", Length::Fixed(60.0)),
            col("Seen", Length::Fill),
        ];
        let cells = rows.iter().map(|row| {
            vec![
                cell(row.name.clone()).into(),
                cell(row.address.clone()).into(),
                cell(row.id.clone().unwrap_or_else(|| "?".to_string()))
                    .style(theme::muted)
                    .into(),
                match row.claimed {
                    Some(true) => cell("claimed").into(),
                    Some(false) => cell("UNCLAIMED").style(text::warning).into(),
                    None => cell("?").style(theme::muted).into(),
                },
                match row.pin {
                    Pin::Known => cell("known").into(),
                    Pin::New => cell("new").style(theme::muted).into(),
                    Pin::Mismatch => cell("MISMATCH").style(text::danger).into(),
                },
                cell(if row.token { "yes" } else { "-" }).into(),
                // Not seen is not offline: nothing asks a known device that
                // mDNS does not announce, as one on another subnet never is.
                if row.online {
                    cell("online").style(text::success).into()
                } else {
                    cell("not seen").style(theme::muted).into()
                },
            ]
        });
        let keys: Vec<String> = rows.iter().map(|row| row.key.clone()).collect();
        let keys_too = keys.clone();
        let table = grid(
            self.tables.state("nodes"),
            Message::Table,
            COLUMNS,
            cells.collect(),
            selected_at,
            move |at| Message::Select(keys[at].clone()),
            move |at| Message::Activate(keys_too[at].clone()),
        );

        let usable = selected.as_ref().filter(|row| row.pin != Pin::Mismatch);
        let known = selected
            .as_ref()
            .and_then(|row| row.id.as_deref())
            .is_some_and(|id| self.known.by_id(id).is_some());
        let list = section::view(
            vec![
                action("Add address", Some(Message::AddAddress)),
                action("Rescan", Some(Message::Rescan)),
            ],
            vec![
                action(
                    "Open",
                    selected
                        .as_ref()
                        .and_then(|row| self.openable(row))
                        .map(|_| Message::Open),
                ),
                action("Login", usable.map(|_| Message::Login)),
                action(
                    "Claim",
                    usable
                        .filter(|row| row.claimed != Some(true))
                        .map(|_| Message::Claim),
                ),
                action("Forget", known.then_some(Message::Forget)),
            ],
            &self.filter,
            Message::Filter,
            table,
        );

        let status = match (&self.message, &self.discovery) {
            (Some(Err(error)), _) => text(error).style(text::danger),
            (Some(Ok(message)), _) => text(message),
            (None, Some(why)) => {
                text(format!("not browsing the network: {why}")).style(text::warning)
            }
            (None, None) => text("browsing the network for devices (mDNS)").style(theme::muted),
        };
        let page = column![
            container(list).padding(6).height(Length::Fill),
            container(status.size(theme::SMALL))
                .width(Length::Fill)
                .padding([3, 8])
                .style(theme::status_bar),
        ];

        match &self.dialog {
            None => page.into(),
            Some(dialog) => dialog::modal(page.into(), self.dialog_view(dialog)),
        }
    }

    fn dialog_view<'a>(&'a self, dialog: &'a Dialog) -> Element<'a, Message> {
        match dialog {
            Dialog::Address { input, error, busy } => dialog::frame(
                "Add a device by address".to_string(),
                column![
                    field(
                        "Address",
                        text_input("192.168.1.20 or 192.168.1.20:7400", input)
                            .id(self.fields.id(0))
                            .on_input(Message::AddressInput)
                            .on_submit(Message::AddressSubmit)
                            .size(theme::SMALL)
                    ),
                    text("For a device mDNS cannot see: another subnet, a VM.")
                        .size(theme::SMALL)
                        .style(theme::muted),
                    dialog::error(error.clone()),
                ]
                .spacing(8)
                .into(),
                vec![
                    theme::default_button("Add", (!busy).then_some(Message::AddressSubmit)),
                    theme::dialog_button("Cancel", Some(Message::Cancel)),
                ],
            ),
            Dialog::Access {
                mode,
                aim,
                peek,
                token,
                name,
                error,
                busy,
            } => {
                let title = match mode {
                    Mode::Login => format!("Log in to {}", aim.label),
                    Mode::Claim => format!("Claim {}", aim.label),
                };
                let mut body = column![].spacing(8);
                match peek {
                    None if *busy => {
                        body = body.push(text(format!("asking {} ...", aim.address)).size(theme::SMALL))
                    }
                    None => {}
                    Some(peek) => {
                        body = body
                            .push(field("Device", text(format!("{} ({})", peek.node.name, peek.node.id)).size(theme::SMALL)))
                            .push(field("Address", text(peek.address.to_string()).size(theme::SMALL)))
                            .push(field("Image", text(format!("{} on {}", peek.node.version, peek.node.machine)).size(theme::SMALL)))
                            .push(field(
                                "Certificate",
                                text(&peek.fingerprint)
                                    .size(theme::SMALL)
                                    .font(iced::Font::MONOSPACE)
                                    .width(Length::Fill)
                                    .wrapping(text::Wrapping::Glyph),
                            ));
                        if !peek.pinned {
                            body = body.push(
                                text("Not pinned yet. Compare it with the fingerprint `tessaro-ctl device id` shows on the device itself: going on pins it, and from then on only this certificate is trusted.")
                                    .size(theme::SMALL)
                                    .style(text::warning),
                            );
                        }
                        body = body.push(match mode {
                            Mode::Login => field(
                                "Token",
                                text_input("tsr_...", token)
                                    .id(self.fields.id(0))
                                    .on_input(Message::TokenInput)
                                    .on_submit(Message::Commit)
                                    .secure(true)
                                    .size(theme::SMALL),
                            ),
                            Mode::Claim => field(
                                "Claim as",
                                text_input("user@host", name)
                                    .id(self.fields.id(0))
                                    .on_input(Message::NameInput)
                                    .on_submit(Message::Commit)
                                    .size(theme::SMALL),
                            ),
                        });
                    }
                }
                body = body.push(dialog::error(error.clone()));
                let ready = peek.is_some()
                    && !busy
                    && (*mode == Mode::Claim || !token.trim().is_empty());
                let label = match (mode, peek.as_ref().is_some_and(|peek| peek.pinned)) {
                    (Mode::Login, true) => "Log in",
                    (Mode::Login, false) => "Pin and log in",
                    (Mode::Claim, true) => "Claim",
                    (Mode::Claim, false) => "Pin and claim",
                };
                dialog::frame(
                    title,
                    body.into(),
                    vec![
                        theme::default_button(label, ready.then_some(Message::Commit)),
                        theme::dialog_button("Cancel", Some(Message::Cancel)),
                    ],
                )
            }
            Dialog::Forget { name, .. } => dialog::frame(
                format!("Forget {name}"),
                text(format!(
                    "Drop {name}'s pin and token from this machine. The device keeps its owners; log in again to manage it."
                ))
                .size(theme::SMALL)
                .into(),
                vec![
                    theme::default_button("Forget", Some(Message::ForgetConfirmed)),
                    theme::dialog_button("Cancel", Some(Message::Cancel)),
                ],
            ),
            Dialog::Secrets { name, claimed, .. } => {
                let mut body = column![
                    text("Shown this once - store them now.")
                        .size(theme::SMALL)
                        .style(text::warning),
                    secret("Root password", &claimed.root_password),
                ]
                .spacing(8);
                if let Some(hotspot) = &claimed.hotspot {
                    body = body.push(secret(&hotspot.ssid, &hotspot.password)).push(
                        text("That is the hotspot's new password: anyone on the hotspot now is dropped.")
                            .size(theme::SMALL)
                            .style(theme::muted),
                    );
                }
                dialog::frame(
                    format!("Claimed {name}"),
                    body.into(),
                    vec![theme::default_button("Done", Some(Message::SecretsDone))],
                )
            }
        }
    }
}

/// A secret with its copy button.
fn secret<'a>(label: &'a str, value: &'a str) -> Element<'a, Message> {
    field(
        label,
        iced::widget::row![
            text(value).size(theme::TEXT).font(iced::Font::MONOSPACE),
            theme::tool("Copy", Some(Message::Copy(value.to_string()))),
        ]
        .spacing(8),
    )
}

fn mismatch(row: &Row) -> String {
    format!(
        "{} presents another certificate than the one pinned: reinstalled, its /data wiped, or someone in the middle. Forget it to pin it again.",
        row.name
    )
}

/// An address typed by hand: an IP, `ip:port`, or a host name.
fn aim(input: &str, known: &Nodes) -> Result<Aim, String> {
    if input.is_empty() {
        return Err("an address, please".to_string());
    }
    match connect::resolve(Some(input), known)? {
        Target::Remote {
            address,
            expected,
            label,
        } => Ok(Aim {
            address,
            expected,
            label,
        }),
        _ => Err(format!(
            "{input}: an IP address or a DNS name; devices on this network appear by themselves"
        )),
    }
}

/// Look at a device without pinning it: who it says it is, and its
/// certificate.
fn peek(aim: Aim) -> Result<Peek, String> {
    let nodes = Nodes::load()?;
    let target = Target::Remote {
        address: aim.address,
        expected: aim.expected,
        label: aim.label,
    };
    let session = connect::open(&target, &nodes, &mut Trust::Peek, CLIENT)?;
    let (address, fingerprint) = session
        .remote
        .clone()
        .ok_or_else(|| "not a network session".to_string())?;
    Ok(Peek {
        pinned: nodes.by_id(&session.node.id).is_some(),
        node: session.node,
        address,
        fingerprint,
    })
}

/// Pin exactly the peeked certificate, then log in or claim, and remember
/// the node with its token.
fn commit(mode: Mode, peek: Peek, token: String, name: String) -> Result<Outcome, String> {
    let mut nodes = Nodes::load()?;
    let target = Target::Remote {
        address: peek.address,
        expected: Some(peek.node.id.clone()),
        label: peek.node.name.clone(),
    };
    let accepted = peek.fingerprint.clone();
    let mut decide = |ask: &PinAsk| Ok(ask.fingerprint == accepted);
    let mut session = connect::open(&target, &nodes, &mut Trust::Pin(&mut decide), CLIENT)?;
    if session.node.id != peek.node.id {
        return Err(format!(
            "another device answers at {} now ({})",
            peek.address, session.node.id
        ));
    }
    match mode {
        Mode::Login => {
            tessaro_client::access::login(&mut session, &token)?;
            nodes.remember(&session, Some(token))?;
            Ok(Outcome::LoggedIn(session.node.name.clone()))
        }
        Mode::Claim => {
            let claimed = tessaro_client::access::claim(&mut session, &name).into_result()?;
            nodes.remember(&session, Some(claimed.token.clone()))?;
            Ok(Outcome::Claimed(
                session.node.name.clone(),
                Box::new(claimed),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, name: &str, token: bool) -> Node {
        Node {
            id: id.to_string(),
            name: name.to_string(),
            address: "10.0.0.5:7400".to_string(),
            fingerprint: "a".repeat(64),
            token: token.then(|| "tsr_x".to_string()),
        }
    }

    fn found(id: &str, name: &str, fingerprint: &str) -> Found {
        Found {
            name: name.to_string(),
            address: "10.0.0.9:7400".parse().unwrap(),
            id: Some(id.to_string()),
            fingerprint: Some(fingerprint.to_string()),
            claimed: Some(true),
        }
    }

    #[test]
    fn a_known_node_that_answers_takes_its_live_address() {
        let known = Nodes {
            nodes: vec![node("n1", "kiosk-1", true)],
        };
        let seen = [found("n1", "kiosk-1", &"a".repeat(64))];
        let rows = merge(&known, &seen);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, "10.0.0.9:7400");
        assert!(rows[0].online && rows[0].token);
        assert_eq!(rows[0].pin, Pin::Known);
    }

    #[test]
    fn a_known_node_with_another_certificate_is_a_mismatch() {
        let known = Nodes {
            nodes: vec![node("n1", "kiosk-1", true)],
        };
        let seen = [found("n1", "kiosk-1", &"b".repeat(64))];
        assert_eq!(merge(&known, &seen)[0].pin, Pin::Mismatch);
    }

    #[test]
    fn a_stranger_gets_a_row_of_its_own_and_a_silent_node_stays() {
        let known = Nodes {
            nodes: vec![node("n1", "kiosk-1", false)],
        };
        let seen = [found("n2", "kiosk-2", "c")];
        let rows = merge(&known, &seen);
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].online && rows[0].pin == Pin::Known);
        assert!(rows[1].online && rows[1].pin == Pin::New && !rows[1].token);
    }

    #[test]
    fn an_unclaimed_device_opens_without_a_pin_or_a_token() {
        let pin = "a".repeat(64);
        let unclaimed = |id: &str, name: &str| Found {
            claimed: Some(false),
            ..found(id, name, &pin)
        };
        let view = NodesView {
            tables: crate::grid::Tables::default(),
            known: Nodes {
                nodes: vec![node("n1", "kiosk-1", false), node("n3", "kiosk-3", false)],
            },
            seen: [
                ("kiosk-1".to_string(), unclaimed("n1", "kiosk-1")),
                ("kiosk-2".to_string(), unclaimed("n2", "kiosk-2")),
                ("kiosk-4".to_string(), found("n4", "kiosk-4", &pin)),
            ]
            .into(),
            added: Vec::new(),
            selected: None,
            filter: String::new(),
            dialog: None,
            fields: dialog::Fields::default(),
            discovery: None,
            message: None,
            generation: 0,
        };
        let openable = |name: &str| {
            let row = view
                .rows()
                .into_iter()
                .find(|row| row.name == name)
                .unwrap();
            view.openable(&row)
        };

        // Known, and a stranger: both open, the stranger at its live address.
        assert!(openable("kiosk-1").is_some());
        let stranger = openable("kiosk-2").unwrap();
        assert_eq!(stranger.address, "10.0.0.9:7400");
        assert!(stranger.token.is_none());
        // Known without a token, and claim state unknown: a login first.
        assert!(openable("kiosk-3").is_none());
        // A claimed stranger: a claim or login first.
        assert!(openable("kiosk-4").is_none());
    }

    #[test]
    fn the_same_device_seen_twice_is_one_row() {
        let known = Nodes::default();
        let seen = [found("n2", "kiosk-2", "c"), found("n2", "kiosk-2", "c")];
        assert_eq!(merge(&known, &seen).len(), 1);
    }

    #[test]
    fn only_an_address_or_a_dns_name_is_added_by_hand() {
        let known = Nodes::default();
        let aimed = aim("10.1.2.3", &known).unwrap();
        assert_eq!(aimed.address.port(), protocol::DEFAULT_PORT);
        assert!(aim("kiosk-1", &known).is_err());
        assert!(aim("", &known).is_err());
    }
}
