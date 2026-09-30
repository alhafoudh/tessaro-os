//! Snapshots: the camera's newest frame as `<dir>/<device>.jpg`, written
//! only while the agent asks for one.
//!
//! The agent touches `<dir>/<device>.want` on every snapshot request and
//! serves `<device>.jpg` when it is fresh enough (`tessaro-agent`'s
//! `camera.rs`). The mirror looks at `.want` at most once a second; while it
//! was touched in the last `WANTED_FOR`, the next good frame goes to
//! `.jpg`, at most one every `EVERY`. Otherwise nothing is written, so an
//! idle camera costs no encoding and no writes.
//!
//! An MJPEG frame is a JPEG already, save that UVC cameras leave out the
//! Huffman tables (the MJPEG convention is that a decoder assumes the
//! standard ones), so `with_huffman` puts the standard tables in. A YUYV
//! frame is encoded, `encode_yuyv`.

use std::borrow::Cow;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

use crate::choice::{MJPEG, YUYV};

/// A `.want` touched this recently means a snapshot is asked for. The agent
/// polls for a fresh frame for 3 s after it touches the file.
const WANTED_FOR: Duration = Duration::from_secs(5);

/// How often `.want` is looked at.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// The most often a snapshot is written: 2 a second.
const EVERY: Duration = Duration::from_millis(500);

/// YUYV's JPEG quality.
const QUALITY: u8 = 80;

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;
const DHT: u8 = 0xC4;

/// Where one camera's snapshots are asked for and written.
pub struct Snapshots {
    want: PathBuf,
    jpg: PathBuf,
    checked: Option<Instant>,
    wanted: bool,
    written: Option<Instant>,
    /// The last frame could not be made into a JPEG, so the next failure is
    /// not said again.
    failing: bool,
}

impl Snapshots {
    pub fn new(dir: &Path, device: &str) -> Snapshots {
        Snapshots {
            want: dir.join(format!("{device}.want")),
            jpg: dir.join(format!("{device}.jpg")),
            checked: None,
            wanted: false,
            written: None,
            failing: false,
        }
    }

    /// Look at `.want`, at most once every `CHECK_EVERY`. Called at every
    /// turn of the capture loop, which wakes at least once a second.
    pub fn check(&mut self) {
        if self.checked.is_some_and(|at| at.elapsed() < CHECK_EVERY) {
            return;
        }
        self.checked = Some(Instant::now());
        let wanted = fs::metadata(&self.want)
            .and_then(|meta| meta.modified())
            .is_ok_and(|touched| {
                // Either way round: a clock set back leaves the touch in the
                // future, and that must not keep snapshots going for hours.
                let age = SystemTime::now()
                    .duration_since(touched)
                    .unwrap_or_else(|err| err.duration());
                age < WANTED_FOR
            });
        if wanted && !self.wanted {
            eprintln!("snapshots asked for; writing {}", self.jpg.display());
            self.written = None;
        } else if !wanted && self.wanted {
            eprintln!("snapshots no longer asked for");
            // A stale picture is not left for anyone to take as the camera's.
            remove(&self.jpg);
        }
        self.wanted = wanted;
    }

    /// Whether the frame in hand should become the snapshot.
    pub fn due(&self) -> bool {
        self.wanted && self.written.is_none_or(|at| at.elapsed() >= EVERY)
    }

    /// Write `frame`, captured as `format` at `width`x`height`, to `.jpg`.
    /// A frame that is not a picture is skipped and said once.
    pub fn write(&mut self, format: &str, frame: &[u8], width: u32, height: u32) {
        self.written = Some(Instant::now());
        match jpeg(format, frame, width, height) {
            Ok(jpeg) => {
                self.failing = false;
                crate::write_whole(&self.jpg, &jpeg);
            }
            Err(why) => {
                if !self.failing {
                    eprintln!("a snapshot from a {format} frame: {why}");
                }
                self.failing = true;
            }
        }
    }

    /// Both files, on the way out.
    pub fn remove(&self) {
        remove(&self.jpg);
        remove(&self.want);
    }
}

fn remove(path: &Path) {
    match fs::remove_file(path) {
        Err(err) if err.kind() != ErrorKind::NotFound => {
            eprintln!("{}: {err}", path.display());
        }
        _ => {}
    }
}

/// `frame` as a JPEG a browser shows.
pub fn jpeg<'a>(
    format: &str,
    frame: &'a [u8],
    width: u32,
    height: u32,
) -> Result<Cow<'a, [u8]>, String> {
    match format {
        MJPEG => with_huffman(frame),
        YUYV => encode_yuyv(frame, width, height).map(Cow::Owned),
        other => Err(format!("{other} is not a format the mirror captures")),
    }
}

// The standard Huffman tables of the JPEG spec, Annex K.3 (tables K.3 to
// K.6), which `DHT_SEGMENT` puts into one segment: luminance DC and AC, then
// chrominance DC and AC. These are the tables an MJPEG frame without its own
// is decoded with.
const LUMA_DC_LENGTHS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const LUMA_DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const CHROMA_DC_LENGTHS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const CHROMA_DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const LUMA_AC_LENGTHS: [u8; 16] = [
    0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7D,
];
const LUMA_AC_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7,
    0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5,
    0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
    0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];
const CHROMA_AC_LENGTHS: [u8; 16] = [
    0x00, 0x02, 0x01, 0x02, 0x04, 0x04, 0x03, 0x04, 0x07, 0x05, 0x04, 0x04, 0x00, 0x01, 0x02, 0x77,
];
const CHROMA_AC_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0,
    0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A, 0x26,
    0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5,
    0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3,
    0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA,
    0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];

/// The marker, a 2-byte length, and per table its class and destination
/// byte, 16 code-length counts and its values.
const DHT_LEN: usize = 2 + 2 + 4 * (1 + 16) + 2 * 12 + 2 * 162;
const DHT_SEGMENT: [u8; DHT_LEN] = dht_segment();

const fn dht_segment() -> [u8; DHT_LEN] {
    let tables: [(u8, &[u8; 16], &[u8]); 4] = [
        (0x00, &LUMA_DC_LENGTHS, &LUMA_DC_VALUES),
        (0x10, &LUMA_AC_LENGTHS, &LUMA_AC_VALUES),
        (0x01, &CHROMA_DC_LENGTHS, &CHROMA_DC_VALUES),
        (0x11, &CHROMA_AC_LENGTHS, &CHROMA_AC_VALUES),
    ];
    let mut out = [0u8; DHT_LEN];
    let length = DHT_LEN - 2;
    out[0] = 0xFF;
    out[1] = DHT;
    out[2] = (length >> 8) as u8;
    out[3] = (length & 0xFF) as u8;
    let mut at = 4;
    let mut t = 0;
    while t < tables.len() {
        let (class, lengths, values) = tables[t];
        out[at] = class;
        at += 1;
        let mut count = 0;
        let mut i = 0;
        while i < 16 {
            out[at] = lengths[i];
            count += lengths[i] as usize;
            at += 1;
            i += 1;
        }
        // A table's code-length counts add up to its number of values.
        assert!(count == values.len());
        let mut i = 0;
        while i < values.len() {
            out[at] = values[i];
            at += 1;
            i += 1;
        }
        t += 1;
    }
    assert!(at == DHT_LEN);
    out
}

/// `frame` with the standard Huffman tables before its first scan, when no
/// DHT segment comes before that scan; untouched when one does. Walks the
/// marker segments from SOI to the first SOS; a frame that does not hold
/// together that far is an error.
pub fn with_huffman(frame: &[u8]) -> Result<Cow<'_, [u8]>, String> {
    if !frame.starts_with(&[0xFF, SOI]) {
        return Err("it does not start with a JPEG's SOI".to_string());
    }
    let mut at = 2;
    loop {
        let start = at;
        if frame.get(at) != Some(&0xFF) {
            return Err(format!("no marker at byte {at}"));
        }
        // Any number of 0xFF fill bytes may come before a marker's code.
        while frame.get(at) == Some(&0xFF) {
            at += 1;
        }
        let Some(&code) = frame.get(at) else {
            return Err("it ends inside a marker".to_string());
        };
        at += 1;
        match code {
            DHT => return Ok(Cow::Borrowed(frame)),
            SOS => {
                let mut out = Vec::with_capacity(frame.len() + DHT_SEGMENT.len());
                out.extend_from_slice(&frame[..start]);
                out.extend_from_slice(&DHT_SEGMENT);
                out.extend_from_slice(&frame[start..]);
                return Ok(Cow::Owned(out));
            }
            EOI => return Err("it ends before its first scan".to_string()),
            0x00 => return Err(format!("a stuffed 0xFF00 at byte {start}, outside a scan")),
            // TEM and RST0 to RST7 stand alone, with no length.
            0x01 | 0xD0..=0xD7 => {}
            _ => {
                let Some(length) = frame.get(at..at + 2) else {
                    return Err("it ends inside a segment's length".to_string());
                };
                let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
                if length < 2 {
                    return Err(format!("a segment at byte {start} is {length} bytes long"));
                }
                at += length;
                if at > frame.len() {
                    return Err("it ends inside a segment".to_string());
                }
            }
        }
    }
}

/// A YUYV frame as interleaved full-range YCbCr, 3 bytes a pixel: each pair
/// of pixels shares its U and V, which both of them get. Rows may be padded
/// past `width`: the stride is the frame's length over `height`.
///
/// UVC cameras send studio-range YUYV (BT.601: Y from 16 to 235, U and V
/// from 16 to 240) and JPEG's YCbCr is full range, so each is stretched to
/// 0 to 255; copied as they are, black would come out grey.
pub fn yuyv_to_ycbcr(frame: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let (width, height) = (width as usize, height as usize);
    if width == 0 || height == 0 {
        return Err(format!("a frame of {width}x{height}"));
    }
    let stride = frame.len() / height;
    let row = width.div_ceil(2) * 4;
    if stride < row {
        return Err(format!(
            "{} bytes is short of a {width}x{height} YUYV frame",
            frame.len()
        ));
    }
    let mut out = Vec::with_capacity(width * height * 3);
    for line in frame.chunks_exact(stride).take(height) {
        for x in 0..width {
            let pair = &line[x / 2 * 4..x / 2 * 4 + 4];
            let y = if x % 2 == 0 { pair[0] } else { pair[2] };
            out.push(full_luma(y));
            out.push(full_chroma(pair[1]));
            out.push(full_chroma(pair[3]));
        }
    }
    Ok(out)
}

/// 16..=235 to 0..=255, rounded.
fn full_luma(y: u8) -> u8 {
    let y = (i32::from(y) - 16) * 255;
    ((y + 219 / 2).div_euclid(219)).clamp(0, 255) as u8
}

/// 16..=240 around 128 to 0..=255 around 128, rounded.
fn full_chroma(c: u8) -> u8 {
    let c = (i32::from(c) - 128) * 255;
    (128 + (c + 224 / 2).div_euclid(224)).clamp(0, 255) as u8
}

/// A YUYV frame as a JPEG, 4:2:2 like the frame, at `QUALITY`.
pub fn encode_yuyv(frame: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let (Ok(w), Ok(h)) = (u16::try_from(width), u16::try_from(height)) else {
        return Err(format!("{width}x{height} is larger than a JPEG can be"));
    };
    let ycbcr = yuyv_to_ycbcr(frame, width, height)?;
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, QUALITY);
    encoder.set_sampling_factor(SamplingFactor::R_4_2_2);
    encoder
        .encode(&ycbcr, w, h, ColorType::Ycbcr)
        .map_err(|err| err.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The segments before the first scan, as (marker code, whole segment).
    fn segments(jpeg: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 2;
        loop {
            while jpeg[at + 1] == 0xFF {
                at += 1;
            }
            let code = jpeg[at + 1];
            if code == SOS {
                return out;
            }
            let length = usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
            out.push((code, jpeg[at..at + 2 + length].to_vec()));
            at += 2 + length;
        }
    }

    /// `jpeg` with every DHT segment taken out, the way a UVC camera sends it.
    fn without_dht(jpeg: &[u8]) -> Vec<u8> {
        let mut out = jpeg[..2].to_vec();
        let mut kept = 2;
        for (code, segment) in segments(jpeg) {
            if code != DHT {
                out.extend_from_slice(&segment);
            }
            kept += segment.len();
        }
        out.extend_from_slice(&jpeg[kept..]);
        out
    }

    /// A small YUYV frame with a gradient, so the scan is not all zeros.
    fn yuyv(width: u32, height: u32) -> Vec<u8> {
        let mut frame = Vec::new();
        for y in 0..height {
            for x in 0..width / 2 {
                let luma = (16 + (x * 8 + y * 4) % 220) as u8;
                frame.extend_from_slice(&[luma, 90, luma.saturating_add(3), 200]);
            }
        }
        frame
    }

    #[test]
    fn the_standard_tables_are_one_segment_of_420_bytes() {
        assert_eq!(DHT_SEGMENT.len(), 420);
        assert_eq!(&DHT_SEGMENT[..4], &[0xFF, 0xC4, 0x01, 0xA2]);
    }

    #[test]
    fn the_tables_are_the_ones_an_encoder_writes() {
        // jpeg-encoder writes the same Annex K tables, one DHT segment each.
        let jpeg = encode_yuyv(&yuyv(16, 8), 16, 8).unwrap();
        let payloads: Vec<u8> = segments(&jpeg)
            .into_iter()
            .filter(|(code, _)| *code == DHT)
            .flat_map(|(_, segment)| segment[4..].to_vec())
            .collect();
        assert_eq!(payloads, &DHT_SEGMENT[4..]);
    }

    #[test]
    fn a_frame_with_tables_is_left_as_it_is() {
        let jpeg = encode_yuyv(&yuyv(16, 8), 16, 8).unwrap();
        let fixed = with_huffman(&jpeg).unwrap();
        assert!(matches!(fixed, Cow::Borrowed(_)));
        assert_eq!(&*fixed, &jpeg[..]);
    }

    #[test]
    fn a_frame_without_gets_them_once_before_its_scan() {
        let jpeg = encode_yuyv(&yuyv(16, 8), 16, 8).unwrap();
        let bare = without_dht(&jpeg);
        assert!(segments(&bare).iter().all(|(code, _)| *code != DHT));

        let fixed = with_huffman(&bare).unwrap().into_owned();
        assert_eq!(fixed.len(), bare.len() + DHT_SEGMENT.len());
        let found = segments(&fixed);
        let dht: Vec<_> = found.iter().filter(|(code, _)| *code == DHT).collect();
        assert_eq!(dht.len(), 1);
        assert_eq!(dht[0].1, DHT_SEGMENT);
        // Last before the scan, and everything else as it was.
        assert_eq!(found.last().unwrap().0, DHT);
        let scan = bare.windows(2).position(|w| w == [0xFF, SOS]).unwrap();
        assert_eq!(&fixed[..scan], &bare[..scan]);
        assert_eq!(&fixed[scan..scan + DHT_SEGMENT.len()], &DHT_SEGMENT);
        assert_eq!(&fixed[scan + DHT_SEGMENT.len()..], &bare[scan..]);
    }

    #[test]
    fn app_and_comment_segments_and_fill_bytes_are_walked_past() {
        let jpeg = encode_yuyv(&yuyv(16, 8), 16, 8).unwrap();
        let bare = without_dht(&jpeg);
        let mut frame = vec![0xFF, SOI];
        // AVI1, as UVC cameras put in APP0, a comment, and fill bytes.
        frame.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x08, b'A', b'V', b'I', b'1', 0, 0]);
        frame.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x05, 0xFF, 0xDA, 0x00]);
        frame.extend_from_slice(&[0xFF, 0xFF]);
        frame.extend_from_slice(&bare[2..]);
        let fixed = with_huffman(&frame).unwrap();
        let dht = segments(&fixed)
            .into_iter()
            .filter(|(code, _)| *code == DHT)
            .count();
        assert_eq!(dht, 1);
        assert_eq!(fixed.len(), frame.len() + DHT_SEGMENT.len());
    }

    #[test]
    fn a_broken_frame_is_an_error() {
        let jpeg = encode_yuyv(&yuyv(16, 8), 16, 8).unwrap();
        let bare = without_dht(&jpeg);
        // Cut at every length up to its scan: none may panic or pass.
        let scan = bare.windows(2).position(|w| w == [0xFF, SOS]).unwrap();
        for cut in 0..scan {
            assert!(with_huffman(&bare[..cut]).is_err(), "cut at {cut}");
        }
        for garbage in [
            &b""[..],
            b"\xff",
            b"\xff\xd8",
            b"\xff\xd8\x00\x00",
            b"\xff\xd8\xff",
            b"\xff\xd8\xff\xe0\x00\x01",
            b"\xff\xd8\xff\xe0\xff\xff",
            b"\xff\xd8\xff\xd9",
            b"\xff\xd8\xff\x00",
            b"not a jpeg at all",
        ] {
            assert!(with_huffman(garbage).is_err(), "{garbage:?}");
        }
    }

    #[test]
    fn yuyv_becomes_full_range_ycbcr_a_pair_at_a_time() {
        // One row of 2 pixel pairs: black and white, then mid grey and a red.
        let frame = [16, 128, 235, 128, 126, 90, 126, 240];
        let out = yuyv_to_ycbcr(&frame, 4, 1).unwrap();
        assert_eq!(
            out,
            [0, 128, 128, 255, 128, 128, 128, 85, 255, 128, 85, 255]
        );
    }

    #[test]
    fn padded_rows_and_short_frames() {
        // 2x2 with 4 bytes of padding at the end of each row.
        let frame = [16, 128, 235, 128, 0, 0, 0, 0, 235, 128, 16, 128, 0, 0, 0, 0];
        let out = yuyv_to_ycbcr(&frame, 2, 2).unwrap();
        assert_eq!(out.len(), 2 * 2 * 3);
        assert_eq!(out[0], 0);
        assert_eq!(out[3], 255);
        assert_eq!(out[6], 255);
        assert_eq!(out[9], 0);
        assert!(yuyv_to_ycbcr(&frame[..7], 2, 2).is_err());
        assert!(yuyv_to_ycbcr(&frame, 0, 2).is_err());
    }

    #[test]
    fn odd_widths_take_the_last_pairs_first_pixel() {
        let frame = [16, 100, 235, 150, 60, 110, 70, 160];
        let out = yuyv_to_ycbcr(&frame, 3, 1).unwrap();
        assert_eq!(out.len(), 9);
        assert_eq!(out[6], full_luma(60));
        assert_eq!(out[7], full_chroma(110));
    }

    #[test]
    fn range_ends_clamp() {
        assert_eq!(full_luma(0), 0);
        assert_eq!(full_luma(255), 255);
        assert_eq!(full_chroma(0), 0);
        assert_eq!(full_chroma(255), 255);
        // 16 is -127.5 from the middle, which rounds up.
        assert_eq!(full_chroma(16), 1);
        assert_eq!(full_chroma(240), 255);
    }

    #[test]
    fn a_yuyv_frame_encodes_to_a_whole_jpeg() {
        let frame = yuyv(64, 48);
        let jpeg = jpeg(YUYV, &frame, 64, 48).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, SOI]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, EOI]);
        assert!(encode_yuyv(&[0; 10], 64, 48).is_err());
        assert!(encode_yuyv(&[0; 4], 70_000, 1).is_err());
    }

    #[test]
    fn a_snapshot_is_written_only_while_asked_for() {
        let dir = std::env::temp_dir().join(format!("tessaro-camera-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut snapshots = Snapshots::new(&dir, "video9");
        snapshots.check();
        assert!(!snapshots.due());

        fs::write(dir.join("video9.want"), b"").unwrap();
        snapshots.checked = None;
        snapshots.check();
        assert!(snapshots.due());
        snapshots.write(YUYV, &yuyv(16, 8), 16, 8);
        assert!(!snapshots.due(), "at most one every {EVERY:?}");
        let written = fs::read(dir.join("video9.jpg")).unwrap();
        assert_eq!(&written[..2], &[0xFF, SOI]);

        snapshots.remove();
        snapshots.checked = None;
        snapshots.check();
        assert!(!snapshots.due());
        assert!(!dir.join("video9.jpg").exists());
        assert!(!dir.join("video9.want").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
