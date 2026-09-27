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
use protocol::api;
use protocol::{keys, AudioSide, AudioStatus};
use tessaro_client::describe::audio as describe;

use crate::connect::Session;
use crate::style;
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
            let status = session.fetch::<api::audio::Show>()?;
            print(json, &status, || show(&status))
        }
        AudioCmd::Outputs => {
            let status = session.fetch::<api::audio::Show>()?;
            print(json, &status.output.devices, || {
                list(&status, &status.output, "output")
            })
        }
        AudioCmd::Inputs => {
            let status = session.fetch::<api::audio::Show>()?;
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
            let tested = session.send::<api::audio::Test>(api::AudioTestBody { input })?;
            print(json, &tested, || lines(describe::test(&tested)))
        }
    }
}

fn set(session: &mut Session, json: bool, key: &str, value: String) -> Result<(), String> {
    let applied = crate::set(session, BTreeMap::from([(key.to_string(), value)]))?;
    print(json, &applied, || show_applied(&applied, false))
}

fn show(status: &AudioStatus) {
    lines(describe::show(status));
}

fn list(status: &AudioStatus, side: &AudioSide, word: &str) {
    lines(describe::list(status, side, word));
}

fn lines(lines: Vec<tessaro_client::text::Line>) {
    for line in lines {
        println!("{}", style::line(&line));
    }
}
