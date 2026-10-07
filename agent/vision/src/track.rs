//! Following faces from frame to frame: each face found is matched to the
//! box it overlaps most in the frames before, keeps that box's id and is
//! smoothed with it, so a page can follow one face and its box does not
//! jitter. An id is a box's, never a person's: a face that leaves and comes
//! back gets a new one.

use protocol::presence::{Detection, FaceBox, Point};

/// A face matches a box from before that overlaps it by at least this.
const MATCH_IOU: f64 = 0.3;

/// Frames a box is kept for without a face in it, so a face missed by one
/// frame keeps its id.
const KEEP_MISSED: u32 = 2;

/// How much of a new box goes into the smoothed one: the rest is the old.
const SMOOTHING: f64 = 0.6;

#[derive(Debug, Clone)]
struct Track {
    id: u64,
    area: FaceBox,
    keypoints: [Point; 6],
    missed: u32,
}

#[derive(Debug, Default)]
pub struct Tracker {
    tracks: Vec<Track>,
    next: u64,
}

fn iou(a: &FaceBox, b: &FaceBox) -> f64 {
    let left = a.x.max(b.x);
    let right = (a.x + a.w).min(b.x + b.w);
    let top = a.y.max(b.y);
    let bottom = (a.y + a.h).min(b.y + b.h);
    let overlap = (right - left).max(0.0) * (bottom - top).max(0.0);
    let union = a.w * a.h + b.w * b.h - overlap;
    if union <= 0.0 {
        0.0
    } else {
        overlap / union
    }
}

fn blend(old: f64, new: f64) -> f64 {
    old + (new - old) * SMOOTHING
}

impl Tracker {
    /// This frame's faces, `(box, score, keypoints)` best first, with ids
    /// and smoothed boxes.
    pub fn update(&mut self, faces: Vec<(FaceBox, f64, [Point; 6])>) -> Vec<Detection> {
        let mut taken = vec![false; self.tracks.len()];
        let mut out = Vec::new();
        for (area, score, keypoints) in faces {
            let best = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(i, _)| !taken[*i])
                .map(|(i, track)| (i, iou(&track.area, &area)))
                .filter(|(_, overlap)| *overlap >= MATCH_IOU)
                .max_by(|a, b| a.1.total_cmp(&b.1));
            let track = match best {
                Some((i, _)) => {
                    taken[i] = true;
                    let track = &mut self.tracks[i];
                    track.area = FaceBox {
                        x: blend(track.area.x, area.x),
                        y: blend(track.area.y, area.y),
                        w: blend(track.area.w, area.w),
                        h: blend(track.area.h, area.h),
                    };
                    for (old, new) in track.keypoints.iter_mut().zip(keypoints) {
                        *old = [blend(old[0], new[0]), blend(old[1], new[1])];
                    }
                    track.missed = 0;
                    track.clone()
                }
                None => {
                    self.next += 1;
                    let track = Track {
                        id: self.next,
                        area,
                        keypoints,
                        missed: 0,
                    };
                    self.tracks.push(track.clone());
                    taken.push(true);
                    track
                }
            };
            out.push(Detection {
                id: track.id,
                area: track.area,
                score,
                keypoints: track.keypoints,
            });
        }
        for (track, taken) in self.tracks.iter_mut().zip(&taken) {
            if !taken {
                track.missed += 1;
            }
        }
        self.tracks.retain(|track| track.missed <= KEEP_MISSED);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f64) -> (FaceBox, f64, [Point; 6]) {
        (
            FaceBox {
                x,
                y: 0.2,
                w: 0.2,
                h: 0.2,
            },
            0.9,
            [[x, 0.3]; 6],
        )
    }

    #[test]
    fn a_face_that_moves_a_little_keeps_its_id_and_is_smoothed() {
        let mut tracker = Tracker::default();
        let first = tracker.update(vec![at(0.1)]);
        let second = tracker.update(vec![at(0.12)]);
        assert_eq!(first[0].id, second[0].id);
        let expected = 0.1 + 0.02 * SMOOTHING;
        assert!((second[0].area.x - expected).abs() < 1e-9);
    }

    #[test]
    fn a_face_elsewhere_is_a_new_id() {
        let mut tracker = Tracker::default();
        let first = tracker.update(vec![at(0.1)]);
        let second = tracker.update(vec![at(0.7)]);
        assert_ne!(first[0].id, second[0].id);
    }

    #[test]
    fn a_face_missed_briefly_keeps_its_id_but_not_for_long() {
        let mut tracker = Tracker::default();
        let id = tracker.update(vec![at(0.1)])[0].id;
        tracker.update(vec![]);
        tracker.update(vec![]);
        assert_eq!(tracker.update(vec![at(0.1)])[0].id, id);
        for _ in 0..=KEEP_MISSED {
            tracker.update(vec![]);
        }
        assert_ne!(tracker.update(vec![at(0.1)])[0].id, id);
    }

    #[test]
    fn two_faces_never_take_the_same_track() {
        let mut tracker = Tracker::default();
        tracker.update(vec![at(0.1)]);
        let both = tracker.update(vec![at(0.1), at(0.11)]);
        assert_ne!(both[0].id, both[1].id);
    }
}
