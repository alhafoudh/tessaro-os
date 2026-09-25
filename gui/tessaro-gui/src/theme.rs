//! The one look, dark: small text, tight padding, grey chrome and striped
//! tables. There is no light variant and no following the system's. Every
//! style the views use comes from here, so the windows cannot drift apart.

use iced::widget::{button, container, text};
use iced::{border, Border, Color, Element, Theme};

/// The size of ordinary text; `default_text_size` in `main.rs`.
pub const TEXT: f32 = 13.0;
/// Cells, toolbars, the status bar.
pub const SMALL: f32 = 12.0;

const fn rgb(hex: u32) -> Color {
    Color::from_rgb(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    )
}

/// Behind everything: the desk the inner windows sit on.
pub const DESK: Color = rgb(0x17_18_1a);
/// A window's body and the default background.
pub const BACKGROUND: Color = rgb(0x1e_1f_22);
/// Tables, dialogs, the message log.
pub const PANEL: Color = rgb(0x26_28_2c);
/// Every other table row.
pub const STRIPE: Color = rgb(0x2a_2c_30);
/// Header strips, table headers, status bars.
pub const CHROME: Color = rgb(0x2f_31_36);
pub const BORDER: Color = rgb(0x3c_3f_44);
pub const TEXT_COLOR: Color = rgb(0xdc_dc_dc);
pub const MUTED: Color = rgb(0x8a_8d_93);
pub const PRIMARY: Color = rgb(0x3d_7b_d9);
const BUTTON: Color = rgb(0x3a_3d_42);
/// A hovered button, and the hovered entry of a cell's Copy menu.
pub const BUTTON_HOVER: Color = rgb(0x46_49_4f);
const BUTTON_BORDER: Color = rgb(0x4a_4d_52);
const BUTTON_TEXT: Color = rgb(0xe6_e6_e6);

pub fn theme() -> Theme {
    Theme::custom(
        "Tessaro",
        iced::theme::Palette {
            background: BACKGROUND,
            text: TEXT_COLOR,
            primary: PRIMARY,
            success: rgb(0x4c_b8_62),
            warning: rgb(0xe0_a4_3a),
            danger: rgb(0xe5_5b_4d),
        },
    )
}

fn line(color: Color) -> Border {
    Border {
        color,
        width: 1.0,
        radius: border::radius(2),
    }
}

/// Buttons in the chrome: light text on a grey that lifts on hover.
fn chrome_button(_: &Theme, status: button::Status) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Active => (BUTTON, BUTTON_TEXT),
        button::Status::Hovered => (BUTTON_HOVER, Color::WHITE),
        button::Status::Pressed => (CHROME, Color::WHITE),
        button::Status::Disabled => (
            Color { a: 0.5, ..BUTTON },
            Color {
                a: 0.4,
                ..BUTTON_TEXT
            },
        ),
    };
    button::Style {
        background: Some(background.into()),
        text_color,
        border: line(BUTTON_BORDER),
        ..button::Style::default()
    }
}

/// The button Enter presses.
fn primary_button(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Active => PRIMARY,
        button::Status::Hovered => rgb(0x52_8d_e6),
        button::Status::Pressed => rgb(0x32_67_b8),
        button::Status::Disabled => Color { a: 0.4, ..PRIMARY },
    };
    button::Style {
        background: Some(background.into()),
        text_color: if status == button::Status::Disabled {
            Color {
                a: 0.5,
                ..Color::WHITE
            }
        } else {
            Color::WHITE
        },
        border: line(background),
        ..button::Style::default()
    }
}

/// A toolbar button. `None` shows it disabled.
pub fn tool<'a, M: Clone + 'a>(label: &'a str, on_press: Option<M>) -> Element<'a, M> {
    button(text(label).size(SMALL))
        .padding([2, 8])
        .style(chrome_button)
        .on_press_maybe(on_press)
        .into()
}

/// A toolbar toggle: pressed-looking while on.
pub fn toggle<'a, M: Clone + 'a>(label: &'a str, on: bool, message: M) -> Element<'a, M> {
    button(text(label).size(SMALL))
        .padding([2, 8])
        .style(if on { primary_button } else { chrome_button })
        .on_press(message)
        .into()
}

/// The button a dialog's Enter presses.
pub fn default_button<'a, M: Clone + 'a>(label: &'a str, on_press: Option<M>) -> Element<'a, M> {
    button(text(label).size(SMALL))
        .padding([3, 14])
        .style(primary_button)
        .on_press_maybe(on_press)
        .into()
}

pub fn dialog_button<'a, M: Clone + 'a>(label: &'a str, on_press: Option<M>) -> Element<'a, M> {
    button(text(label).size(SMALL))
        .padding([3, 14])
        .style(chrome_button)
        .on_press_maybe(on_press)
        .into()
}

/// The small flat buttons in an inner window's title bar.
pub fn title_button<'a, M: Clone + 'a>(label: &'a str, message: M) -> Element<'a, M> {
    button(text(label).size(SMALL))
        .padding([0, 6])
        .style(|_, status| button::Style {
            background: match status {
                button::Status::Hovered | button::Status::Pressed => Some(BUTTON_HOVER.into()),
                _ => None,
            },
            text_color: TEXT_COLOR,
            border: border::rounded(2),
            ..button::Style::default()
        })
        .on_press(message)
        .into()
}

fn filled(background: Color) -> container::Style {
    container::Style {
        background: Some(background.into()),
        text_color: Some(TEXT_COLOR),
        ..container::Style::default()
    }
}

/// A panel with a thin border: tables, dialogs, the message log.
pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: border::radius(0),
        },
        ..filled(PANEL)
    }
}

pub fn table_header(_: &Theme) -> container::Style {
    filled(CHROME)
}

/// A table row: the selected one in the primary colour, every other one
/// faintly striped so a wide row is easy to follow.
pub fn table_row(selected: bool, odd: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| match (selected, odd) {
        (true, _) => container::Style {
            text_color: Some(Color::WHITE),
            ..filled(PRIMARY)
        },
        (false, true) => filled(STRIPE),
        (false, false) => filled(PANEL),
    }
}

/// The dialog title strip.
pub fn title_bar(_: &Theme) -> container::Style {
    filled(CHROME)
}

/// Behind an open dialog: its window dimmed.
pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(
            Color {
                a: 0.45,
                ..Color::BLACK
            }
            .into(),
        ),
        ..container::Style::default()
    }
}

/// Header strips and status bars.
pub fn status_bar(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: border::radius(0),
        },
        ..filled(CHROME)
    }
}

pub fn desk(_: &Theme) -> container::Style {
    filled(DESK)
}

/// An inner window: its body and a border that marks the one on top.
pub fn frame(focused: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        border: Border {
            color: if focused { PRIMARY } else { BORDER },
            width: 1.0,
            radius: border::radius(3),
        },
        shadow: iced::Shadow {
            color: Color {
                a: 0.5,
                ..Color::BLACK
            },
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 14.0,
        },
        ..filled(BACKGROUND)
    }
}

/// An inner window's title bar: the one on top in the primary colour.
pub fn frame_title(focused: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        if focused {
            container::Style {
                text_color: Some(Color::WHITE),
                ..filled(rgb(0x2b_4f_86))
            }
        } else {
            container::Style {
                text_color: Some(MUTED),
                ..filled(CHROME)
            }
        }
    }
}

/// The section list, left in a device window: the selected entry marked.
pub fn nav(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = match (selected, status) {
            (true, _) => PRIMARY,
            (false, button::Status::Hovered) => CHROME,
            (false, _) => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(background.into()),
            text_color: if selected { Color::WHITE } else { TEXT_COLOR },
            border: border::rounded(0),
            ..button::Style::default()
        }
    }
}

/// Muted text: defaults, read-only values, hints.
pub fn muted(_: &Theme) -> text::Style {
    text::Style { color: Some(MUTED) }
}

/// The app's own header strip, above the desk.
pub fn app_header(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: border::radius(0),
        },
        ..filled(CHROME)
    }
}
