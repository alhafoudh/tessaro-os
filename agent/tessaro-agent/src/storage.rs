//! The disk the device runs from: its partitions, how full each filesystem
//! is, and `tessaro-ctl storage grow`, which gives `/data` the space after
//! it.
//!
//! Every image carries `/data` at `IMAGE_DATA_MIN_SIZE` as the last
//! partition, and `image:flash` and `update send --repartition` leave it at
//! that size whatever the disk holds. The grow happens only on command, on
//! the running device, with `/data` mounted: `sfdisk` moves the end of
//! partition 3 to the end of the disk, `partx` hands the kernel the new size
//! through BLKPG (BLKRRPART refuses a disk with mounted partitions), and
//! `resize2fs` grows ext4 online. Each step is safe to cut: the partition
//! write is one table write, and the kernel journals an online resize. The
//! plan is made again from the disk on every run, so a grow that stopped
//! half way - partition grown, filesystem not - is finished by the next one.
//!
//! Everything here is blocking file I/O and child processes; call it
//! through `deadline::blocking`, or from the grow's own thread.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use protocol::{size_label, FsUsage, Partition, Storage, StorageGrowEvent, GROW_MIN};
use tokio::sync::{mpsc, OwnedMutexGuard};
use update::fsutil;
use update::layout::{self, Probe};
use update::DATA_PARTITION;

use crate::log::Log;
use crate::paths::Paths;

/// How long the server waits for a whole grow. resize2fs on a large card is
/// the slow part.
pub const TOTAL: Duration = Duration::from_secs(600);

const SECTOR: u64 = 512;
/// The backup GPT header and its entries, at the very end of the disk.
const GPT_BACKUP: u64 = 33 * SECTOR;
/// An MBR entry counts sectors in 32 bits.
const MBR_END: u64 = (1 << 32) * SECTOR;
/// sfdisk aligns partitions to this grain.
const ALIGN: u64 = 1 << 20;

/// Filesystems worth reporting. Pseudo filesystems, and overlays that only
/// repeat their upper layer's numbers, are left out.
const REPORTED: &[&str] = &[
    "ext4", "ext3", "ext2", "vfat", "tmpfs", "btrfs", "xfs", "f2fs",
];

pub type Step = Result<StorageGrowEvent, String>;

/// Where to look: sysfs and udev's links, the mount table, `/dev`.
#[derive(Debug, Clone)]
pub struct Sources {
    pub probe: Probe,
    pub by_label: PathBuf,
    pub mountinfo: PathBuf,
    pub dev: PathBuf,
}

impl Sources {
    pub fn new(paths: &Paths) -> Self {
        Self {
            probe: Probe {
                cmdline: paths.cmdline.clone(),
                sys_block: paths.sys_block.clone(),
                by_partuuid: paths.by_partuuid.clone(),
                by_label: paths.by_label.clone(),
            },
            by_label: paths.by_label.clone(),
            mountinfo: paths.mountinfo.clone(),
            dev: paths.dev.clone(),
        }
    }
}

/// One line of `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mount {
    /// `8:3`.
    device: String,
    /// The directory of the filesystem that is mounted: `/` for a whole
    /// filesystem, something else for a bind mount of part of one.
    root: String,
    mountpoint: String,
    fstype: String,
    source: String,
}

/// The disk holding root, and every filesystem mounted from anywhere.
pub fn snapshot(paths: &Paths) -> Result<Storage, String> {
    snapshot_with(&Sources::new(paths), &|path| fsutil::usage(path))
}

type UsageOf<'a> = &'a dyn Fn(&Path) -> io::Result<fsutil::Usage>;

fn snapshot_with(sources: &Sources, usage_of: UsageOf) -> Result<Storage, String> {
    let layout = layout::probe(&sources.probe)?;
    let gpt = !layout::is_mbr(&layout.root.partuuid);
    let disk_dir = fs::canonicalize(sources.probe.sys_block.join(&layout.disk))
        .map_err(|err| format!("{}: {err}", layout.disk))?;
    let mounts = fs::read_to_string(&sources.mountinfo)
        .map(|text| parse_mountinfo(&text))
        .unwrap_or_default();
    let labels = labels(&sources.by_label);

    let mut partitions = Vec::new();
    for entry in fs::read_dir(&disk_dir).map_err(|err| format!("{}: {err}", disk_dir.display()))? {
        let Ok(entry) = entry else { continue };
        let dir = entry.path();
        let Some(number) = read_number(&dir.join("partition")) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let device = read(&dir.join("dev"));
        let mount = device
            .as_deref()
            .and_then(|device| whole_mount(&mounts, device));
        partitions.push(Partition {
            number: number as u32,
            start: read_number(&dir.join("start")).unwrap_or(0) * SECTOR,
            size: read_number(&dir.join("size")).unwrap_or(0) * SECTOR,
            label: labels.get(&name).cloned(),
            fstype: mount.map(|mount| mount.fstype.clone()),
            mountpoint: mount.map(|mount| mount.mountpoint.clone()),
            name,
        });
    }
    partitions.sort_by_key(|partition| partition.number);

    let end = partitions
        .iter()
        .map(|partition| partition.start + partition.size)
        .max()
        .unwrap_or(0);
    let usable = usable_end(layout.disk_size, gpt);
    let tail = usable.saturating_sub(end);

    Ok(Storage {
        device: layout.disk,
        size: layout.disk_size,
        table: if gpt { "gpt" } else { "dos" }.to_string(),
        model: read(&disk_dir.join("device/model")),
        unallocated: if tail >= GROW_MIN { tail } else { 0 },
        partitions,
        filesystems: filesystems(&mounts, usage_of),
    })
}

/// Where the last partition may end: before the backup GPT header, and
/// within what an MBR entry can count.
fn usable_end(disk_size: u64, gpt: bool) -> u64 {
    if gpt {
        disk_size.saturating_sub(GPT_BACKUP)
    } else {
        disk_size.min(MBR_END)
    }
}

/// Every mounted filesystem worth reporting, once each: a device mounted in
/// several places (the binds of `/data` over `/home` and `/root`) is shown
/// where it is mounted whole.
fn filesystems(mounts: &[Mount], usage_of: UsageOf) -> Vec<FsUsage> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for mount in mounts {
        if !REPORTED.contains(&mount.fstype.as_str())
            || mount.root != "/"
            || ["/dev", "/sys", "/proc"].iter().any(|pseudo| {
                mount.mountpoint == *pseudo || mount.mountpoint.starts_with(&format!("{pseudo}/"))
            })
            || seen.contains(&mount.device)
        {
            continue;
        }
        let Ok(usage) = usage_of(Path::new(&mount.mountpoint)) else {
            continue;
        };
        seen.push(mount.device.clone());
        out.push(FsUsage {
            mountpoint: mount.mountpoint.clone(),
            source: mount.source.clone(),
            fstype: mount.fstype.clone(),
            size: usage.size,
            used: usage.size.saturating_sub(usage.free),
            available: usage.available,
        });
    }
    out
}

/// The mount of the whole filesystem on `device` (`8:3`), not of a
/// directory inside it.
fn whole_mount<'a>(mounts: &'a [Mount], device: &str) -> Option<&'a Mount> {
    mounts
        .iter()
        .find(|mount| mount.device == device && mount.root == "/")
}

fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (head, tail) = line.split_once(" - ")?;
            let head: Vec<&str> = head.split(' ').collect();
            let mut tail = tail.split(' ');
            Some(Mount {
                device: head.get(2)?.to_string(),
                root: unescape(head.get(3)?),
                mountpoint: unescape(head.get(4)?),
                fstype: tail.next()?.to_string(),
                source: unescape(tail.next()?),
            })
        })
        .collect()
}

/// The kernel writes a space in a path as `\040`.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            let digits = &bytes[i + 1..i + 4];
            let code = digits
                .iter()
                .try_fold(0u32, |code, digit| {
                    (b'0'..=b'7')
                        .contains(digit)
                        .then(|| code * 8 + u32::from(digit - b'0'))
                })
                .and_then(|code| u8::try_from(code).ok());
            if let Some(code) = code {
                out.push(code);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Partition name to filesystem label, from udev's `/dev/disk/by-label`,
/// where a space in a label is `\x20`.
fn labels(dir: &Path) -> BTreeMap<String, String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let target = fs::read_link(entry.path()).ok()?;
            let name = target.file_name()?.to_string_lossy().into_owned();
            let label = unescape_hex(&entry.file_name().to_string_lossy());
            Some((name, label))
        })
        .collect()
}

fn unescape_hex(name: &str) -> String {
    let mut out = Vec::new();
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1) == Some(&b'x')
            && i + 4 <= bytes.len()
            && bytes[i + 2..i + 4].iter().all(u8::is_ascii_hexdigit)
        {
            if let Ok(code) = u8::from_str_radix(&name[i + 2..i + 4], 16) {
                out.push(code);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `storage.*` read-only keys.
pub fn values(storage: &Storage) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    values.insert("storage.size".to_string(), size_label(storage.size));
    values.insert(
        "storage.unallocated".to_string(),
        size_label(storage.unallocated),
    );
    let at = |mountpoint: &str| {
        storage
            .filesystems
            .iter()
            .find(|fs| fs.mountpoint == mountpoint)
    };
    let data = at("/data");
    values.insert(
        "storage.data_size".to_string(),
        data.map(|fs| size_label(fs.size)).unwrap_or_default(),
    );
    values.insert(
        "storage.data_free".to_string(),
        data.map(|fs| size_label(fs.available)).unwrap_or_default(),
    );
    values.insert(
        "storage.data_used".to_string(),
        data.map(|fs| format!("{}%", fs.used_percent()))
            .unwrap_or_default(),
    );
    values.insert(
        "storage.root_free".to_string(),
        at("/")
            .map(|fs| size_label(fs.available))
            .unwrap_or_default(),
    );
    values
}

/// `/data`'s usage, for `device status`.
pub fn data(storage: &Storage) -> Option<FsUsage> {
    storage
        .filesystems
        .iter()
        .find(|fs| fs.mountpoint == "/data")
        .cloned()
}

/// What a grow would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// `/dev/sda`.
    pub disk: PathBuf,
    /// `/dev/sda3`.
    pub partition: PathBuf,
    pub gpt: bool,
    pub partition_from: u64,
    pub partition_to: u64,
    pub filesystem_from: u64,
    pub filesystem_to: u64,
}

impl Plan {
    pub fn grows_partition(&self) -> bool {
        self.partition_to > self.partition_from
    }

    pub fn grows_filesystem(&self) -> bool {
        self.filesystem_to > self.filesystem_from
    }

    pub fn event(&self) -> StorageGrowEvent {
        StorageGrowEvent::Plan {
            partition: self.partition.display().to_string(),
            partition_from: self.partition_from,
            partition_to: self.partition_to,
            filesystem_from: self.filesystem_from,
            filesystem_to: self.filesystem_to,
        }
    }

    /// Each step, what it is for and the command, in order.
    pub fn steps(&self) -> Vec<(String, Invocation)> {
        let disk = self.disk.display().to_string();
        let mut steps = Vec::new();
        if self.grows_partition() {
            if self.gpt {
                steps.push((
                    "moving the backup GPT header to the end of the disk".to_string(),
                    Invocation::new(
                        "sfdisk",
                        &[
                            "--no-reread",
                            "--no-tell-kernel",
                            "--relocate",
                            "gpt-bak-std",
                            &disk,
                        ],
                    ),
                ));
            }
            steps.push((
                format!("growing partition {DATA_PARTITION} to the end of the disk"),
                Invocation::new(
                    "sfdisk",
                    &["--no-reread", "--no-tell-kernel", "-N", "3", &disk],
                )
                .input(",+\n"),
            ));
            steps.push((
                "telling the kernel the partition's new size".to_string(),
                Invocation::new("partx", &["-u", "-n", "3", &disk]),
            ));
        }
        if self.grows_partition() || self.grows_filesystem() {
            steps.push((
                "growing the filesystem".to_string(),
                Invocation::new("resize2fs", &[&self.partition.display().to_string()]),
            ));
        }
        steps
    }
}

/// The plan for this disk, made fresh from sysfs and the filesystem's own
/// superblock.
pub fn plan(sources: &Sources) -> Result<Plan, String> {
    let layout = layout::probe(&sources.probe)?;
    let gpt = !layout::is_mbr(&layout.root.partuuid);
    let disk_dir = fs::canonicalize(sources.probe.sys_block.join(&layout.disk))
        .map_err(|err| format!("{}: {err}", layout.disk))?;

    let mut data = None;
    let mut last_start = 0;
    for entry in fs::read_dir(&disk_dir).map_err(|err| format!("{}: {err}", disk_dir.display()))? {
        let Ok(entry) = entry else { continue };
        let dir = entry.path();
        let Some(number) = read_number(&dir.join("partition")) else {
            continue;
        };
        let start = read_number(&dir.join("start")).unwrap_or(0) * SECTOR;
        let size = read_number(&dir.join("size")).unwrap_or(0) * SECTOR;
        last_start = last_start.max(start);
        if number == DATA_PARTITION as u64 {
            data = Some((
                entry.file_name().to_string_lossy().into_owned(),
                start,
                size,
            ));
        }
    }
    let (name, start, size) =
        data.ok_or_else(|| format!("{} has no partition {DATA_PARTITION}", layout.disk))?;
    if start != last_start {
        return Err(format!(
            "partition {DATA_PARTITION} ({name}) is not the last one on {}, so it cannot grow: \
             this disk has the old layout with swap after /data, which only a reflash or \
             `tessaro-ctl update send --repartition` changes",
            layout.disk
        ));
    }

    let partition = sources.dev.join(&name);
    let filesystem_from = ext4_size(&partition)?;

    let room = usable_end(layout.disk_size, gpt).saturating_sub(start);
    let room = room - room % ALIGN;
    let partition_to = if room >= size + GROW_MIN { room } else { size };
    let filesystem_to = if partition_to > size || size >= filesystem_from + GROW_MIN {
        partition_to
    } else {
        filesystem_from
    };

    Ok(Plan {
        disk: sources.dev.join(&layout.disk),
        partition,
        gpt,
        partition_from: size,
        partition_to,
        filesystem_from,
        filesystem_to,
    })
}

/// The size of the ext4 filesystem on `device`, from its superblock.
fn ext4_size(device: &Path) -> Result<u64, String> {
    let mut file = fs::File::open(device).map_err(|err| format!("{}: {err}", device.display()))?;
    let mut block = [0u8; 1024];
    file.seek(SeekFrom::Start(1024))
        .and_then(|_| file.read_exact(&mut block))
        .map_err(|err| format!("reading the superblock of {}: {err}", device.display()))?;
    superblock_size(&block)
        .ok_or_else(|| format!("{} does not hold an ext4 filesystem", device.display()))
}

/// Block count times block size, from the 1024 bytes of an ext2/3/4
/// superblock.
fn superblock_size(block: &[u8; 1024]) -> Option<u64> {
    let u16_at = |at: usize| u16::from_le_bytes([block[at], block[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(block[at..at + 4].try_into().unwrap());
    if u16_at(0x38) != 0xEF53 {
        return None;
    }
    const INCOMPAT_64BIT: u32 = 0x80;
    let mut blocks = u32_at(0x04) as u64;
    if u32_at(0x60) & INCOMPAT_64BIT != 0 {
        blocks |= (u32_at(0x150) as u64) << 32;
    }
    let log = u32_at(0x18);
    if log > 6 {
        return None;
    }
    Some(blocks * (1024 << log))
}

/// One program to run, with what goes to its stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub input: Option<String>,
}

impl Invocation {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            input: None,
        }
    }

    fn input(mut self, input: &str) -> Self {
        self.input = Some(input.to_string());
        self
    }

    pub fn command_line(&self) -> String {
        let mut line = std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(input) = &self.input {
            line = format!("echo '{}' | {line}", input.trim_end());
        }
        line
    }
}

/// Runs programs. The real ones on a device, a recording in the tests.
pub trait Run {
    fn run(&mut self, invocation: &Invocation) -> Result<(), String>;
}

pub struct System;

impl Run for System {
    fn run(&mut self, invocation: &Invocation) -> Result<(), String> {
        let output = crate::proc::run(
            Command::new(&invocation.program).args(&invocation.args),
            invocation.input.as_deref().map(str::as_bytes),
        )?;
        if output.status.success() {
            return Ok(());
        }
        let said = crate::proc::said(&output);
        Err(if said.is_empty() {
            format!("{} failed: {}", invocation.command_line(), output.status)
        } else {
            format!("{} failed: {said}", invocation.command_line())
        })
    }
}

/// Start a grow on its own thread; `lock` is held until that thread is done.
/// With `check`, only the plan is sent.
pub fn start(
    sources: Sources,
    check: bool,
    lock: OwnedMutexGuard<()>,
    log: Arc<Log>,
) -> mpsc::Receiver<Step> {
    crate::sync::spawn_steps(lock, move |send| {
        grow(&sources, check, &mut System, send, &log)
    })
}

/// Everything on the grow's thread. Steps run to the end even if whoever
/// asked has gone: stopping between them would only leave the next run
/// more to do.
fn grow(sources: &Sources, check: bool, run: &mut dyn Run, send: &dyn Fn(Step) -> bool, log: &Log) {
    let plan = match plan(sources) {
        Ok(plan) => plan,
        Err(error) => {
            send(Err(error));
            return;
        }
    };
    send(Ok(plan.event()));
    let steps = plan.steps();
    if check || steps.is_empty() {
        return;
    }

    log.info(format!(
        "storage: growing {} from {} to {}",
        plan.partition.display(),
        size_label(plan.filesystem_from),
        size_label(plan.filesystem_to)
    ));
    for (what, invocation) in steps {
        log.info(format!("storage: {what}: {}", invocation.command_line()));
        send(Ok(StorageGrowEvent::Step {
            what: what.clone(),
            command: invocation.command_line(),
        }));
        if let Err(error) = run.run(&invocation) {
            let error = format!("{what}: {error}");
            log.info(format!("storage: grow stopped: {error}"));
            send(Err(error));
            return;
        }
    }

    match self::plan(sources) {
        Ok(after) => {
            log.info(format!(
                "storage: grew /data from {} to {}",
                size_label(plan.filesystem_from),
                size_label(after.filesystem_from)
            ));
            send(Ok(StorageGrowEvent::Grown {
                partition: after.partition_from,
                filesystem: after.filesystem_from,
            }));
        }
        Err(error) => {
            send(Err(format!(
                "the grow ran, but reading the disk again failed: {error}"
            )));
        }
    }
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn read_number(path: &Path) -> Option<u64> {
    read(path)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;

    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;

    /// A fake device: `update::testing::device`'s sda (boot, root, data at
    /// 8 MiB, 4 MiB long), on a disk of `disk` bytes, with an ext4 of `fs`
    /// bytes in /dev/sda3.
    fn device(dir: &Path, disk: u64, fs_size: u64) -> Sources {
        let probe = update::testing::device(dir);
        let devices = dir.join("sys/devices/pci0/sda");
        fs::write(devices.join("size"), format!("{}\n", disk / SECTOR)).unwrap();
        for (name, dev) in [("sda1", "8:1"), ("sda2", "8:2"), ("sda3", "8:3")] {
            fs::write(devices.join(name).join("dev"), format!("{dev}\n")).unwrap();
        }
        let dev = dir.join("dev");
        write_superblock(&dev.join("sda3"), fs_size);
        let by_label = dir.join("dev/disk/by-label");
        fs::create_dir_all(&by_label).unwrap();
        symlink("../../sda3", by_label.join("data")).unwrap();
        symlink("../../sda2", by_label.join("my\\x20root")).unwrap();
        let mountinfo = dir.join("mountinfo");
        fs::write(&mountinfo, MOUNTINFO).unwrap();
        Sources {
            probe,
            by_label,
            mountinfo,
            dev,
        }
    }

    fn write_superblock(path: &Path, size: u64) {
        let mut image = vec![0u8; 2048];
        let block = 4096u64;
        let blocks = size / block;
        image[1024 + 0x04..1024 + 0x08].copy_from_slice(&(blocks as u32).to_le_bytes());
        image[1024 + 0x18..1024 + 0x1c].copy_from_slice(&2u32.to_le_bytes());
        image[1024 + 0x38..1024 + 0x3a].copy_from_slice(&0xEF53u16.to_le_bytes());
        image[1024 + 0x60..1024 + 0x64].copy_from_slice(&0x80u32.to_le_bytes());
        image[1024 + 0x150..1024 + 0x154].copy_from_slice(&((blocks >> 32) as u32).to_le_bytes());
        fs::write(path, image).unwrap();
    }

    fn set_data_size(dir: &Path, bytes: u64) {
        let part = dir.join("sys/devices/pci0/sda/sda3");
        fs::write(part.join("size"), format!("{}\n", bytes / SECTOR)).unwrap();
    }

    const MOUNTINFO: &str = "\
22 1 8:2 / / ro,relatime shared:1 - ext4 /dev/root ro
23 22 0:5 / /dev rw,nosuid shared:2 - devtmpfs devtmpfs rw
24 22 0:21 / /proc rw shared:3 - proc proc rw
25 22 0:22 / /run rw,nosuid shared:4 - tmpfs tmpfs rw,mode=755
30 22 8:3 / /data rw,relatime shared:5 - ext4 /dev/sda3 rw
31 22 0:30 / /etc rw shared:6 - overlay overlay rw,lowerdir=/etc,upperdir=/data/overlay-etc/upper
32 22 8:3 /overlay-home /home rw shared:7 - ext4 /dev/sda3 rw
33 22 8:1 / /boot rw shared:8 - vfat /dev/sda1 rw
34 25 0:40 / /run/my\\040dir rw - tmpfs tmpfs rw
";

    fn usage(path: &Path) -> io::Result<fsutil::Usage> {
        Ok(match path.to_str().unwrap() {
            "/" => fsutil::Usage {
                size: 1000,
                free: 100,
                available: 100,
            },
            "/data" => fsutil::Usage {
                size: 4000,
                free: 3000,
                available: 2800,
            },
            _ => fsutil::Usage {
                size: 10,
                free: 10,
                available: 10,
            },
        })
    }

    #[test]
    fn a_snapshot_names_partitions_mounts_and_free_space() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        let storage = snapshot_with(&sources, &usage).unwrap();

        assert_eq!(storage.device, "sda");
        assert_eq!(storage.table, "gpt");
        assert_eq!(storage.size, GIB);
        assert_eq!(storage.unallocated, GIB - 12 * MIB - GPT_BACKUP);

        let names: Vec<_> = storage.partitions.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["sda1", "sda2", "sda3"]);
        let data = &storage.partitions[2];
        assert_eq!(data.label.as_deref(), Some("data"));
        assert_eq!(data.mountpoint.as_deref(), Some("/data"));
        assert_eq!(data.fstype.as_deref(), Some("ext4"));
        assert_eq!((data.start, data.size), (8 * MIB, 4 * MIB));
        // Root is mounted as /dev/root, found by its device number.
        assert_eq!(storage.partitions[1].mountpoint.as_deref(), Some("/"));
        assert_eq!(storage.partitions[1].label.as_deref(), Some("my root"));

        let mounted: Vec<_> = storage
            .filesystems
            .iter()
            .map(|fs| fs.mountpoint.as_str())
            .collect();
        assert_eq!(mounted, ["/", "/run", "/data", "/boot", "/run/my dir"]);
        let data = storage
            .filesystems
            .iter()
            .find(|fs| fs.mountpoint == "/data")
            .unwrap();
        assert_eq!((data.size, data.used, data.available), (4000, 1000, 2800));
    }

    #[test]
    fn a_small_tail_is_not_unallocated() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), 16 * MIB, 4 * MIB);
        assert_eq!(snapshot_with(&sources, &usage).unwrap().unallocated, 0);
    }

    #[test]
    fn values_cover_every_storage_key() {
        let dir = tempfile::tempdir().unwrap();
        let storage = snapshot_with(&device(dir.path(), GIB, 4 * MIB), &usage).unwrap();
        let values = values(&storage);
        for key in protocol::keys::KEYS {
            if key.name.starts_with("storage.") {
                assert!(values.contains_key(key.name), "{}", key.name);
            }
        }
        assert_eq!(values["storage.data_used"], "27%");
        assert_eq!(values["storage.data_free"], "2.8 kB");
    }

    #[test]
    fn a_gpt_grow_relocates_grows_tells_and_resizes() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        let plan = plan(&sources).unwrap();
        let room = GIB - GPT_BACKUP - 8 * MIB;
        assert_eq!(plan.partition_to, room - room % ALIGN);
        assert_eq!(plan.filesystem_to, plan.partition_to);

        let programs: Vec<_> = plan.steps().iter().map(|(_, i)| i.command_line()).collect();
        let disk = dir.path().join("dev/sda").display().to_string();
        let part = dir.path().join("dev/sda3").display().to_string();
        assert_eq!(
            programs,
            [
                format!("sfdisk --no-reread --no-tell-kernel --relocate gpt-bak-std {disk}"),
                format!("echo ',+' | sfdisk --no-reread --no-tell-kernel -N 3 {disk}"),
                format!("partx -u -n 3 {disk}"),
                format!("resize2fs {part}"),
            ]
        );
    }

    #[test]
    fn an_mbr_grow_has_no_gpt_header_to_move() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        // An MBR disk: PARTUUIDs are the disk signature and the number.
        let by_partuuid = &sources.probe.by_partuuid;
        for entry in fs::read_dir(by_partuuid).unwrap() {
            let entry = entry.unwrap();
            let target = fs::read_link(entry.path()).unwrap();
            let number = target.to_string_lossy().chars().last().unwrap();
            fs::remove_file(entry.path()).unwrap();
            symlink(&target, by_partuuid.join(format!("1234abcd-0{number}"))).unwrap();
        }
        fs::write(&sources.probe.cmdline, "root=/dev/sda2 rootwait\n").unwrap();
        let plan = plan(&sources).unwrap();
        assert!(!plan.gpt);
        let room = GIB - 8 * MIB;
        assert_eq!(plan.partition_to, room);
        assert!(plan
            .steps()
            .iter()
            .all(|(_, i)| !i.args.contains(&"--relocate".to_string())));
    }

    #[test]
    fn a_grown_disk_has_nothing_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), 16 * MIB, 4 * MIB);
        let plan = plan(&sources).unwrap();
        assert!(!plan.grows_partition() && !plan.grows_filesystem());
        assert!(plan.steps().is_empty());
    }

    #[test]
    fn a_partition_grown_without_its_filesystem_only_resizes() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        let room = GIB - GPT_BACKUP - 8 * MIB;
        set_data_size(dir.path(), room - room % ALIGN);
        let plan = plan(&sources).unwrap();
        assert!(!plan.grows_partition());
        assert!(plan.grows_filesystem());
        let programs: Vec<_> = plan
            .steps()
            .iter()
            .map(|(_, i)| i.program.clone())
            .collect();
        assert_eq!(programs, ["resize2fs"]);
    }

    #[test]
    fn data_that_is_not_last_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        let swap = dir.path().join("sys/devices/pci0/sda/sda4");
        fs::create_dir_all(&swap).unwrap();
        fs::write(swap.join("partition"), "4\n").unwrap();
        fs::write(swap.join("start"), format!("{}\n", 12 * MIB / SECTOR)).unwrap();
        fs::write(swap.join("size"), "2048\n").unwrap();
        let error = plan(&sources).unwrap_err();
        assert!(error.contains("not the last one"), "{error}");
    }

    #[test]
    fn a_partition_without_ext4_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        fs::write(sources.dev.join("sda3"), vec![0u8; 2048]).unwrap();
        assert!(plan(&sources).unwrap_err().contains("ext4"));
    }

    #[derive(Default)]
    struct Recorder {
        ran: Vec<String>,
        fail: Option<&'static str>,
    }

    impl Run for Recorder {
        fn run(&mut self, invocation: &Invocation) -> Result<(), String> {
            self.ran.push(invocation.program.clone());
            if self.fail == Some(invocation.program.as_str()) {
                return Err("no".to_string());
            }
            Ok(())
        }
    }

    fn collect(sources: &Sources, check: bool, run: &mut Recorder) -> Vec<Step> {
        let events = RefCell::new(Vec::new());
        let send = |step: Step| {
            events.borrow_mut().push(step);
            true
        };
        grow(sources, check, run, &send, &Log::buffered(false));
        events.into_inner()
    }

    #[test]
    fn check_only_sends_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        let mut run = Recorder::default();
        let events = collect(&sources, true, &mut run);
        assert!(run.ran.is_empty());
        assert!(matches!(
            events.as_slice(),
            [Ok(StorageGrowEvent::Plan { .. })]
        ));
    }

    #[test]
    fn a_failed_step_stops_the_ones_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        let mut run = Recorder {
            fail: Some("sfdisk"),
            ..Recorder::default()
        };
        let events = collect(&sources, false, &mut run);
        assert_eq!(run.ran, ["sfdisk"]);
        let error = events.last().unwrap().clone().unwrap_err();
        assert!(error.starts_with("moving the backup GPT header"), "{error}");
    }

    #[test]
    fn a_grow_ends_with_the_sizes_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let sources = device(dir.path(), GIB, 4 * MIB);
        let mut run = Recorder::default();
        let events = collect(&sources, false, &mut run);
        assert_eq!(run.ran, ["sfdisk", "sfdisk", "partx", "resize2fs"]);
        // Nothing really ran, so the sizes read back are the old ones.
        assert_eq!(
            events.last().unwrap(),
            &Ok(StorageGrowEvent::Grown {
                partition: 4 * MIB,
                filesystem: 4 * MIB
            })
        );
    }

    #[test]
    fn superblocks_are_read_with_their_high_bits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sb");
        write_superblock(&path, 20 * (1 << 40));
        assert_eq!(ext4_size(&path).unwrap(), 20 * (1 << 40));
    }

    #[test]
    fn mountinfo_fields_are_unescaped() {
        let mounts = parse_mountinfo("1 0 0:1 / /a\\040b rw - tmpfs my\\040src rw\n");
        assert_eq!(mounts[0].mountpoint, "/a b");
        assert_eq!(mounts[0].source, "my src");
    }

    #[test]
    fn a_snapshot_of_this_host_does_not_fail_to_build() {
        // The host has no Tessaro disk; this only proves the path is taken.
        let paths = Paths::load(&std::collections::HashMap::new());
        let _ = snapshot(&paths);
    }
}
