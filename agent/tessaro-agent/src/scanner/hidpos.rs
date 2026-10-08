//! A HID POS barcode scanner: where its reports keep a scan.
//!
//! HID's Bar Code Scanner page (usage page 0x8C, HID Usage Tables 1.5,
//! section 25) names the fields of a scanner's input report: the three
//! bytes of the AIM symbology identifier (`Symbology Identifier 1` to `3`,
//! usages 0xFB to 0xFD), the scanned bytes (`Decoded Data`, 0xFE) and a flag
//! saying more of the same scan follows in the next report (`Decode Data
//! Continued`, 0xFF). The report descriptor says where each lies, so the
//! layout is read from the device's own descriptor rather than assumed:
//! makers order and size them differently. A report that says how many of
//! its data bytes are the scan (Generic Desktop's `Byte Count`, 0x3B, as
//! Honeywell's do) is cut to that many; without one the data's padding NULs
//! are taken off its end. Vendor fields are left alone.

/// The Bar Code Scanner usage page.
pub const PAGE: u16 = 0x8c;
const GENERIC_DESKTOP: u16 = 0x01;
const BYTE_COUNT: u16 = 0x3b;
const SYMBOLOGY_1: u16 = 0xfb;
const SYMBOLOGY_3: u16 = 0xfd;
const DECODED_DATA: u16 = 0xfe;
const DATA_CONTINUED: u16 = 0xff;

/// Bits from the start of a report, its id included when it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Field {
    offset: usize,
    bits: usize,
}

/// Where a scan lies in the input report that carries one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The report's id, its first byte; `None` for a device with only one
    /// report.
    pub report_id: Option<u8>,
    symbology: Vec<Field>,
    data: Field,
    continued: Option<Field>,
    /// How many of the data bytes are the scan's.
    count: Option<Field>,
}

/// One report's part of a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub data: Vec<u8>,
    /// `]Q1`, when the scanner reports one.
    pub symbology: Option<String>,
    /// More of the same scan comes in the next report.
    pub continued: bool,
}

/// Does the descriptor have a Bar Code Scanner page at all?
pub fn has_page(descriptor: &[u8]) -> bool {
    let mut found = false;
    walk(descriptor, |item| {
        if let Item::UsagePage(page) = item {
            found |= page == PAGE;
        }
    });
    found
}

enum Item {
    UsagePage(u16),
    Usage(u32),
    UsageMinimum(u32),
    UsageMaximum(u32),
    ReportId(u8),
    ReportSize(usize),
    ReportCount(usize),
    Input(u32),
    Output,
    Feature,
    /// A collection's start or end: a main item, which ends the usages
    /// before it as an input does.
    Collection,
    Other,
}

/// Every short item of a descriptor, in order. A long item is skipped.
fn walk(descriptor: &[u8], mut each: impl FnMut(Item)) {
    let mut at = 0;
    while at < descriptor.len() {
        let prefix = descriptor[at];
        if prefix == 0xfe {
            // Long item: its size is the next byte.
            let size = descriptor.get(at + 1).copied().unwrap_or(0) as usize;
            at += 3 + size;
            continue;
        }
        let size = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let Some(bytes) = descriptor.get(at + 1..at + 1 + size) else {
            return;
        };
        let value = bytes
            .iter()
            .rev()
            .fold(0u32, |value, byte| (value << 8) | u32::from(*byte));
        let item = match prefix & 0xfc {
            0x04 => Item::UsagePage(value as u16),
            0x08 => Item::Usage(value),
            0x18 => Item::UsageMinimum(value),
            0x28 => Item::UsageMaximum(value),
            0x84 => Item::ReportId(value as u8),
            0x74 => Item::ReportSize(value as usize),
            0x94 => Item::ReportCount(value as usize),
            0x80 => Item::Input(value),
            0x90 => Item::Output,
            0xb0 => Item::Feature,
            0xa0 | 0xc0 => Item::Collection,
            _ => Item::Other,
        };
        each(item);
        at += 1 + size;
    }
}

/// The layout of the input report that carries Decoded Data, if the
/// descriptor has one.
pub fn layout(descriptor: &[u8]) -> Option<Layout> {
    let mut page: u16 = 0;
    let mut usages: Vec<(u16, u16)> = Vec::new();
    let mut minimum: Option<u32> = None;
    let mut report_id: Option<u8> = None;
    let mut size = 0usize;
    let mut count = 0usize;
    // Bits so far in each input report, by id.
    let mut offsets: std::collections::HashMap<Option<u8>, usize> = Default::default();
    let mut found: std::collections::HashMap<Option<u8>, Layout> = Default::default();

    walk(descriptor, |item| match item {
        Item::UsagePage(value) => page = value,
        Item::Usage(value) => usages.push(usage(page, value)),
        Item::UsageMinimum(value) => minimum = Some(value),
        Item::UsageMaximum(maximum) => {
            if let Some(minimum) = minimum.take() {
                for value in minimum..=maximum.min(minimum + 255) {
                    usages.push(usage(page, value));
                }
            }
        }
        Item::ReportId(id) => report_id = Some(id),
        Item::ReportSize(bits) => size = bits,
        Item::ReportCount(n) => count = n,
        Item::Input(flags) => {
            let start =
                *offsets
                    .entry(report_id)
                    .or_insert(if report_id.is_some() { 8 } else { 0 });
            let constant = flags & 0x01 != 0;
            let array = flags & 0x02 == 0;
            if !constant {
                let entry = found.entry(report_id).or_insert_with(|| Layout {
                    report_id,
                    symbology: Vec::new(),
                    data: Field { offset: 0, bits: 0 },
                    continued: None,
                    count: None,
                });
                if array || usages.len() <= 1 {
                    // One usage for every one of `count` fields: a buffer
                    // of bytes, or a single value.
                    if let Some(&(used_page, used)) = usages.first() {
                        let field = Field {
                            offset: start,
                            bits: size * count,
                        };
                        place(entry, used_page, used, field, size);
                    }
                } else {
                    for (n, &(used_page, used)) in usages.iter().enumerate().take(count) {
                        let field = Field {
                            offset: start + n * size,
                            bits: size,
                        };
                        place(entry, used_page, used, field, size);
                    }
                }
            }
            offsets.insert(report_id, start + size * count);
            usages.clear();
            minimum = None;
        }
        Item::Output | Item::Feature | Item::Collection => {
            usages.clear();
            minimum = None;
        }
        Item::Other => {}
    });

    let mut layouts: Vec<Layout> = found
        .into_values()
        .filter(|layout| layout.data.bits >= 8)
        .collect();
    layouts.sort_by_key(|layout| layout.report_id);
    layouts.into_iter().next()
}

/// A usage with its page: an extended one names its own.
fn usage(page: u16, value: u32) -> (u16, u16) {
    if value > 0xffff {
        ((value >> 16) as u16, value as u16)
    } else {
        (page, value as u16)
    }
}

fn place(layout: &mut Layout, page: u16, usage: u16, field: Field, size: usize) {
    if page == GENERIC_DESKTOP && usage == BYTE_COUNT && size == 8 {
        layout.count = Some(field);
        return;
    }
    if page != PAGE {
        return;
    }
    match usage {
        SYMBOLOGY_1..=SYMBOLOGY_3 => {
            // Each identifier byte its own field, or the three in one.
            for n in 0..(field.bits / 8).max(1) {
                layout.symbology.push(Field {
                    offset: field.offset + n * 8,
                    bits: 8,
                });
            }
        }
        DECODED_DATA if size == 8 => layout.data = field,
        DATA_CONTINUED => {
            layout.continued = Some(Field {
                offset: field.offset,
                bits: size.max(1),
            })
        }
        _ => {}
    }
}

impl Layout {
    /// One report as hidraw reads it, its id first when the device has
    /// ids; `None` for another report.
    pub fn read(&self, report: &[u8]) -> Option<Piece> {
        if let Some(id) = self.report_id {
            if report.first() != Some(&id) {
                return None;
            }
        }
        let mut data = bytes(report, self.data);
        match self
            .count
            .and_then(|field| bytes(report, field).first().copied())
        {
            Some(count) => data.truncate(count as usize),
            None => {
                while data.last() == Some(&0) {
                    data.pop();
                }
            }
        }
        let symbology: Vec<u8> = self
            .symbology
            .iter()
            .flat_map(|field| bytes(report, *field))
            .filter(|byte| *byte != 0)
            .collect();
        let symbology = (!symbology.is_empty() && symbology.iter().all(u8::is_ascii_graphic))
            .then(|| String::from_utf8_lossy(&symbology).to_string());
        let continued = self
            .continued
            .is_some_and(|field| bit(report, field.offset));
        Some(Piece {
            data,
            symbology,
            continued,
        })
    }
}

/// A byte-aligned field's bytes, as far as the report has them.
fn bytes(report: &[u8], field: Field) -> Vec<u8> {
    if !field.offset.is_multiple_of(8) {
        return Vec::new();
    }
    let start = field.offset / 8;
    let end = (start + field.bits / 8).min(report.len());
    report
        .get(start..end)
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

fn bit(report: &[u8], offset: usize) -> bool {
    report
        .get(offset / 8)
        .is_some_and(|byte| byte & (1 << (offset % 8)) != 0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A HID POS scanner's report 2, as Honeywell's and the e2e suite's fake
    /// one lay it out: three symbology bytes, 56 bytes of decoded data, and
    /// the continued flag in a byte of its own.
    pub(crate) const DESCRIPTOR: &[u8] = &[
        0x05, 0x8c, // Usage Page (Bar Code Scanner)
        0x09, 0x02, // Usage (Bar Code Scanner)
        0xa1, 0x01, // Collection (Application)
        0x85, 0x02, //   Report ID (2)
        0x15, 0x00, //   Logical Minimum (0)
        0x26, 0xff, 0x00, // Logical Maximum (255)
        0x75, 0x08, //   Report Size (8)
        0x95, 0x01, //   Report Count (1)
        0x09, 0xfb, //   Usage (Symbology Identifier 1)
        0x81, 0x02, //   Input (Data, Var, Abs)
        0x09, 0xfc, //   Usage (Symbology Identifier 2)
        0x81, 0x02, //   Input
        0x09, 0xfd, //   Usage (Symbology Identifier 3)
        0x81, 0x02, //   Input
        0x95, 0x38, //   Report Count (56)
        0x09, 0xfe, //   Usage (Decoded Data)
        0x82, 0x02, 0x01, // Input (Data, Var, Abs, Buffered Bytes)
        0x75, 0x01, //   Report Size (1)
        0x95, 0x01, //   Report Count (1)
        0x25, 0x01, //   Logical Maximum (1)
        0x09, 0xff, //   Usage (Decode Data Continued)
        0x81, 0x02, //   Input
        0x75, 0x07, //   Report Size (7)
        0x81, 0x03, //   Input (Constant): padding
        0xc0, // End Collection
    ];

    fn report(symbology: &[u8; 3], data: &[u8], continued: bool) -> Vec<u8> {
        let mut report = vec![0x02];
        report.extend_from_slice(symbology);
        let mut padded = data.to_vec();
        padded.resize(56, 0);
        report.extend(padded);
        report.push(u8::from(continued));
        report
    }

    #[test]
    fn the_layout_is_read_from_the_descriptor() {
        assert!(has_page(DESCRIPTOR));
        let layout = layout(DESCRIPTOR).unwrap();
        assert_eq!(layout.report_id, Some(2));
        assert_eq!(
            layout.data,
            Field {
                offset: 32,
                bits: 448
            }
        );
        assert_eq!(
            layout.continued,
            Some(Field {
                offset: 480,
                bits: 1
            })
        );
        assert_eq!(layout.symbology.len(), 3);
    }

    #[test]
    fn a_report_gives_its_data_symbology_and_whether_more_follows() {
        let layout = layout(DESCRIPTOR).unwrap();
        let piece = layout
            .read(&report(b"]E0", b"5901234123457", false))
            .unwrap();
        assert_eq!(piece.data, b"5901234123457");
        assert_eq!(piece.symbology.as_deref(), Some("]E0"));
        assert!(!piece.continued);
        let piece = layout.read(&report(b"]Q1", &[b'x'; 56], true)).unwrap();
        assert_eq!(piece.data.len(), 56);
        assert!(piece.continued);
        // Another report, or a short one.
        assert_eq!(layout.read(&[0x01, 0x00]), None);
        assert_eq!(layout.read(&[0x02, b']']).unwrap().data, Vec::<u8>::new());
    }

    /// Honeywell's layout, which the e2e suite's fake scanner
    /// (`test/usbscanner`) sends: a byte count first, the three symbology
    /// identifiers as one field, in a nested collection.
    const COUNTED: &[u8] = &[
        0x05, 0x8c, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x02, 0x09, 0x12, 0xa1, 0x02, 0x15, 0x00, 0x26,
        0xff, 0x00, 0x75, 0x08, 0x95, 0x01, 0x05, 0x01, 0x09, 0x3b, 0x81, 0x02, 0x95, 0x03, 0x05,
        0x8c, 0x09, 0xfb, 0x09, 0xfc, 0x09, 0xfd, 0x81, 0x02, 0x95, 0x38, 0x09, 0xfe, 0x82, 0x02,
        0x01, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x09, 0xff, 0x81, 0x02, 0x75, 0x07, 0x81, 0x03,
        0xc0, 0xc0,
    ];

    #[test]
    fn a_byte_count_cuts_the_data_even_where_it_holds_nuls() {
        let layout = layout(COUNTED).unwrap();
        let mut report = vec![0x02, 4];
        report.extend_from_slice(b"]Q1");
        let mut data = b"a\0b\0".to_vec();
        data.resize(56, 0);
        report.extend(data);
        report.push(0x01);
        assert_eq!(report.len(), 62);
        let piece = layout.read(&report).unwrap();
        assert_eq!(piece.data, b"a\0b\0");
        assert_eq!(piece.symbology.as_deref(), Some("]Q1"));
        assert!(piece.continued);
    }

    #[test]
    fn a_keyboard_descriptor_has_no_scanner_page() {
        let keyboard = [
            0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00,
            0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x03,
            0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65,
            0x81, 0x00, 0xc0,
        ];
        assert!(!has_page(&keyboard));
        assert_eq!(layout(&keyboard), None);
    }
}
