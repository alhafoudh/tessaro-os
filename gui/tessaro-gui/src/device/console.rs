//! A device's HDMI-CEC console, an inner window of its own beside the
//! device window (`main.rs`, `consoles`): a raw message to send, as
//! `tessaro-ctl screen cec send`, and the device's message log followed
//! live, as `tessaro-ctl screen cec messages --follow`.
//!
//! The device keeps the console's state, as it keeps a settings window's;
//! the log is followed on a connection of its own (`cec_log.rs`) for as
//! long as the console is open. What a send got is drawn in the console
//! while it is open, else in Messages.

use iced::widget::text_editor::Action;
use iced::widget::{column, row, rule, space, text, text_input, Column};
use iced::{Element, Font, Length, Task};
use protocol::api;
use protocol::CecActed;
use serde_json::Value;
use tessaro_client::describe;
use tessaro_client::text::{Line, Tone};

use super::{Device, Link, Message};
use crate::cec_log;
use crate::messages::Messages;
use crate::theme;
use crate::worker::send;

/// How many lines of the bus the console keeps.
const LINES: usize = 500;

/// The tag of a send, whose answer the console takes while it is open.
pub(super) const SEND: &str = "screen.cec.send";

/// What the console does.
#[derive(Debug, Clone)]
pub enum Console {
    Data(String),
    To(String),
    Reply(String),
    Send,
    Pause,
    Clear,
    /// A click, selection or scroll in the log.
    Log(Action),
}

/// An open console: what is typed, what the last send got, and the log.
pub(super) struct CecConsole {
    data: String,
    to: String,
    reply: String,
    /// What the last send got, or why it was not sent.
    answer: Vec<Line>,
    log: Messages,
    /// While paused, the log stays as it was; what arrives meanwhile waits
    /// here.
    held: Option<Vec<Line>>,
    state: Option<Result<(), String>>,
    /// Which stream it follows: a console opened again is a new one.
    generation: u64,
}

impl CecConsole {
    pub(super) fn new(generation: u64) -> Self {
        Self {
            data: String::new(),
            to: "tv".to_string(),
            reply: String::new(),
            answer: Vec::new(),
            log: Messages::keeping(LINES),
            held: None,
            state: None,
            generation,
        }
    }
}

impl Device {
    /// The message log to follow: while the console is open.
    pub fn console_stream(&self) -> Option<u64> {
        self.console.as_ref().map(|console| console.generation)
    }

    /// What the message log stream said.
    pub fn console_event(&mut self, event: cec_log::Event) {
        let Some(console) = &mut self.console else {
            return;
        };
        match event {
            cec_log::Event::Connected => console.state = Some(Ok(())),
            cec_log::Event::Lost(why) => console.state = Some(Err(why)),
            cec_log::Event::Page(page) => {
                console.state = Some(Ok(()));
                let lines = page.messages.iter().map(describe::cec::message);
                match &mut console.held {
                    Some(held) => {
                        held.extend(lines);
                        // Paused for long, it keeps only what the log would.
                        if held.len() > LINES {
                            held.drain(..held.len() - LINES);
                        }
                    }
                    None => console.log.extend(lines),
                }
            }
        }
    }

    pub fn console_title(&self) -> String {
        format!("{} - CEC console", self.name())
    }

    /// Whether Send can go now.
    fn can_send(&self) -> bool {
        self.link == Link::Online && !self.waiting(SEND)
    }

    pub(super) fn console_update(&mut self, message: Console) -> Task<Message> {
        let can_send = self.can_send();
        let Some(console) = &mut self.console else {
            return Task::none();
        };
        match message {
            Console::Data(data) => console.data = data,
            Console::To(to) => console.to = to,
            Console::Reply(reply) => console.reply = reply,
            Console::Send if can_send => {
                let body =
                    tessaro_client::cec::send_body(&console.data, &console.to, &console.reply, "");
                match body {
                    Ok(body) => {
                        console.answer.clear();
                        self.call(SEND, send::<api::screen::CecSend>(body));
                    }
                    Err(why) => console.answer = vec![Line::of(Tone::Bad, why)],
                }
            }
            Console::Send => {}
            Console::Pause => match console.held.take() {
                Some(held) => console.log.extend(held),
                None => console.held = Some(Vec::new()),
            },
            Console::Clear => {
                console.log.clear();
                if let Some(held) = &mut console.held {
                    held.clear();
                }
            }
            Console::Log(action) => console.log.perform(action),
        }
        Task::none()
    }

    /// A send's answer, drawn in the console while it is open; else handed
    /// back for Messages.
    pub(super) fn console_answer(
        &mut self,
        result: Result<Value, String>,
    ) -> Option<Result<Value, String>> {
        let Some(console) = &mut self.console else {
            return Some(result);
        };
        console.answer = match result.and_then(|value| {
            serde_json::from_value::<CecActed>(value).map_err(|err| err.to_string())
        }) {
            Ok(acted) => describe::cec::acted(&acted),
            Err(why) => vec![Line::of(Tone::Bad, why)],
        };
        None
    }

    pub fn console_view(&self) -> Element<'_, Message> {
        let Some(console) = &self.console else {
            return space().into();
        };
        let to = Message::Console;
        let can_send = self.can_send();
        let sending = row![
            text("Data").size(theme::SMALL),
            field("hex, e.g. 8f or 44 41", &console.data, Console::Data).width(Length::Fill),
            text("To").size(theme::SMALL),
            field("tv, audio, all, 0 to 15", &console.to, Console::To).width(150),
            text("Reply").size(theme::SMALL),
            field("opcode, e.g. 90", &console.reply, Console::Reply).width(120),
            theme::tool("Send", can_send.then(|| to(Console::Send))),
        ]
        .spacing(6)
        .height(24)
        .align_y(iced::alignment::Vertical::Center);
        let answer = Column::with_children(
            console
                .answer
                .iter()
                .map(|line| theme::text_line(line, Font::MONOSPACE)),
        );

        let state: Element<'_, Message> = match (&console.state, console.held.is_some()) {
            (_, true) => text("paused").size(theme::SMALL).style(theme::muted).into(),
            (Some(Err(why)), false) => text(why.clone())
                .size(theme::SMALL)
                .style(text::danger)
                .into(),
            (Some(Ok(())), false) => text("following")
                .size(theme::SMALL)
                .style(text::success)
                .into(),
            (None, false) => text("connecting")
                .size(theme::SMALL)
                .style(theme::muted)
                .into(),
        };
        let toolbar = row![
            text("Messages on the bus").size(theme::SMALL),
            rule::vertical(1),
            theme::toggle("Pause", console.held.is_some(), to(Console::Pause)),
            theme::tool("Clear", Some(to(Console::Clear))),
            state,
        ]
        .spacing(6)
        .height(24)
        .align_y(iced::alignment::Vertical::Center);

        iced::widget::container(
            column![
                text("A message as tessaro-ctl screen cec send: its opcode and operands in hex, to an address, and with a reply opcode the answer waited for.")
                    .size(theme::SMALL)
                    .style(theme::muted),
                sending,
                answer,
                rule::horizontal(1),
                toolbar,
                console.log.view_in(|action| Message::Console(Console::Log(action)), Length::Fill),
            ]
            .spacing(6),
        )
        .padding(6)
        .into()
    }
}

/// A field of the message to send, which Enter sends from.
fn field<'a>(
    placeholder: &'a str,
    value: &'a str,
    on_input: fn(String) -> Console,
) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .on_input(move |typed| Message::Console(on_input(typed)))
        .on_submit(Message::Console(Console::Send))
        .font(Font::MONOSPACE)
        .size(theme::SMALL)
        .padding([2, 6])
}
