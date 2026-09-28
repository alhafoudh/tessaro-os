//! The QR code on the welcome page that joins a phone to the open hotspot,
//! after which the phone's captive portal check opens the setup portal
//! (docs/setup-portal.md).
//!
//! The payload is the `WIFI:` format every phone camera reads. The SVG is
//! drawn here from the module matrix, with rounded modules on a viewBox
//! with a quiet zone, so the page can scale it to any size and nginx serves
//! it inside `welcome.json` with no image files involved.

use std::fmt::Write;

use qrcode::{Color, EcLevel, QrCode};

/// Modules of light margin around the code, the minimum the standard asks.
const QUIET: usize = 4;

/// Geometry of the approved badge, in its own SVG coordinates.
const BADGE_SIZE: f64 = 8.8;
const BADGE_RADIUS: f64 = 2.4;
const WIFI_ICON: &str = include_str!("qr-wifi.svg");

/// `WIFI:T:nopass;S:<ssid>;;` for an open network, which is what the hotspot
/// is while the device is unclaimed - the only time the QR is shown.
pub fn wifi_payload(ssid: &str) -> String {
    format!("WIFI:T:nopass;S:{};;", escape(ssid))
}

/// The format's escapes: a backslash before each of `\ ; , : "`.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The code as a self-contained `<svg>`: modules in the logo's gradient on a
/// square of the page's background, so it sits in the dark page instead of
/// lighting up a white square in it. Light on dark is the inverse of a
/// printed code; phone cameras read it, but some older scanner apps do not.
/// The gradient's id is its own, since the SVG goes into the same document
/// as the logo's. High error correction leaves room for the central icon.
pub fn svg(payload: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::H)
        .map_err(|err| format!("QR code: {err}"))?;
    let width = code.width();
    let size = width + 2 * QUIET;
    let center = size as f64 / 2.0;
    let badge_scale = size as f64 / 37.0;
    let badge_start = center - BADGE_SIZE * badge_scale / 2.0;
    let markers = [
        (QUIET, QUIET),
        (QUIET + width - 7, QUIET),
        (QUIET, QUIET + width - 7),
    ];
    let mut path = String::new();
    for (index, color) in code.to_colors().into_iter().enumerate() {
        let (x, y) = (index % width + QUIET, index / width + QUIET);
        if color != Color::Dark
            || markers
                .iter()
                .any(|&(mx, my)| (mx..mx + 7).contains(&x) && (my..my + 7).contains(&y))
            || touches_badge(x, y, center, badge_scale)
        {
            continue;
        }
        // Inset each module, retaining a small gap and rounded corners.
        write!(path, "M{:.2} {:.2}h.5q.18 0 .18 .18v.5q0 .18 -.18 .18h-.5q-.18 0 -.18 -.18v-.5q0 -.18 .18 -.18z", x as f64 + 0.25, y as f64 + 0.07).unwrap();
    }
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {size} {size}\" \
         shape-rendering=\"geometricPrecision\"><defs><linearGradient id=\"qr-accent\" \
         gradientUnits=\"userSpaceOnUse\" x1=\"0\" y1=\"0\" x2=\"{size}\" y2=\"{size}\">\
         <stop offset=\"0\" stop-color=\"#5cc8ff\"/>\
         <stop offset=\"1\" stop-color=\"#9d8cff\"/></linearGradient></defs>\
         <rect width=\"{size}\" height=\"{size}\" fill=\"#0a0d14\"/>\
         <path fill=\"url(#qr-accent)\" d=\"{path}\"/>"
    );
    for (x, y) in markers {
        write!(svg,
            "<rect x=\"{x}\" y=\"{y}\" width=\"7\" height=\"7\" rx=\"1.1\" fill=\"url(#qr-accent)\"/>\
             <rect x=\"{}\" y=\"{}\" width=\"5\" height=\"5\" rx=\"0.7\" fill=\"#0a0d14\"/>\
             <rect x=\"{}\" y=\"{}\" width=\"3\" height=\"3\" rx=\"0.5\" fill=\"url(#qr-accent)\"/>",
            x + 1, y + 1, x + 2, y + 2).unwrap();
    }
    write!(svg, "<g transform=\"translate({badge_start} {badge_start}) scale({badge_scale})\">{WIFI_ICON}</g></svg>").unwrap();
    Ok(svg)
}

/// Reject a whole module if its visible bounds touch the rounded badge.
/// The rounded square is its inset core expanded by a disk of the radius;
/// distance from the module's bounds to that core handles the curved corners.
fn touches_badge(x: usize, y: usize, center: f64, scale: f64) -> bool {
    let radius = BADGE_RADIUS * scale;
    let core_half = (BADGE_SIZE / 2.0 - BADGE_RADIUS) * scale;
    let distance = |coordinate: usize| {
        let start = coordinate as f64 + 0.07;
        let end = coordinate as f64 + 0.93;
        (center - core_half - end)
            .max(start - center - core_half)
            .max(0.0)
    };
    distance(x).powi(2) + distance(y).powi(2) <= radius.powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_ssid_goes_in_as_is() {
        assert_eq!(
            wifi_payload("tessaro-amber-fox"),
            "WIFI:T:nopass;S:tessaro-amber-fox;;"
        );
    }

    #[test]
    fn the_special_characters_are_escaped() {
        assert_eq!(
            wifi_payload(r#"a;b,c:d"e\f"#),
            r#"WIFI:T:nopass;S:a\;b\,c\:d\"e\\f;;"#
        );
    }

    #[test]
    fn the_svg_is_square_with_a_quiet_zone() {
        let payload = wifi_payload("tessaro-amber-fox");
        let svg = svg(&payload).unwrap();
        let width = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::H)
            .unwrap()
            .width();
        let size = width + 2 * QUIET;
        assert!(
            svg.contains(&format!("viewBox=\"0 0 {size} {size}\"")),
            "{svg}"
        );
        assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>"));
        // The finder pattern's top-left module sits just inside the margin.
        assert!(svg.contains("<rect x=\"4\" y=\"4\" width=\"7\" height=\"7\""));
        assert!(!svg.contains("M0 0h"));
    }

    #[test]
    fn the_modules_take_the_gradient_on_the_page_background() {
        let svg = svg(&wifi_payload("tessaro-amber-fox")).unwrap();
        assert!(svg.contains("<linearGradient id=\"qr-accent\""), "{svg}");
        assert!(svg.contains("<path fill=\"url(#qr-accent)\""), "{svg}");
        assert!(svg.contains("fill=\"#0a0d14\""), "{svg}");
    }

    #[test]
    fn the_badge_omits_intersecting_modules_but_keeps_clear_corners() {
        let center = 18.5;
        assert!(touches_badge(18, 18, center, 1.0));
        assert!(touches_badge(14, 18, center, 1.0));
        assert!(touches_badge(15, 14, center, 1.0));
        assert!(touches_badge(14, 14, center, 1.0));
        assert!(!touches_badge(13, 13, 18.0, 1.0));
        assert!(!touches_badge(13, 18, center, 1.0));
        assert!(!touches_badge(23, 18, center, 1.0));
    }

    #[test]
    fn a_payload_too_long_for_any_code_is_an_error() {
        assert!(svg(&"x".repeat(5000)).is_err());
    }
}
