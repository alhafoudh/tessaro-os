//! A screenshot made smaller: Chromium's `Page.captureScreenshot` scales
//! its clip only from the compositor's surface (`fromSurface: true`), which
//! can hang on this stack, so a screenshot is captured whole and scaled
//! here - for a broker or a slow link (`screen screenshot --width`,
//! mqtt.screenshot.width). CPU work: callers run it on `blocking()`.
//!
//! The decoder does most of it from the DCT (1/2, 1/4, 1/8 of the size, as
//! the vision service does with camera frames), an area average the rest.

use std::io::Cursor;

use jpeg_decoder::{Decoder, PixelFormat};
use jpeg_encoder::{ColorType, Encoder};

/// `jpeg` scaled down to `width` pixels across, its height in proportion,
/// encoded at `quality`. One no wider than `width` comes back as it is.
pub fn scale(jpeg: &[u8], width: u32, quality: u8) -> Result<Vec<u8>, String> {
    let mut decoder = Decoder::new(Cursor::new(jpeg));
    decoder
        .read_info()
        .map_err(|err| format!("the screenshot: {err}"))?;
    let info = decoder.info().ok_or("the screenshot has no JPEG header")?;
    let (across, down) = (u32::from(info.width), u32::from(info.height));
    if across <= width || width == 0 {
        return Ok(jpeg.to_vec());
    }
    let height = (u64::from(down) * u64::from(width) / u64::from(across)).max(1) as u32;
    // The smallest DCT scale that is still at least the size wanted.
    decoder
        .scale(width as u16, height as u16)
        .map_err(|err| format!("the screenshot: {err}"))?;
    let pixels = decoder
        .decode()
        .map_err(|err| format!("the screenshot: {err}"))?;
    let info = decoder.info().ok_or("the screenshot has no JPEG header")?;
    let (from_w, from_h) = (usize::from(info.width), usize::from(info.height));
    let rgb = match info.pixel_format {
        PixelFormat::RGB24 => pixels,
        PixelFormat::L8 => pixels
            .iter()
            .flat_map(|&value| [value, value, value])
            .collect(),
        other => return Err(format!("a screenshot in {other:?}, not RGB")),
    };
    if rgb.len() < from_w * from_h * 3 {
        return Err("the screenshot decoded short".to_string());
    }
    let scaled = average(&rgb, from_w, from_h, width as usize, height as usize);
    let mut out = Vec::new();
    Encoder::new(&mut out, quality.clamp(10, 100))
        .encode(&scaled, width as u16, height as u16, ColorType::Rgb)
        .map_err(|err| format!("the scaled screenshot: {err}"))?;
    Ok(out)
}

/// `rgb`, `from_w` by `from_h`, down to `to_w` by `to_h`: each pixel the
/// average of the source pixels it covers.
fn average(rgb: &[u8], from_w: usize, from_h: usize, to_w: usize, to_h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(to_w * to_h * 3);
    for y in 0..to_h {
        let (top, bottom) = (
            y * from_h / to_h,
            ((y + 1) * from_h / to_h).max(y * from_h / to_h + 1),
        );
        for x in 0..to_w {
            let (left, right) = (
                x * from_w / to_w,
                ((x + 1) * from_w / to_w).max(x * from_w / to_w + 1),
            );
            let mut sum = [0u32; 3];
            for row in top..bottom.min(from_h) {
                for column in left..right.min(from_w) {
                    let at = (row * from_w + column) * 3;
                    for (total, value) in sum.iter_mut().zip(&rgb[at..at + 3]) {
                        *total += u32::from(*value);
                    }
                }
            }
            let count = ((bottom.min(from_h) - top) * (right.min(from_w) - left)).max(1) as u32;
            out.extend(sum.iter().map(|total| (total / count) as u8));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let rgb: Vec<u8> = (0..usize::from(width) * usize::from(height))
            .flat_map(|at| [(at % 251) as u8, 128, 64])
            .collect();
        let mut out = Vec::new();
        Encoder::new(&mut out, 90)
            .encode(&rgb, width, height, ColorType::Rgb)
            .unwrap();
        out
    }

    fn size(jpeg: &[u8]) -> (u16, u16) {
        let mut decoder = Decoder::new(Cursor::new(jpeg));
        decoder.read_info().unwrap();
        let info = decoder.info().unwrap();
        (info.width, info.height)
    }

    #[test]
    fn a_wide_screenshot_comes_back_as_wide_as_asked() {
        assert_eq!(size(&scale(&jpeg(800, 600), 320, 60).unwrap()), (320, 240));
        assert_eq!(
            size(&scale(&jpeg(1920, 1080), 640, 60).unwrap()),
            (640, 360)
        );
        // Not a DCT scale: the average does all of it.
        assert_eq!(size(&scale(&jpeg(1000, 500), 999, 60).unwrap()), (999, 499));
    }

    #[test]
    fn a_narrow_one_is_left_as_it_is() {
        let shot = jpeg(320, 240);
        assert_eq!(scale(&shot, 640, 60).unwrap(), shot);
        assert_eq!(scale(&shot, 0, 60).unwrap(), shot);
    }

    #[test]
    fn the_average_keeps_a_flat_colour() {
        let flat = [10u8, 20, 30].repeat(4 * 4);
        assert_eq!(average(&flat, 4, 4, 2, 2), [10u8, 20, 30].repeat(4));
    }
}
