//! Sound: `tessaro-ctl audio`, the Audio page, and the line of
//! `device status`.

use protocol::{AudioDevice, AudioSide, AudioStatus, AudioTested};

use crate::text::{Line, Tone};

/// What a change of the audio.* keys did on the device, a line a side.
pub fn outcome(audio: &str) -> Vec<Line> {
    if audio.starts_with("saved,") {
        return vec![Line::of(Tone::Warn, audio)];
    }
    audio
        .split("; ")
        .map(|side| Line::of(Tone::Ok, side))
        .collect()
}

/// `muted`, `off`, or the volume.
pub fn level(side: &AudioSide) -> Line {
    if side.setting == "off" {
        Line::of(Tone::Warn, "off")
    } else if side.muted {
        Line::of(Tone::Warn, "muted")
    } else {
        Line::plain(format!("{}%", side.volume))
    }
}

/// The one line `device status` shows: `hdmi 80%`, and why when it is not
/// what the setting says.
pub fn summary(status: &AudioStatus) -> Line {
    if !status.running {
        return Line::of(Tone::Bad, "sound server not answering");
    }
    let output = &status.output;
    let line = match &output.using {
        Some(device) => Line::plain(&device.kind),
        None => Line::of(Tone::Warn, "no output"),
    }
    .text(" ")
    .join(level(output));
    match &output.fallback {
        Some(why) => line.text(" ").add(Tone::Muted, format!("({why})")),
        None => line,
    }
}

/// `audio show`: each side's setting, what it resolved to, and why.
pub fn show(status: &AudioStatus) -> Vec<Line> {
    let mut lines = Vec::new();
    if let Some(err) = &status.error {
        lines.push(
            Line::of(Tone::Bad, "the sound server is not answering:").text(format!(" {err}")),
        );
        lines.push(Line::new());
    }
    for (label, side) in [("output", &status.output), ("input", &status.input)] {
        let using = match &side.using {
            Some(device) => Line::of(Tone::Heading, &device.description)
                .text(" ")
                .add(Tone::Muted, format!("({})", device.kind)),
            None if status.running => Line::of(Tone::Warn, "(none)"),
            None => Line::of(Tone::Muted, "(unknown)"),
        };
        lines.push(
            Line::new()
                .pad(Tone::Label, label, 8)
                .text(format!(" {} ", side.setting))
                .add(Tone::Muted, "->")
                .text(" ")
                .join(using)
                .text("  ")
                .join(level(side)),
        );
        if let Some(why) = &side.fallback {
            lines.push(
                Line::new()
                    .pad(Tone::Label, "", 8)
                    .text(" ")
                    .add(Tone::Warn, why),
            );
        }
    }
    lines.push(Line::new());
    lines.push(Line::of(
        Tone::Muted,
        "`tessaro-ctl audio outputs` and `audio inputs` list what is plugged in.",
    ));
    lines
}

/// `audio outputs` or `audio inputs`: every device of `side`, the one in
/// use marked. `word` is `output` or `input`.
pub fn list(status: &AudioStatus, side: &AudioSide, word: &str) -> Vec<Line> {
    if !status.running {
        return vec![Line::of(Tone::Bad, "the sound server is not answering:")
            .text(format!(" {}", status.error.as_deref().unwrap_or("")))];
    }
    let mut lines = Vec::new();
    if side.devices.is_empty() {
        lines.push(Line::of(Tone::Warn, format!("no {word}s")));
    }
    for device in &side.devices {
        let marker = if device.in_use {
            Line::of(Tone::Ok, "*")
        } else {
            Line::plain(" ")
        };
        lines.push(
            marker
                .text(" ")
                .pad(Tone::Label, &device.kind, 10)
                .text(" ")
                .add(Tone::Heading, &device.description)
                .join(note(device)),
        );
        lines.push(Line::plain("    ").add(Tone::Muted, &device.name));
    }
    lines.push(Line::new());
    lines.push(
        Line::of(Tone::Muted, "* in use. Choose one with")
            .text(" ")
            .add(Tone::Cmd, format!("tessaro-ctl audio {word} NAME")),
    );
    lines.push(Line::of(Tone::Muted, "or a kind:").text(" ").add(
        Tone::Cmd,
        format!("tessaro-ctl audio {word} auto|usb|jack|..."),
    ));
    lines
}

/// Why a device may not do what its name says.
pub fn note(device: &AudioDevice) -> Line {
    let mut notes = Vec::new();
    if device.available == Some(false) {
        notes.push("nothing plugged in");
    }
    if device.needs_profile {
        notes.push("switches its sound card over");
    }
    if notes.is_empty() {
        Line::new()
    } else {
        Line::plain("  ").add(Tone::Muted, format!("({})", notes.join(", ")))
    }
}

/// What `audio test` did: the tone played, or how loud the recording was
/// and where it was kept.
pub fn test(tested: &AudioTested) -> Vec<Line> {
    let mut lines = vec![Line::of(Tone::Ok, &tested.message)];
    if let (Some(peak), Some(rms)) = (tested.peak_dbfs, tested.rms_dbfs) {
        lines.push(
            Line::of(Tone::Label, "peak")
                .text(format!(" {peak:.1} dBFS  "))
                .add(Tone::Label, "average")
                .text(format!(" {rms:.1} dBFS")),
        );
    }
    if let Some(saved) = &tested.saved {
        lines.push(
            Line::of(Tone::Muted, "listen with")
                .text(" ")
                .add(Tone::Cmd, format!("tessaro-ctl files download {saved}")),
        );
    }
    lines
}
