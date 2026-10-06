//! The screen: what `tessaro-ctl screen show` and the Screen page say about
//! the connected displays (their EDID) and the TV over HDMI-CEC.

use protocol::{CecAdapter, CecDevice, CecPower, Connector, DisplayIdentity, ScreenShow, TvStatus};

use crate::text::{Line, Tone};

const LABEL: usize = 9;

fn indent() -> Line {
    Line::plain("    ")
}

fn field(label: &str) -> Line {
    indent().pad(Tone::Label, label, LABEL).text(" ")
}

fn none() -> Line {
    Line::of(Tone::Muted, "(none)")
}

/// Who made a display and what it calls itself: `Samsung SAMSUNG`, `DEL
/// DELL U2720Q` for a maker the client does not know by name.
pub fn display_name(display: &DisplayIdentity) -> String {
    let maker = display.vendor.as_deref().unwrap_or(&display.vendor_id);
    match &display.model {
        Some(model) => format!("{maker} {model}"),
        None => maker.to_string(),
    }
}

/// When it was made: `2023, week 12`.
fn made(display: &DisplayIdentity) -> Option<String> {
    let year = display.year?;
    Some(match display.week {
        Some(week) => format!("{year}, week {week}"),
        None => year.to_string(),
    })
}

/// `input 2` for a physical address `2.0.0.0`: the TV's own HDMI input the
/// device is plugged into, when it is plugged straight into the TV.
fn input(address: &str) -> Line {
    let mut parts = address.split('.');
    let first = parts.next().unwrap_or("0");
    let direct = parts.all(|part| part == "0");
    if direct && first != "0" {
        Line::plain(format!("HDMI {first}"))
            .text(" ")
            .add(Tone::Muted, format!("(physical address {address})"))
    } else {
        Line::plain(format!("physical address {address}"))
    }
}

/// One connector: what is plugged into it and what it can show.
pub fn connector(connector: &Connector) -> Vec<Line> {
    let mut lines = vec![Line::of(Tone::Heading, &connector.name)];
    match &connector.display {
        Some(display) => {
            lines.push(field("display").text(display_name(display)).text(" ").add(
                Tone::Muted,
                format!("({} {:04x})", display.vendor_id, display.product_code),
            ));
            lines.push(field("serial").join(match &display.serial {
                Some(serial) => Line::plain(serial),
                None => none(),
            }));
            if let Some(made) = made(display) {
                lines.push(field("made").text(made));
            }
            if let (Some(width), Some(height)) = (display.width_cm, display.height_cm) {
                lines.push(field("size").text(format!("{width} x {height} cm")));
            }
            if let Some(address) = &display.hdmi_address {
                lines.push(field("input").join(input(address)));
            }
        }
        None => lines.push(field("display").add(Tone::Muted, "(it does not say what it is)")),
    }
    lines.push(
        field("modes").join(match connector.modes.first() {
            Some(preferred) => {
                Line::plain(format!("{}, preferred {preferred}", connector.modes.len()))
                    .text(" ")
                    .add(Tone::Muted, "- tessaro-ctl screen modes lists them")
            }
            None => none(),
        }),
    );
    lines
}

/// What a device's power reads as.
pub fn power(power: CecPower) -> Line {
    let tone = match power {
        CecPower::On => Tone::Ok,
        CecPower::Standby => Tone::Muted,
        CecPower::TurningOn | CecPower::TurningOff => Tone::Warn,
    };
    Line::of(tone, power.name())
}

/// The TV's power and whether it shows the device: `on, showing this
/// device`.
fn tv_line(power: Option<CecPower>, showing: bool) -> Line {
    let mut line = match power {
        Some(power) => self::power(power),
        None => Line::of(Tone::Muted, "(not answered yet)"),
    };
    if power == Some(CecPower::On) {
        line = if showing {
            line.text(", showing this device")
        } else {
            line.text(", ").add(Tone::Warn, "showing another input")
        };
    }
    line
}

/// The TV in `device status`: `on, showing this device (Samsung TV)`.
pub fn tv(status: &TvStatus) -> Line {
    let line = tv_line(status.power, status.showing);
    match &status.name {
        Some(name) => line.text(" ").add(Tone::Muted, format!("({name})")),
        None => line,
    }
}

/// One device on the bus, in columns: address, kind, name, maker,
/// physical address, power.
fn bus_device(device: &CecDevice) -> Line {
    let mut line = Line::new()
        .pad(Tone::Plain, device.address.to_string(), 3)
        .pad(Tone::Label, &device.kind, 10)
        .pad(Tone::Plain, device.name.as_deref().unwrap_or("-"), 15)
        .pad(Tone::Plain, device.vendor.as_deref().unwrap_or("-"), 14)
        .pad(Tone::Muted, device.physical.as_deref().unwrap_or("-"), 9);
    if let Some(power) = device.power {
        line = line.join(self::power(power));
    }
    line
}

/// One adapter: where it is, what the device is on its bus, the TV and
/// everything else that answered.
pub fn adapter(adapter: &CecAdapter) -> Vec<Line> {
    let mut heading = Line::of(Tone::Heading, "HDMI-CEC")
        .text(" ")
        .add(Tone::Muted, &adapter.device);
    if let Some(connector) = &adapter.connector {
        heading = heading.add(Tone::Muted, format!(" on {connector}"));
    }
    let mut lines = vec![heading];
    if let Some(problem) = &adapter.problem {
        lines.push(field("problem").add(Tone::Bad, problem));
    }
    lines.push(field("name").text(&adapter.name));
    lines.push(
        field("address").join(match (adapter.address, &adapter.physical) {
            (Some(logical), Some(physical)) => {
                Line::plain(format!("logical {logical}, physical {physical}"))
            }
            (logical, physical) => Line::of(Tone::Warn, "none yet").text(" ").add(
                Tone::Muted,
                match (logical, physical) {
                    (None, None) => {
                        "- the TV gives none: off, or dropping hot-plug in standby".to_string()
                    }
                    (logical, physical) => format!(
                        "- logical {}, physical {}",
                        logical.map_or("none".to_string(), |logical| logical.to_string()),
                        physical.as_deref().unwrap_or("none")
                    ),
                },
            ),
        }),
    );
    lines.push(field("tv").join(tv_line(adapter.tv, adapter.active)));
    if adapter.devices.is_empty() {
        lines.push(field("bus").join(none()));
    } else {
        for (at, device) in adapter.devices.iter().enumerate() {
            let label = if at == 0 { "bus" } else { "" };
            lines.push(field(label).join(bus_device(device)));
        }
    }
    lines
}

/// `screen show`: every connected display, then the HDMI-CEC bus, or how
/// to switch it on.
pub fn show(show: &ScreenShow) -> Vec<Line> {
    let mut lines = Vec::new();
    if show.connectors.is_empty() {
        lines.push(Line::of(Tone::Warn, "no display is connected"));
    }
    for one in &show.connectors {
        lines.extend(connector(one));
        lines.push(Line::new());
    }
    if !show.cec {
        lines.push(
            Line::of(Tone::Heading, "HDMI-CEC")
                .text(" ")
                .add(Tone::Muted, "off -")
                .text(" ")
                .add(Tone::Cmd, "tessaro-ctl config set screen.cec.enable=1")
                .text(" ")
                .add(Tone::Muted, "talks to the TV"),
        );
    } else if show.adapters.is_empty() {
        lines.push(
            Line::of(Tone::Heading, "HDMI-CEC")
                .text(" ")
                .add(Tone::Warn, "on, but no adapter on a display connector")
                .text(" ")
                .add(
                    Tone::Muted,
                    "- this hardware has no CEC, or the TV is not plugged in",
                ),
        );
    }
    for (at, one) in show.adapters.iter().enumerate() {
        if at > 0 {
            lines.push(Line::new());
        }
        lines.extend(adapter(one));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tv_plugged_straight_in_names_its_input() {
        assert_eq!(
            input("2.0.0.0").to_string(),
            "HDMI 2 (physical address 2.0.0.0)"
        );
        assert_eq!(input("2.1.0.0").to_string(), "physical address 2.1.0.0");
    }
}
