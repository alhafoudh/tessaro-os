//! The USB devices that may be scanners, from sysfs: keyboards, serial
//! ports and HID POS devices, each with the USB device it belongs to. Reads
//! files: call it from `blocking`.

use std::path::{Path, PathBuf};

use protocol::scanner::{device_id, Transport};

/// Where to look: `/sys/class/{input,tty,hidraw}` and `/dev`.
#[derive(Debug, Clone)]
pub struct Sysfs {
    pub input: PathBuf,
    pub tty: PathBuf,
    pub hidraw: PathBuf,
    pub dev: PathBuf,
}

impl Sysfs {
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            input: paths.input.clone(),
            tty: paths.sys_tty.clone(),
            hidraw: paths.sys_hidraw.clone(),
            dev: paths.dev.clone(),
        }
    }
}

/// A device node that may be a scanner, and its USB device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub transport: Transport,
    pub node: PathBuf,
    pub vendor: String,
    pub product: String,
    pub serial: Option<String>,
    /// The USB device's own name in sysfs: its port, `1-1.2`.
    pub port: String,
    pub description: String,
    /// A HID POS device's report descriptor.
    pub descriptor: Vec<u8>,
}

impl Found {
    pub fn id(&self) -> String {
        device_id(
            self.transport,
            &self.vendor,
            &self.product,
            self.serial.as_deref(),
            Some(&self.port),
        )
    }

    pub fn candidate(&self, known: Option<String>) -> protocol::scanner::ScannerCandidate {
        protocol::scanner::ScannerCandidate {
            device: self.id(),
            transport: self.transport,
            vendor: self.vendor.clone(),
            product: self.product.clone(),
            serial: self.serial.clone(),
            port: self.port.clone(),
            node: self.node.display().to_string(),
            description: self.description.clone(),
            known,
            scan: None,
        }
    }
}

/// Every candidate, keyboards first, each kind in its node's order.
pub fn find(sysfs: &Sysfs) -> Vec<Found> {
    let mut found = Vec::new();
    for name in entries(&sysfs.input, "event") {
        let class = sysfs.input.join(&name);
        if !is_keyboard(&class.join("device")) {
            continue;
        }
        if let Some(usb) = usb_device(&class) {
            found.push(usb.found(Transport::Keyboard, sysfs.dev.join("input").join(&name)));
        }
    }
    for prefix in ["ttyACM", "ttyUSB"] {
        for name in entries(&sysfs.tty, prefix) {
            if let Some(usb) = usb_device(&sysfs.tty.join(&name)) {
                found.push(usb.found(Transport::Serial, sysfs.dev.join(&name)));
            }
        }
    }
    for name in entries(&sysfs.hidraw, "hidraw") {
        let class = sysfs.hidraw.join(&name);
        let descriptor = std::fs::read(class.join("device/report_descriptor")).unwrap_or_default();
        if !super::hidpos::has_page(&descriptor) {
            continue;
        }
        if let Some(usb) = usb_device(&class) {
            let mut one = usb.found(Transport::Hidpos, sysfs.dev.join(&name));
            one.descriptor = descriptor;
            found.push(one);
        }
    }
    found
}

/// Every USB serial port's class device, `/sys/class/tty/ttyACM0`, scanner
/// or not: what `scanner::apply_rules` replays.
pub fn usb_ttys(sysfs: &Sysfs) -> Vec<PathBuf> {
    ["ttyACM", "ttyUSB"]
        .into_iter()
        .flat_map(|prefix| entries(&sysfs.tty, prefix))
        .map(|name| sysfs.tty.join(name))
        .collect()
}

/// The names in `dir` that are `prefix` and a number, in numeric order.
fn entries(dir: &Path, prefix: &str) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<(u32, String)> = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let number = name.strip_prefix(prefix)?.parse().ok()?;
            Some((number, name))
        })
        .collect();
    names.sort();
    names.into_iter().map(|(_, name)| name).collect()
}

/// An input device that types: Enter, and the letters from A to Z, as
/// `capabilities/key` has them. A mouse, a power button or a remote's keys
/// are none.
fn is_keyboard(input: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(input.join("capabilities/key")) else {
        return false;
    };
    // Words of the bitmap, the highest first, as the kernel prints them:
    // 64-bit on every target the image builds for.
    let words: Vec<u64> = text
        .split_whitespace()
        .rev()
        .map(|word| u64::from_str_radix(word, 16).unwrap_or(0))
        .collect();
    let has = |code: usize| {
        words
            .get(code / 64)
            .is_some_and(|word| word & (1u64 << (code % 64)) != 0)
    };
    // KEY_ENTER, KEY_A, KEY_Z, KEY_1.
    [28, 30, 44, 2].into_iter().all(has)
}

/// The USB device a class device belongs to.
struct Usb {
    vendor: String,
    product: String,
    serial: Option<String>,
    port: String,
    description: String,
}

impl Usb {
    fn found(&self, transport: Transport, node: PathBuf) -> Found {
        Found {
            transport,
            node,
            vendor: self.vendor.clone(),
            product: self.product.clone(),
            serial: self.serial.clone(),
            port: self.port.clone(),
            description: self.description.clone(),
            descriptor: Vec::new(),
        }
    }
}

/// Up from the class device to the first parent that is a USB device: the
/// one with `idVendor`.
fn usb_device(class: &Path) -> Option<Usb> {
    let mut dir = std::fs::canonicalize(class.join("device")).ok()?;
    for _ in 0..12 {
        if dir.join("idVendor").exists() {
            let read = |name: &str| {
                std::fs::read_to_string(dir.join(name))
                    .ok()
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty())
            };
            let vendor = read("idVendor")?.to_ascii_lowercase();
            let product = read("idProduct")?.to_ascii_lowercase();
            let description = [read("manufacturer"), read("product")]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            return Some(Usb {
                description: if description.is_empty() {
                    format!("USB device {vendor}:{product}")
                } else {
                    description
                },
                vendor,
                product,
                serial: read("serial").filter(|serial| serial.len() <= 64),
                port: dir.file_name()?.to_str()?.to_string(),
            });
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A sysfs with one USB device at `port` and its class devices.
    pub(crate) struct Fake {
        pub dir: tempfile::TempDir,
    }

    impl Fake {
        pub fn new() -> Self {
            let fake = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            for class in ["class/input", "class/tty", "class/hidraw", "dev/input"] {
                std::fs::create_dir_all(fake.dir.path().join(class)).unwrap();
            }
            fake
        }

        pub fn sysfs(&self) -> Sysfs {
            let root = self.dir.path();
            Sysfs {
                input: root.join("class/input"),
                tty: root.join("class/tty"),
                hidraw: root.join("class/hidraw"),
                dev: root.join("dev"),
            }
        }

        /// A USB device with an interface, its files written.
        pub fn usb(
            &self,
            port: &str,
            vendor: &str,
            product: &str,
            serial: Option<&str>,
        ) -> PathBuf {
            let usb = self.dir.path().join("devices/usb1").join(port);
            std::fs::create_dir_all(&usb).unwrap();
            std::fs::write(usb.join("idVendor"), format!("{vendor}\n")).unwrap();
            std::fs::write(usb.join("idProduct"), format!("{product}\n")).unwrap();
            std::fs::write(usb.join("manufacturer"), "Honeywell\n").unwrap();
            std::fs::write(usb.join("product"), "Xenon 1950g\n").unwrap();
            if let Some(serial) = serial {
                std::fs::write(usb.join("serial"), format!("{serial}\n")).unwrap();
            }
            let interface = usb.join(format!("{port}:1.0"));
            std::fs::create_dir_all(&interface).unwrap();
            interface
        }

        pub fn keyboard(&self, interface: &Path, event: &str, keys: &str) {
            let input = interface.join("0003:0C2E:0B61.0001/input/input5");
            std::fs::create_dir_all(input.join("capabilities")).unwrap();
            std::fs::write(input.join("capabilities/key"), keys).unwrap();
            let class = self.dir.path().join("class/input").join(event);
            std::fs::create_dir_all(&class).unwrap();
            symlink(&input, class.join("device")).unwrap();
        }

        pub fn tty(&self, interface: &Path, name: &str) {
            let class = self.dir.path().join("class/tty").join(name);
            std::fs::create_dir_all(&class).unwrap();
            symlink(interface, class.join("device")).unwrap();
        }

        pub fn hidraw(&self, interface: &Path, name: &str, descriptor: &[u8]) {
            let hid = interface.join("0003:0C2E:0B62.0002");
            std::fs::create_dir_all(&hid).unwrap();
            std::fs::write(hid.join("report_descriptor"), descriptor).unwrap();
            let class = self.dir.path().join("class/hidraw").join(name);
            std::fs::create_dir_all(&class).unwrap();
            symlink(&hid, class.join("device")).unwrap();
        }
    }

    /// `capabilities/key` of a full keyboard, and of a power button.
    pub(crate) const KEYBOARD_KEYS: &str =
        "1000000000007 ff9f207ac14057ff febeffdfffefffff fffffffffffffffe";
    const POWER_KEYS: &str = "10000000000000 0";

    #[test]
    fn keyboards_serial_ports_and_hid_pos_devices_are_found() {
        let fake = Fake::new();
        let one = fake.usb("1-1.2", "0C2E", "0B61", Some("S1"));
        fake.keyboard(&one, "event3", KEYBOARD_KEYS);
        let button = fake.usb("1-1.4", "0001", "0002", None);
        fake.keyboard(&button, "event4", POWER_KEYS);
        let serial = fake.usb("1-1.3", "1a86", "7523", None);
        fake.tty(&serial, "ttyUSB0");
        // A tty with no USB device: the console.
        std::fs::create_dir_all(fake.dir.path().join("class/tty/ttyS0")).unwrap();
        let pos = fake.usb("2-1", "0c2e", "0b62", None);
        fake.hidraw(&pos, "hidraw1", super::super::hidpos::tests::DESCRIPTOR);
        fake.hidraw(&one, "hidraw0", &[0x05, 0x01, 0x09, 0x06]);

        let found = find(&fake.sysfs());
        let ids: Vec<String> = found.iter().map(Found::id).collect();
        assert_eq!(
            ids,
            vec![
                "keyboard:0c2e:0b61:S1",
                "serial:1a86:7523:@1-1.3",
                "hidpos:0c2e:0b62:@2-1"
            ]
        );
        assert_eq!(found[0].description, "Honeywell Xenon 1950g");
        assert!(found[0].node.ends_with("dev/input/event3"));
        assert!(found[1].node.ends_with("dev/ttyUSB0"));
        assert_eq!(found[2].port, "2-1");
        assert!(!found[2].descriptor.is_empty());
        // Every USB serial port, never the console.
        let ttys = usb_ttys(&fake.sysfs());
        assert_eq!(ttys.len(), 1);
        assert!(ttys[0].ends_with("class/tty/ttyUSB0"));
        let candidate = found[0].candidate(Some("front".into()));
        assert_eq!(candidate.known.as_deref(), Some("front"));
        assert_eq!(candidate.port, "1-1.2");
    }

    #[test]
    fn nothing_is_found_where_there_is_no_sysfs() {
        let sysfs = Sysfs {
            input: "/nonexistent/input".into(),
            tty: "/nonexistent/tty".into(),
            hidraw: "/nonexistent/hidraw".into(),
            dev: "/nonexistent/dev".into(),
        };
        assert!(find(&sysfs).is_empty());
    }
}
