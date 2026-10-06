//! The main window: every device this machine knows (`nodes.rs`) and
//! every one answering on the network (mDNS), in one list keyed by node id.
//!
//! A device is opened only once it is pinned and has a token. Getting there
//! is the same as with `tessaro-ctl access login|claim`: the device is
//! peeked at first (nothing secret is sent), the dialog shows its
//! certificate's fingerprint, and pinning accepts exactly that fingerprint -
//! if another one answers by the time the token goes out, nothing is sent.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;

use iced::keyboard::Modifiers;
use iced::widget::{column, container, row, text, text_input};
use iced::{Element, Length, Task};
use protocol::{Claimed, NodeInfo};
use tessaro_client::connect::{self, Found, PinAsk, Target, Trust};
use tessaro_client::nodes::{Node, Nodes};
use tessaro_client::tags;

use crate::dialog::{self, field};
use crate::grid::{cell, col, grid_marked, widget, Cell, Col};
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
    /// device.tags: as announced, else as the store last kept them.
    pub tags: Vec<String>,
}

impl Row {
    /// What the list shows and filters by: `unclaimed` too, when it is.
    pub fn all_tags(&self) -> Vec<String> {
        tags::effective(&self.tags, self.claimed)
    }
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
            tags: node.tags.clone(),
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
                    tags: Vec::new(),
                });
                rows.len() - 1
            }
        };
        let row = &mut rows[at];
        row.online = true;
        row.name.clone_from(&found.name);
        row.address = found.address.to_string();
        row.claimed = found.claimed.or(row.claimed);
        row.tags.clone_from(&found.tags);
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

/// A row's tags as badges; pressing one filters the list by it.
fn tag_cell<'a>(tags: Vec<String>) -> Cell<'a, Message> {
    let sort = tags.join(" ");
    let badges = row(tags.into_iter().map(|tag| {
        theme::badge(
            tag.clone(),
            theme::badge_colour(&tag),
            Some(Message::TagFilter(tag)),
        )
    }))
    .spacing(3);
    widget(sort, badges)
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
    /// A tag's badge: add it to the tag filter, or take it out.
    TagFilter(String),
    ClearTags,
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
    /// Shift-Up or Shift-Down: the row there is marked too.
    Extend(i32),
    /// Cmd-A: every row shown, marked.
    MarkAll,
    /// The keys held down, for what a click on a row does.
    Modifiers(Modifiers),
    /// Run something on every marked device, in a bulk window.
    Bulk,
    Enter,
}

/// Several rows marked for a bulk run: Cmd-click adds or takes out one,
/// Shift-click marks every row from the selected one, Cmd-A every row the
/// filters show. Plain clicks select one row and drop the marks.
#[derive(Debug, Default)]
pub struct Marks {
    rows: BTreeSet<String>,
    modifiers: Modifiers,
    /// Asked for by Run on, for the app to open a bulk window with.
    opening: Option<Vec<Node>>,
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
    marks: Marks,
    filter: String,
    /// Only rows with every one of these tags, besides the text filter.
    tag_filter: BTreeSet<String>,
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
            marks: Marks::default(),
            filter: String::new(),
            tag_filter: BTreeSet::new(),
            dialog: None,
            fields: dialog::Fields::default(),
            discovery: None,
            message,
            generation: 0,
        }
    }

    /// The devices Run on asked a bulk window for, once.
    pub fn take_bulk(&mut self) -> Option<Vec<Node>> {
        self.marks.opening.take()
    }

    /// The rows shown, in the order the table shows them.
    fn shown_keys(&self) -> Vec<String> {
        let keys: Vec<String> = self.visible_rows().into_iter().map(|row| row.key).collect();
        self.tables.ordered("nodes", &keys)
    }

    /// Marks only on rows the filters still show.
    fn prune_marks(&mut self) {
        let shown: BTreeSet<String> = self.shown_keys().into_iter().collect();
        self.marks.rows.retain(|key| shown.contains(key));
    }

    /// The marked rows' devices that can be opened, and how many marked
    /// rows cannot.
    fn marked_nodes(&self) -> (Vec<Node>, usize) {
        let rows: Vec<Row> = self
            .rows()
            .into_iter()
            .filter(|row| self.marks.rows.contains(&row.key))
            .collect();
        let nodes: Vec<Node> = rows.iter().filter_map(|row| self.openable(row)).collect();
        let left_out = rows.len() - nodes.len();
        (nodes, left_out)
    }

    /// A click on the row `key`, with what is held down.
    fn click(&mut self, key: String) {
        let modifiers = self.marks.modifiers;
        if modifiers.shift() {
            let shown = self.shown_keys();
            let anchor = self
                .selected
                .as_ref()
                .and_then(|selected| shown.iter().position(|shown| shown == selected));
            let at = shown.iter().position(|shown| *shown == key);
            if let (Some(anchor), Some(at)) = (anchor, at) {
                let (from, to) = (anchor.min(at), anchor.max(at));
                if !modifiers.command() {
                    self.marks.rows.clear();
                }
                self.marks.rows.extend(shown[from..=to].iter().cloned());
                return;
            }
        } else if modifiers.command() {
            // The row selected so far is the first of the marks.
            if self.marks.rows.is_empty() {
                if let Some(selected) = self.selected.clone() {
                    self.marks.rows.insert(selected);
                }
            }
            if !self.marks.rows.remove(&key) {
                self.marks.rows.insert(key.clone());
            }
            self.selected = Some(key);
            return;
        }
        self.marks.rows.clear();
        self.selected = Some(key);
    }

    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    /// Tab in a dialog: each has its one field, which takes the cursor back.
    pub fn tab<T: Send + 'static>(&self, back: bool) -> Task<T> {
        let count = if self.has_dialog() { 1 } else { 0 };
        self.fields.step(count, back)
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
                tags: row.tags.clone(),
            }),
            _ => None,
        }
    }

    pub fn discovered(&mut self, event: discovery::Event) {
        match event {
            discovery::Event::Seen(found) => {
                // A known device keeps the tags it announced, for while it
                // is offline; a store that cannot be written only costs that.
                let _ = self.known.note_found(&found);
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
                self.click(key);
                none
            }
            Message::Modifiers(modifiers) => {
                self.marks.modifiers = modifiers;
                none
            }
            Message::MarkAll => {
                if self.dialog.is_none() {
                    self.marks.rows = self.shown_keys().into_iter().collect();
                }
                none
            }
            Message::Extend(by) => {
                if self.dialog.is_none() {
                    let shown = self.shown_keys();
                    if let Some(selected) = self.selected.clone() {
                        self.marks.rows.insert(selected);
                    }
                    self.selected = section::step(&shown, self.selected.as_ref(), by);
                    if let Some(selected) = self.selected.clone() {
                        self.marks.rows.insert(selected);
                    }
                }
                none
            }
            Message::Bulk => {
                let (nodes, left_out) = self.marked_nodes();
                if left_out > 0 {
                    self.message = Some(Err(format!(
                        "left out {left_out} marked {}: log in or claim first",
                        if left_out == 1 { "device" } else { "devices" }
                    )));
                }
                if !nodes.is_empty() {
                    self.marks.opening = Some(nodes);
                }
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
                self.prune_marks();
                none
            }
            Message::TagFilter(tag) => {
                if !self.tag_filter.remove(&tag) {
                    self.tag_filter.insert(tag);
                }
                self.prune_marks();
                none
            }
            Message::ClearTags => {
                self.tag_filter.clear();
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
                    let keys = self.shown_keys();
                    self.selected = section::step(&keys, self.selected.as_ref(), by);
                    self.marks.rows.clear();
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
            tags: peek.node.tags.clone(),
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
                tags: peek.node.tags.clone(),
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

    /// The rows the filters let through, as the table shows them: the text
    /// in any cell, tags included, and every tag of the tag filter.
    fn visible_rows(&self) -> Vec<Row> {
        let wanted: Vec<String> = self.tag_filter.iter().cloned().collect();
        self.rows()
            .into_iter()
            .filter(|row| {
                let all = row.all_tags();
                let tags = all.join(" ");
                section::matches(
                    &self.filter,
                    &[
                        &row.name,
                        &row.address,
                        row.id.as_deref().unwrap_or(""),
                        &tags,
                    ],
                ) && tags::matches(&all, &wanted)
            })
            .collect()
    }

    /// The tag filter under the toolbar, each tag a badge that takes it out
    /// again. Nothing while no tag is picked.
    fn tag_bar(&self) -> Option<Element<'_, Message>> {
        if self.tag_filter.is_empty() {
            return None;
        }
        let mut bar = row![text("Only devices tagged")
            .size(theme::SMALL)
            .style(theme::muted)]
        .spacing(4)
        .align_y(iced::alignment::Vertical::Center);
        for tag in &self.tag_filter {
            bar = bar.push(theme::badge(
                format!("{tag} ×"),
                theme::badge_colour(tag),
                Some(Message::TagFilter(tag.clone())),
            ));
        }
        Some(
            bar.push(theme::tool("Clear", Some(Message::ClearTags)))
                .into(),
        )
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
            col("Seen", Length::Fixed(80.0)),
            col("Tags", Length::Fill),
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
                tag_cell(row.all_tags()),
            ]
        });
        let keys: Vec<String> = rows.iter().map(|row| row.key.clone()).collect();
        let keys_too = keys.clone();
        let marked: Vec<bool> = rows
            .iter()
            .map(|row| self.marks.rows.contains(&row.key))
            .collect();
        let marking = !self.marks.rows.is_empty();
        let table = grid_marked(
            self.tables.state("nodes"),
            Message::Table,
            COLUMNS,
            cells.collect(),
            move |at| {
                if marking {
                    marked[at]
                } else {
                    Some(at) == selected_at
                }
            },
            move |at| Message::Select(keys[at].clone()),
            move |at| Message::Activate(keys_too[at].clone()),
        );
        // The tags picked go between the toolbar and the table.
        let table: Element<'_, Message> = match self.tag_bar() {
            Some(bar) => column![bar, table].spacing(4).into(),
            None => table,
        };

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
                action(
                    "Run on marked",
                    (self.marks.rows.len() > 1).then_some(Message::Bulk),
                ),
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
            tags: Vec::new(),
        }
    }

    fn found(id: &str, name: &str, fingerprint: &str) -> Found {
        Found {
            name: name.to_string(),
            address: "10.0.0.9:7400".parse().unwrap(),
            id: Some(id.to_string()),
            fingerprint: Some(fingerprint.to_string()),
            claimed: Some(true),
            tags: Vec::new(),
        }
    }

    fn tagged(list: &[&str]) -> Vec<String> {
        list.iter().map(|tag| tag.to_string()).collect()
    }

    #[test]
    fn a_silent_node_keeps_its_stored_tags_and_an_announcement_wins() {
        let known = Nodes {
            nodes: vec![
                Node {
                    tags: tagged(&["lobby"]),
                    ..node("n1", "kiosk-1", true)
                },
                Node {
                    tags: tagged(&["old"]),
                    ..node("n2", "kiosk-2", true)
                },
            ],
        };
        let seen = [Found {
            tags: tagged(&["floor-2"]),
            claimed: Some(false),
            ..found("n2", "kiosk-2", &"a".repeat(64))
        }];
        let rows = merge(&known, &seen);
        assert_eq!(rows[0].all_tags(), tagged(&["lobby"]));
        assert_eq!(rows[1].all_tags(), tagged(&["unclaimed", "floor-2"]));
    }

    #[test]
    fn badges_filter_the_list_by_every_tag_picked() {
        let pin = "a".repeat(64);
        let mut view = NodesView {
            tables: crate::grid::Tables::default(),
            known: Nodes::default(),
            seen: [
                (
                    "kiosk-1".to_string(),
                    Found {
                        tags: tagged(&["floor-2", "lobby"]),
                        ..found("n1", "kiosk-1", &pin)
                    },
                ),
                (
                    "kiosk-2".to_string(),
                    Found {
                        tags: tagged(&["lobby"]),
                        claimed: Some(false),
                        ..found("n2", "kiosk-2", &pin)
                    },
                ),
            ]
            .into(),
            added: Vec::new(),
            selected: None,
            marks: Marks::default(),
            filter: String::new(),
            tag_filter: BTreeSet::new(),
            dialog: None,
            fields: dialog::Fields::default(),
            discovery: None,
            message: None,
            generation: 0,
        };
        let names = |view: &NodesView| {
            view.visible_rows()
                .into_iter()
                .map(|row| row.name)
                .collect::<Vec<_>>()
        };

        let _ = view.update(Message::TagFilter("lobby".to_string()));
        assert_eq!(names(&view), ["kiosk-1", "kiosk-2"]);
        let _ = view.update(Message::TagFilter("floor-2".to_string()));
        assert_eq!(names(&view), ["kiosk-1"]);
        // A second press takes a tag out again.
        let _ = view.update(Message::TagFilter("floor-2".to_string()));
        let _ = view.update(Message::TagFilter("unclaimed".to_string()));
        assert_eq!(names(&view), ["kiosk-2"]);
        let _ = view.update(Message::ClearTags);
        // The text filter finds tags too.
        let _ = view.update(Message::Filter("floor".to_string()));
        assert_eq!(names(&view), ["kiosk-1"]);
    }

    #[test]
    fn rows_are_marked_by_cmd_and_shift_and_run_on_the_openable_ones() {
        let pin = "a".repeat(64);
        let mut view = NodesView {
            tables: crate::grid::Tables::default(),
            known: Nodes {
                nodes: (1..=4)
                    .map(|at| node(&format!("n{at}"), &format!("kiosk-{at}"), at != 4))
                    .collect(),
            },
            seen: (1..=4)
                .map(|at| {
                    let name = format!("kiosk-{at}");
                    let found = Found {
                        tags: if at == 3 {
                            tagged(&["lobby"])
                        } else {
                            Vec::new()
                        },
                        ..found(&format!("n{at}"), &name, &pin)
                    };
                    (name, found)
                })
                .collect(),
            added: Vec::new(),
            selected: None,
            marks: Marks::default(),
            filter: String::new(),
            tag_filter: BTreeSet::new(),
            dialog: None,
            fields: dialog::Fields::default(),
            discovery: None,
            message: None,
            generation: 0,
        };
        let key = |view: &NodesView, name: &str| {
            view.rows()
                .into_iter()
                .find(|row| row.name == name)
                .unwrap()
                .key
        };
        let marked = |view: &NodesView| {
            let mut names: Vec<String> = view
                .rows()
                .into_iter()
                .filter(|row| view.marks.rows.contains(&row.key))
                .map(|row| row.name)
                .collect();
            names.sort();
            names
        };
        let held = |view: &mut NodesView, modifiers: Modifiers| {
            let _ = view.update(Message::Modifiers(modifiers));
        };

        let first = key(&view, "kiosk-1");
        let _ = view.update(Message::Select(first));
        held(&mut view, Modifiers::SHIFT);
        let third = key(&view, "kiosk-3");
        let _ = view.update(Message::Select(third));
        assert_eq!(marked(&view), ["kiosk-1", "kiosk-2", "kiosk-3"]);
        held(&mut view, Modifiers::COMMAND);
        let second = key(&view, "kiosk-2");
        let _ = view.update(Message::Select(second));
        assert_eq!(marked(&view), ["kiosk-1", "kiosk-3"]);

        // A plain click is one row again.
        held(&mut view, Modifiers::empty());
        let fourth = key(&view, "kiosk-4");
        let _ = view.update(Message::Select(fourth));
        assert!(marked(&view).is_empty());

        // Cmd-A marks what the filters show, and a filter drops the rest.
        let _ = view.update(Message::MarkAll);
        assert_eq!(marked(&view).len(), 4);
        let _ = view.update(Message::TagFilter("lobby".to_string()));
        assert_eq!(marked(&view), ["kiosk-3"]);
        let _ = view.update(Message::ClearTags);
        let _ = view.update(Message::MarkAll);

        // kiosk-4 has no token: it is left out of the run, and said so.
        let _ = view.update(Message::Bulk);
        let opened = view.take_bulk().unwrap();
        assert_eq!(opened.len(), 3);
        assert!(matches!(view.message, Some(Err(_))));
        assert!(view.take_bulk().is_none());
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
            marks: Marks::default(),
            filter: String::new(),
            tag_filter: BTreeSet::new(),
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
