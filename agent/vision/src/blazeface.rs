//! MediaPipe's BlazeFace around the network: the anchors its boxes are
//! relative to, decoding its raw output into faces, and the weighted
//! non-maximum suppression that merges the many boxes one face gets.
//!
//! The numbers are MediaPipe's own, from the graphs that run these models
//! (`face_detection_short_range.pbtxt` and `face_detection_full_range.pbtxt`
//! at v0.8.9: `SsdAnchorsCalculatorOptions`,
//! `TensorsToDetectionsCalculatorOptions` and
//! `NonMaxSuppressionCalculatorOptions`). `@vladmandic/human` does the same
//! in `blazeface.ts`, which is MIT.

/// One of the models camera.presence.model names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub name: &'static str,
    /// The ONNX file in the models directory.
    pub file: &'static str,
    /// The square input the network takes, in pixels.
    pub size: usize,
    /// Feature map strides, MediaPipe's `strides`. Consecutive equal ones
    /// share one grid.
    strides: &'static [usize],
    /// Anchors per cell of each layer: the aspect ratio 1, plus one more at
    /// an interpolated scale when `interpolated_scale_aspect_ratio` is set.
    per_layer: usize,
}

/// `face_detection_full_range`, which Human calls blazeface-back: faces up
/// to about 5 m, 192x192, one anchor per cell of a 48x48 grid.
pub const FULL: Model = Model {
    name: "face-full",
    file: "face-full.onnx",
    size: 192,
    strides: &[4],
    per_layer: 1,
};

/// `face_detection_short_range`, Human's blazeface-front: faces up to about
/// 2 m, 128x128, two anchors per cell of a 16x16 and six of an 8x8 grid.
pub const SHORT: Model = Model {
    name: "face-short",
    file: "face-short.onnx",
    size: 128,
    strides: &[8, 16, 16, 16],
    per_layer: 2,
};

pub fn model(name: &str) -> Option<Model> {
    [FULL, SHORT].into_iter().find(|model| model.name == name)
}

/// The values per anchor in the regressor output: the box's centre and size,
/// then six keypoints, each x then y.
pub const COORDS: usize = 16;
pub const KEYPOINTS: usize = 6;

/// MediaPipe clips raw scores to this before the sigmoid.
const SCORE_CLIP: f32 = 100.0;

/// Boxes overlapping a better one by more than this are the same face.
const SUPPRESSION_IOU: f32 = 0.3;

/// An anchor's centre, as a share of the input. Every anchor of these
/// models has a fixed size of 1, so the centre is all there is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub x: f32,
    pub y: f32,
}

/// MediaPipe's `SsdAnchorsCalculator` with `fixed_anchor_size`: a grid per
/// run of equal strides, each cell's anchors at its centre.
pub fn anchors(model: &Model) -> Vec<Anchor> {
    let mut anchors = Vec::new();
    let mut layer = 0;
    while layer < model.strides.len() {
        let stride = model.strides[layer];
        let mut same = 0;
        while layer < model.strides.len() && model.strides[layer] == stride {
            same += 1;
            layer += 1;
        }
        let cells = model.size.div_ceil(stride);
        for y in 0..cells {
            for x in 0..cells {
                let anchor = Anchor {
                    x: (x as f32 + 0.5) / cells as f32,
                    y: (y as f32 + 0.5) / cells as f32,
                };
                anchors.extend(std::iter::repeat_n(anchor, same * model.per_layer));
            }
        }
    }
    anchors
}

/// A face in the network's input, every value a share of its side.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// Centre and size.
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    pub keypoints: [[f32; 2]; KEYPOINTS],
}

impl Found {
    fn iou(&self, other: &Found) -> f32 {
        let left = (self.cx - self.w / 2.0).max(other.cx - other.w / 2.0);
        let right = (self.cx + self.w / 2.0).min(other.cx + other.w / 2.0);
        let top = (self.cy - self.h / 2.0).max(other.cy - other.h / 2.0);
        let bottom = (self.cy + self.h / 2.0).min(other.cy + other.h / 2.0);
        let overlap = (right - left).max(0.0) * (bottom - top).max(0.0);
        let union = self.w * self.h + other.w * other.h - overlap;
        if union <= 0.0 {
            0.0
        } else {
            overlap / union
        }
    }
}

fn sigmoid(raw: f32) -> f32 {
    1.0 / (1.0 + (-raw.clamp(-SCORE_CLIP, SCORE_CLIP)).exp())
}

/// The network's two outputs, `regressors` (`COORDS` values an anchor) and
/// `classificators` (one raw score an anchor), as faces scoring at least
/// `floor`, before suppression.
pub fn decode(
    model: &Model,
    anchors: &[Anchor],
    boxes: &[f32],
    scores: &[f32],
    floor: f32,
) -> Vec<Found> {
    let scale = model.size as f32;
    let mut found = Vec::new();
    for (i, anchor) in anchors.iter().enumerate() {
        let (Some(&raw), Some(values)) = (scores.get(i), boxes.get(i * COORDS..(i + 1) * COORDS))
        else {
            break;
        };
        let score = sigmoid(raw);
        if score < floor {
            continue;
        }
        let mut keypoints = [[0.0; 2]; KEYPOINTS];
        for (k, point) in keypoints.iter_mut().enumerate() {
            *point = [
                values[4 + 2 * k] / scale + anchor.x,
                values[5 + 2 * k] / scale + anchor.y,
            ];
        }
        found.push(Found {
            cx: values[0] / scale + anchor.x,
            cy: values[1] / scale + anchor.y,
            w: values[2] / scale,
            h: values[3] / scale,
            score,
            keypoints,
        });
    }
    found
}

/// MediaPipe's weighted non-maximum suppression: the best box and every box
/// overlapping it become one, their coordinates averaged by score, with the
/// best one's score. Best first.
pub fn suppress(mut found: Vec<Found>) -> Vec<Found> {
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out = Vec::new();
    while !found.is_empty() {
        let best = found[0].clone();
        let (same, rest): (Vec<Found>, Vec<Found>) = found
            .into_iter()
            .partition(|other| best.iou(other) > SUPPRESSION_IOU);
        found = rest;
        let total: f32 = same.iter().map(|f| f.score).sum();
        if total <= 0.0 {
            out.push(best);
            continue;
        }
        let weigh = |value: &dyn Fn(&Found) -> f32| {
            same.iter().map(|f| value(f) * f.score).sum::<f32>() / total
        };
        let mut keypoints = [[0.0; 2]; KEYPOINTS];
        for (k, point) in keypoints.iter_mut().enumerate() {
            *point = [weigh(&|f| f.keypoints[k][0]), weigh(&|f| f.keypoints[k][1])];
        }
        out.push(Found {
            cx: weigh(&|f| f.cx),
            cy: weigh(&|f| f.cy),
            w: weigh(&|f| f.w),
            h: weigh(&|f| f.h),
            score: best.score,
            keypoints,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_anchor_counts_are_mediapipes() {
        assert_eq!(anchors(&FULL).len(), 2304);
        assert_eq!(anchors(&SHORT).len(), 896);
    }

    #[test]
    fn anchors_sit_at_cell_centres_row_by_row() {
        let short = anchors(&SHORT);
        // The 16x16 grid's first cell, twice, then the next one along x.
        assert_eq!(
            short[0],
            Anchor {
                x: 0.5 / 16.0,
                y: 0.5 / 16.0
            }
        );
        assert_eq!(short[1], short[0]);
        assert_eq!(
            short[2],
            Anchor {
                x: 1.5 / 16.0,
                y: 0.5 / 16.0
            }
        );
        // The 8x8 grid follows, six anchors a cell.
        assert_eq!(
            short[512],
            Anchor {
                x: 0.5 / 8.0,
                y: 0.5 / 8.0
            }
        );
        assert_eq!(short[517], short[512]);
        assert_eq!(
            short[518],
            Anchor {
                x: 1.5 / 8.0,
                y: 0.5 / 8.0
            }
        );
    }

    fn raw(
        model: &Model,
        count: usize,
        at: usize,
        values: [f32; 4],
        logit: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut boxes = vec![0.0; count * COORDS];
        let mut scores = vec![-10.0; count];
        let scale = model.size as f32;
        for (k, value) in values.iter().enumerate() {
            boxes[at * COORDS + k] = value * scale;
        }
        scores[at] = logit;
        (boxes, scores)
    }

    #[test]
    fn a_box_is_its_anchor_plus_the_offset() {
        let anchors = anchors(&SHORT);
        let (boxes, scores) = raw(&SHORT, anchors.len(), 2, [0.0, 0.0, 0.25, 0.5], 3.0);
        let found = decode(&SHORT, &anchors, &boxes, &scores, 0.5);
        assert_eq!(found.len(), 1);
        assert!((found[0].cx - anchors[2].x).abs() < 1e-6);
        assert!((found[0].w - 0.25).abs() < 1e-6);
        assert!((found[0].h - 0.5).abs() < 1e-6);
        assert!((found[0].score - sigmoid(3.0)).abs() < 1e-6);
        // The keypoints, all offsets of 0, sit on the anchor.
        assert_eq!(found[0].keypoints[0], [anchors[2].x, anchors[2].y]);
    }

    #[test]
    fn scores_below_the_floor_are_dropped() {
        let anchors = anchors(&SHORT);
        let (boxes, scores) = raw(&SHORT, anchors.len(), 0, [0.0, 0.0, 0.2, 0.2], -1.0);
        assert!(decode(&SHORT, &anchors, &boxes, &scores, 0.5).is_empty());
    }

    fn face(cx: f32, w: f32, score: f32) -> Found {
        Found {
            cx,
            cy: 0.5,
            w,
            h: w,
            score,
            keypoints: [[cx, 0.5]; KEYPOINTS],
        }
    }

    #[test]
    fn overlapping_boxes_become_one_weighted_by_score() {
        let merged = suppress(vec![
            face(0.5, 0.2, 0.9),
            face(0.52, 0.2, 0.3),
            face(0.1, 0.1, 0.8),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].score, 0.9);
        let expected = (0.5 * 0.9 + 0.52 * 0.3) / 1.2;
        assert!((merged[0].cx - expected).abs() < 1e-6);
        assert!((merged[1].cx - 0.1).abs() < 1e-6);
    }

    #[test]
    fn models_are_found_by_name() {
        assert_eq!(model("face-full"), Some(FULL));
        assert_eq!(model("face-short"), Some(SHORT));
        assert_eq!(model("face-mesh"), None);
        for name in protocol::presence::MODELS {
            assert!(model(name).is_some(), "{name}");
        }
    }
}
