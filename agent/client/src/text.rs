//! Text for the user, said once for both clients and drawn by each.
//!
//! A `Line` is spans of text, each with a `Tone` that says what it is - a
//! label, a healthy value, a command to run - never which color. The ctl
//! paints each tone with its palette (`style.rs`), the GUI with its theme,
//! so both show the same words and the terminal keeps its colors. The plain
//! text of a line (`Display`) is what the ctl prints with colors off.

use std::fmt;

/// What a piece of text is. The ctl's palette in `style.rs` has one style
/// per tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// Nothing to say about it.
    #[default]
    Plain,
    /// Field names in front of a value.
    Label,
    /// What a block is about.
    Heading,
    /// Healthy: answering, active, up, a value that was set.
    Ok,
    /// Needs attention, not broken.
    Warn,
    /// Broken or refused.
    Bad,
    /// Background: defaults, notes, `(none)`.
    Muted,
    /// Shown once and has to be written down.
    Secret,
    /// A command the reader is told to run.
    Cmd,
    /// Who wrote a journal line.
    Source,
}

/// A piece of a line in one tone, optionally left-aligned to a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub tone: Tone,
    pub text: String,
    /// Pad to this many columns. The padding belongs to the span, so a
    /// painter can put it inside the color codes.
    pub width: usize,
}

impl Span {
    /// The text padded to its column.
    pub fn padded(&self) -> String {
        format!("{:<width$}", self.text, width = self.width)
    }
}

/// One line of text in spans.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line(pub Vec<Span>);

impl Line {
    pub fn new() -> Self {
        Self::default()
    }

    /// A line in one tone.
    pub fn of(tone: Tone, text: impl Into<String>) -> Self {
        Self::new().add(tone, text)
    }

    /// A line of plain text.
    pub fn plain(text: impl Into<String>) -> Self {
        Self::of(Tone::Plain, text)
    }

    /// Append `text` in `tone`.
    pub fn add(mut self, tone: Tone, text: impl Into<String>) -> Self {
        self.0.push(Span {
            tone,
            text: text.into(),
            width: 0,
        });
        self
    }

    /// Append plain text.
    pub fn text(self, text: impl Into<String>) -> Self {
        self.add(Tone::Plain, text)
    }

    /// Append `text` in `tone`, padded to `width` columns.
    pub fn pad(mut self, tone: Tone, text: impl Into<String>, width: usize) -> Self {
        self.0.push(Span {
            tone,
            text: text.into(),
            width,
        });
        self
    }

    /// Append every span of `other`.
    pub fn join(mut self, other: Line) -> Self {
        self.0.extend(other.0);
        self
    }

    /// The loudest tone in the line, for a painter that colors a whole line
    /// at once (the GUI's output panes).
    pub fn tone(&self) -> Tone {
        let rank = |tone: Tone| match tone {
            Tone::Bad => 3,
            Tone::Warn => 2,
            Tone::Ok => 1,
            _ => 0,
        };
        self.0
            .iter()
            .map(|span| span.tone)
            .max_by_key(|tone| rank(*tone))
            .filter(|tone| rank(*tone) > 0)
            .unwrap_or(Tone::Plain)
    }

    pub fn is_empty(&self) -> bool {
        self.0
            .iter()
            .all(|span| span.text.is_empty() && span.width == 0)
    }
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for span in &self.0 {
            f.write_str(&span.padded())?;
        }
        Ok(())
    }
}

impl From<String> for Line {
    fn from(text: String) -> Self {
        Self::plain(text)
    }
}

impl From<&str> for Line {
    fn from(text: &str) -> Self {
        Self::plain(text)
    }
}

/// A labelled value: a row of a status listing, a fact on a GUI page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub label: String,
    pub value: Line,
}

impl Fact {
    pub fn new(label: impl Into<String>, value: impl Into<Line>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// `label value` with the label in a column of its own, as a line: a row
/// of a listing in monospace.
pub fn row(label: &str, value: impl Into<Line>) -> Line {
    Line::new()
        .pad(Tone::Label, label, 12)
        .text(" ")
        .join(value.into())
}

/// A systemd unit's `ActiveState`.
pub fn unit_state(state: &str) -> Tone {
    match state {
        "active" => Tone::Ok,
        "failed" => Tone::Bad,
        "inactive" => Tone::Muted,
        _ => Tone::Warn,
    }
}

/// An interface's operstate.
pub fn link_state(state: &str) -> Tone {
    match state {
        "up" => Tone::Ok,
        "down" | "lowerlayerdown" => Tone::Bad,
        _ => Tone::Muted,
    }
}

/// How full a filesystem is, in percent.
pub fn usage_level(percent: u64) -> Tone {
    match percent {
        0..80 => Tone::Ok,
        80..95 => Tone::Warn,
        _ => Tone::Bad,
    }
}

/// How hot a sensor runs, in millidegrees Celsius: against its own `max` and
/// `crit` where it has them (a threshold of 0 or below is a sensor that has
/// none). Without `crit` it is bad 10°C past `max`; without either, 80°C
/// warns and 90°C is bad.
pub fn temperature_level(millicelsius: i32, max: Option<i32>, crit: Option<i32>) -> Tone {
    let max = max.filter(|&m| m > 0);
    let crit = crit.filter(|&c| c > 0);
    let bad = crit.unwrap_or_else(|| max.map_or(90_000, |m| m + 10_000));
    let warn = max.unwrap_or(80_000).min(bad);
    if millicelsius >= bad {
        Tone::Bad
    } else if millicelsius >= warn {
        Tone::Warn
    } else {
        Tone::Ok
    }
}

/// `yes` healthy, `no` worth a look.
pub fn yes_no(yes: bool) -> Line {
    if yes {
        Line::of(Tone::Ok, "yes")
    } else {
        Line::of(Tone::Warn, "no")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_reads_as_its_padded_text() {
        let line = Line::new()
            .pad(Tone::Label, "reply", 9)
            .text(" ")
            .add(Tone::Heading, "1.2 ms");
        assert_eq!(line.to_string(), "reply     1.2 ms");
    }

    #[test]
    fn a_line_is_as_loud_as_its_loudest_span() {
        let line = Line::of(Tone::Label, "x")
            .add(Tone::Ok, "a")
            .add(Tone::Bad, "b");
        assert_eq!(line.tone(), Tone::Bad);
        assert_eq!(Line::of(Tone::Heading, "x").tone(), Tone::Plain);
    }

    #[test]
    fn fuller_is_louder() {
        assert_eq!(usage_level(10), Tone::Ok);
        assert_eq!(usage_level(85), Tone::Warn);
        assert_eq!(usage_level(99), Tone::Bad);
    }

    #[test]
    fn hotter_is_louder() {
        // The sensor's own thresholds.
        assert_eq!(
            temperature_level(52_000, Some(84_000), Some(100_000)),
            Tone::Ok
        );
        assert_eq!(
            temperature_level(85_000, Some(84_000), Some(100_000)),
            Tone::Warn
        );
        assert_eq!(
            temperature_level(100_000, Some(84_000), Some(100_000)),
            Tone::Bad
        );
        // Only a max: bad 10°C past it.
        assert_eq!(temperature_level(95_000, Some(100_000), None), Tone::Ok);
        assert_eq!(temperature_level(105_000, Some(100_000), None), Tone::Warn);
        assert_eq!(temperature_level(110_000, Some(100_000), None), Tone::Bad);
        // Only a crit below the default warning.
        assert_eq!(temperature_level(70_000, None, Some(75_000)), Tone::Ok);
        assert_eq!(temperature_level(75_000, None, Some(75_000)), Tone::Bad);
        // None, or a zero the driver means as none.
        assert_eq!(temperature_level(79_000, Some(0), None), Tone::Ok);
        assert_eq!(temperature_level(80_000, None, None), Tone::Warn);
        assert_eq!(temperature_level(90_000, None, None), Tone::Bad);
    }
}
