//! Following faces from frame to frame: each face found is matched to the
//! box it overlaps most in the frames before, keeps that box's id and is
//! smoothed with it, so a page can follow one face and its box does not
//! jitter. An id is a box's, never a person's: a face that leaves and comes
//! back gets a new one.
//!
//! With camera.presence.demographics on, a track also collects a few looks
//! at its face's age and gender and then settles them for good: the estimate
//! is the person's while they stay, not each frame's, so it does not flicker
//! and costs nothing once settled.

use protocol::presence::{Demographics, Detection, FaceBox, Gender, Point};

use crate::demographics::Look;

/// A face matches a box from before that overlaps it by at least this.
const MATCH_IOU: f64 = 0.3;

/// Frames a box is kept for without a face in it, so a face missed by one
/// frame keeps its id.
const KEEP_MISSED: u32 = 2;

/// How much of a new box goes into the smoothed one: the rest is the old.
const SMOOTHING: f64 = 0.6;

/// Looks at a face before its age and gender settle.
pub const LOOKS: u32 = 5;

/// A face is a man's when its looks average at least this likely male, a
/// woman's at most `1 - SURE`, and `unknown` in between.
pub const SURE: f64 = 0.65;

#[derive(Debug, Clone)]
struct Track {
    id: u64,
    area: FaceBox,
    keypoints: [Point; 6],
    missed: u32,
    /// The looks so far: how many, and their sums.
    looks: u32,
    age: f64,
    male: f64,
    settled: Option<Demographics>,
}

/// What `LOOKS` looks add up to.
fn settle(looks: u32, age: f64, male: f64) -> Demographics {
    let (age, male) = (age / f64::from(looks), male / f64::from(looks));
    let gender = if male >= SURE {
        Gender::Male
    } else if male <= 1.0 - SURE {
        Gender::Female
    } else {
        Gender::Unknown
    };
    Demographics {
        age: age.round().max(0.0) as u32,
        gender,
        male: (male * 100.0).round() / 100.0,
    }
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
                        looks: 0,
                        age: 0.0,
                        male: 0.0,
                        settled: None,
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
                demographics: track.settled,
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

    /// One more look at the face of track `id`: its age and gender once this
    /// was the last look they needed, and nothing before, after, or for a
    /// track that is gone.
    pub fn look(&mut self, id: u64, look: Look) -> Option<Demographics> {
        let track = self.tracks.iter_mut().find(|track| track.id == id)?;
        if track.settled.is_some() {
            return None;
        }
        track.looks += 1;
        track.age += look.age;
        track.male += look.male;
        if track.looks < LOOKS {
            return None;
        }
        track.settled = Some(settle(track.looks, track.age, track.male));
        track.settled
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
    fn a_face_settles_after_its_looks_and_keeps_it() {
        let mut tracker = Tracker::default();
        let id = tracker.update(vec![at(0.1)])[0].id;
        let look = Look {
            age: 30.0,
            male: 0.2,
        };
        for _ in 1..LOOKS {
            assert_eq!(tracker.look(id, look), None);
        }
        let settled = tracker.look(id, look).unwrap();
        assert_eq!(settled.gender, Gender::Female);
        assert_eq!(settled.age, 30);
        // Settled once: no second announcement, and every frame carries it.
        assert_eq!(tracker.look(id, look), None);
        assert_eq!(tracker.update(vec![at(0.1)])[0].demographics, Some(settled));
        assert_eq!(tracker.look(99, look), None);
    }

    #[test]
    fn looks_that_do_not_agree_are_unknown() {
        assert_eq!(settle(2, 60.0, 1.0).gender, Gender::Unknown);
        assert_eq!(settle(2, 60.0, 1.4).gender, Gender::Male);
        assert_eq!(settle(2, 60.0, 0.6).gender, Gender::Female);
        assert_eq!(settle(2, 61.0, 1.0).age, 31);
    }

    #[test]
    fn two_faces_never_take_the_same_track() {
        let mut tracker = Tracker::default();
        tracker.update(vec![at(0.1)]);
        let both = tracker.update(vec![at(0.1), at(0.11)]);
        assert_ne!(both[0].id, both[1].id);
    }
}
