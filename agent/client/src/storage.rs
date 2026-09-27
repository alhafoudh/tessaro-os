//! The disk a device runs from, for `tessaro-ctl storage` and the GUI's
//! Storage page alike: how full it is, and growing `/data` over the rest of
//! the disk.
//!
//! The device makes its plan from the disk every time, so a grow is safe to
//! run again; both clients ask for the plan first (`check`) and only grow
//! when it would change something.

use protocol::api::{self, GrowBody};
use protocol::{size_label, FsUsage, StorageGrowEvent};

use crate::connect::Session;
use crate::text::{usage_level, Fact, Line, Tone};

/// `2.8 GB free of 4.0 GB (27% used)`, the use in how full it is.
pub fn usage_line(fs: &FsUsage) -> Line {
    free_line(fs.available, fs.size, fs.used_percent())
}

/// The same line for anything with a size and free space: RAM too.
pub fn free_line(available: u64, size: u64, percent: u64) -> Line {
    Line::plain(format!(
        "{} free of {} ",
        size_label(available),
        size_label(size)
    ))
    .add(usage_level(percent), format!("({percent}% used)"))
}

/// What growing `/data` would do, from the device's own dry run.
pub struct Plan(pub StorageGrowEvent);

impl Plan {
    /// Whether the grow would change anything.
    pub fn grows(&self) -> bool {
        match &self.0 {
            StorageGrowEvent::Plan {
                partition_from,
                partition_to,
                filesystem_from,
                filesystem_to,
                ..
            } => partition_to > partition_from || filesystem_to > filesystem_from,
            _ => false,
        }
    }

    /// The plan as facts, and the line to add when there is nothing to grow.
    pub fn facts(&self) -> (Vec<Fact>, Option<Line>) {
        let StorageGrowEvent::Plan {
            partition,
            partition_from,
            partition_to,
            filesystem_from,
            filesystem_to,
        } = &self.0
        else {
            return (Vec::new(), None);
        };
        let change = |from: u64, to: u64| {
            if to > from {
                Line::plain(format!("{} -> ", size_label(from))).add(Tone::Ok, size_label(to))
            } else {
                Line::plain(format!("{} ", size_label(from))).add(Tone::Muted, "(unchanged)")
            }
        };
        let facts = vec![
            Fact::new(
                "partition",
                Line::of(Tone::Heading, partition)
                    .text(" ")
                    .join(change(*partition_from, *partition_to)),
            ),
            Fact::new("filesystem", change(*filesystem_from, *filesystem_to)),
        ];
        let nothing = (!self.grows())
            .then(|| Line::of(Tone::Ok, "nothing to grow: /data already fills the disk"));
        (facts, nothing)
    }
}

/// Ask the device what a grow would do, changing nothing.
pub fn check(session: &mut Session) -> Result<Plan, String> {
    let mut plan = None;
    session.job::<api::storage::Grow, StorageGrowEvent>(
        GrowBody { check: true },
        &|| false,
        |event| {
            if let Ok(event @ StorageGrowEvent::Plan { .. }) = event {
                plan = Some(event);
            }
        },
    )?;
    plan.map(Plan)
        .ok_or_else(|| "the device sent no plan".to_string())
}

/// A step of the grow, as a line; the plan it starts with has none.
pub fn event_line(step: &StorageGrowEvent) -> Option<Line> {
    match step {
        StorageGrowEvent::Plan { .. } => None,
        StorageGrowEvent::Step { what, command } => {
            Some(Line::plain(format!("{what} ")).add(Tone::Muted, format!("({command})")))
        }
        StorageGrowEvent::Grown {
            partition,
            filesystem,
        } => Some(
            Line::of(
                Tone::Ok,
                format!("grew /data to {}", size_label(*filesystem)),
            )
            .text(" ")
            .add(
                Tone::Muted,
                format!("(partition {})", size_label(*partition)),
            ),
        ),
    }
}

/// What to tell the user when a grow stops half way.
pub const STOPPED_HINT: &str =
    "running `tessaro-ctl storage grow` again picks up where this one stopped";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usage_line_reads_as_plain_text() {
        let fs = FsUsage {
            mountpoint: "/data".into(),
            source: "/dev/sda3".into(),
            fstype: "ext4".into(),
            size: 4_000_000_000,
            used: 1_000_000_000,
            available: 2_800_000_000,
        };
        assert_eq!(
            usage_line(&fs).to_string(),
            "2.8 GB free of 4.0 GB (27% used)"
        );
    }

    #[test]
    fn a_full_disk_has_nothing_to_grow() {
        let plan = Plan(StorageGrowEvent::Plan {
            partition: "/dev/mmcblk0p3".into(),
            partition_from: 10,
            partition_to: 10,
            filesystem_from: 9,
            filesystem_to: 9,
        });
        assert!(!plan.grows());
        assert!(plan.facts().1.is_some());
    }
}
