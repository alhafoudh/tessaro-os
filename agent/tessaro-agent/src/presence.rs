//! What the faces tessaro-vision sends mean: which are confident enough,
//! how far away each is and whether it faces the screen, and when someone
//! has arrived, left, come near or gone far again. Pure, so it is tested
//! without a camera, a socket or a clock; `control/presence.rs` feeds it.
//!
//! Every threshold is a live setting (`config::Presence`): the vision service
//! only finds faces, so changing what counts restarts nothing.

use std::time::{Duration, Instant};

use protocol::presence::{Detection, Face, FacesFrame, Keypoints, VisionFrame};

use crate::config;

/// The width of an average adult face across the cheeks, in meters, which
/// BlazeFace's box roughly spans. `camera calibrate` makes up for the rest.
pub const FACE_WIDTH: f64 = 0.15;

/// A face missed for this long breaks the run of frames `arrive` counts.
const GAP: Duration = Duration::from_secs(1);

/// Someone near goes far only beyond `near` plus this share of it, so a
/// person standing right at the line does not flap between the two.
const HYSTERESIS: f64 = 0.1;

/// The nose further off the middle of the eyes than this share of the eyes'
/// distance is a head turned away from the screen.
const FACING_OFFSET: f64 = 0.35;

/// The thresholds, from the settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub confidence: f64,
    /// Meters; `None` when camera.presence.near is off.
    pub near: Option<f64>,
    /// Degrees.
    pub fov: f64,
    pub arrive: Duration,
    pub linger: Duration,
}

impl From<&config::Presence> for Settings {
    fn from(presence: &config::Presence) -> Settings {
        let seconds = |hundredths: i64| Duration::from_millis(hundredths.max(0) as u64 * 10);
        Settings {
            confidence: presence.confidence as f64 / 100.0,
            near: presence.near.map(|cm| cm as f64 / 100.0),
            fov: presence.fov as f64,
            arrive: seconds(presence.arrive),
            linger: seconds(presence.linger),
        }
    }
}

/// How far away a face `width` of the frame wide is, with the camera's
/// horizontal field of view `fov` in degrees: the frame spans
/// `2 * d * tan(fov / 2)` meters at a distance `d`.
pub fn distance(width: f64, fov: f64) -> f64 {
    let span = 2.0 * (fov.to_radians() / 2.0).tan();
    if width <= 0.0 || span <= 0.0 {
        return f64::INFINITY;
    }
    FACE_WIDTH / (span * width)
}

/// The field of view that puts a face `width` of the frame wide at
/// `distance` meters: `distance` the other way round, in whole degrees
/// within camera.presence.fov's range.
pub fn fov(width: f64, distance: f64) -> f64 {
    let half = (FACE_WIDTH / (2.0 * distance * width)).atan();
    (2.0 * half).to_degrees().round().clamp(20.0, 170.0)
}

/// Whether a face looks at the camera: its nose sits between its eyes.
pub fn facing(keypoints: &Keypoints) -> bool {
    let eyes = (keypoints.left_eye[0] - keypoints.right_eye[0]).abs();
    if eyes <= 0.0 {
        return false;
    }
    let middle = (keypoints.left_eye[0] + keypoints.right_eye[0]) / 2.0;
    ((keypoints.nose[0] - middle) / eyes).abs() <= FACING_OFFSET
}

fn face(detection: &Detection, settings: &Settings) -> Face {
    let keypoints = Keypoints::from(detection.keypoints);
    let distance = distance(detection.area.w, settings.fov);
    Face {
        id: detection.id,
        area: detection.area,
        score: detection.score,
        distance: (distance * 100.0).round() / 100.0,
        near: settings.near.is_some_and(|near| distance <= near),
        facing: facing(&keypoints),
        keypoints,
    }
}

/// What presence detection last saw and decided.
#[derive(Debug, Clone, Default)]
pub struct Presence {
    pub present: bool,
    pub near: bool,
    /// The first frame of the current run of frames with a face.
    seen_since: Option<Instant>,
    /// The last frame with a face.
    last_seen: Option<Instant>,
    /// The newest frame, when it came, and its faces.
    pub frame: Option<(Instant, FacesFrame)>,
    /// The last event and when it happened, seconds since the epoch.
    pub last: Option<(&'static str, i64)>,
}

impl Presence {
    /// One frame from the vision service, at `now`: its faces become the
    /// newest frame, and what changed is returned as events, in order.
    pub fn frame(
        &mut self,
        now: Instant,
        frame: &VisionFrame,
        settings: &Settings,
    ) -> Vec<&'static str> {
        let faces: Vec<Face> = frame
            .faces
            .iter()
            .filter(|detection| detection.score >= settings.confidence)
            .map(|detection| face(detection, settings))
            .collect();
        let nearest = faces
            .iter()
            .map(|face| face.distance)
            .fold(f64::INFINITY, f64::min);
        if faces.is_empty() {
            if self
                .last_seen
                .is_none_or(|at| now.duration_since(at) >= GAP)
            {
                self.seen_since = None;
            }
        } else {
            self.last_seen = Some(now);
            self.seen_since.get_or_insert(now);
        }
        self.frame = Some((
            now,
            FacesFrame {
                t: frame.t,
                width: frame.width,
                height: frame.height,
                faces: faces.clone(),
            },
        ));

        let mut events = Vec::new();
        if !self.present
            && self
                .seen_since
                .is_some_and(|since| now.duration_since(since) >= settings.arrive)
        {
            self.present = true;
            events.push("arrived");
        }
        if self.present && !faces.is_empty() {
            match settings.near {
                Some(near) if !self.near && nearest <= near => {
                    self.near = true;
                    events.push("near");
                }
                Some(near) if self.near && nearest > near * (1.0 + HYSTERESIS) => {
                    self.near = false;
                    events.push("far");
                }
                None if self.near => {
                    self.near = false;
                    events.push("far");
                }
                _ => {}
            }
        }
        events.extend(self.tick(now, settings));
        events
    }

    /// Time passing with or without frames: someone not seen for `linger`
    /// has left, and someone near goes far first.
    pub fn tick(&mut self, now: Instant, settings: &Settings) -> Vec<&'static str> {
        let gone = self
            .last_seen
            .is_none_or(|at| now.duration_since(at) >= settings.linger);
        if !self.present || !gone {
            return Vec::new();
        }
        let mut events = Vec::new();
        if self.near {
            self.near = false;
            events.push("far");
        }
        self.present = false;
        self.seen_since = None;
        events.push("left");
        events
    }

    /// Faces in the newest frame that came within `fresh`.
    pub fn count(&self, now: Instant, fresh: Duration) -> u32 {
        match &self.frame {
            Some((at, frame)) if now.duration_since(*at) <= fresh => frame.faces.len() as u32,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::presence::FaceBox;

    fn settings() -> Settings {
        Settings {
            confidence: 0.6,
            near: Some(1.5),
            fov: 65.0,
            arrive: Duration::from_millis(500),
            linger: Duration::from_secs(3),
        }
    }

    /// A frame with one face `width` of the frame wide, scoring `score`.
    fn with_face(width: f64, score: f64) -> VisionFrame {
        VisionFrame {
            t: 0,
            width: 640,
            height: 480,
            faces: vec![Detection {
                id: 1,
                area: FaceBox {
                    x: 0.4,
                    y: 0.3,
                    w: width,
                    h: width,
                },
                score,
                // Eyes level, the nose between them, the ears outside.
                keypoints: [
                    [0.45, 0.35],
                    [0.55, 0.35],
                    [0.5, 0.4],
                    [0.5, 0.45],
                    [0.4, 0.37],
                    [0.6, 0.37],
                ],
            }],
        }
    }

    fn empty() -> VisionFrame {
        VisionFrame {
            t: 0,
            width: 640,
            height: 480,
            faces: Vec::new(),
        }
    }

    /// The width of a face at `meters` with the test's field of view.
    fn at(meters: f64) -> f64 {
        FACE_WIDTH / (2.0 * (65f64.to_radians() / 2.0).tan() * meters)
    }

    #[test]
    fn distance_and_fov_undo_each_other() {
        let width = at(2.0);
        assert!((distance(width, 65.0) - 2.0).abs() < 1e-9);
        assert_eq!(fov(width, 2.0), 65.0);
        assert_eq!(distance(0.0, 65.0), f64::INFINITY);
    }

    #[test]
    fn someone_arrives_only_after_staying_for_arrive() {
        let mut presence = Presence::default();
        let start = Instant::now();
        let s = settings();
        assert!(presence
            .frame(start, &with_face(at(3.0), 0.9), &s)
            .is_empty());
        let later = start + Duration::from_millis(300);
        assert!(presence
            .frame(later, &with_face(at(3.0), 0.9), &s)
            .is_empty());
        let later = start + Duration::from_millis(600);
        assert_eq!(
            presence.frame(later, &with_face(at(3.0), 0.9), &s),
            ["arrived"]
        );
        assert!(presence.present && !presence.near);
    }

    #[test]
    fn a_face_below_confidence_is_nobody() {
        let mut presence = Presence::default();
        let start = Instant::now();
        let s = settings();
        for ms in [0, 600, 1200] {
            let now = start + Duration::from_millis(ms);
            assert!(presence.frame(now, &with_face(at(1.0), 0.5), &s).is_empty());
        }
        assert_eq!(
            presence.count(start + Duration::from_millis(1200), Duration::from_secs(2)),
            0
        );
    }

    #[test]
    fn near_and_far_have_a_margin_and_far_comes_before_left() {
        let mut presence = Presence::default();
        let start = Instant::now();
        let s = Settings {
            arrive: Duration::ZERO,
            ..settings()
        };
        assert_eq!(
            presence.frame(start, &with_face(at(1.0), 0.9), &s),
            ["arrived", "near"]
        );
        // Just past the line, within the margin: still near.
        let now = start + Duration::from_millis(200);
        assert!(presence.frame(now, &with_face(at(1.6), 0.9), &s).is_empty());
        let now = start + Duration::from_millis(400);
        assert_eq!(presence.frame(now, &with_face(at(2.0), 0.9), &s), ["far"]);
        let now = start + Duration::from_millis(600);
        assert_eq!(presence.frame(now, &with_face(at(1.0), 0.9), &s), ["near"]);
        // Gone: nothing until linger, then far and left together.
        let now = start + Duration::from_secs(2);
        assert!(presence.frame(now, &empty(), &s).is_empty());
        let now = start + Duration::from_millis(3700);
        assert_eq!(presence.tick(now, &s), ["far", "left"]);
        assert!(!presence.present && !presence.near);
    }

    #[test]
    fn looking_away_briefly_does_not_leave() {
        let mut presence = Presence::default();
        let start = Instant::now();
        let s = Settings {
            arrive: Duration::ZERO,
            near: None,
            ..settings()
        };
        assert_eq!(
            presence.frame(start, &with_face(at(3.0), 0.9), &s),
            ["arrived"]
        );
        let now = start + Duration::from_secs(2);
        assert!(presence.frame(now, &empty(), &s).is_empty());
        let now = start + Duration::from_millis(2500);
        assert!(presence.frame(now, &with_face(at(3.0), 0.9), &s).is_empty());
        assert!(presence.tick(start + Duration::from_secs(5), &s).is_empty());
        assert_eq!(presence.tick(start + Duration::from_secs(6), &s), ["left"]);
    }

    #[test]
    fn near_off_never_says_near() {
        let mut presence = Presence::default();
        let s = Settings {
            arrive: Duration::ZERO,
            near: None,
            ..settings()
        };
        assert_eq!(
            presence.frame(Instant::now(), &with_face(at(0.5), 0.9), &s),
            ["arrived"]
        );
        let faces = &presence.frame.as_ref().unwrap().1.faces;
        assert!(!faces[0].near);
    }

    #[test]
    fn a_face_turned_away_is_not_facing() {
        let mut keypoints = Keypoints::from(with_face(0.1, 0.9).faces[0].keypoints);
        assert!(facing(&keypoints));
        keypoints.nose = [0.57, 0.4];
        assert!(!facing(&keypoints));
    }

    #[test]
    fn the_settings_come_in_hundredths() {
        let config = config::Presence {
            enable: true,
            model: "face-full".into(),
            confidence: 60,
            near: Some(150),
            fov: 65,
            arrive: 50,
            linger: 300,
            page: true,
            scripts: true,
        };
        let s = Settings::from(&config);
        assert_eq!(s.confidence, 0.6);
        assert_eq!(s.near, Some(1.5));
        assert_eq!(s.arrive, Duration::from_millis(500));
        assert_eq!(s.linger, Duration::from_secs(3));
    }
}
