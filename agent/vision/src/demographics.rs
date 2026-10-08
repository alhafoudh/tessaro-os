//! Age and gender from a face, with camera.presence.demographics on: HSE
//! FaceRes, the weights `@vladmandic/human` runs by default (`faceres.ts`),
//! converted to ONNX and run by tract like the detector.
//!
//! The network takes the face as a 224x224 RGB square from 0 to 255, as Human
//! feeds it. It answers how likely the face is a man's (a sigmoid), the age
//! as 100 classes of a year each, and a 1024-value descriptor of the face.
//! The descriptor is what face recognition compares faces by: it is never
//! read, so it never leaves this process.
//!
//! The age is the classes' expected value, not Human's reading (the likeliest
//! year moved towards a neighbour, `predict` in `faceres.ts`): the classes
//! peak at years ending in 9, a habit of the photos it learnt from, so the
//! likeliest year jumps by ten between two looks at the same face where the
//! expected value moves by a year or two.
//!
//! The square is cut around BlazeFace's box with a margin and turned so the
//! eyes are level, as Human straightens a face before it. The margin is the
//! one that told the test photos' men and women apart best
//! (`tests/demographics.rs`).

use std::path::Path;

use protocol::presence::{FaceBox, Point};
use tract_onnx::prelude::*;

use crate::picture::{self, Rgb};

type Plan = std::sync::Arc<TypedRunnableModel>;

/// The square the network takes, in pixels.
pub const INPUT: usize = 224;

pub const FILE: &str = "faceres.onnx";

/// The square cut out is this many times BlazeFace's box.
const MARGIN: f32 = 1.8;

/// A face narrower than this many pixels in the camera's frame is too small
/// to tell anything from: its square would be mostly made up.
pub const MIN_FACE: f32 = 48.0;

/// The box's side in pixels of the decoded picture that fills the network's
/// input without stretching it.
pub const WANTED_FACE: f32 = INPUT as f32 / MARGIN;

/// What one look at a face says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Estimated age in years.
    pub age: f64,
    /// How likely the face is a man's, 0 to 1.
    pub male: f64,
}

pub struct Classifier {
    plan: Plan,
}

impl Classifier {
    /// `<dir>/faceres.onnx`, optimised for this CPU once at start.
    pub fn load(dir: &Path) -> Result<Classifier, String> {
        let path = dir.join(FILE);
        let plan = tract_onnx::onnx()
            .model_for_path(&path)
            .and_then(|graph| graph.with_input_fact(0, f32::fact([1, INPUT, INPUT, 3]).into()))
            .and_then(|graph| graph.into_optimized())
            .and_then(|graph| graph.into_runnable())
            .map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(Classifier { plan })
    }

    /// One look at a face cut out by `square`.
    pub fn look(&self, square: Vec<f32>) -> Result<Look, String> {
        let input =
            Tensor::from_shape(&[1, INPUT, INPUT, 3], &square).map_err(|err| err.to_string())?;
        let outputs = self
            .plan
            .run(tvec!(input.into()))
            .map_err(|err| err.to_string())?;
        // Told apart by size rather than trusting the order; the descriptor
        // is skipped.
        let (mut male, mut ages) = (None, None);
        for output in &outputs {
            let values = || -> Result<Vec<f32>, String> {
                Ok(output
                    .to_plain_array_view::<f32>()
                    .map_err(|err| err.to_string())?
                    .iter()
                    .copied()
                    .collect())
            };
            match output.shape().last() {
                Some(1) => male = values()?.first().copied(),
                Some(100) => ages = Some(values()?),
                _ => {}
            }
        }
        let (Some(male), Some(ages)) = (male, ages) else {
            return Err("the network's outputs are not FaceRes's".into());
        };
        if !male.is_finite() || ages.iter().any(|p| !p.is_finite()) {
            return Err("FaceRes answered nothing usable".into());
        }
        Ok(Look {
            age: age(&ages),
            male: f64::from(male.clamp(0.0, 1.0)),
        })
    }
}

/// The age the 100 classes add up to: each year weighted by its share.
fn age(classes: &[f32]) -> f64 {
    let total: f64 = classes.iter().map(|&p| f64::from(p)).sum();
    if total <= 0.0 {
        return 0.0;
    }
    classes
        .iter()
        .enumerate()
        .map(|(year, &p)| year as f64 * f64::from(p))
        .sum::<f64>()
        / total
}

/// The face's box side in pixels of a frame `width` x `height`.
pub fn face_pixels(area: &FaceBox, width: usize, height: usize) -> f32 {
    (area.w * width as f64).max(area.h * height as f64) as f32
}

/// The longer side to decode a frame at so a face of `area` fills the
/// network's input, for `jpeg` and `yuyv`'s `side`.
pub fn decode_side(area: &FaceBox, width: usize, height: usize) -> usize {
    let longer = width.max(height) as f32;
    let face = face_pixels(area, width, height).max(1.0);
    (longer * WANTED_FACE / face).ceil() as usize
}

/// The face of `area` cut out of `picture`, level, as the network's square
/// from 0 to 255. `keypoints` are BlazeFace's, the eyes first.
pub fn square(picture: &Rgb, area: &FaceBox, keypoints: &[Point; 6]) -> Vec<f32> {
    let (w, h) = (picture.width as f32, picture.height as f32);
    let center = [
        (area.x + area.w / 2.0) as f32 * w,
        (area.y + area.h / 2.0) as f32 * h,
    ];
    let side = face_pixels(area, picture.width, picture.height) * MARGIN;
    // The person's right eye is on the picture's left: the line from it to
    // the left eye is level when the head is.
    let [right, left] = [keypoints[0], keypoints[1]];
    let dx = (left[0] - right[0]) as f32 * w;
    let dy = (left[1] - right[1]) as f32 * h;
    let angle = if dx.abs() + dy.abs() > 0.0 {
        dy.atan2(dx)
    } else {
        0.0
    };
    picture::crop(picture, center, side, angle, INPUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_face_asks_for_a_bigger_decode() {
        let area = FaceBox {
            x: 0.4,
            y: 0.4,
            w: 0.05,
            h: 0.0889,
        };
        // 96 px wide in 1920x1080 would need 2489 across to fill the input:
        // more than the frame has, so it is decoded whole.
        assert!((face_pixels(&area, 1920, 1080) - 96.0).abs() < 0.1);
        assert_eq!(decode_side(&area, 1920, 1080), 2489);
    }

    #[test]
    fn the_age_is_the_expected_year() {
        let mut classes = vec![0.0f32; 100];
        classes[29] = 0.5;
        classes[39] = 0.5;
        assert!((age(&classes) - 34.0).abs() < 1e-5);
        assert_eq!(age(&[0.0; 100]), 0.0);
    }

    #[test]
    fn a_level_face_is_cut_unturned() {
        let picture = Rgb {
            width: 8,
            height: 8,
            pixels: (0..64).flat_map(|i| [(i % 8 * 30) as u8; 3]).collect(),
        };
        let area = FaceBox {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
        };
        let eyes = [
            [0.3, 0.4],
            [0.7, 0.4],
            [0.5; 2],
            [0.5; 2],
            [0.2, 0.4],
            [0.8, 0.4],
        ];
        let out = square(&picture, &area, &eyes);
        assert_eq!(out.len(), INPUT * INPUT * 3);
        // Brighter to the right along the top row, as in the picture.
        assert!(out[0] < out[(INPUT - 1) * 3]);
    }
}
