//! The network itself: a BlazeFace ONNX file run by tract, from a picture to
//! the faces in it as shares of the frame.

use std::path::Path;

use protocol::presence::{FaceBox, Point};
use tract_onnx::prelude::*;

use crate::blazeface::{self, Anchor, Model};
use crate::picture::{self, Rgb};

type Plan = std::sync::Arc<TypedRunnableModel>;

pub struct Detector {
    pub model: Model,
    plan: Plan,
    anchors: Vec<Anchor>,
}

/// A face as a share of the frame: its box, score and keypoints.
pub type Face = (FaceBox, f64, [Point; 6]);

impl Detector {
    /// `<dir>/<model.file>`, optimised for this CPU once at start.
    pub fn load(dir: &Path, model: Model) -> Result<Detector, String> {
        let path = dir.join(model.file);
        let shape = [1, model.size, model.size, 3];
        let plan = tract_onnx::onnx()
            .model_for_path(&path)
            .and_then(|graph| graph.with_input_fact(0, f32::fact(shape).into()))
            .and_then(|graph| graph.into_optimized())
            .and_then(|graph| graph.into_runnable())
            .map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(Detector {
            model,
            plan,
            anchors: blazeface::anchors(&model),
        })
    }

    /// The faces in `picture` scoring at least `floor`, best first, merged.
    pub fn detect(&self, picture: &Rgb, floor: f32) -> Result<Vec<Face>, String> {
        let side = self.model.size;
        let (input, letterbox) = picture::tensor(picture, side);
        let input =
            Tensor::from_shape(&[1, side, side, 3], &input).map_err(|err| err.to_string())?;
        let outputs = self
            .plan
            .run(tvec!(input.into()))
            .map_err(|err| err.to_string())?;
        // regressors is [1, N, 16] and classificators [1, N, 1]; tell them
        // apart by shape rather than trust the order.
        let (mut boxes, mut scores) = (None, None);
        for output in &outputs {
            let values: Vec<f32> = output
                .to_plain_array_view::<f32>()
                .map_err(|err| err.to_string())?
                .iter()
                .copied()
                .collect();
            match output.shape().last() {
                Some(&blazeface::COORDS) => boxes = Some(values),
                Some(1) => scores = Some(values),
                _ => {}
            }
        }
        let (Some(boxes), Some(scores)) = (boxes, scores) else {
            return Err("the model's outputs are not BlazeFace's".into());
        };
        let found = blazeface::decode(&self.model, &self.anchors, &boxes, &scores, floor);
        Ok(blazeface::suppress(found)
            .into_iter()
            .map(|face| {
                let [x, y] = letterbox.unmap(face.cx - face.w / 2.0, face.cy - face.h / 2.0);
                let area = FaceBox {
                    x: f64::from(x),
                    y: f64::from(y),
                    w: f64::from(face.w / letterbox.w),
                    h: f64::from(face.h / letterbox.h),
                };
                let keypoints = face.keypoints.map(|[kx, ky]| {
                    let [x, y] = letterbox.unmap(kx, ky);
                    [f64::from(x), f64::from(y)]
                });
                (area, f64::from(face.score), keypoints)
            })
            .collect())
    }
}
