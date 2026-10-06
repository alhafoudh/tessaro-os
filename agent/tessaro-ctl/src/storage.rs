//! `tessaro-ctl storage ...`: the disk the device runs from, how full its
//! filesystems are, and growing `/data` over the rest of the disk.
//!
//! An image carries `/data` at a fixed size, so a device flashed onto a
//! larger card or disk has space after it that nothing uses until
//! `storage grow` gives it to `/data`. The device does the work with `/data`
//! mounted and the kiosk running, and makes its plan from the disk every
//! time, so the command is safe to run again.

use crate::out::{eprintln, println};
use clap::Subcommand;
use protocol::api::{self, GrowBody};
use protocol::{size_label, FsUsage, Partition, Storage, StorageGrowEvent};
use tessaro_client::storage as shared;

use crate::connect::{follow_job, Session};
use crate::print;
use crate::style::{self, pad, paint};

#[derive(Subcommand)]
pub enum StorageCmd {
    /// The disk: its size, partition table and unallocated space, and how
    /// full / and /data are.
    Show,
    /// Every partition on the disk: number, name, start, size, label,
    /// filesystem, where it is mounted.
    Partitions,
    /// Every mounted filesystem: size, used, available, use%.
    Usage,
    /// Grow /data over the unallocated space at the end of the disk. /data
    /// stays mounted and the kiosk keeps running; no reboot. Safe to run
    /// again: a grow that was cut short is finished by the next one.
    Grow {
        /// Only show what would change.
        #[arg(long)]
        check: bool,
        /// Do not ask first.
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

pub fn run(session: &mut Session, command: StorageCmd, json: bool) -> Result<(), String> {
    match command {
        StorageCmd::Show => {
            let storage = session.fetch::<api::storage::Show>()?;
            print(json, &storage, || show(&storage))
        }
        StorageCmd::Partitions => {
            let storage = session.fetch::<api::storage::Show>()?;
            print(json, &storage.partitions, || {
                show_partitions(&storage.partitions)
            })
        }
        StorageCmd::Usage => {
            let storage = session.fetch::<api::storage::Show>()?;
            print(json, &storage.filesystems, || {
                show_usage(&storage.filesystems)
            })
        }
        StorageCmd::Grow { check, yes } => grow(session, json, check, yes),
    }
}

/// `2.8 GB free of 4.0 GB (27% used)`, the use colored by how full it is.
pub fn usage_line(fs: &FsUsage) -> String {
    style::line(&shared::usage_line(fs))
}

fn show(storage: &Storage) {
    let row = |label: &str, value: String| style::row(label, &value);
    let model = storage
        .model
        .as_deref()
        .map(|model| format!(" {}", paint(style::MUTED, model)))
        .unwrap_or_default();
    row(
        "disk",
        format!(
            "{} {} {}{model}",
            paint(style::HEADING, &storage.device),
            size_label(storage.size),
            paint(style::MUTED, &storage.table)
        ),
    );
    if storage.unallocated > 0 {
        row(
            "unallocated",
            format!(
                "{} {}",
                paint(style::WARN, size_label(storage.unallocated)),
                paint(
                    style::MUTED,
                    format!(
                        "- `{}` gives it to /data",
                        paint(style::CMD, "tessaro-ctl storage grow")
                    )
                )
            ),
        );
    } else {
        row("unallocated", paint(style::MUTED, "none"));
    }
    for mountpoint in ["/", "/data"] {
        match storage
            .filesystems
            .iter()
            .find(|fs| fs.mountpoint == mountpoint)
        {
            Some(fs) => row(mountpoint, usage_line(fs)),
            None => row(mountpoint, paint(style::MUTED, "not mounted")),
        }
    }
}

fn dash(value: Option<&str>) -> String {
    value.unwrap_or("-").to_string()
}

fn show_partitions(partitions: &[Partition]) {
    println!(
        "{} {} {} {} {} {} {}",
        pad(style::HEADING, "#", 3),
        pad(style::HEADING, "name", 12),
        pad(style::HEADING, "start", 10),
        pad(style::HEADING, "size", 10),
        pad(style::HEADING, "label", 10),
        pad(style::HEADING, "fs", 6),
        paint(style::HEADING, "mountpoint"),
    );
    for partition in partitions {
        println!(
            "{} {} {} {} {} {} {}",
            pad(style::LABEL, partition.number, 3),
            pad(style::HEADING, &partition.name, 12),
            pad(style::MUTED, size_label(partition.start), 10),
            pad(anstyle::Style::new(), size_label(partition.size), 10),
            pad(anstyle::Style::new(), dash(partition.label.as_deref()), 10),
            pad(style::MUTED, dash(partition.fstype.as_deref()), 6),
            dash(partition.mountpoint.as_deref()),
        );
    }
}

fn show_usage(filesystems: &[FsUsage]) {
    println!(
        "{} {} {} {} {} {}",
        pad(style::HEADING, "mountpoint", 16),
        pad(style::HEADING, "fs", 6),
        pad(style::HEADING, "size", 10),
        pad(style::HEADING, "used", 10),
        pad(style::HEADING, "available", 10),
        paint(style::HEADING, "use%"),
    );
    for fs in filesystems {
        let percent = fs.used_percent();
        println!(
            "{} {} {} {} {} {}",
            pad(style::HEADING, &fs.mountpoint, 16),
            pad(style::MUTED, &fs.fstype, 6),
            pad(anstyle::Style::new(), size_label(fs.size), 10),
            pad(anstyle::Style::new(), size_label(fs.used), 10),
            pad(anstyle::Style::new(), size_label(fs.available), 10),
            paint(style::usage_level(percent), format!("{percent}%")),
        );
    }
}

/// The plan, then - unless `check` - a confirmation and the grow itself.
fn grow(session: &mut Session, json: bool, check: bool, yes: bool) -> Result<(), String> {
    let plan = shared::check(session)?;
    if !json {
        let (facts, nothing) = plan.facts();
        style::facts(&facts);
        if let Some(nothing) = nothing {
            println!("{}", style::line(&nothing));
        }
    }
    if check || !plan.grows() {
        if json {
            println!(
                "{}",
                serde_json::to_string(&plan.0).expect("events serialize")
            );
        }
        return Ok(());
    }

    crate::prompt::confirm(
        yes,
        &format!(
            "Grow /data on {}? It stays mounted and the kiosk keeps running.",
            session.node.name
        ),
    )?;

    follow_job::<api::storage::Grow, _>(
        session,
        GrowBody { check: false },
        json,
        |step: StorageGrowEvent| {
            if let Some(line) = shared::event_line(&step) {
                println!("{}", style::line(&line));
            }
        },
    )
    .map_err(|error| {
        if json {
            error
        } else {
            eprintln!("{}", paint(style::BAD, "the grow stopped"));
            format!("{error}\n{}", shared::STOPPED_HINT)
        }
    })
}
