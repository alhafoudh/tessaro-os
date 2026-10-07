//! The shipped models on a real picture: `two-faces.jpg` is two portrait
//! photographs pinned side by side (Nationaal Archief, CC0; see
//! sbom/vendored.yml), one face each.

use std::path::PathBuf;

use tessaro_vision::blazeface::{self, Model};
use tessaro_vision::detector::Detector;
use tessaro_vision::picture;

fn here(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn faces(model: Model) -> Vec<(f64, f64)> {
    let detector = Detector::load(&here("models"), model).unwrap();
    let frame = std::fs::read(here("tests/two-faces.jpg")).unwrap();
    let picture = picture::jpeg(&frame, model.size).unwrap();
    let mut centres: Vec<(f64, f64)> = detector
        .detect(&picture, 0.5)
        .unwrap()
        .into_iter()
        .map(|(area, _, _)| (area.x + area.w / 2.0, area.y + area.h / 2.0))
        .collect();
    centres.sort_by(|a, b| a.0.total_cmp(&b.0));
    centres
}

fn two_faces_where_they_are(centres: &[(f64, f64)]) {
    assert_eq!(centres.len(), 2, "{centres:?}");
    // The man's face on the left half, the woman's on the right, both in
    // the upper half of the picture.
    let (left, right) = (centres[0], centres[1]);
    assert!((0.2..0.4).contains(&left.0), "{left:?}");
    assert!((0.6..0.8).contains(&right.0), "{right:?}");
    for (_, y) in centres {
        assert!((0.25..0.6).contains(y), "{y}");
    }
}

#[test]
fn the_full_range_model_finds_both_faces() {
    two_faces_where_they_are(&faces(blazeface::FULL));
}

#[test]
fn the_short_range_model_finds_both_faces() {
    two_faces_where_they_are(&faces(blazeface::SHORT));
}
