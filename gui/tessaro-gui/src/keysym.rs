//! A key as iced reports it, as the X keysym an RFB KeyEvent carries.
//!
//! The device's VNC backend turns the keysym back into a keycode with its
//! own keymap (`vnc_handle_key_event` in Weston's `vnc.c`), adding Shift
//! where the keysym needs it. So a character is sent as the character the
//! user typed, Shift applied, and a named key as its keysym.

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Location};

/// The keysym for `key`, typed at `location`, or `None` for a key VNC has
/// no use for.
pub fn of(key: &Key, location: Location) -> Option<u32> {
    match key {
        Key::Character(text) => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(character(c)),
                _ => None,
            }
        }
        Key::Named(named) => named_key(*named, location),
        Key::Unidentified => None,
    }
}

/// Latin-1 characters are their own keysyms; everything else is the
/// Unicode keysym, 0x01000000 plus the code point.
fn character(c: char) -> u32 {
    match c as u32 {
        code @ (0x20..=0x7e | 0xa0..=0xff) => code,
        code => 0x0100_0000 + code,
    }
}

fn named_key(named: Named, location: Location) -> Option<u32> {
    let right = location == Location::Right;
    let side = |left: u32, right_side: u32| Some(if right { right_side } else { left });
    match named {
        Named::Space => Some(0x0020),
        Named::Backspace => Some(0xff08),
        Named::Tab => Some(0xff09),
        Named::Enter if location == Location::Numpad => Some(0xff8d),
        Named::Enter => Some(0xff0d),
        Named::Pause => Some(0xff13),
        Named::ScrollLock => Some(0xff14),
        Named::Escape => Some(0xff1b),
        Named::Home => Some(0xff50),
        Named::ArrowLeft => Some(0xff51),
        Named::ArrowUp => Some(0xff52),
        Named::ArrowRight => Some(0xff53),
        Named::ArrowDown => Some(0xff54),
        Named::PageUp => Some(0xff55),
        Named::PageDown => Some(0xff56),
        Named::End => Some(0xff57),
        Named::PrintScreen => Some(0xff61),
        Named::Insert => Some(0xff63),
        Named::ContextMenu => Some(0xff67),
        Named::NumLock => Some(0xff7f),
        Named::Delete => Some(0xffff),
        Named::Shift => side(0xffe1, 0xffe2),
        Named::Control => side(0xffe3, 0xffe4),
        Named::CapsLock => Some(0xffe5),
        Named::Alt => side(0xffe9, 0xffea),
        Named::AltGraph => Some(0xfe03),
        Named::Super | Named::Meta => side(0xffeb, 0xffec),
        Named::F1 => Some(0xffbe),
        Named::F2 => Some(0xffbf),
        Named::F3 => Some(0xffc0),
        Named::F4 => Some(0xffc1),
        Named::F5 => Some(0xffc2),
        Named::F6 => Some(0xffc3),
        Named::F7 => Some(0xffc4),
        Named::F8 => Some(0xffc5),
        Named::F9 => Some(0xffc6),
        Named::F10 => Some(0xffc7),
        Named::F11 => Some(0xffc8),
        Named::F12 => Some(0xffc9),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Option<u32> {
        of(&Key::Character(text.into()), Location::Standard)
    }

    #[test]
    fn characters_are_latin1_or_unicode_keysyms() {
        assert_eq!(typed("a"), Some(0x61));
        assert_eq!(typed("A"), Some(0x41));
        assert_eq!(typed("é"), Some(0xe9));
        assert_eq!(typed("č"), Some(0x0100_010d));
        assert_eq!(typed("€"), Some(0x0100_20ac));
        assert_eq!(typed("ab"), None);
    }

    #[test]
    fn named_keys_and_their_sides() {
        let named = |n, at| of(&Key::Named(n), at);
        assert_eq!(named(Named::Enter, Location::Standard), Some(0xff0d));
        assert_eq!(named(Named::Enter, Location::Numpad), Some(0xff8d));
        assert_eq!(named(Named::Shift, Location::Left), Some(0xffe1));
        assert_eq!(named(Named::Shift, Location::Right), Some(0xffe2));
        assert_eq!(named(Named::Control, Location::Left), Some(0xffe3));
        assert_eq!(named(Named::Space, Location::Standard), Some(0x20));
        assert_eq!(named(Named::F12, Location::Standard), Some(0xffc9));
        assert_eq!(named(Named::BrowserBack, Location::Standard), None);
        assert_eq!(of(&Key::Unidentified, Location::Standard), None);
    }
}
