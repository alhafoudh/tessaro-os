//! `tessaro-ctl update`: put a new image on a device.
//!
//! The flow - upload, check, stage, commit, wait for the reboot - is
//! `tessaro_client::update`, shared with the GUI. This is its command line:
//! the flags, the confirmation, and progress on stderr (one line that
//! redraws itself on a terminal, one line per step otherwise).

use std::path::PathBuf;

use crate::out::println;
use protocol::api;
use tessaro_client::nodes::Nodes;
use tessaro_client::transfer;
use tessaro_client::update::{self as flow, Plan};

use crate::connect::{self, Session, Target, Trust};
use crate::progress::Progress;
use crate::prompt;
use crate::style;

/// `update send`, as it is typed.
#[derive(clap::Args)]
pub struct Send {
    pub image: PathBuf,
    /// The block map, if it is not IMAGE without .zst or .bz2 plus .bmap.
    #[arg(long)]
    pub bmap: Option<PathBuf>,
    /// Also re-create /data: every setting, the claim, the browser
    /// profile and the device's identity go. It comes back unclaimed.
    #[arg(long)]
    wipe_data: bool,
    /// Write the whole disk - partition table, boot, root and /data - as
    /// `mise run image:flash` would, for a device on another disk layout.
    /// Implies --wipe-data. The device holds the upload in RAM while it
    /// writes, and a power cut before it is done needs a physical
    /// reflash.
    #[arg(long)]
    pub repartition: bool,
    /// Stage and commit it, but leave the reboot for later.
    #[arg(long)]
    pub no_reboot: bool,
    /// Do not wait for the device to come back.
    #[arg(long)]
    pub no_wait: bool,
    /// Skip the device's check of the whole upload against its SHA-256
    /// before preparing it. The bmap's checksums still cover every block
    /// that is written. Needs a device on an image that knows the flag;
    /// an older one checks anyway.
    #[arg(long)]
    pub no_verify: bool,
    #[arg(long, short)]
    pub yes: bool,
}

/// What `send` left for the caller to do with the known nodes.
pub enum Sent {
    /// Nothing changes on this machine.
    Kept,
    /// `/data` is being wiped: the device comes back with a new identity,
    /// so its pin and token here are worthless.
    Wiped,
}

pub fn send(
    session: &mut Session,
    target: &Target,
    nodes: &Nodes,
    options: Send,
    json: bool,
) -> Result<Sent, String> {
    let plan = Plan {
        bmap: options
            .bmap
            .clone()
            .unwrap_or_else(|| transfer::bmap_for(&options.image)),
        image: options.image,
        wipe_data: options.wipe_data,
        repartition: options.repartition,
        verify: !options.no_verify,
        reboot: !options.no_reboot,
    };
    if !plan.bmap.is_file() {
        return Err(format!(
            "{}: no such file (pass --bmap if it is somewhere else)",
            plan.bmap.display()
        ));
    }
    let name = plan.name()?;
    let node = session.node.name.clone();
    match plan.warning(&name) {
        Some(warning) => prompt::confirm_destructive(session, options.yes, &warning)?,
        None => prompt::confirm(
            options.yes,
            &format!(
                "Write {name} to {node}{}?",
                if plan.reboot { " and reboot it" } else { "" }
            ),
        )?,
    }

    let mut progress = Progress::new(json);
    match flow::send(session, &plan, &mut progress)? {
        flow::Sent::Wiped => Ok(Sent::Wiped),
        flow::Sent::Staged => Ok(Sent::Kept),
        flow::Sent::Rebooting if options.no_wait || matches!(target, Target::Local(_)) => {
            Ok(Sent::Kept)
        }
        flow::Sent::Rebooting => {
            flow::wait_back(
                &node,
                || connect::open(target, nodes, Trust::KnownOnly),
                &mut progress,
            )?;
            Ok(Sent::Kept)
        }
    }
}

/// `update fetch`, as it is typed.
#[derive(clap::Args)]
pub struct Fetch {
    /// The image's URL, http or https: a .wic.zst (or .wic.bz2), with its
    /// .wic.bmap beside it.
    pub url: String,
    /// The block map's URL, if it is not beside the image.
    #[arg(long, value_name = "URL")]
    pub bmap: Option<String>,
    /// Write the whole disk, as `update send --repartition` does. Implies
    /// --wipe-data once committed.
    #[arg(long)]
    pub repartition: bool,
    /// Once it is ready, apply it at the next boot.
    #[arg(long)]
    pub commit: bool,
    /// With --commit: also re-create /data, as `update send --wipe-data`.
    #[arg(long, requires = "commit")]
    pub wipe_data: bool,
    /// With --commit: reboot into it at once.
    #[arg(long, requires = "commit")]
    pub reboot: bool,
    /// Only start it, and leave it to the device: `update status` says how
    /// far it got, `update cancel` stops it.
    #[arg(long)]
    pub no_wait: bool,
    #[arg(long, short)]
    pub yes: bool,
}

pub fn fetch(session: &mut Session, options: Fetch, json: bool) -> Result<(), String> {
    let spec = protocol::FetchSpec {
        url: options.url,
        bmap: options.bmap,
        repartition: options.repartition,
        commit: options.commit,
        wipe_data: options.wipe_data,
        reboot: options.reboot,
    };
    let node = session.node.name.clone();
    if spec.commit {
        let plan = Plan {
            image: PathBuf::from(&spec.url),
            bmap: PathBuf::new(),
            wipe_data: spec.wipe_data,
            repartition: spec.repartition,
            verify: false,
            reboot: spec.reboot,
        };
        let name = spec.url.rsplit('/').next().unwrap_or(&spec.url).to_string();
        match plan.warning(&name) {
            Some(warning) => prompt::confirm_destructive(session, options.yes, &warning)?,
            None => prompt::confirm(
                options.yes,
                &format!(
                    "Fetch {name} onto {node} and apply it at the next boot{}?",
                    if spec.reboot { ", rebooting now" } else { "" }
                ),
            )?,
        }
    }
    if options.no_wait {
        let started = session.send::<api::update::Fetch>(spec)?;
        return crate::print(json, &started, || {
            println!(
                "fetching on {node} as job {}; {} says how far it got",
                started.job,
                style::paint(style::CMD, "tessaro-ctl update status")
            )
        });
    }
    let mut progress = Progress::new(json);
    flow::fetch(session, spec, &|| false, &mut progress)
}

pub fn status(session: &mut Session, json: bool) -> Result<(), String> {
    let status = session.fetch::<api::update::Status>()?;
    crate::print(json, &status, || {
        for line in flow::status_lines(&status) {
            println!("{}", style::line(&line));
        }
    })
}

pub fn cancel(session: &mut Session, json: bool) -> Result<(), String> {
    crate::done::<api::update::Cancel>(session, api::Empty {}, (), json)
}
