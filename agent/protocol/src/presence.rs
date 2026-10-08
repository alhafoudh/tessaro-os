//! Presence detection as clients, the agent and `tessaro-vision` name it:
//! the events, the presence events a script runs on, what the vision service
//! sends the agent, and what the agent answers about it (docs/presence.md).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Moment;

/// What the agent reports when the people in front of the screen change, to
/// the journal, the page (`tessaro:presence`) and the scripts that run on
/// them. `classified` is a face's age and gender settling, with
/// camera.presence.demographics on.
pub const EVENTS: &[&str] = &["arrived", "left", "near", "far", "classified"];

/// The face detectors camera.presence.model picks from, each a MediaPipe
/// BlazeFace converted to ONNX: `face-full` sees faces up to about 5 m,
/// `face-short` up to about 2 m for a fraction of the work.
pub const MODELS: &[&str] = &["face-full", "face-short"];

/// One entry of what a script runs on, as typed. Stored lower-case.
pub fn trigger(typed: &str) -> Result<String, String> {
    let lower = typed.trim().to_ascii_lowercase();
    if EVENTS.contains(&lower.as_str()) {
        return Ok(lower);
    }
    Err(format!(
        "{:?} is not a presence event; one of {}",
        typed.trim(),
        EVENTS.join(", ")
    ))
}

/// Every entry of a comma-separated list, checked, without repeats.
pub fn triggers(typed: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for entry in typed.split(',').filter(|entry| !entry.trim().is_empty()) {
        let entry = trigger(entry)?;
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// Does a script that runs on `triggers` run on this event?
pub fn runs_on(triggers: &[String], event: &str) -> bool {
    triggers.iter().any(|trigger| trigger == event)
}

/// A rectangle in a camera frame, every value a share of the frame's width
/// or height (0 to 1), from its top left corner. It is the camera's own
/// view, never mirrored: a page drawing over a selfie-style preview flips
/// `x` itself.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct FaceBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A point in a camera frame, as shares of its width and height.
pub type Point = [f64; 2];

/// One face as `tessaro-vision` finds it: before the agent applies
/// camera.presence.confidence and turns it into a `Face`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    /// Stable while the face stays in view, a new one when it comes back.
    /// It follows a box from frame to frame and is never an identity.
    pub id: u64,
    #[serde(rename = "box")]
    pub area: FaceBox,
    pub score: f64,
    /// BlazeFace's keypoints in its order: the right eye, the left eye, the
    /// nose tip, the mouth, the right ear and the left ear, the person's own
    /// right and left.
    pub keypoints: [Point; 6],
    /// The face's estimated age and gender, once settled, with
    /// camera.presence.demographics on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demographics: Option<Demographics>,
}

/// A face's gender as FaceRes estimates it from the face alone: `unknown`
/// when its looks do not lean far enough either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Gender {
    Male,
    Female,
    Unknown,
}

impl Gender {
    pub fn as_str(self) -> &'static str {
        match self {
            Gender::Male => "male",
            Gender::Female => "female",
            Gender::Unknown => "unknown",
        }
    }
}

/// What camera.presence.demographics estimates of a face, settled from a few
/// looks at it and never changed after: an estimate from the face alone, not
/// an identity.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Demographics {
    /// Estimated age in years.
    pub age: u32,
    pub gender: Gender,
    /// How likely the face is a man's, 0 to 1, averaged over its looks.
    pub male: f64,
}

/// How many faces of a frame were estimated as each gender. A face whose
/// estimate has not settled yet is in none of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Genders {
    pub male: u32,
    pub female: u32,
    pub unknown: u32,
}

impl Genders {
    pub fn of(faces: &[Face]) -> Genders {
        let mut out = Genders::default();
        for face in faces {
            match face.demographics.map(|d| d.gender) {
                Some(Gender::Male) => out.male += 1,
                Some(Gender::Female) => out.female += 1,
                Some(Gender::Unknown) => out.unknown += 1,
                None => {}
            }
        }
        out
    }
}

/// What `tessaro-vision` sends the agent for every frame it looks at, as one
/// JSON datagram on `/run/tessaro-kiosk/vision.sock`. A frame with no face
/// is sent too: it is how the agent knows the service still looks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionFrame {
    /// When the frame was captured, milliseconds since the epoch.
    pub t: u64,
    /// The frame's size as the camera captures it.
    pub width: u32,
    pub height: u32,
    pub faces: Vec<Detection>,
}

/// How `tessaro-vision` is doing, written to `/run/tessaro-vision/status.json`
/// whole every few seconds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VisionStatus {
    /// The camera it reads, by name, and the hidden mirror it reads it from.
    #[serde(default)]
    pub camera: Option<String>,
    #[serde(default)]
    pub mirror: Option<String>,
    pub model: String,
    /// Frames it looked at a second, measured.
    #[serde(default)]
    pub fps: Option<f64>,
    /// Decoding and inference of one frame, averaged over the last ones.
    #[serde(default)]
    pub decode_ms: Option<f64>,
    #[serde(default)]
    pub inference_ms: Option<f64>,
    /// camera.presence.demographics, as it runs.
    #[serde(default)]
    pub demographics: bool,
    /// Estimating the age and gender of one face, averaged over the last
    /// ones.
    #[serde(default)]
    pub classify_ms: Option<f64>,
    /// Why it is not looking: no camera, the camera has no hidden mirror, a
    /// model that does not load.
    #[serde(default)]
    pub error: Option<String>,
    /// When it wrote this, seconds since the epoch.
    pub updated: i64,
}

/// The landmarks of a face, as `Point`s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Keypoints {
    pub right_eye: Point,
    pub left_eye: Point,
    pub nose: Point,
    pub mouth: Point,
    pub right_ear: Point,
    pub left_ear: Point,
}

impl From<[Point; 6]> for Keypoints {
    fn from(points: [Point; 6]) -> Keypoints {
        let [right_eye, left_eye, nose, mouth, right_ear, left_ear] = points;
        Keypoints {
            right_eye,
            left_eye,
            nose,
            mouth,
            right_ear,
            left_ear,
        }
    }
}

/// A face in front of the screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Face {
    /// Stable while the face stays in view; never an identity.
    pub id: u64,
    #[serde(rename = "box")]
    pub area: FaceBox,
    /// How sure the detector is, 0 to 1.
    pub score: f64,
    /// How far away it is, in meters, from its width and camera.presence.fov.
    pub distance: f64,
    /// Closer than camera.presence.near.
    pub near: bool,
    /// It looks at the camera, judged from how its eyes and ears sit: a
    /// guess, not gaze tracking.
    pub facing: bool,
    pub keypoints: Keypoints,
    /// Its estimated age and gender, once settled, with
    /// camera.presence.demographics on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demographics: Option<Demographics>,
}

/// The faces of one frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FacesFrame {
    /// When the frame was captured, milliseconds since the epoch.
    pub t: u64,
    /// The frame's size as the camera captures it, for its aspect ratio.
    pub width: u32,
    pub height: u32,
    /// The faces at or above camera.presence.confidence.
    pub faces: Vec<Face>,
}

/// One presence event and when it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceEvent {
    /// One of `EVENTS`.
    pub event: String,
    pub at: Moment,
}

/// `tessaro-ctl camera presence`: whether anyone is in front of the screen,
/// and how the detection runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceStatus {
    /// camera.presence.enable.
    pub enabled: bool,
    /// The vision service reported in the last few seconds.
    pub running: bool,
    /// The camera it reads, by name.
    #[serde(default)]
    pub camera: Option<String>,
    /// camera.presence.model.
    pub model: String,
    /// Frames looked at a second, measured.
    #[serde(default)]
    pub fps: Option<f64>,
    /// Inference of one frame, in milliseconds.
    #[serde(default)]
    pub inference_ms: Option<f64>,
    /// camera.presence.demographics.
    #[serde(default)]
    pub demographics: bool,
    /// Estimating the age and gender of one face, in milliseconds.
    #[serde(default)]
    pub classify_ms: Option<f64>,
    /// Why it is not looking.
    #[serde(default)]
    pub error: Option<String>,
    /// Someone has been in view for camera.presence.arrive, and not gone for
    /// camera.presence.linger.
    pub present: bool,
    /// Someone present is closer than camera.presence.near.
    pub near: bool,
    /// camera.presence.near in meters; `None` when it is off.
    #[serde(default)]
    pub near_m: Option<f64>,
    /// camera.presence.fov, the horizontal field of view distances use.
    pub fov: f64,
    /// The last event, if there was one since the agent started.
    #[serde(default)]
    pub last: Option<PresenceEvent>,
    /// The newest frame's faces.
    #[serde(default)]
    pub frame: Option<FacesFrame>,
}

/// Presence in `device status`, with camera.presence.enable on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceSummary {
    pub present: bool,
    pub near: bool,
    /// Faces in the newest frame.
    pub count: u32,
    /// Those faces by estimated gender, with camera.presence.demographics on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genders: Option<Genders>,
}

/// What `tessaro-ctl camera calibrate` measured and saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Calibrated {
    /// The distance the person stood at, in meters, as given.
    pub distance: f64,
    /// Their face's width as a share of the frame's.
    pub width: f64,
    /// The field of view that makes the two agree, saved as
    /// camera.presence.fov.
    pub fov: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_are_events() {
        assert_eq!(
            triggers(" Arrived, left ,arrived,").unwrap(),
            vec!["arrived", "left"]
        );
        assert!(triggers("gone").is_err());
        assert!(triggers("").unwrap().is_empty());
    }

    #[test]
    fn a_script_runs_on_its_events() {
        let on = vec!["arrived".to_string()];
        assert!(runs_on(&on, "arrived"));
        assert!(!runs_on(&on, "left"));
    }

    #[test]
    fn a_detection_travels_as_json_with_a_box() {
        let frame = VisionFrame {
            t: 1,
            width: 640,
            height: 480,
            faces: vec![Detection {
                id: 3,
                area: FaceBox {
                    x: 0.25,
                    y: 0.5,
                    w: 0.125,
                    h: 0.25,
                },
                score: 0.75,
                keypoints: [[0.5, 0.5]; 6],
                demographics: None,
            }],
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"box\":{"));
        assert!(!json.contains("demographics"));
        assert_eq!(serde_json::from_str::<VisionFrame>(&json).unwrap(), frame);
    }

    #[test]
    fn a_settled_estimate_travels_too() {
        let estimate = Demographics {
            age: 34,
            gender: Gender::Female,
            male: 0.125,
        };
        let json = serde_json::to_string(&estimate).unwrap();
        assert_eq!(json, r#"{"age":34,"gender":"female","male":0.125}"#);
        assert_eq!(
            serde_json::from_str::<Demographics>(&json).unwrap(),
            estimate
        );
    }
}
