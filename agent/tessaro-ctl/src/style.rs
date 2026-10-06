//! The palette, by meaning rather than by color.
//!
//! Everything printed goes through anstream (`out.rs`'s `println!` and
//! friends), which strips these codes again when the stream is not a
//! terminal, under `NO_COLOR` or `TERM=dumb`, or with `--color never`. So a
//! call site only says what a piece of text *is*; whether it ends up colored
//! is decided in one place. Styles decorate the text, never change it: the
//! plain output is byte for byte what it was before there were colors.

// Shadow the std macro: this strips colors when stdout is not a terminal.
use crate::out::println;
use anstyle::{AnsiColor, Style};
use tessaro_client::text::{self, Line, Tone};

/// Field names in front of a value: `hostname`, `accepts`, `restarts`.
pub const LABEL: Style = Style::new().dimmed();
/// What a block is about: a key in `keys`, an interface, a connector.
pub const HEADING: Style = Style::new().bold();
/// Healthy: answering, claimed, active, up, a value that was set.
pub const OK: Style = AnsiColor::Green.on_default();
/// Needs attention, not broken: unclaimed, on probation, restarting.
pub const WARN: Style = AnsiColor::Yellow.on_default();
/// Broken or refused: not answering, failed, a pin mismatch, an error.
pub const BAD: Style = AnsiColor::Red.on_default().bold();
/// Background: defaults, read-only notes, `(none)`, revisions.
pub const MUTED: Style = Style::new().dimmed();
/// Shown once and has to be written down: a root password, a token.
pub const SECRET: Style = AnsiColor::Yellow.on_default().bold();
/// A command the reader is told to run.
pub const CMD: Style = Style::new().bold();
/// Who wrote a journal line.
pub const SOURCE: Style = AnsiColor::Cyan.on_default();

/// `text` in `style`.
pub fn paint(style: Style, text: impl std::fmt::Display) -> String {
    format!("{style}{text}{style:#}")
}

/// `text` in `style`, left-aligned to `width` columns. The padding goes
/// inside the codes, since `{:<N}` around a painted string counts the
/// escape bytes and the columns stop lining up.
pub fn pad(style: Style, text: impl std::fmt::Display, width: usize) -> String {
    format!("{style}{text:<width$}{style:#}")
}

/// The style of a tone of the shared text (`tessaro_client::text`).
pub fn of(tone: Tone) -> Style {
    match tone {
        Tone::Plain => Style::new(),
        Tone::Label => LABEL,
        Tone::Heading => HEADING,
        Tone::Ok => OK,
        Tone::Warn => WARN,
        Tone::Bad => BAD,
        Tone::Muted => MUTED,
        Tone::Secret => SECRET,
        Tone::Cmd => CMD,
        Tone::Source => SOURCE,
    }
}

/// A shared line, painted: each span in its tone, padding inside the codes.
pub fn line(line: &Line) -> String {
    line.0
        .iter()
        .map(|span| match span.tone {
            Tone::Plain => span.padded(),
            tone => pad(of(tone), &span.text, span.width),
        })
        .collect()
}

/// An interface's operstate.
pub fn link_state(state: &str) -> Style {
    of(text::link_state(state))
}

/// How full a filesystem is, in percent.
pub fn usage_level(percent: u64) -> Style {
    of(text::usage_level(percent))
}

/// `label value` with the label in a column of its own: a row of a
/// top-level listing, `status` or `network`.
pub fn row(label: &str, value: &str) {
    println!("{} {value}", pad(LABEL, label, 12));
}

/// A row inside a block, indented under its heading: one key, one interface.
pub fn sub_row(label: &str, value: &str) {
    println!("    {} {value}", pad(LABEL, label, 9));
}

pub fn yes_no(yes: bool) -> String {
    line(&text::yes_no(yes))
}

/// Shared facts as rows.
pub fn facts(facts: &[text::Fact]) {
    for fact in facts {
        row(&fact.label, &line(&fact.value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anstream::adapter::strip_str;

    #[test]
    fn stripped_output_is_the_plain_text() {
        assert_eq!(
            strip_str(&paint(BAD, "not answering")).to_string(),
            "not answering"
        );
        assert_eq!(strip_str(&paint(SECRET, "hunter2")).to_string(), "hunter2");
    }

    #[test]
    fn a_painted_line_is_its_plain_text() {
        let shared = Line::new()
            .pad(Tone::Label, "reply", 9)
            .text(" ")
            .add(Tone::Bad, "lost");
        assert_eq!(strip_str(&line(&shared)).to_string(), shared.to_string());
        assert_eq!(
            line(&shared),
            format!("{} {}", pad(LABEL, "reply", 9), paint(BAD, "lost"))
        );
    }

    #[test]
    fn padding_survives_the_codes() {
        let painted = pad(OK, "eth0", 12);
        assert_eq!(strip_str(&painted).to_string(), format!("{:<12}", "eth0"));
        // A value wider than the column is not cut.
        let wide = pad(WARN, "a-very-long-interface", 4);
        assert_eq!(strip_str(&wide).to_string(), "a-very-long-interface");
    }
}
