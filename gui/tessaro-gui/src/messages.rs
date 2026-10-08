//! A device window's Messages: what its changes did and what its commands
//! printed, as text that can be selected and copied. iced's text widgets
//! only paint, so the log is a `text_editor` that drops every edit, and a
//! highlighter paints each span in its tone.

use std::ops::Range;
use std::sync::Arc;

use iced::advanced::text::highlighter::{self, Highlighter};
use iced::widget::text_editor::{Action, Content, Motion};
use iced::{Element, Font, Length, Theme};
use tessaro_client::text::{Line, Span, Tone};

use crate::copy_menu::copy_menu_with;
use crate::theme;

/// The log keeps this many lines.
const LINES: usize = 300;

pub struct Messages {
    lines: Vec<Line>,
    /// How many lines it keeps.
    cap: usize,
    content: Content,
    tones: Tones,
}

impl Default for Messages {
    fn default() -> Self {
        Self::keeping(LINES)
    }
}

impl Messages {
    /// An empty log keeping the newest `cap` lines.
    pub fn keeping(cap: usize) -> Self {
        Self {
            lines: Vec::new(),
            cap,
            content: Content::new(),
            tones: Tones::default(),
        }
    }

    /// Add `line`, one per line of its text, dropping the oldest past the cap.
    pub fn push(&mut self, line: Line) {
        self.extend(std::iter::once(line));
    }

    /// Add every line, with one rebuild for all of them.
    pub fn extend(&mut self, lines: impl IntoIterator<Item = Line>) {
        self.lines.extend(lines.into_iter().flat_map(split));
        if self.lines.len() > self.cap {
            self.lines.drain(..self.lines.len() - self.cap);
        }
        self.rebuild();
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.rebuild();
    }

    /// Everything but an edit: clicks, selection, scrolling.
    pub fn perform(&mut self, action: Action) {
        if !matches!(action, Action::Edit(_)) {
            self.content.perform(action);
        }
    }

    /// The whole log as plain text, for its Copy.
    pub fn text(&self) -> String {
        self.content.text()
    }

    /// The log, with a right-click menu copying the selection, or the whole
    /// log when nothing is selected.
    pub fn view<'a, M: Clone + 'a>(&'a self, on_action: fn(Action) -> M) -> Element<'a, M> {
        self.view_in(on_action, Length::Fixed(140.0))
    }

    /// The log at `height`.
    pub fn view_in<'a, M: Clone + 'a>(
        &'a self,
        on_action: fn(Action) -> M,
        height: Length,
    ) -> Element<'a, M> {
        let editor = iced::widget::text_editor(&self.content)
            .on_action(on_action)
            .font(Font::MONOSPACE)
            .size(theme::SMALL)
            .padding(0)
            .height(height)
            .style(theme::log_text)
            .highlight_with::<ToneHighlighter>(self.tones.clone(), format);
        copy_menu_with(editor, move || self.to_copy())
    }

    /// What the right-click menu offers.
    fn to_copy(&self) -> (&'static str, String) {
        match self.content.selection() {
            Some(selection) if !selection.is_empty() => ("Copy", selection),
            _ => ("Copy all", self.text()),
        }
    }

    /// A new line replaces the whole text, so a selection does not survive
    /// it; the cursor goes to the end, which keeps the newest line in view.
    fn rebuild(&mut self) {
        let text = self
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        self.content = Content::with_text(&text);
        self.content.perform(Action::Move(Motion::DocumentEnd));
        self.tones = Tones {
            revision: self.tones.revision + 1,
            lines: Arc::new(self.lines.iter().map(ranges).collect()),
        };
    }
}

/// `line` as the editor shows it: a span's newlines start new lines, which
/// keep the span's tone and drop its padding.
fn split(line: Line) -> Vec<Line> {
    if !line.0.iter().any(|span| span.text.contains('\n')) {
        return vec![line];
    }
    let mut out = vec![Line::new()];
    for span in line.0 {
        for (at, part) in span.text.split('\n').enumerate() {
            if at > 0 {
                out.push(Line::new());
            }
            if let Some(last) = out.last_mut() {
                last.0.push(Span {
                    tone: span.tone,
                    text: part.to_string(),
                    width: 0,
                });
            }
        }
    }
    out
}

fn ranges(line: &Line) -> LineTones {
    let mut start = 0;
    line.0
        .iter()
        .map(|span| {
            let end = start + span.padded().len();
            let range = (start..end, span.tone);
            start = end;
            range
        })
        .collect()
}

fn format(tone: &Tone, _: &Theme) -> highlighter::Format<Font> {
    highlighter::Format {
        color: theme::tone_color(*tone),
        font: Some(theme::tone_font(*tone, Font::MONOSPACE)),
    }
}

/// The tones of every line, told apart by a revision so the editor does not
/// compare the lines themselves on every frame.
#[derive(Clone, Default)]
struct Tones {
    revision: u64,
    lines: Arc<Vec<LineTones>>,
}

/// A line's spans: where each sits in the line's text, in bytes, and its tone.
type LineTones = Vec<(Range<usize>, Tone)>;

impl PartialEq for Tones {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision
    }
}

/// Hands the editor each line's tones, in the order it asks for lines.
struct ToneHighlighter {
    tones: Tones,
    line: usize,
}

impl Highlighter for ToneHighlighter {
    type Settings = Tones;
    type Highlight = Tone;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Tone)>;

    fn new(settings: &Tones) -> Self {
        Self {
            tones: settings.clone(),
            line: 0,
        }
    }

    fn update(&mut self, settings: &Tones) {
        self.tones = settings.clone();
        self.line = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.line = self.line.min(line);
    }

    fn highlight_line(&mut self, _: &str) -> Self::Iterator<'_> {
        let tones = self.tones.lines.get(self.line).cloned().unwrap_or_default();
        self.line += 1;
        tones.into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newline_in_a_span_starts_a_line_in_its_tone() {
        let line = Line::of(Tone::Muted, "a: ").add(Tone::Ok, "one\ntwo");
        let lines = split(line);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].to_string(), "a: one");
        assert_eq!(lines[1].to_string(), "two");
        assert_eq!(lines[1].0[0].tone, Tone::Ok);
    }

    #[test]
    fn ranges_cover_the_padded_text() {
        let line = Line::new()
            .pad(Tone::Label, "key", 6)
            .add(Tone::Plain, "value");
        let text = line.to_string();
        let ranges = ranges(&line);
        assert_eq!(&text[ranges[0].0.clone()], "key   ");
        assert_eq!(&text[ranges[1].0.clone()], "value");
    }

    #[test]
    fn the_log_keeps_its_newest_lines() {
        let mut messages = Messages::default();
        for at in 0..LINES + 5 {
            messages.push(Line::plain(at.to_string()));
        }
        assert_eq!(messages.lines.len(), LINES);
        assert_eq!(messages.lines[0].to_string(), "5");
        assert!(messages.text().ends_with(&(LINES + 4).to_string()));
    }

    #[test]
    fn a_log_of_its_own_size_keeps_the_newest_of_a_batch() {
        let mut messages = Messages::keeping(3);
        messages.extend((0..5).map(|at| Line::plain(at.to_string())));
        assert_eq!(messages.text(), "2\n3\n4");
    }

    #[test]
    fn the_menu_copies_the_selection_or_else_everything() {
        let mut messages = Messages::default();
        messages.push(Line::plain("one"));
        messages.push(Line::plain("two"));
        assert_eq!(messages.to_copy(), ("Copy all", "one\ntwo".to_string()));
        messages.perform(Action::Move(Motion::DocumentStart));
        messages.perform(Action::SelectWord);
        assert_eq!(messages.to_copy(), ("Copy", "one".to_string()));
    }

    #[test]
    fn an_edit_changes_nothing() {
        let mut messages = Messages::default();
        messages.push(Line::plain("kept"));
        messages.perform(Action::Edit(iced::widget::text_editor::Edit::Insert('x')));
        assert_eq!(messages.text(), "kept");
    }
}
