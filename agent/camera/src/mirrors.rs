//! How many virtual cameras a mirror makes and what each is called. Pure, so
//! the naming is tested without a camera.
//!
//! v4l2loopback streams a device to one reader at a time (`vidioc_reqbufs`
//! wants "exclusive ownership for each stream"), so a camera gets several
//! loopbacks and the mirror writes every frame into each: one reader apiece.

/// When `KIOSK_CAMERA_MIRRORS` is missing or not a number the agent saves:
/// the page and one more reader.
pub const DEFAULT_COUNT: u32 = 2;
pub const MAX_COUNT: u32 = 8;

/// V4L2's `card` field and v4l2loopback's `card_label` are 32 bytes, NUL
/// included.
const CARD_BYTES: usize = 31;

/// `KIOSK_CAMERA_MIRRORS` as the agent renders it (camera.mirrors). Missing
/// or empty is the default; anything else it would not have saved is the
/// default too, with the reason.
pub fn count(value: Option<&str>) -> (u32, Option<String>) {
    let value = value.map(str::trim).unwrap_or("");
    if value.is_empty() {
        return (DEFAULT_COUNT, None);
    }
    match value.parse::<u32>() {
        Ok(count) if (1..=MAX_COUNT).contains(&count) => (count, None),
        _ => (
            DEFAULT_COUNT,
            Some(format!(
                "camera.mirrors is '{value}', not 1 to {MAX_COUNT}; making {DEFAULT_COUNT}"
            )),
        ),
    }
}

/// The camera's own name from its QUERYCAP card. uvcvideo writes
/// `product: product` there, cut at 31 bytes (`Integrated Camera: Integrated
/// C`), so only the part before the first `: ` is kept.
pub fn camera_name(card: &str) -> String {
    let card = card.trim();
    match card.split_once(": ") {
        Some((product, _)) if !product.trim().is_empty() => product.trim().to_string(),
        _ => card.to_string(),
    }
}

/// `<camera> Mirror <k>` for k from 1 to `count`, each within V4L2's 31
/// bytes: the camera's name is cut, on a character boundary, so the suffix
/// always stays whole.
pub fn labels(camera: &str, count: u32) -> Vec<String> {
    (1..=count).map(|k| label(camera, k)).collect()
}

fn label(camera: &str, k: u32) -> String {
    let suffix = format!("Mirror {k}");
    let room = CARD_BYTES.saturating_sub(suffix.len() + 1);
    let mut end = camera.len().min(room);
    while !camera.is_char_boundary(end) {
        end -= 1;
    }
    let camera = camera[..end].trim_end();
    if camera.is_empty() {
        suffix
    } else {
        format!("{camera} {suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_defaults_when_missing_or_empty() {
        assert_eq!(count(None), (DEFAULT_COUNT, None));
        assert_eq!(count(Some(" ")), (DEFAULT_COUNT, None));
    }

    #[test]
    fn count_takes_a_number_in_range() {
        assert_eq!(count(Some("1")), (1, None));
        assert_eq!(count(Some(" 8\n")), (8, None));
    }

    #[test]
    fn count_out_of_range_or_garbage_is_the_default_with_a_reason() {
        for value in ["0", "9", "-1", "two"] {
            let (count, why) = count(Some(value));
            assert_eq!(count, DEFAULT_COUNT, "{value}");
            assert!(why.unwrap().contains(value));
        }
    }

    #[test]
    fn camera_name_drops_uvcvideos_repeated_product() {
        assert_eq!(
            camera_name("Tessaro Test Camera: Tessaro Te"),
            "Tessaro Test Camera"
        );
        assert_eq!(
            camera_name("Integrated Camera: Integrated C"),
            "Integrated Camera"
        );
    }

    #[test]
    fn camera_name_keeps_a_card_without_the_colon_form() {
        assert_eq!(camera_name("HD Pro Webcam C920"), "HD Pro Webcam C920");
        assert_eq!(camera_name("Cam:NoSpace"), "Cam:NoSpace");
        assert_eq!(camera_name(": odd"), ": odd");
    }

    #[test]
    fn labels_number_every_mirror() {
        assert_eq!(
            labels("Tessaro Test Camera", 2),
            [
                "Tessaro Test Camera Mirror 1",
                "Tessaro Test Camera Mirror 2"
            ]
        );
    }

    #[test]
    fn a_long_name_is_cut_and_the_suffix_stays_whole() {
        let name = "A Really Long Camera Product Name";
        for label in labels(name, MAX_COUNT) {
            assert!(label.len() <= CARD_BYTES, "{label}");
        }
        assert_eq!(label(name, 1), "A Really Long Camera P Mirror 1");
    }

    #[test]
    fn a_cut_ending_in_a_space_is_trimmed() {
        // 22 bytes of room: the cut keeps the space after "Camera".
        assert_eq!(
            label("Long Name Here Camera  Extra", 3),
            "Long Name Here Camera Mirror 3"
        );
    }

    #[test]
    fn a_name_that_fits_exactly_is_kept() {
        let name = "Exactly Twenty-Two Byt";
        assert_eq!(name.len(), 22);
        let label = label(name, 5);
        assert_eq!(label, "Exactly Twenty-Two Byt Mirror 5");
        assert_eq!(label.len(), CARD_BYTES);
    }

    #[test]
    fn a_multibyte_name_is_cut_on_a_character_boundary() {
        // 21 bytes of ASCII, then a 2-byte character across the cut at 22.
        let name = "Webkamera 1234567890a\u{e9}";
        assert_eq!(label(name, 1), "Webkamera 1234567890a Mirror 1");
    }

    #[test]
    fn an_empty_name_leaves_the_suffix() {
        assert_eq!(label("", 1), "Mirror 1");
    }
}
