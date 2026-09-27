//! The QR code on the welcome page that joins a phone to the open hotspot,
//! after which the phone's captive portal check opens the setup portal
//! (docs/setup-portal.md).
//!
//! The payload is the `WIFI:` format every phone camera reads. The SVG is
//! drawn here from the module matrix, one path of unit squares on a viewBox
//! with a quiet zone, so the page can scale it to any size and nginx serves
//! it inside `welcome.json` with no image files involved.

use qrcode::{Color, EcLevel, QrCode};

/// Modules of light margin around the code, the minimum the standard asks.
const QUIET: usize = 4;

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
/// as the logo's. Medium error correction, which a screen photographed at an
/// angle needs more than a smaller code.
pub fn svg(payload: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::M)
        .map_err(|err| format!("QR code: {err}"))?;
    let width = code.width();
    let size = width + 2 * QUIET;
    let mut path = String::new();
    for (index, color) in code.to_colors().into_iter().enumerate() {
        if color == Color::Dark {
            let (x, y) = (index % width + QUIET, index / width + QUIET);
            path.push_str(&format!("M{x} {y}h1v1h-1z"));
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {size} {size}\" \
         shape-rendering=\"crispEdges\"><defs><linearGradient id=\"qr-accent\" \
         x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"#5cc8ff\"/>\
         <stop offset=\"1\" stop-color=\"#9d8cff\"/></linearGradient></defs>\
         <rect width=\"{size}\" height=\"{size}\" fill=\"#0a0d14\"/>\
         <path fill=\"url(#qr-accent)\" d=\"{path}\"/></svg>"
    ))
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
        let width = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::M)
            .unwrap()
            .width();
        let size = width + 2 * QUIET;
        assert!(
            svg.contains(&format!("viewBox=\"0 0 {size} {size}\"")),
            "{svg}"
        );
        assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>"));
        // The finder pattern's top-left module sits just inside the margin.
        assert!(svg.contains("M4 4h1v1h-1z"));
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
    fn a_payload_too_long_for_any_code_is_an_error() {
        assert!(svg(&"x".repeat(5000)).is_err());
    }
}
