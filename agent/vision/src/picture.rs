//! A camera frame as the networks' input: decoded no larger than needed,
//! then letterboxed into the detector's square with its aspect ratio kept, as
//! MediaPipe's `ImageToTensorCalculator` does (`keep_aspect_ratio`, zero
//! border), with values from -1 to 1; or a face cut out of it, level, for
//! the age and gender estimators.

use std::io::Cursor;

use jpeg_decoder::{Decoder, PixelFormat};

/// RGB, 3 bytes a pixel, row by row.
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// An MJPEG frame decoded at the smallest of 1/1, 1/2, 1/4 and 1/8 of its
/// size that still covers `side` on its longer side: jpeg-decoder scales in
/// the IDCT, so a 1080p frame for a 192 pixel input costs an eighth of a
/// full decode. A UVC frame without Huffman tables gets the standard ones
/// first, by the mirror's own snapshot code: jpeg-decoder fills them in only
/// for frames that carry an AVI1 marker, which many cameras leave out.
pub fn jpeg(frame: &[u8], side: usize) -> Result<Rgb, String> {
    let frame = tessaro_camera::snapshot::with_huffman(frame)?;
    let mut decoder = Decoder::new(Cursor::new(&*frame));
    decoder.read_info().map_err(|err| err.to_string())?;
    let info = decoder.info().ok_or("no JPEG header")?;
    let (width, height) = (usize::from(info.width), usize::from(info.height));
    let longer = width.max(height).max(1);
    let shrink = longer / side.max(1);
    let wanted = |full: usize| (full / shrink.max(1)).max(1) as u16;
    decoder
        .scale(wanted(width), wanted(height))
        .map_err(|err| err.to_string())?;
    let pixels = decoder.decode().map_err(|err| err.to_string())?;
    let info = decoder.info().ok_or("no JPEG header")?;
    let (width, height) = (usize::from(info.width), usize::from(info.height));
    let pixels = match info.pixel_format {
        PixelFormat::RGB24 => pixels,
        PixelFormat::L8 => pixels.iter().flat_map(|&v| [v, v, v]).collect(),
        other => return Err(format!("a JPEG in {other:?}, not RGB")),
    };
    if pixels.len() < width * height * 3 {
        return Err("a JPEG shorter than its size".into());
    }
    Ok(Rgb {
        width,
        height,
        pixels,
    })
}

/// A YUYV frame, every `step`th pixel of every `step`th row converted to
/// RGB, with `step` the largest that still covers `side`.
pub fn yuyv(frame: &[u8], width: usize, height: usize, side: usize) -> Result<Rgb, String> {
    if frame.len() < width * height * 2 {
        return Err(format!(
            "a YUYV frame of {} bytes for {width}x{height}",
            frame.len()
        ));
    }
    let step = (width.max(height) / side.max(1)).max(1);
    let (out_w, out_h) = (width.div_ceil(step), height.div_ceil(step));
    let mut pixels = Vec::with_capacity(out_w * out_h * 3);
    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            // Two pixels share their chroma: Y0 U Y1 V.
            let pair = (y * width + (x & !1)) * 2;
            let luma = frame[(y * width + x) * 2];
            let (u, v) = (frame[pair + 1], frame[pair + 3]);
            pixels.extend(rgb(luma, u, v));
        }
    }
    Ok(Rgb {
        width: out_w,
        height: out_h,
        pixels,
    })
}

/// BT.601 studio range, which is what UVC cameras send.
fn rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = (f32::from(y) - 16.0) * 1.164;
    let d = f32::from(u) - 128.0;
    let e = f32::from(v) - 128.0;
    let clamp = |value: f32| value.round().clamp(0.0, 255.0) as u8;
    [
        clamp(c + 1.596 * e),
        clamp(c - 0.392 * d - 0.813 * e),
        clamp(c + 2.017 * d),
    ]
}

/// Where the picture sits in the square: its scale and the border on each
/// side, so a face found in the square maps back onto the frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Letterbox {
    /// The picture's width and height as shares of the square's side.
    pub w: f32,
    pub h: f32,
    /// The border left of and above it, as shares of the side.
    pub x: f32,
    pub y: f32,
}

impl Letterbox {
    pub fn of(width: usize, height: usize) -> Letterbox {
        let longer = width.max(height).max(1) as f32;
        let (w, h) = (width as f32 / longer, height as f32 / longer);
        Letterbox {
            w,
            h,
            x: (1.0 - w) / 2.0,
            y: (1.0 - h) / 2.0,
        }
    }

    /// A point in the square as shares of the frame.
    pub fn unmap(&self, x: f32, y: f32) -> [f32; 2] {
        [(x - self.x) / self.w, (y - self.y) / self.h]
    }
}

/// `picture` letterboxed into a `side` square, bilinear, as the NHWC floats
/// from -1 to 1 the network takes.
pub fn tensor(picture: &Rgb, side: usize) -> (Vec<f32>, Letterbox) {
    let letterbox = Letterbox::of(picture.width, picture.height);
    let mut out = vec![0.0f32; side * side * 3];
    let (left, top) = (letterbox.x * side as f32, letterbox.y * side as f32);
    let (inner_w, inner_h) = (letterbox.w * side as f32, letterbox.h * side as f32);
    let scale_x = picture.width as f32 / inner_w;
    let scale_y = picture.height as f32 / inner_h;
    for ty in 0..side {
        let fy = (ty as f32 + 0.5 - top) * scale_y - 0.5;
        if fy < -0.5 || fy > picture.height as f32 - 0.5 {
            continue;
        }
        for tx in 0..side {
            let fx = (tx as f32 + 0.5 - left) * scale_x - 0.5;
            if fx < -0.5 || fx > picture.width as f32 - 0.5 {
                continue;
            }
            let rgb = bilinear(picture, fx, fy);
            for (c, value) in rgb.into_iter().enumerate() {
                out[(ty * side + tx) * 3 + c] = value / 127.5 - 1.0;
            }
        }
    }
    (out, letterbox)
}

/// The colour at a point between pixel centres, 0 to 255, from the four
/// pixels around it; a point past the edge takes the edge's.
fn bilinear(picture: &Rgb, fx: f32, fy: f32) -> [f32; 3] {
    let (last_x, last_y) = (picture.width - 1, picture.height - 1);
    let fx = fx.clamp(0.0, last_x as f32);
    let fy = fy.clamp(0.0, last_y as f32);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(last_x), (y0 + 1).min(last_y));
    let (wx, wy) = (fx - x0 as f32, fy - y0 as f32);
    let at =
        |x: usize, y: usize, c: usize| f32::from(picture.pixels[(y * picture.width + x) * 3 + c]);
    std::array::from_fn(|c| {
        let top = at(x0, y0, c) * (1.0 - wx) + at(x1, y0, c) * wx;
        let bottom = at(x0, y1, c) * (1.0 - wx) + at(x1, y1, c) * wx;
        top * (1.0 - wy) + bottom * wy
    })
}

/// A square of `picture` resized to `size`, bilinear, as NHWC floats from 0
/// to 255: centred on `center` (in pixels), `side` pixels across and turned
/// by `angle` radians, so a tilted head comes out level. A part past the
/// picture's edge repeats the edge.
pub fn crop(picture: &Rgb, center: [f32; 2], side: f32, angle: f32, size: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(size * size * 3);
    let step = side / size as f32;
    let (sin, cos) = angle.sin_cos();
    for ty in 0..size {
        let dy = (ty as f32 + 0.5) * step - side / 2.0;
        for tx in 0..size {
            let dx = (tx as f32 + 0.5) * step - side / 2.0;
            let fx = center[0] + dx * cos - dy * sin - 0.5;
            let fy = center[1] + dx * sin + dy * cos - 0.5;
            out.extend(bilinear(picture, fx, fy));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_frame_is_centred_with_bars_above_and_below() {
        let letterbox = Letterbox::of(1920, 1080);
        assert_eq!(letterbox.w, 1.0);
        assert!((letterbox.h - 0.5625).abs() < 1e-6);
        assert!((letterbox.y - 0.21875).abs() < 1e-6);
        // The square's centre is the frame's, its picture's top the frame's.
        assert_eq!(letterbox.unmap(0.5, 0.5), [0.5, 0.5]);
        assert!(letterbox.unmap(0.5, letterbox.y)[1].abs() < 1e-6);
    }

    #[test]
    fn the_border_is_black_and_the_picture_white() {
        let picture = Rgb {
            width: 4,
            height: 2,
            pixels: vec![255; 4 * 2 * 3],
        };
        let (tensor, _) = tensor(&picture, 8);
        // Row 0 is border, which stays 0.
        assert_eq!(tensor[0], 0.0);
        // The middle rows are the picture: 255 is 1.
        assert!((tensor[(4 * 8 + 4) * 3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn yuyv_grey_stays_grey_and_is_subsampled() {
        // 4x2 of Y=126 with neutral chroma.
        let frame: Vec<u8> = [126u8, 128].repeat(4 * 2);
        let picture = yuyv(&frame, 4, 2, 2).unwrap();
        assert_eq!((picture.width, picture.height), (2, 1));
        let [r, g, b] = [picture.pixels[0], picture.pixels[1], picture.pixels[2]];
        assert_eq!((r, g), (g, b));
        assert!((r as i32 - 128).abs() <= 1);
    }

    /// 4x4, black on the left half and white on the right.
    fn halves() -> Rgb {
        let pixels = (0..16)
            .flat_map(|i| [if i % 4 < 2 { 0 } else { 255 }; 3])
            .collect();
        Rgb {
            width: 4,
            height: 4,
            pixels,
        }
    }

    #[test]
    fn a_crop_keeps_what_is_where() {
        let out = crop(&halves(), [2.0, 2.0], 4.0, 0.0, 4);
        // Same size and place: pixel for pixel.
        assert_eq!(out[0], 0.0);
        assert_eq!(out[3 * 3], 255.0);
        assert_eq!(out.len(), 4 * 4 * 3);
    }

    #[test]
    fn a_turned_crop_turns_the_picture() {
        // The crop's x axis runs along `angle` in the picture: a quarter
        // turn reads the picture top to bottom, so its right half (white)
        // comes out on top.
        let out = crop(&halves(), [2.0, 2.0], 4.0, std::f32::consts::FRAC_PI_2, 4);
        let at = |x: usize, y: usize| out[(y * 4 + x) * 3];
        assert_eq!(at(0, 0), 255.0);
        assert_eq!(at(3, 0), 255.0);
        assert_eq!(at(0, 3), 0.0);
    }

    #[test]
    fn a_crop_past_the_edge_repeats_it() {
        let out = crop(&halves(), [4.0, 2.0], 4.0, 0.0, 4);
        assert_eq!(out[(4 - 1) * 3], 255.0);
    }

    #[test]
    fn a_short_yuyv_frame_is_refused() {
        assert!(yuyv(&[0; 10], 4, 2, 2).is_err());
    }
}
