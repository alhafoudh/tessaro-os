//! Barcode scanners: the `scanners` table of `tessaro.db`, the devices that
//! may be one, and turning what a scanner sends into scans
//! (docs/scanners.md). The running side is `control/scanners.rs`.

pub mod devices;
pub mod frame;
pub mod hidpos;
pub mod io;
pub mod keys;
pub mod log;

use protocol::scanner::{ScannerSpec, Transport};
use tessaro_db::rusqlite::{self, params, types::Type, Connection};

use crate::db::Stored;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scanners {
    pub scanners: Vec<ScannerSpec>,
}

impl Stored for Scanners {
    const WHAT: &'static str = "the scanners";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows = db.prepare(
            "SELECT name, transport, vendor, product, serial, port, layout, terminator, gap_ms, \
             baud, strip_prefix, strip_suffix, enabled FROM scanners ORDER BY position",
        )?;
        let scanners = rows
            .query_map([], |row| {
                let transport: String = row.get(1)?;
                Ok(ScannerSpec {
                    name: row.get(0)?,
                    transport: transport.parse().map_err(|err: String| {
                        rusqlite::Error::FromSqlConversionFailure(1, Type::Text, err.into())
                    })?,
                    vendor: row.get(2)?,
                    product: row.get(3)?,
                    serial: row.get(4)?,
                    port: row.get(5)?,
                    layout: row.get(6)?,
                    terminator: row.get(7)?,
                    gap_ms: row.get(8)?,
                    baud: row.get(9)?,
                    strip_prefix: row.get(10)?,
                    strip_suffix: row.get(11)?,
                    enabled: row.get(12)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { scanners })
    }

    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM scanners", [])?;
        let mut insert = db.prepare(
            "INSERT INTO scanners (name, position, transport, vendor, product, serial, port, \
             layout, terminator, gap_ms, baud, strip_prefix, strip_suffix, enabled) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )?;
        for (position, scanner) in self.scanners.iter().enumerate() {
            insert.execute(params![
                scanner.name,
                position as i64,
                scanner.transport.name(),
                scanner.vendor,
                scanner.product,
                scanner.serial,
                scanner.port,
                scanner.layout,
                scanner.terminator,
                scanner.gap_ms,
                scanner.baud,
                scanner.strip_prefix,
                scanner.strip_suffix,
                scanner.enabled,
            ])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM scanners", []).map(drop)
    }
}

impl Scanners {
    pub fn find(&self, name: &str) -> Result<&ScannerSpec, String> {
        self.scanners
            .iter()
            .find(|scanner| scanner.name == name)
            .ok_or_else(|| missing(name))
    }

    /// The scanner a device is, if it is one.
    pub fn of(&self, found: &devices::Found) -> Option<&ScannerSpec> {
        self.scanners.iter().find(|scanner| {
            scanner.transport == found.transport
                && scanner.matches(
                    &found.vendor,
                    &found.product,
                    found.serial.as_deref(),
                    &found.port,
                )
        })
    }
}

pub fn missing(name: &str) -> String {
    format!("no scanner {name:?}; `tessaro-ctl scanner list` shows them")
}

/// The udev rule for the scanners read, or `None` for none. Every rule
/// names the USB device by the same parent: its ids, and its serial number
/// or its port.
///
/// * A keyboard's input devices get `LIBINPUT_IGNORE_DEVICE`, so a Weston
///   that opens them afresh never does, and `TESSARO_SCANNER`, which keeps
///   it from counting as a keyboard for screen.osk=auto
///   (`tessaro-weston-config`).
/// * Its serial ports and hidraw nodes, and a keyboard's hidraw nodes, are
///   root's alone: the browser's WebSerial and WebHID grants open them
///   otherwise. `:=` makes that final over the image's device rules.
pub fn rules(scanners: &[&ScannerSpec]) -> Option<String> {
    let mut lines = Vec::new();
    for scanner in scanners {
        let mut device = format!(
            "ATTRS{{idVendor}}==\"{}\", ATTRS{{idProduct}}==\"{}\"",
            scanner.vendor, scanner.product
        );
        match (&scanner.serial, &scanner.port) {
            (Some(serial), _) => {
                device.push_str(&format!(", ATTRS{{serial}}==\"{}\"", escape(serial)))
            }
            (None, Some(port)) => device.push_str(&format!(", KERNELS==\"{port}\"")),
            (None, None) => {}
        }
        lines.push(format!("# {}", scanner.name));
        if scanner.transport == Transport::Keyboard {
            lines.push(format!(
                "SUBSYSTEM==\"input\", {device}, ENV{{LIBINPUT_IGNORE_DEVICE}}=\"1\", \
                 ENV{{TESSARO_SCANNER}}=\"{}\"",
                scanner.name
            ));
        }
        if scanner.transport == Transport::Serial {
            lines.push(format!(
                "SUBSYSTEM==\"tty\", {device}, OWNER:=\"root\", GROUP:=\"root\", MODE:=\"0600\""
            ));
        }
        lines.push(format!(
            "SUBSYSTEM==\"hidraw\", {device}, OWNER:=\"root\", GROUP:=\"root\", MODE:=\"0600\""
        ));
    }
    (!lines.is_empty()).then(|| {
        format!(
            "# Rendered by tessaro-agent from the scanners table while scanner.enable is on:\n\
             # the barcode scanners, kept from libinput and the browser.\n{}\n",
            lines.join("\n")
        )
    })
}

/// The rule written, or removed for none, and replayed through the devices
/// already present when it changed: a scanner plugged in before it was set
/// up gets its owner, its mode and its properties now, and one that is no
/// scanner any more gets the image's back. Whether it changed. Blocking:
/// call it from `blocking`.
///
/// The USB serial ports, `ttys`, are replayed with `add`, not `change`: the
/// image gives them only a GROUP, no MODE (`50-udev-default.rules`), and udev
/// sets neither again on a `change`, so a port that was root's would stay
/// root's. Input devices keep `change`, which libinput ignores, where a
/// second `add` would open them twice.
pub fn apply_rules(
    path: &std::path::Path,
    body: Option<&str>,
    udevadm: &std::path::Path,
    ttys: &[std::path::PathBuf],
) -> Result<bool, String> {
    let changed = match body {
        Some(body) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
            }
            crate::store::replace_if_changed(path, body.as_bytes(), 0o644)
                .map_err(|err| format!("{}: {err}", path.display()))?
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(format!("{}: {err}", path.display())),
        },
    };
    if !changed || udevadm.as_os_str().is_empty() {
        return Ok(changed);
    }
    let mut steps: Vec<Vec<String>> = vec![
        vec!["control".into(), "--reload".into()],
        vec![
            "trigger".into(),
            "--action=change".into(),
            "--subsystem-match=input".into(),
            "--subsystem-match=hidraw".into(),
        ],
    ];
    if !ttys.is_empty() {
        let mut add = vec!["trigger".to_string(), "--action=add".to_string()];
        add.extend(ttys.iter().map(|tty| tty.display().to_string()));
        steps.push(add);
    }
    steps.push(vec!["settle".into(), "--timeout=5".into()]);
    for args in steps {
        let output = crate::proc::run(
            std::process::Command::new(udevadm)
                .args(&args)
                // udevadm would tell systemd about itself on the agent's
                // notify socket, which only the agent may use.
                .env_remove("NOTIFY_SOCKET"),
            None,
        )?;
        if !output.status.success() {
            return Err(format!(
                "udevadm {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Ok(changed)
}

/// A serial number inside a rule's double quotes: udev's patterns take
/// `*`, `?` and `[` as globs, and a quote ends the value.
fn escape(serial: &str) -> String {
    serial
        .chars()
        .map(|ch| match ch {
            '"' | '\\' | '*' | '?' | '[' | ']' | '|' => '?',
            ch if ch.is_control() => '?',
            ch => ch,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, transport: Transport, serial: Option<&str>) -> ScannerSpec {
        ScannerSpec {
            name: name.into(),
            transport,
            vendor: "0c2e".into(),
            product: "0b61".into(),
            serial: serial.map(str::to_string),
            port: Some("1-1.2".into()),
            layout: None,
            terminator: None,
            gap_ms: None,
            baud: None,
            strip_prefix: None,
            strip_suffix: None,
            enabled: true,
        }
    }

    #[test]
    fn the_store_keeps_every_field_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let log = crate::log::Log::buffered(true);
        let db = crate::db::Db::open(dir.path(), &log);
        let mut serial = spec("back", Transport::Serial, None);
        serial.terminator = Some("crlf".into());
        serial.baud = Some(115_200);
        serial.strip_prefix = Some("]C1".into());
        serial.enabled = false;
        let mut keyboard = spec("front", Transport::Keyboard, Some("S1"));
        keyboard.layout = Some("sk(qwerty)".into());
        keyboard.gap_ms = Some(40);
        let all = Scanners {
            scanners: vec![keyboard, serial],
        };
        db.update(|stored: &mut Scanners| {
            *stored = all.clone();
            Ok(())
        })
        .unwrap();
        assert_eq!(db.read::<Scanners>(&log), all);
        assert!(log.lines().is_empty(), "{:?}", log.lines());
        assert_eq!(all.find("back").unwrap().baud, Some(115_200));
        assert!(all.find("nope").unwrap_err().contains("scanner list"));
    }

    #[test]
    fn the_rule_names_each_device_and_keeps_it_from_the_browser() {
        let keyboard = spec("front", Transport::Keyboard, Some("S\"1*"));
        let serial = spec("back", Transport::Serial, None);
        let rules = rules(&[&keyboard, &serial]).unwrap();
        assert!(rules.contains(
            "SUBSYSTEM==\"input\", ATTRS{idVendor}==\"0c2e\", ATTRS{idProduct}==\"0b61\", \
             ATTRS{serial}==\"S?1?\", ENV{LIBINPUT_IGNORE_DEVICE}=\"1\", ENV{TESSARO_SCANNER}=\"front\""
        ));
        assert!(rules.contains(
            "SUBSYSTEM==\"tty\", ATTRS{idVendor}==\"0c2e\", ATTRS{idProduct}==\"0b61\", \
             KERNELS==\"1-1.2\", OWNER:=\"root\""
        ));
        assert_eq!(rules.matches("SUBSYSTEM==\"hidraw\"").count(), 2);
        assert_eq!(super::rules(&[]), None);
    }

    #[test]
    fn a_device_is_the_scanner_it_matches() {
        let fake = devices::tests::Fake::new();
        let interface = fake.usb("1-1.2", "0c2e", "0b61", None);
        fake.keyboard(&interface, "event3", devices::tests::KEYBOARD_KEYS);
        fake.tty(&interface, "ttyACM0");
        let found = devices::find(&fake.sysfs());
        let all = Scanners {
            scanners: vec![spec("front", Transport::Keyboard, None)],
        };
        assert_eq!(all.of(&found[0]).map(|s| s.name.as_str()), Some("front"));
        // The same device's serial port is no keyboard scanner.
        assert_eq!(all.of(&found[1]), None);
    }
}
