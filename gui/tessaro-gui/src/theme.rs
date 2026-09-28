//! The one look, dark: small text, tight padding and striped tables, in the
//! welcome page's palette (`tessaro-selftest/files/index.html`): its near-black
//! background, faintly lifted panels, its blue and purple glows as chrome and
//! selection, its cyan and purple accents and its status colours. Solid
//! colours only, no gradients. There is no light variant and no following the
//! system's. Every style the views use comes from here, so the windows cannot
//! drift apart.

use iced::widget::{button, container, text};
use iced::{border, Border, Color, Element, Font, Theme};
use tessaro_client::text::{Line, Tone};

/// Manrope, bundled in `fonts/` and loaded in `main.rs`, so the GUI reads the
/// same on every OS instead of taking whatever sans the host has. Monospace
/// text keeps `Font::MONOSPACE`, the host's.
pub const FONT: Font = Font::with_name("Manrope");

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

/// Behind everything: the desk the inner windows sit on. The page's `--bg`.
pub const DESK: Color = rgb(0x0a_0d_14);
/// A window's body and the default background: the desk lifted a little,
/// so a window stands off it.
pub const BACKGROUND: Color = rgb(0x0f_13_1d);
/// Tables, dialogs, the message log: the page's `--card` over the body.
pub const PANEL: Color = rgb(0x14_19_25);
/// Every other table row.
pub const STRIPE: Color = rgb(0x18_1e_2c);
/// Header strips, table headers, status bars: the page's blue glow.
pub const CHROME: Color = rgb(0x10_24_3a);
/// The page's `--line`, a step stronger so edges hold at 1px.
pub const BORDER: Color = rgb(0x22_2c_3e);
pub const TEXT_COLOR: Color = rgb(0xea_f0_f8);
/// The page's `--dim`.
pub const MUTED: Color = rgb(0x8e_9b_b0);
/// The page's `--accent`.
pub const PRIMARY: Color = rgb(0x5c_c8_ff);
/// The page's `--accent-2`: the window with the keyboard.
const ACCENT: Color = rgb(0x9d_8c_ff);
const SUCCESS: Color = rgb(0x5f_e0_a6);
const WARNING: Color = rgb(0xff_c6_6b);
const DANGER: Color = rgb(0xff_7a_7a);
/// Who wrote a journal line, as the ctl's cyan.
const SOURCE: Color = PRIMARY;
/// The selected table row and section list entry: the page's purple glow
/// lifted, so `MUTED` text stays readable on it, not only the text colour.
const SELECTION: Color = rgb(0x2a_23_4d);
const BUTTON: Color = rgb(0x16_2a_42);
/// A hovered button, and the hovered entry of a cell's Copy menu.
pub const BUTTON_HOVER: Color = rgb(0x1e_37_55);
const BUTTON_BORDER: Color = rgb(0x2a_42_62);
const BUTTON_TEXT: Color = TEXT_COLOR;

pub fn theme() -> Theme {
    Theme::custom(
        "Tessaro",
        iced::theme::Palette {
            background: BACKGROUND,
            text: TEXT_COLOR,
            primary: PRIMARY,
            success: SUCCESS,
            warning: WARNING,
            danger: DANGER,
        },
    )
}

/// The color of a tone of the shared text (`tessaro_client::text`), where
/// it has one; the rest is the text color.
pub fn tone_color(tone: Tone) -> Option<Color> {
    match tone {
        Tone::Ok => Some(SUCCESS),
        Tone::Warn | Tone::Secret => Some(WARNING),
        Tone::Bad => Some(DANGER),
        Tone::Label | Tone::Muted => Some(MUTED),
        Tone::Source => Some(SOURCE),
        Tone::Plain | Tone::Heading | Tone::Cmd => None,
    }
}

/// A text style in a tone's color, for a cell that shows a shared line whole.
pub fn toned(tone: Tone) -> impl Fn(&Theme) -> text::Style {
    move |_| text::Style {
        color: tone_color(tone),
    }
}

/// A line of the shared text: each span in its tone's color, headings,
/// commands and secrets bold, the padding kept for monospace columns.
pub fn text_line<'a, M: 'a>(line: &Line, font: Font) -> Element<'a, M> {
    let bold = Font {
        weight: iced::font::Weight::Bold,
        ..font
    };
    let spans: Vec<iced::widget::text::Span<'a, (), Font>> = line
        .0
        .iter()
        .map(|part| {
            let span = iced::widget::span(part.padded()).font(match part.tone {
                Tone::Heading | Tone::Cmd | Tone::Secret | Tone::Bad => bold,
                _ => font,
            });
            match tone_color(part.tone) {
                Some(color) => span.color(color),
                None => span,
            }
        })
        .collect();
    iced::widget::rich_text(spans).size(SMALL).font(font).into()
}

fn line(color: Color) -> Border {
    Border {
        color,
        width: 1.0,
        radius: border::radius(2),
    }
}

/// Buttons in the chrome: light text on a blue that lifts on hover.
fn chrome_button(_: &Theme, status: button::Status) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Active => (BUTTON, BUTTON_TEXT),
        button::Status::Hovered => (BUTTON_HOVER, TEXT_COLOR),
        button::Status::Pressed => (CHROME, TEXT_COLOR),
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
        button::Status::Hovered => rgb(0x80_d4_ff),
        button::Status::Pressed => rgb(0x45_b2_eb),
        button::Status::Disabled => Color { a: 0.4, ..PRIMARY },
    };
    button::Style {
        background: Some(background.into()),
        text_color: if status == button::Status::Disabled {
            Color { a: 0.5, ..DESK }
        } else {
            DESK
        },
        border: line(background),
        ..button::Style::default()
    }
}

/// Keeping a change that otherwise reverts on its own. Dark text, as on the
/// primary button: the page's green is too light for white.
fn success_button(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Active | button::Status::Disabled => SUCCESS,
        button::Status::Hovered => rgb(0x86_e9_bd),
        button::Status::Pressed => rgb(0x4c_c7_92),
    };
    button::Style {
        background: Some(background.into()),
        text_color: DESK,
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

/// A green toolbar button, for confirming a guarded change.
pub fn confirm_tool<'a, M: Clone + 'a>(label: String, message: M) -> Element<'a, M> {
    button(text(label).size(SMALL))
        .padding([2, 8])
        .style(success_button)
        .on_press(message)
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

/// The small flat buttons in an inner window's title bar, around an icon
/// (`icon.rs`).
pub fn title_button<'a, M: Clone + 'a>(icon: Element<'a, M>, message: M) -> Element<'a, M> {
    button(icon)
        .padding([5, 6])
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

/// iced_table2 paints the entire row, including the space below header dividers.
/// Give it our table colors instead of painting separate cell backgrounds.
pub fn table_theme() -> Theme {
    use iced::theme::palette::{Extended, Pair};
    Theme::custom_with_fn("Tessaro tables", theme().palette(), |palette| {
        let mut extended = Extended::generate(palette);
        extended.background.base = Pair::new(PANEL, TEXT_COLOR);
        extended.background.weak = Pair::new(STRIPE, TEXT_COLOR);
        extended.background.strong = Pair::new(CHROME, TEXT_COLOR);
        extended.primary.weak = Pair::new(SELECTION, TEXT_COLOR);
        extended
    })
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
            color: if focused { ACCENT } else { BORDER },
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

/// An inner window's title bar: dark, the one with the keyboard in the blue
/// glow and full text colour. The frame's border is what marks it in colour.
pub fn frame_title(focused: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        if focused {
            container::Style {
                text_color: Some(TEXT_COLOR),
                ..filled(CHROME)
            }
        } else {
            container::Style {
                text_color: Some(MUTED),
                ..filled(PANEL)
            }
        }
    }
}

/// The section list, left in a device window: the selected entry marked.
pub fn nav(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let background = match (selected, status) {
            (true, _) => SELECTION,
            (false, button::Status::Hovered) => CHROME,
            (false, _) => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(background.into()),
            text_color: TEXT_COLOR,
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
