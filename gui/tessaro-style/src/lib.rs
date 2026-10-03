//! The look of every Tessaro desktop app, `tessaro-gui` and Try Tessaro: the
//! welcome page's palette, the widget styles, the bundled fonts and the drawn
//! icons. One crate, so the apps cannot drift apart.

pub mod icon;
pub mod theme;

/// Manrope, bundled in `fonts/`, for `iced::Settings::fonts`.
pub fn fonts() -> Vec<std::borrow::Cow<'static, [u8]>> {
    vec![
        include_bytes!("../fonts/Manrope-Regular.ttf")
            .as_slice()
            .into(),
        include_bytes!("../fonts/Manrope-Bold.ttf")
            .as_slice()
            .into(),
    ]
}

/// iced's settings with the fonts loaded and Manrope as the default.
pub fn settings() -> iced::Settings {
    iced::Settings {
        fonts: fonts(),
        default_font: theme::FONT,
        default_text_size: theme::TEXT.into(),
        ..iced::Settings::default()
    }
}
