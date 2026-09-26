//! `tessaro-ctl storage ...`: the disk the device runs from, how full its
//! filesystems are, and growing `/data` over the rest of the disk.
//!
//! An image carries `/data` at a fixed size, so a device flashed onto a
//! larger card or disk has space after it that nothing uses until
//! `storage grow` gives it to `/data`. The device does the work with `/data`
//! mounted and the kiosk running, and makes its plan from the disk every
//! time, so the command is safe to run again.

use anstream::{eprintln, println};
use clap::Subcommand;
use protocol::{size_label, Command, FsUsage, Partition, Storage, StorageGrowEvent};

use crate::connect::{Session, StreamEvents};
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
            let storage: Storage = session.call(Command::Storage)?;
            print(json, &storage, || show(&storage))
        }
        StorageCmd::Partitions => {
            let storage: Storage = session.call(Command::Storage)?;
            print(json, &storage.partitions, || {
                show_partitions(&storage.partitions)
            })
        }
        StorageCmd::Usage => {
            let storage: Storage = session.call(Command::Storage)?;
            print(json, &storage.filesystems, || {
                show_usage(&storage.filesystems)
            })
        }
        StorageCmd::Grow { check, yes } => grow(session, json, check, yes),
    }
}

/// `2.8 GB free of 4.0 GB (27% used)`, the use colored by how full it is.
pub fn usage_line(fs: &FsUsage) -> String {
    free_line(fs.available, fs.size, fs.used_percent())
}

/// The same line for anything with a size and free space: RAM too.
pub fn free_line(available: u64, size: u64, percent: u64) -> String {
    format!(
        "{} free of {} {}",
        size_label(available),
        size_label(size),
        paint(style::usage_level(percent), format!("({percent}% used)"))
    )
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
    let mut plan = None;
    session.stream(Command::StorageGrow { check: true }, |event| {
        if let Ok(event @ StorageGrowEvent::Plan { .. }) = serde_json::from_value(event) {
            plan = Some(event);
        }
    })?;
    let Some(plan) = plan else {
        return Err("the device sent no plan".to_string());
    };
    if !json {
        show_plan(&plan);
    }
    let StorageGrowEvent::Plan {
        partition_from,
        partition_to,
        filesystem_from,
        filesystem_to,
        ..
    } = &plan
    else {
        unreachable!("only a plan is kept");
    };
    if check || (partition_to <= partition_from && filesystem_to <= filesystem_from) {
        if json {
            println!(
                "{}",
                serde_json::to_string(&plan).expect("events serialize")
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

    session
        .stream_events(
            Command::StorageGrow { check: false },
            json,
            |step: StorageGrowEvent| match step {
                StorageGrowEvent::Plan { .. } => {}
                StorageGrowEvent::Step { what, command } => {
                    println!("{} {}", what, paint(style::MUTED, format!("({command})")));
                }
                StorageGrowEvent::Grown {
                    partition,
                    filesystem,
                } => println!(
                    "{} {}",
                    paint(
                        style::OK,
                        format!("grew /data to {}", size_label(filesystem))
                    ),
                    paint(
                        style::MUTED,
                        format!("(partition {})", size_label(partition))
                    )
                ),
            },
        )
        .map_err(|error| {
            if json {
                error
            } else {
                eprintln!("{}", paint(style::BAD, "the grow stopped"));
                format!(
                "{error}\nrunning `tessaro-ctl storage grow` again picks up where this one stopped"
            )
            }
        })
}

fn show_plan(plan: &StorageGrowEvent) {
    let StorageGrowEvent::Plan {
        partition,
        partition_from,
        partition_to,
        filesystem_from,
        filesystem_to,
    } = plan
    else {
        return;
    };
    let change = |from: u64, to: u64| {
        if to > from {
            format!(
                "{} -> {}",
                size_label(from),
                paint(style::OK, size_label(to))
            )
        } else {
            format!(
                "{} {}",
                size_label(from),
                paint(style::MUTED, "(unchanged)")
            )
        }
    };
    let row = |label: &str, value: String| style::row(label, &value);
    row(
        "partition",
        format!(
            "{} {}",
            paint(style::HEADING, partition),
            change(*partition_from, *partition_to)
        ),
    );
    row("filesystem", change(*filesystem_from, *filesystem_to));
    if partition_to <= partition_from && filesystem_to <= filesystem_from {
        println!(
            "{}",
            paint(style::OK, "nothing to grow: /data already fills the disk")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anstream::adapter::strip_str;

    #[test]
    fn a_usage_line_strips_to_plain_text() {
        let fs = FsUsage {
            mountpoint: "/data".into(),
            source: "/dev/sda3".into(),
            fstype: "ext4".into(),
            size: 4_000_000_000,
            used: 1_000_000_000,
            available: 2_800_000_000,
        };
        assert_eq!(
            strip_str(&usage_line(&fs)).to_string(),
            "2.8 GB free of 4.0 GB (27% used)"
        );
    }
}
