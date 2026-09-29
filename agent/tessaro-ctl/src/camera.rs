//! `tessaro-ctl camera ...`: the USB cameras, and how their mirrors capture.
//!
//! Only a camera's mirror, `tessaro-camera@<device>.service`, opens the
//! camera itself; the page and anything else read the virtual camera it
//! republishes, all at once. `format` and `size` are each a `config set` of
//! one camera.* key: every running mirror restarts to capture that way.

use std::collections::BTreeMap;

use anstream::println;
use clap::builder::PossibleValuesParser;
use clap::Subcommand;
use protocol::api;
use protocol::keys;
use tessaro_client::describe::camera as describe;

use crate::connect::Session;
use crate::style;
use crate::{print, show_applied};

#[derive(Subcommand)]
pub enum CameraCmd {
    /// Every USB camera: its node and the virtual camera pages read, what
    /// its mirror captures and why, every format and size it has, and the
    /// saved camera.* settings.
    List,
    /// How cameras capture: auto (MJPEG where the camera has it, else
    /// YUYV), mjpeg or yuyv. A camera without the format uses auto. The
    /// same as `tessaro-ctl config set camera.format=...`.
    Format {
        #[arg(value_parser = PossibleValuesParser::new(keys::CAMERA_FORMATS))]
        format: String,
    },
    /// The frame size cameras capture at: auto, or WIDTHxHEIGHT from
    /// `tessaro-ctl camera list`. A camera without the size uses auto. The
    /// same as `tessaro-ctl config set camera.size=...`.
    ///
    ///   tessaro-ctl camera size 1280x720
    ///   tessaro-ctl camera size auto
    Size { size: String },
}

pub fn run(session: &mut Session, command: CameraCmd, json: bool) -> Result<(), String> {
    match command {
        CameraCmd::List => {
            let list = session.fetch::<api::camera::List>()?;
            print(json, &list, || {
                for line in describe::list(&list) {
                    println!("{}", style::line(&line));
                }
            })
        }
        CameraCmd::Format { format } => set(session, json, keys::CAMERA_FORMAT, format),
        CameraCmd::Size { size } => set(session, json, keys::CAMERA_SIZE, size),
    }
}

fn set(session: &mut Session, json: bool, key: &str, value: String) -> Result<(), String> {
    let applied = crate::set(session, BTreeMap::from([(key.to_string(), value)]))?;
    print(json, &applied, || show_applied(&applied, false))
}
