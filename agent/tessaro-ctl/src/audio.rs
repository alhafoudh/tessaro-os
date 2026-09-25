//! `tessaro-ctl audio ...`: where sound plays, where it is recorded from, how
//! loud, and a test.
//!
//! `output`, `volume`, `mute`, `input` and `input-volume` are each a
//! `config set` of one audio.* key: the device applies it to its sound
//! server at once and nothing restarts. A kind of output (`usb`) may be
//! chosen before one is plugged in - sound plays on `auto` until it is.

use std::collections::BTreeMap;

use anstream::println;
use clap::Subcommand;
use protocol::{keys, AudioDevice, AudioSide, AudioStatus, AudioTested, Command};

use crate::connect::Session;
use crate::style::{self, pad, paint};
use crate::{print, show_applied, Toggle};

#[derive(Subcommand)]
pub enum AudioCmd {
    /// Where sound plays and is recorded from, how loud, and why that
    /// output: the settings and what they resolved to.
    Show,
    /// Every output the device has - HDMI, the jack, USB and Bluetooth
    /// speakers - with the one in use marked.
    Outputs,
    /// Every input (microphone) the device has, with the one in use marked.
    Inputs,
    /// Where sound plays: auto, hdmi, jack, usb, bluetooth, off, or one
    /// output's name from `tessaro-ctl audio outputs`. The same as
    /// `tessaro-ctl config set audio.output=...`.
    ///
    ///   tessaro-ctl audio output hdmi
    ///   tessaro-ctl audio output auto
    Output { output: String },
    /// Output volume, 0 to 100, on whichever output is playing. The same as
    /// `tessaro-ctl config set audio.volume=...`.
    Volume {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },
    /// Mute or unmute the output; the volume is kept. The same as
    /// `tessaro-ctl config set audio.mute=1|0`.
    Mute { state: Toggle },
    /// Where sound is recorded from, for pages that use the microphone:
    /// auto, usb, jack, bluetooth, off, or one input's name from
    /// `tessaro-ctl audio inputs`. The same as
    /// `tessaro-ctl config set audio.input=...`.
    Input { input: String },
    /// Input level, 0 to 100. The same as
    /// `tessaro-ctl config set audio.input_volume=...`.
    InputVolume {
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },
    /// Play a 1s tone on the output in use. With --input, record 3s from
    /// the input in use instead, print how loud it was, and keep the
    /// recording in the file store as audio-recording.wav.
    Test {
        #[arg(long)]
        input: bool,
    },
}

pub fn run(session: &mut Session, command: AudioCmd, json: bool) -> Result<(), String> {
    match command {
        AudioCmd::Show => {
            let status: AudioStatus = session.call(Command::AudioStatus)?;
            print(json, &status, || show(&status))
        }
        AudioCmd::Outputs => {
            let status: AudioStatus = session.call(Command::AudioStatus)?;
            print(json, &status.output.devices, || {
                list(&status, &status.output, "output")
            })
        }
        AudioCmd::Inputs => {
            let status: AudioStatus = session.call(Command::AudioStatus)?;
            print(json, &status.input.devices, || {
                list(&status, &status.input, "input")
            })
        }
        AudioCmd::Output { output } => set(session, json, keys::AUDIO_OUTPUT, output),
        AudioCmd::Volume { percent } => set(session, json, keys::AUDIO_VOLUME, percent.to_string()),
        AudioCmd::Mute { state } => set(session, json, keys::AUDIO_MUTE, state.flag().to_string()),
        AudioCmd::Input { input } => set(session, json, keys::AUDIO_INPUT, input),
        AudioCmd::InputVolume { percent } => {
            set(session, json, keys::AUDIO_INPUT_VOLUME, percent.to_string())
        }
        AudioCmd::Test { input } => {
            let tested: AudioTested = session.call(Command::AudioTest { input })?;
            print(json, &tested, || {
                println!("{}", paint(style::OK, &tested.message));
                if let (Some(peak), Some(rms)) = (tested.peak_dbfs, tested.rms_dbfs) {
                    println!(
                        "{} {peak:.1} dBFS  {} {rms:.1} dBFS",
                        paint(style::LABEL, "peak"),
                        paint(style::LABEL, "average")
                    );
                }
                if let Some(saved) = &tested.saved {
                    println!(
                        "{} {}",
                        paint(style::MUTED, "listen with"),
                        paint(style::CMD, format!("tessaro-ctl files download {saved}"))
                    );
                }
            })
        }
    }
}

fn set(session: &mut Session, json: bool, key: &str, value: String) -> Result<(), String> {
    let applied = crate::set(session, BTreeMap::from([(key.to_string(), value)]))?;
    print(json, &applied, || show_applied(&applied, false))
}

/// What a change of the audio.* keys did on the device, one line per side.
pub fn show_outcome(audio: &str) {
    if audio.starts_with("saved,") {
        println!("{}", paint(style::WARN, audio));
        return;
    }
    for side in audio.split("; ") {
        println!("{}", paint(style::OK, side));
    }
}

/// `muted`, `off`, or the volume.
fn level(side: &AudioSide) -> String {
    if side.setting == "off" {
        paint(style::WARN, "off")
    } else if side.muted {
        paint(style::WARN, "muted")
    } else {
        format!("{}%", side.volume)
    }
}

/// The one line `device status` shows: `hdmi 80%`, and why when it is not
/// what the setting says.
pub fn summary(status: &AudioStatus) -> String {
    if !status.running {
        return paint(style::BAD, "sound server not answering");
    }
    let output = &status.output;
    let playing = match &output.using {
        Some(device) => device.kind.clone(),
        None => paint(style::WARN, "no output"),
    };
    let why = output
        .fallback
        .as_ref()
        .map(|why| format!(" {}", paint(style::MUTED, format!("({why})"))))
        .unwrap_or_default();
    format!("{playing} {}{why}", level(output))
}

fn show(status: &AudioStatus) {
    if let Some(err) = &status.error {
        println!(
            "{} {}",
            paint(style::BAD, "the sound server is not answering:"),
            err
        );
        println!();
    }
    for (label, side) in [("output", &status.output), ("input", &status.input)] {
        let using = match &side.using {
            Some(device) => format!(
                "{} {}",
                paint(style::HEADING, &device.description),
                paint(style::MUTED, format!("({})", device.kind))
            ),
            None if status.running => paint(style::WARN, "(none)"),
            None => paint(style::MUTED, "(unknown)"),
        };
        println!(
            "{} {} {} {using}  {}",
            pad(style::LABEL, label, 8),
            side.setting,
            paint(style::MUTED, "->"),
            level(side)
        );
        if let Some(why) = &side.fallback {
            println!("{} {}", pad(style::LABEL, "", 8), paint(style::WARN, why));
        }
    }
    println!(
        "\n{}",
        paint(
            style::MUTED,
            "`tessaro-ctl audio outputs` and `audio inputs` list what is plugged in."
        )
    );
}

fn list(status: &AudioStatus, side: &AudioSide, word: &str) {
    if !status.running {
        println!(
            "{} {}",
            paint(style::BAD, "the sound server is not answering:"),
            status.error.as_deref().unwrap_or("")
        );
        return;
    }
    if side.devices.is_empty() {
        println!("{}", paint(style::WARN, format!("no {word}s")));
    }
    for device in &side.devices {
        let marker = if device.in_use {
            paint(style::OK, "*")
        } else {
            " ".to_string()
        };
        println!(
            "{marker} {} {}{}",
            pad(style::LABEL, &device.kind, 10),
            paint(style::HEADING, &device.description),
            note(device)
        );
        println!("    {}", paint(style::MUTED, &device.name));
    }
    println!(
        "\n{} {}",
        paint(style::MUTED, "* in use. Choose one with"),
        paint(style::CMD, format!("tessaro-ctl audio {word} NAME"))
    );
    println!(
        "{} {}",
        paint(style::MUTED, "or a kind:"),
        paint(
            style::CMD,
            format!("tessaro-ctl audio {word} auto|usb|jack|...")
        )
    );
}

fn note(device: &AudioDevice) -> String {
    let mut notes = Vec::new();
    if device.available == Some(false) {
        notes.push("nothing plugged in");
    }
    if device.needs_profile {
        notes.push("switches its sound card over");
    }
    if notes.is_empty() {
        String::new()
    } else {
        format!(
            "  {}",
            paint(style::MUTED, format!("({})", notes.join(", ")))
        )
    }
}
