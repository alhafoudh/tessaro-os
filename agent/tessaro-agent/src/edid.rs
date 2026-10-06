//! What a display says it is, from the EDID DRM exposes next to its modes
//! (`/sys/class/drm/card0-HDMI-A-1/edid`).
//!
//! Only the base block's identity is read - maker, product code, serial,
//! the name and serial descriptors, when it was made, its size - plus the
//! HDMI vendor block of a CTA extension for the HDMI-CEC physical address.
//! Modes stay `modes`' business: DRM has already parsed them.

use protocol::DisplayIdentity;

const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
const BLOCK: usize = 128;

/// The identity in an EDID, or `None` when it is not one (too short, no
/// header, an empty file for a connector without a display).
pub fn identity(edid: &[u8]) -> Option<DisplayIdentity> {
    if edid.len() < BLOCK || edid[..8] != HEADER {
        return None;
    }

    let packed = u16::from_be_bytes([edid[8], edid[9]]);
    let letter = |shift: u16| {
        let value = ((packed >> shift) & 0x1f) as u8;
        (1..=26)
            .contains(&value)
            .then(|| (b'A' + value - 1) as char)
    };
    let vendor_id: String = [letter(10)?, letter(5)?, letter(0)?].iter().collect();

    let product_code = u16::from_le_bytes([edid[10], edid[11]]);
    let serial_number = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);
    // Week 0xff marks the year as a model year, with no week.
    let week = edid[16];
    let year = edid[17];

    let mut model = None;
    let mut serial_text = None;
    for start in [54, 72, 90, 108] {
        let descriptor = &edid[start..start + 18];
        // A display descriptor, not a detailed timing: a zero pixel clock.
        if descriptor[0] != 0 || descriptor[1] != 0 {
            continue;
        }
        match descriptor[3] {
            0xfc => model = text(&descriptor[5..]),
            0xff => serial_text = text(&descriptor[5..]),
            _ => {}
        }
    }

    let serial = serial_text.or_else(|| {
        (serial_number != 0 && serial_number != 0x0101_0101).then(|| serial_number.to_string())
    });

    Some(DisplayIdentity {
        vendor: vendor_name(&vendor_id).map(str::to_string),
        vendor_id,
        product_code,
        model,
        serial,
        year: (year != 0).then_some(1990 + year as u16),
        week: (1..=54).contains(&week).then_some(week),
        width_cm: (edid[21] != 0 && edid[22] != 0).then_some(edid[21]),
        height_cm: (edid[21] != 0 && edid[22] != 0).then_some(edid[22]),
        hdmi_address: hdmi_address(edid),
    })
}

/// A descriptor's text: up to 13 bytes, ended by a line feed, padded with
/// spaces. Anything but printable ASCII is dropped.
fn text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0x0a).unwrap_or(bytes.len());
    let text: String = bytes[..end]
        .iter()
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .map(|&b| b as char)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The physical address in the HDMI vendor-specific data block (IEEE OUI
/// 00-0C-03) of the first CTA-861 extension that has one: `1.0.0.0`.
fn hdmi_address(edid: &[u8]) -> Option<String> {
    let extensions = edid[126] as usize;
    for n in 1..=extensions {
        let block = edid.get(n * BLOCK..(n + 1) * BLOCK)?;
        if block[0] != 0x02 {
            continue;
        }
        // Data blocks run from byte 4 to where the detailed timings start.
        let end = (block[2] as usize).clamp(4, BLOCK);
        let mut at = 4;
        while at < end {
            let tag = block[at] >> 5;
            let len = (block[at] & 0x1f) as usize;
            let body = block.get(at + 1..at + 1 + len)?;
            if tag == 3 && len >= 5 && body[..3] == [0x03, 0x0c, 0x00] {
                let (ab, cd) = (body[3], body[4]);
                return Some(format!("{}.{}.{}.{}", ab >> 4, ab & 0xf, cd >> 4, cd & 0xf));
            }
            at += 1 + len;
        }
    }
    None
}

/// The makers a TV or monitor is likeliest to come from, by PNP id.
const VENDORS: &[(&str, &str)] = &[
    ("ACR", "Acer"),
    ("AOC", "AOC"),
    ("APP", "Apple"),
    ("AUS", "ASUS"),
    ("BNQ", "BenQ"),
    ("DEL", "Dell"),
    ("ENC", "EIZO"),
    ("GBT", "Gigabyte"),
    ("GSM", "LG"),
    ("HEC", "Hisense"),
    ("HWP", "HP"),
    ("IVM", "iiyama"),
    ("LEN", "Lenovo"),
    ("LGD", "LG Display"),
    ("MEI", "Panasonic"),
    ("MSI", "MSI"),
    ("NEC", "NEC"),
    ("PHL", "Philips"),
    ("RHT", "Red Hat (QEMU)"),
    ("SAM", "Samsung"),
    ("SHP", "Sharp"),
    ("SNY", "Sony"),
    ("TSB", "Toshiba"),
    ("VIZ", "Vizio"),
    ("VSC", "ViewSonic"),
    ("XMI", "Xiaomi"),
];

fn vendor_name(id: &str) -> Option<&'static str> {
    VENDORS
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, name)| *name)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A base block with what the identity reads, checksum left at 0: the
    /// parser does not insist on it, as DRM has already.
    pub(crate) fn edid(
        vendor: &str,
        product: u16,
        serial: u32,
        descriptors: &[(u8, &str)],
    ) -> Vec<u8> {
        let mut edid = vec![0u8; BLOCK];
        edid[..8].copy_from_slice(&HEADER);
        let v: Vec<u16> = vendor.bytes().map(|b| (b - b'A' + 1) as u16).collect();
        edid[8..10].copy_from_slice(&((v[0] << 10) | (v[1] << 5) | v[2]).to_be_bytes());
        edid[10..12].copy_from_slice(&product.to_le_bytes());
        edid[12..16].copy_from_slice(&serial.to_le_bytes());
        edid[16] = 12;
        edid[17] = 33;
        edid[21] = 121;
        edid[22] = 68;
        // A detailed timing first, as real EDIDs have.
        edid[54] = 0x02;
        edid[55] = 0x3a;
        for (i, (tag, text)) in descriptors.iter().enumerate() {
            let at = 72 + 18 * i;
            edid[at + 3] = *tag;
            let mut body = text.as_bytes().to_vec();
            body.push(0x0a);
            body.resize(13, b' ');
            edid[at + 5..at + 18].copy_from_slice(&body);
        }
        edid
    }

    /// `edid` with a CTA-861 extension carrying an HDMI block at `address`.
    pub(crate) fn with_hdmi(mut edid: Vec<u8>, address: [u8; 2]) -> Vec<u8> {
        edid[126] = 1;
        let mut cta = vec![0u8; BLOCK];
        cta[0] = 0x02;
        cta[1] = 0x03;
        // A video block of one mode, then the HDMI block.
        let blocks = [
            &[0x41, 0x10][..],
            &[0x65, 0x03, 0x0c, 0x00, address[0], address[1]][..],
        ]
        .concat();
        cta[4..4 + blocks.len()].copy_from_slice(&blocks);
        cta[2] = (4 + blocks.len()) as u8;
        edid.extend(cta);
        edid
    }

    #[test]
    fn a_tv_names_its_maker_model_and_input() {
        let tv = with_hdmi(
            edid("SAM", 0x7372, 0x0101_0101, &[(0xfd, ""), (0xfc, "SAMSUNG")]),
            [0x20, 0x00],
        );
        let found = identity(&tv).unwrap();
        assert_eq!(found.vendor_id, "SAM");
        assert_eq!(found.vendor.as_deref(), Some("Samsung"));
        assert_eq!(found.product_code, 0x7372);
        assert_eq!(found.model.as_deref(), Some("SAMSUNG"));
        // 0x01010101 is the placeholder TVs fill in.
        assert_eq!(found.serial, None);
        assert_eq!(found.year, Some(2023));
        assert_eq!(found.week, Some(12));
        assert_eq!((found.width_cm, found.height_cm), (Some(121), Some(68)));
        assert_eq!(found.hdmi_address.as_deref(), Some("2.0.0.0"));
    }

    #[test]
    fn a_monitor_has_a_serial_and_no_hdmi_address() {
        let monitor = edid(
            "DEL",
            0x41a8,
            1_112_100_684,
            &[(0xff, "7XJ3K23"), (0xfc, "DELL U2720Q")],
        );
        let found = identity(&monitor).unwrap();
        assert_eq!(found.vendor.as_deref(), Some("Dell"));
        assert_eq!(found.serial.as_deref(), Some("7XJ3K23"));
        assert_eq!(found.model.as_deref(), Some("DELL U2720Q"));
        assert_eq!(found.hdmi_address, None);
    }

    #[test]
    fn a_serial_number_stands_in_for_a_missing_serial_text() {
        let found = identity(&edid("RHT", 0x1234, 42, &[(0xfc, "QEMU Monitor")])).unwrap();
        assert_eq!(found.serial.as_deref(), Some("42"));
        assert_eq!(found.vendor.as_deref(), Some("Red Hat (QEMU)"));
        let unknown = identity(&edid("ZZZ", 1, 0, &[])).unwrap();
        assert_eq!(
            (unknown.vendor, unknown.serial, unknown.model),
            (None, None, None)
        );
    }

    #[test]
    fn what_is_not_an_edid_has_no_identity() {
        assert_eq!(identity(&[]), None);
        assert_eq!(identity(&[0u8; 128]), None);
        let mut short = edid("SAM", 1, 0, &[]);
        short.truncate(100);
        assert_eq!(identity(&short), None);
    }
}
