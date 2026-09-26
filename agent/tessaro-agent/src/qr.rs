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

/// The code as a self-contained `<svg>`: black modules on a white square, so
/// it stays readable on the page's dark background. Medium
/// error correction, which a screen photographed at an angle needs more
/// than a smaller code.
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
         shape-rendering=\"crispEdges\"><rect width=\"{size}\" height=\"{size}\" \
         fill=\"#fff\"/><path fill=\"#000\" d=\"{path}\"/></svg>"
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
    fn a_payload_too_long_for_any_code_is_an_error() {
        assert!(svg(&"x".repeat(5000)).is_err());
    }
}
