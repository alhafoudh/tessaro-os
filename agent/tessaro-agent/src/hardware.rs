//! What the hardware is, how much RAM it has, how busy its CPU is and how
//! warm it runs, for `status`.
//!
//! Plain reads of what the kernel already exposes: DMI on x86
//! (`/sys/class/dmi/id`), the device tree on the Pi (`/proc/device-tree`),
//! and `/proc/cpuinfo`, `/proc/meminfo`, `/proc/stat` and `/sys/class/hwmon`
//! everywhere. DMI wins when both exist. Anything unreadable, or one of the
//! placeholders firmware vendors leave in DMI, is `None`. See
//! docs/hardware.md.

use std::fs;
use std::path::Path;

use protocol::{Hardware, MemUsage, Temperature};

use crate::paths::Paths;

pub fn hardware(paths: &Paths) -> Hardware {
    let mut out = dmi(&paths.dmi)
        .or_else(|| device_tree(&paths.device_tree))
        .unwrap_or_default();
    let (cpu, cores) = cpuinfo(&paths.cpuinfo);
    out.cpu = cpu.or(out.cpu);
    out.cores = cores;
    out.arch = std::env::consts::ARCH.to_string();
    out
}

/// `MemTotal` and `MemAvailable`, or `None` without both.
pub fn memory(meminfo: &Path) -> Option<MemUsage> {
    let text = fs::read_to_string(meminfo).ok()?;
    Some(MemUsage {
        total: meminfo_bytes(&text, "MemTotal")?,
        available: meminfo_bytes(&text, "MemAvailable")?,
    })
}

/// One `/proc/meminfo` line, `MemTotal:  4000000 kB`, in bytes.
pub fn meminfo_bytes(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .and_then(|rest| {
            rest.trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|kib| kib * 1024)
}

/// Every `temp*_input` of every hwmon device, in the kernel's order, leaving
/// out coretemp's per-core readings (its package reading stands for them)
/// and any reading that fails, such as a disk that does not answer.
/// Thermal zones are here too: `CONFIG_THERMAL_HWMON` registers each as a
/// hwmon device named after its type (`acpitz`, `cpu_thermal`).
pub fn temperatures(hwmon: &Path) -> Vec<Temperature> {
    let mut out = Vec::new();
    for dir in numbered(hwmon, "hwmon", "") {
        let text = |name: &str| {
            fs::read_to_string(dir.join(name))
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let millis = |name: &str| text(name)?.parse::<i32>().ok();
        let Some(sensor) = text("name") else {
            continue;
        };
        for input in numbered(&dir, "temp", "_input") {
            let file = input.file_name().unwrap_or_default().to_string_lossy();
            let prefix = file.trim_end_matches("_input");
            let Some(millicelsius) = millis(&file) else {
                continue;
            };
            let label = text(&format!("{prefix}_label"));
            if sensor == "coretemp" && label.as_deref().is_some_and(|l| l.starts_with("Core ")) {
                continue;
            }
            out.push(Temperature {
                sensor: sensor.clone(),
                label,
                millicelsius,
                max_millicelsius: millis(&format!("{prefix}_max")),
                crit_millicelsius: millis(&format!("{prefix}_crit")),
            });
        }
    }
    out
}

/// The entries of `dir` named `<prefix>N<suffix>`, by N.
fn numbered(dir: &Path, prefix: &str, suffix: &str) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(u32, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let n = name
                .to_str()?
                .strip_prefix(prefix)?
                .strip_suffix(suffix)?
                .parse()
                .ok()?;
            Some((n, entry.path()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, path)| path).collect()
}

/// The reading that stands for the CPU: coretemp's package (Intel), else
/// k10temp's control temperature (AMD), else the SoC's thermal zone (the
/// Pi), else Intel's package thermal zone, else ACPI's, which on a PC is
/// usually near the CPU.
pub fn cpu_millicelsius(temperatures: &[Temperature]) -> Option<i32> {
    let find = |sensor: &str, label: Option<&str>| {
        temperatures
            .iter()
            .find(|t| t.sensor == sensor && label.is_none_or(|l| t.label.as_deref() == Some(l)))
            .map(|t| t.millicelsius)
    };
    find("coretemp", Some("Package id 0"))
        .or_else(|| find("coretemp", None))
        .or_else(|| find("k10temp", Some("Tctl")))
        .or_else(|| find("k10temp", Some("Tdie")))
        .or_else(|| find("k10temp", None))
        .or_else(|| find("cpu_thermal", None))
        .or_else(|| find("x86_pkg_temp", None))
        .or_else(|| find("acpitz", None))
}

/// The aggregate `cpu` line of `/proc/stat`, in clock ticks since boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    busy: u64,
    total: u64,
}

pub fn cpu_times(stat: &Path) -> Option<CpuTimes> {
    let text = fs::read_to_string(stat).ok()?;
    let fields: Vec<u64> = text
        .lines()
        .find_map(|line| line.strip_prefix("cpu "))?
        .split_whitespace()
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    // user nice system idle iowait irq softirq steal guest guest_nice; the
    // guest times are already inside user and nice.
    let total: u64 = fields.iter().take(8).sum();
    let idle = fields.get(3).copied()? + fields.get(4).copied().unwrap_or(0);
    Some(CpuTimes {
        busy: total.saturating_sub(idle),
        total,
    })
}

/// Percent of the CPU busy between two samples, every core counted: 100 is
/// all of them.
pub fn cpu_percent(before: CpuTimes, after: CpuTimes) -> Option<u8> {
    let total = after.total.checked_sub(before.total)?;
    let busy = after.busy.checked_sub(before.busy)?;
    (total > 0).then(|| (busy.min(total) * 100 / total) as u8)
}

/// What firmware vendors put in DMI fields they did not fill in.
const PLACEHOLDERS: &[&str] = &[
    "to be filled by o.e.m.",
    "default string",
    "system manufacturer",
    "system product name",
    "system version",
    "system serial number",
    "base board manufacturer",
    "base board product name",
    "base board serial number",
    "not specified",
    "not applicable",
    "none",
    "unknown",
    "n/a",
    "o.e.m.",
    "oem",
    "0123456789",
    "123456789",
];

/// A DMI value, or `None` for a missing file, an empty value or a
/// placeholder.
fn meaningful(value: &str) -> Option<String> {
    let value = value.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    let lower = value.to_lowercase();
    if value.is_empty()
        || PLACEHOLDERS.contains(&lower.as_str())
        || value.chars().all(|c| c == '0' || c == ' ')
    {
        None
    } else {
        Some(value.to_string())
    }
}

fn dmi(dir: &Path) -> Option<Hardware> {
    if !dir.is_dir() {
        return None;
    }
    let field = |name: &str| {
        fs::read_to_string(dir.join(name))
            .ok()
            .and_then(|value| meaningful(&value))
    };
    let vendor = field("sys_vendor");
    let model = match (field("product_name"), field("product_version")) {
        (Some(name), Some(version)) if name != version => Some(format!("{name} ({version})")),
        (name, version) => name.or(version),
    };
    // The board vendor is usually the system's; it is said once.
    let board = match (field("board_vendor"), field("board_name")) {
        (Some(board_vendor), Some(name)) if Some(&board_vendor) != vendor.as_ref() => {
            Some(format!("{board_vendor} {name}"))
        }
        (_, Some(name)) => Some(name),
        (board_vendor, None) => board_vendor.filter(|v| Some(v) != vendor.as_ref()),
    };
    let firmware = join(field("bios_version"), field("bios_date"));
    let serial = field("product_serial").or_else(|| field("board_serial"));
    Some(Hardware {
        vendor,
        model,
        board,
        firmware,
        serial,
        ..Hardware::default()
    })
}

fn device_tree(dir: &Path) -> Option<Hardware> {
    let string = |name: &str| {
        fs::read(dir.join(name))
            .ok()
            .and_then(|bytes| meaningful(&String::from_utf8_lossy(&bytes)))
    };
    let model = string("model");
    // NUL-separated, most specific first: `raspberrypi,3-model-b-plus`,
    // ..., `brcm,bcm2837`.
    let compatible: Vec<String> = fs::read(dir.join("compatible"))
        .map(|bytes| {
            bytes
                .split(|b| *b == 0)
                .filter(|entry| !entry.is_empty())
                .map(|entry| String::from_utf8_lossy(entry).into_owned())
                .collect()
        })
        .unwrap_or_default();
    if model.is_none() && compatible.is_empty() {
        return None;
    }
    let vendor = compatible
        .first()
        .and_then(|entry| entry.split_once(','))
        .map(|(vendor, _)| match vendor {
            "raspberrypi" => "Raspberry Pi".to_string(),
            other => other.to_string(),
        });
    let soc = compatible
        .last()
        .and_then(|entry| entry.split_once(','))
        .map(|(_, chip)| chip.to_uppercase());
    let board = fs::read(dir.join("system/linux,revision"))
        .ok()
        .and_then(|bytes| <[u8; 4]>::try_from(bytes.as_slice()).ok())
        .and_then(|bytes| pi_manufacturer(u32::from_be_bytes(bytes)))
        .map(str::to_string);
    Some(Hardware {
        vendor,
        model,
        board,
        serial: string("serial-number"),
        cpu: soc,
        ..Hardware::default()
    })
}

/// Who built a Pi, from its new-style revision code: bit 23 marks the
/// style, bits 16-19 are the manufacturer (the Raspberry Pi documentation's
/// "Raspberry Pi revision codes").
fn pi_manufacturer(revision: u32) -> Option<&'static str> {
    if revision & (1 << 23) == 0 {
        return None;
    }
    match (revision >> 16) & 0xf {
        0 => Some("Sony UK"),
        1 => Some("Egoman"),
        2 | 4 => Some("Embest"),
        3 => Some("Sony Japan"),
        5 => Some("Stadium"),
        _ => None,
    }
}

/// The CPU's name (`model name`, which arm64 does not have) and how many
/// cores the kernel runs.
fn cpuinfo(path: &Path) -> (Option<String>, Option<u32>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (None, None);
    };
    let value = |line: &str, name: &str| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == name).then(|| value.trim().to_string())
    };
    let name = text
        .lines()
        .find_map(|line| value(line, "model name"))
        .and_then(|name| meaningful(&name));
    let cores = text
        .lines()
        .filter(|line| value(line, "processor").is_some())
        .count() as u32;
    (name, (cores > 0).then_some(cores))
}

fn join(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn paths(dir: &Path) -> Paths {
        let at = |name: &str| dir.join(name).display().to_string();
        let env: HashMap<String, String> = [
            ("KIOSK_DMI", at("dmi")),
            ("KIOSK_DEVICE_TREE", at("device-tree")),
            ("KIOSK_CPUINFO", at("cpuinfo")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        Paths::load(&env)
    }

    fn write(dir: &Path, name: &str, contents: impl AsRef<[u8]>) {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn dmi_names_the_machine_and_drops_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "dmi/sys_vendor", "LENOVO\n");
        write(root, "dmi/product_name", "10T7002VMC\n");
        write(root, "dmi/product_version", "ThinkCentre M720q\n");
        write(root, "dmi/board_vendor", "LENOVO\n");
        write(root, "dmi/board_name", "3136\n");
        write(root, "dmi/bios_version", "M1UKT4BA\n");
        write(root, "dmi/bios_date", "05/10/2023\n");
        write(root, "dmi/product_serial", "To Be Filled By O.E.M.\n");
        write(root, "dmi/board_serial", "L1HF0AB123\n");
        write(
            root,
            "cpuinfo",
            "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i5-8500T CPU @ 2.10GHz\n\n\
             processor\t: 1\nmodel name\t: Intel(R) Core(TM) i5-8500T CPU @ 2.10GHz\n",
        );
        // A device tree next to DMI loses.
        write(root, "device-tree/model", "Not this\0");

        let hw = hardware(&paths(root));
        assert_eq!(hw.vendor.as_deref(), Some("LENOVO"));
        assert_eq!(hw.model.as_deref(), Some("10T7002VMC (ThinkCentre M720q)"));
        assert_eq!(hw.board.as_deref(), Some("3136"));
        assert_eq!(hw.firmware.as_deref(), Some("M1UKT4BA 05/10/2023"));
        assert_eq!(hw.serial.as_deref(), Some("L1HF0AB123"));
        assert_eq!(
            hw.cpu.as_deref(),
            Some("Intel(R) Core(TM) i5-8500T CPU @ 2.10GHz")
        );
        assert_eq!(hw.cores, Some(2));
        assert_eq!(hw.arch, std::env::consts::ARCH);
    }

    #[test]
    fn a_board_from_another_vendor_is_named_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "dmi/sys_vendor", "System manufacturer\n");
        write(root, "dmi/product_name", "Default string\n");
        write(root, "dmi/board_vendor", "ASUSTeK COMPUTER INC.\n");
        write(root, "dmi/board_name", "PRIME H310I-PLUS\n");
        write(root, "dmi/product_serial", "0000000000\n");
        // What a qemu VM reports as its BIOS version.
        write(root, "dmi/bios_version", "unknown\n");
        write(root, "dmi/bios_date", "2/2/2022\n");

        let hw = hardware(&paths(root));
        assert_eq!(hw.vendor, None);
        assert_eq!(hw.model, None);
        assert_eq!(
            hw.board.as_deref(),
            Some("ASUSTeK COMPUTER INC. PRIME H310I-PLUS")
        );
        assert_eq!(hw.serial, None);
        assert_eq!(hw.firmware.as_deref(), Some("2/2/2022"));
    }

    #[test]
    fn the_pi_is_read_from_its_device_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "device-tree/model",
            "Raspberry Pi 3 Model B Plus Rev 1.3\0",
        );
        write(
            root,
            "device-tree/compatible",
            "raspberrypi,3-model-b-plus\0brcm,bcm2837\0",
        );
        write(root, "device-tree/serial-number", "00000000a1b2c3d4\0");
        // 0xa020d3: new style, manufacturer 0, Sony UK.
        write(
            root,
            "device-tree/system/linux,revision",
            0x00a0_20d3u32.to_be_bytes(),
        );
        write(
            root,
            "cpuinfo",
            "processor\t: 0\nBogoMIPS\t: 38.40\n\nprocessor\t: 1\n\nprocessor\t: 2\n\nprocessor\t: 3\n",
        );

        let hw = hardware(&paths(root));
        assert_eq!(hw.vendor.as_deref(), Some("Raspberry Pi"));
        assert_eq!(
            hw.model.as_deref(),
            Some("Raspberry Pi 3 Model B Plus Rev 1.3")
        );
        assert_eq!(hw.board.as_deref(), Some("Sony UK"));
        assert_eq!(hw.serial.as_deref(), Some("00000000a1b2c3d4"));
        assert_eq!(hw.cpu.as_deref(), Some("BCM2837"));
        assert_eq!(hw.firmware, None);
        assert_eq!(hw.cores, Some(4));
    }

    #[test]
    fn an_old_style_revision_names_no_manufacturer() {
        assert_eq!(pi_manufacturer(0x000e), None);
        assert_eq!(pi_manufacturer(0x00a2_2082), Some("Embest"));
        assert_eq!(pi_manufacturer(0x00a0_2082), Some("Sony UK"));
    }

    #[test]
    fn nothing_readable_leaves_only_the_architecture() {
        let dir = tempfile::tempdir().unwrap();
        let hw = hardware(&paths(dir.path()));
        assert_eq!(
            hw,
            Hardware {
                arch: std::env::consts::ARCH.to_string(),
                ..Hardware::default()
            }
        );
    }

    #[test]
    fn cpu_use_is_the_busy_share_of_the_ticks_between_samples() {
        let dir = tempfile::tempdir().unwrap();
        let stat = dir.path().join("stat");
        fs::write(
            &stat,
            "cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 50 0 50 350 50 0 0 0 0 0\n",
        )
        .unwrap();
        let before = cpu_times(&stat).unwrap();
        // 100 more ticks: 30 user, 10 system, 50 idle, 10 iowait.
        fs::write(&stat, "cpu  130 0 110 750 110 0 0 0 0 0\n").unwrap();
        let after = cpu_times(&stat).unwrap();
        assert_eq!(cpu_percent(before, after), Some(40));
        // No time between them, or a counter that went back: no answer.
        assert_eq!(cpu_percent(after, after), None);
        assert_eq!(cpu_percent(after, before), None);
        assert_eq!(cpu_times(&dir.path().join("missing")), None);
    }

    #[test]
    fn temperatures_read_every_sensor_but_the_cores() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // An Intel PC: ACPI's zone, coretemp with two cores, an NVMe drive.
        write(root, "hwmon/hwmon0/name", "acpitz\n");
        write(root, "hwmon/hwmon0/temp1_input", "27800\n");
        write(root, "hwmon/hwmon0/temp1_crit", "119000\n");
        write(root, "hwmon/hwmon2/name", "coretemp\n");
        write(root, "hwmon/hwmon2/temp1_label", "Package id 0\n");
        write(root, "hwmon/hwmon2/temp1_input", "52000\n");
        write(root, "hwmon/hwmon2/temp1_max", "84000\n");
        write(root, "hwmon/hwmon2/temp1_crit", "100000\n");
        write(root, "hwmon/hwmon2/temp2_label", "Core 0\n");
        write(root, "hwmon/hwmon2/temp2_input", "50000\n");
        write(root, "hwmon/hwmon2/temp10_label", "Core 8\n");
        write(root, "hwmon/hwmon2/temp10_input", "51000\n");
        write(root, "hwmon/hwmon10/name", "nvme\n");
        write(root, "hwmon/hwmon10/temp1_label", "Composite\n");
        write(root, "hwmon/hwmon10/temp1_input", "41850\n");
        write(root, "hwmon/hwmon10/temp1_max", "81850\n");
        // A disk that does not answer, and a device with no name.
        write(root, "hwmon/hwmon3/name", "drivetemp\n");
        write(root, "hwmon/hwmon3/temp1_input", "");
        write(root, "hwmon/hwmon4/temp1_input", "30000\n");

        let found = temperatures(&root.join("hwmon"));
        let short: Vec<(&str, Option<&str>, i32)> = found
            .iter()
            .map(|t| (t.sensor.as_str(), t.label.as_deref(), t.millicelsius))
            .collect();
        // hwmon10 after hwmon2: by number, not by name.
        assert_eq!(
            short,
            [
                ("acpitz", None, 27800),
                ("coretemp", Some("Package id 0"), 52000),
                ("nvme", Some("Composite"), 41850),
            ]
        );
        assert_eq!(found[0].crit_millicelsius, Some(119_000));
        assert_eq!(found[0].max_millicelsius, None);
        assert_eq!(found[1].max_millicelsius, Some(84_000));
        assert_eq!(cpu_millicelsius(&found), Some(52000));
    }

    #[test]
    fn the_cpu_reading_falls_back_by_platform() {
        let reading = |sensor: &str, label: Option<&str>, millicelsius| Temperature {
            sensor: sensor.to_string(),
            label: label.map(str::to_string),
            millicelsius,
            max_millicelsius: None,
            crit_millicelsius: None,
        };
        // AMD: Tctl, not the CCD.
        let amd = [
            reading("k10temp", Some("Tccd1"), 40000),
            reading("k10temp", Some("Tctl"), 45000),
        ];
        assert_eq!(cpu_millicelsius(&amd), Some(45000));
        // The Pi: the SoC's zone.
        let pi = [reading("cpu_thermal", None, 48000)];
        assert_eq!(cpu_millicelsius(&pi), Some(48000));
        // A PC without coretemp: the package zone over ACPI's.
        let pc = [
            reading("acpitz", None, 30000),
            reading("x86_pkg_temp", None, 55000),
        ];
        assert_eq!(cpu_millicelsius(&pc), Some(55000));
        // A disk alone says nothing about the CPU, and a VM has nothing.
        assert_eq!(cpu_millicelsius(&[reading("nvme", None, 40000)]), None);
        assert_eq!(cpu_millicelsius(&[]), None);
    }

    #[test]
    fn no_hwmon_is_no_temperatures() {
        let dir = tempfile::tempdir().unwrap();
        assert!(temperatures(&dir.path().join("hwmon")).is_empty());
    }

    #[test]
    fn memory_needs_total_and_available() {
        let dir = tempfile::tempdir().unwrap();
        let meminfo = dir.path().join("meminfo");
        fs::write(
            &meminfo,
            "MemTotal:        4000000 kB\nMemFree:          100000 kB\nMemAvailable:    1000000 kB\n",
        )
        .unwrap();
        let memory = memory(&meminfo).unwrap();
        assert_eq!(memory.total, 4_000_000 * 1024);
        assert_eq!(memory.available, 1_000_000 * 1024);
        assert_eq!(memory.used_percent(), 75);

        fs::write(&meminfo, "MemTotal:        4000000 kB\n").unwrap();
        assert_eq!(super::memory(&meminfo), None);
        assert_eq!(super::memory(&dir.path().join("missing")), None);
    }
}
