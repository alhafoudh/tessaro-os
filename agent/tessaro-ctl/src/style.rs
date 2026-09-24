//! The palette, by meaning rather than by color.
//!
//! Everything printed goes through anstream's `println!` and friends, which
//! strip these codes again when the stream is not a terminal, under
//! `NO_COLOR` or `TERM=dumb`, or with `--color never`. So a call site only
//! says what a piece of text *is*; whether it ends up colored is decided in
//! one place. Styles decorate the text, never change it: the plain output is
//! byte for byte what it was before there were colors.

use anstyle::{AnsiColor, Style};

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

/// A systemd unit's `ActiveState`.
pub fn unit_state(state: &str) -> Style {
    match state {
        "active" => OK,
        "failed" => BAD,
        "inactive" => MUTED,
        _ => WARN,
    }
}

/// An interface's operstate.
pub fn link_state(state: &str) -> Style {
    match state {
        "up" => OK,
        "down" | "lowerlayerdown" => BAD,
        _ => MUTED,
    }
}

/// How full a filesystem is, in percent.
pub fn usage_level(percent: u64) -> Style {
    match percent {
        0..80 => OK,
        80..95 => WARN,
        _ => BAD,
    }
}

pub fn yes_no(yes: bool) -> String {
    if yes {
        paint(OK, "yes")
    } else {
        paint(WARN, "no")
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
    fn fuller_is_louder() {
        assert_eq!(usage_level(10), OK);
        assert_eq!(usage_level(85), WARN);
        assert_eq!(usage_level(99), BAD);
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
