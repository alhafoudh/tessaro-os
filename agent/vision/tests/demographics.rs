//! FaceRes on real faces: the man on the left of `two-faces.jpg` and the
//! woman of `woman.jpg` (Unsplash via Wikimedia Commons, CC0; see
//! sbom/vendored.yml), both adults. The woman on the right of
//! `two-faces.jpg`, a small black and white passport photo, is too much for
//! it and comes out `unknown`, which is what the threshold is for.

use std::path::PathBuf;

use tessaro_vision::blazeface;
use tessaro_vision::demographics::{self, Classifier, Look};
use tessaro_vision::detector::Detector;
use tessaro_vision::picture;

fn here(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

/// Each face's look in `file`, left to right.
fn looks(file: &str) -> Vec<Look> {
    let models = here("models");
    let detector = Detector::load(&models, blazeface::FULL).unwrap();
    let classifier = Classifier::load(&models).unwrap();
    let frame = std::fs::read(here(file)).unwrap();
    let small = picture::jpeg(&frame, blazeface::FULL.size).unwrap();
    let mut faces = detector.detect(&small, 0.5).unwrap();
    faces.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    faces
        .iter()
        .map(|(area, _, keypoints)| {
            let side = demographics::decode_side(area, small.width, small.height)
                .max(blazeface::FULL.size);
            let picture = picture::jpeg(&frame, side).unwrap();
            classifier
                .look(demographics::square(&picture, area, keypoints))
                .unwrap()
        })
        .collect()
}

fn adult(look: &Look) {
    assert!((18.0..70.0).contains(&look.age), "{look:?}");
}

#[test]
fn the_man_is_told_male() {
    let looks = looks("tests/two-faces.jpg");
    assert_eq!(looks.len(), 2, "{looks:?}");
    assert!(looks[0].male > 0.8, "{looks:?}");
    adult(&looks[0]);
}

#[test]
fn the_woman_is_told_female() {
    let looks = looks("tests/woman.jpg");
    assert_eq!(looks.len(), 1, "{looks:?}");
    assert!(looks[0].male < 0.3, "{looks:?}");
    adult(&looks[0]);
}
