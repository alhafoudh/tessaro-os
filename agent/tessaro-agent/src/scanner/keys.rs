//! A scanner in keyboard mode: its key presses back into the text it read.
//!
//! The scanner turns each character into the key that types it in the
//! layout it is set to, so reading it back needs that layout. The layouts
//! are xkeyboard-config's `symbols/<layout>` files, the ones Weston reads,
//! and only the part a scanner uses is read here: the alphanumeric keys and
//! their four levels (plain, Shift, AltGr, Shift+AltGr), with `include`s.
//! No libxkbcommon: the agent is pure Rust, and a scanner presses no dead
//! keys, compose sequences or group switches.
//!
//! Besides the layout: Enter and Tab, the keypad, Ctrl+letter and Ctrl+]
//! for the control characters (GS, 0x1D, is how a GS1 code separates its
//! fields), and Alt with keypad digits, the way Windows types a character
//! by its number.

use std::collections::HashMap;
use std::path::Path;

use super::frame::Input;

/// The evdev codes of the keys a layout names, by xkb's names for them
/// (`keycodes/evdev`, less 8).
const KEYCODES: &[(&str, u16)] = &[
    ("TLDE", 41),
    ("AE01", 2),
    ("AE02", 3),
    ("AE03", 4),
    ("AE04", 5),
    ("AE05", 6),
    ("AE06", 7),
    ("AE07", 8),
    ("AE08", 9),
    ("AE09", 10),
    ("AE10", 11),
    ("AE11", 12),
    ("AE12", 13),
    ("AD01", 16),
    ("AD02", 17),
    ("AD03", 18),
    ("AD04", 19),
    ("AD05", 20),
    ("AD06", 21),
    ("AD07", 22),
    ("AD08", 23),
    ("AD09", 24),
    ("AD10", 25),
    ("AD11", 26),
    ("AD12", 27),
    ("AC01", 30),
    ("AC02", 31),
    ("AC03", 32),
    ("AC04", 33),
    ("AC05", 34),
    ("AC06", 35),
    ("AC07", 36),
    ("AC08", 37),
    ("AC09", 38),
    ("AC10", 39),
    ("AC11", 40),
    ("AC12", 43),
    ("BKSL", 43),
    ("AB01", 44),
    ("AB02", 45),
    ("AB03", 46),
    ("AB04", 47),
    ("AB05", 48),
    ("AB06", 49),
    ("AB07", 50),
    ("AB08", 51),
    ("AB09", 52),
    ("AB10", 53),
    ("LSGT", 86),
    ("SPCE", 57),
];

const KEY_ENTER: u16 = 28;
const KEY_KPENTER: u16 = 96;
const KEY_TAB: u16 = 15;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_LEFTCTRL: u16 = 29;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_LEFTALT: u16 = 56;
const KEY_RIGHTALT: u16 = 100;
const KEY_CAPSLOCK: u16 = 58;
/// `[`, `\` and `]` where a US keyboard has them: Ctrl with them types ESC,
/// FS and GS whatever the layout, as scanners send them.
const KEY_LEFTBRACE: u16 = 26;
const KEY_BACKSLASH: u16 = 43;
const KEY_RIGHTBRACE: u16 = 27;

/// The keypad: its digit, or what else it types.
const KEYPAD: &[(u16, char)] = &[
    (82, '0'),
    (79, '1'),
    (80, '2'),
    (81, '3'),
    (75, '4'),
    (76, '5'),
    (77, '6'),
    (71, '7'),
    (72, '8'),
    (73, '9'),
    (83, '.'),
    (98, '/'),
    (55, '*'),
    (74, '-'),
    (78, '+'),
];

/// The deepest `include` followed: xkeyboard-config's go three deep.
const INCLUDE_DEPTH: usize = 8;

/// What each key types at each level, by evdev code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Keymap {
    keys: HashMap<u16, [Option<char>; 4]>,
}

impl Keymap {
    /// `layout` (`de`, `sk(qwerty)`) from the xkb data in `xkb`. Reads
    /// files: call it from `blocking`.
    pub fn load(xkb: &Path, layout: &str) -> Result<Keymap, String> {
        let mut keymap = Keymap::default();
        let read = |file: &str| std::fs::read_to_string(xkb.join("symbols").join(file)).ok();
        keymap.include(&read, layout, 0)?;
        if keymap.keys.is_empty() {
            return Err(format!("the keyboard layout {layout:?} has no keys"));
        }
        Ok(keymap)
    }

    /// `name` or `name(variant)` from what `read` finds, its own includes
    /// first.
    fn include(
        &mut self,
        read: &dyn Fn(&str) -> Option<String>,
        spec: &str,
        depth: usize,
    ) -> Result<(), String> {
        if depth > INCLUDE_DEPTH {
            return Err(format!("the keyboard layout includes too deep at {spec:?}"));
        }
        for part in spec
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let (file, variant) = match part.split_once('(') {
                Some((file, rest)) => (file, Some(rest.trim_end_matches(')'))),
                None => (part, None),
            };
            // Group switches (`:2`) and options are no concern of a scanner.
            let file = file.split(':').next().unwrap_or(file);
            let source = read(file).ok_or_else(|| {
                format!("no keyboard layout {file:?}; one of xkeyboard-config's, e.g. us or de")
            })?;
            let block = block(&source, variant).ok_or_else(|| {
                format!(
                    "the keyboard layout {file:?} has no variant {:?}",
                    variant.unwrap_or("default")
                )
            })?;
            for statement in statements(block) {
                match statement {
                    Statement::Include(spec) => self.include(read, &spec, depth + 1)?,
                    Statement::Key(name, levels) => {
                        if let Some((_, code)) = KEYCODES.iter().find(|(known, _)| *known == name) {
                            let entry = self.keys.entry(*code).or_default();
                            for (at, level) in levels.into_iter().enumerate().take(4) {
                                entry[at] = level;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// What `code` types at `level` (0 plain, 1 Shift, 2 AltGr, 3 both),
    /// falling back to the level below as xkb's key types do.
    pub fn char(&self, code: u16, level: usize) -> Option<char> {
        let levels = self.keys.get(&code)?;
        levels[level.min(3)].or_else(|| match level {
            3 => levels[2],
            1 => None,
            _ => None,
        })
    }
}

/// The text of the `xkb_symbols` block named `variant`, else the one marked
/// `default`, else the first.
fn block<'a>(source: &'a str, variant: Option<&str>) -> Option<&'a str> {
    let source_clean = source;
    let mut blocks = Vec::new();
    let mut rest = 0;
    while let Some(found) = source_clean[rest..].find("xkb_symbols") {
        let start = rest + found;
        let header_end = source_clean[start..].find('{')? + start;
        let name = source_clean[start..header_end]
            .split('"')
            .nth(1)
            .unwrap_or_default()
            .to_string();
        // What is between the previous block's end and this one's start:
        // its flags, `default` among them.
        let flags_from = blocks
            .last()
            .map(|(_, _, _, end): &(String, bool, usize, usize)| *end)
            .unwrap_or(0);
        let flags = strip_comments(&source_clean[flags_from..start]);
        let is_default = flags.split_whitespace().any(|word| word == "default");
        let end = matching_brace(source_clean, header_end)?;
        blocks.push((name, is_default, header_end + 1, end));
        rest = end;
    }
    let pick = match variant {
        Some(variant) => blocks.iter().find(|(name, ..)| name == variant),
        None => blocks
            .iter()
            .find(|(_, is_default, ..)| *is_default)
            .or(blocks.first()),
    }?;
    Some(&source_clean[pick.2..pick.3])
}

/// Where the brace opened at `open` closes, comments and strings skipped.
fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut at = open;
    let mut in_string = false;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => in_string = !in_string,
            b'/' if !in_string && bytes.get(at + 1) == Some(&b'/') => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
                continue;
            }
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, PartialEq)]
enum Statement {
    Include(String),
    Key(String, Vec<Option<char>>),
}

/// The `include`s and `key`s of a block, in order.
fn statements(block: &str) -> Vec<Statement> {
    let text = strip_comments(block);
    let mut out = Vec::new();
    for statement in text.split(';') {
        let mut statement = statement.trim();
        // An include ends with its quote, not a `;`: the statement after it
        // shares the piece.
        while let Some(rest) = statement.strip_prefix("include") {
            let mut quoted = rest.splitn(3, '"');
            let (Some(_), Some(spec), Some(after)) = (quoted.next(), quoted.next(), quoted.next())
            else {
                break;
            };
            out.push(Statement::Include(spec.to_string()));
            statement = after.trim();
        }
        let Some(at) = statement.find("key <") else {
            continue;
        };
        let words: Vec<&str> = statement[..at].split_whitespace().collect();
        if !words
            .iter()
            .all(|word| matches!(*word, "replace" | "override" | "augment"))
        {
            continue;
        }
        let rest = &statement[at + "key <".len()..];
        let Some((name, body)) = rest.split_once('>') else {
            continue;
        };
        if let Some(levels) = symbols(body) {
            out.push(Statement::Key(name.to_string(), levels));
        }
    }
    out
}

/// The first group's keysyms of a key's body: its bare `[ ... ]`, or its
/// `symbols[Group1]= [ ... ]`.
fn symbols(body: &str) -> Option<Vec<Option<char>>> {
    let bytes = body.as_bytes();
    for (at, byte) in bytes.iter().enumerate() {
        if *byte != b'[' {
            continue;
        }
        let before = body[..at].trim_end();
        let wanted = match before.chars().last() {
            Some('{') | Some(',') => true,
            Some('=') => {
                let name = before[..before.len() - 1].trim_end();
                name.trim_end_matches(|ch: char| ch != '[')
                    .trim_end_matches('[')
                    .trim_end()
                    .ends_with("symbols")
            }
            _ => false,
        };
        if !wanted {
            continue;
        }
        let end = body[at..].find(']')? + at;
        return Some(
            body[at + 1..end]
                .split(',')
                .map(|name| keysym(name.trim()))
                .collect(),
        );
    }
    None
}

/// The character a keysym types, by its xkb name; none for a dead key or
/// one that types nothing.
pub fn keysym(name: &str) -> Option<char> {
    let mut chars = name.chars();
    if let (Some(only), None) = (chars.next(), chars.next()) {
        return only.is_ascii_alphanumeric().then_some(only);
    }
    if let Some(hex) = name.strip_prefix('U') {
        if (4..=6).contains(&hex.len()) {
            return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
        }
    }
    KEYSYMS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, ch)| *ch)
}

/// The keysyms with names: ASCII's punctuation, Latin-1 and the letters of
/// the Central European layouts.
const KEYSYMS: &[(&str, char)] = &[
    ("space", ' '),
    ("exclam", '!'),
    ("quotedbl", '"'),
    ("numbersign", '#'),
    ("dollar", '$'),
    ("percent", '%'),
    ("ampersand", '&'),
    ("apostrophe", '\''),
    ("quoteright", '\''),
    ("parenleft", '('),
    ("parenright", ')'),
    ("asterisk", '*'),
    ("plus", '+'),
    ("comma", ','),
    ("minus", '-'),
    ("period", '.'),
    ("slash", '/'),
    ("colon", ':'),
    ("semicolon", ';'),
    ("less", '<'),
    ("equal", '='),
    ("greater", '>'),
    ("question", '?'),
    ("at", '@'),
    ("bracketleft", '['),
    ("backslash", '\\'),
    ("bracketright", ']'),
    ("asciicircum", '^'),
    ("underscore", '_'),
    ("grave", '`'),
    ("quoteleft", '`'),
    ("braceleft", '{'),
    ("bar", '|'),
    ("braceright", '}'),
    ("asciitilde", '~'),
    ("nobreakspace", '\u{a0}'),
    ("exclamdown", '¡'),
    ("cent", '¢'),
    ("sterling", '£'),
    ("currency", '¤'),
    ("yen", '¥'),
    ("brokenbar", '¦'),
    ("section", '§'),
    ("diaeresis", '¨'),
    ("copyright", '©'),
    ("ordfeminine", 'ª'),
    ("guillemotleft", '«'),
    ("notsign", '¬'),
    ("hyphen", '\u{ad}'),
    ("registered", '®'),
    ("macron", '¯'),
    ("degree", '°'),
    ("plusminus", '±'),
    ("twosuperior", '²'),
    ("threesuperior", '³'),
    ("acute", '´'),
    ("mu", 'µ'),
    ("paragraph", '¶'),
    ("periodcentered", '·'),
    ("cedilla", '¸'),
    ("onesuperior", '¹'),
    ("masculine", 'º'),
    ("guillemotright", '»'),
    ("onequarter", '¼'),
    ("onehalf", '½'),
    ("threequarters", '¾'),
    ("questiondown", '¿'),
    ("multiply", '×'),
    ("division", '÷'),
    ("EuroSign", '€'),
    ("Agrave", 'À'),
    ("Aacute", 'Á'),
    ("Acircumflex", 'Â'),
    ("Atilde", 'Ã'),
    ("Adiaeresis", 'Ä'),
    ("Aring", 'Å'),
    ("AE", 'Æ'),
    ("Ccedilla", 'Ç'),
    ("Egrave", 'È'),
    ("Eacute", 'É'),
    ("Ecircumflex", 'Ê'),
    ("Ediaeresis", 'Ë'),
    ("Igrave", 'Ì'),
    ("Iacute", 'Í'),
    ("Icircumflex", 'Î'),
    ("Idiaeresis", 'Ï'),
    ("ETH", 'Ð'),
    ("Ntilde", 'Ñ'),
    ("Ograve", 'Ò'),
    ("Oacute", 'Ó'),
    ("Ocircumflex", 'Ô'),
    ("Otilde", 'Õ'),
    ("Odiaeresis", 'Ö'),
    ("Ooblique", 'Ø'),
    ("Oslash", 'Ø'),
    ("Ugrave", 'Ù'),
    ("Uacute", 'Ú'),
    ("Ucircumflex", 'Û'),
    ("Udiaeresis", 'Ü'),
    ("Yacute", 'Ý'),
    ("THORN", 'Þ'),
    ("ssharp", 'ß'),
    ("agrave", 'à'),
    ("aacute", 'á'),
    ("acircumflex", 'â'),
    ("atilde", 'ã'),
    ("adiaeresis", 'ä'),
    ("aring", 'å'),
    ("ae", 'æ'),
    ("ccedilla", 'ç'),
    ("egrave", 'è'),
    ("eacute", 'é'),
    ("ecircumflex", 'ê'),
    ("ediaeresis", 'ë'),
    ("igrave", 'ì'),
    ("iacute", 'í'),
    ("icircumflex", 'î'),
    ("idiaeresis", 'ï'),
    ("eth", 'ð'),
    ("ntilde", 'ñ'),
    ("ograve", 'ò'),
    ("oacute", 'ó'),
    ("ocircumflex", 'ô'),
    ("otilde", 'õ'),
    ("odiaeresis", 'ö'),
    ("oslash", 'ø'),
    ("ugrave", 'ù'),
    ("uacute", 'ú'),
    ("ucircumflex", 'û'),
    ("udiaeresis", 'ü'),
    ("yacute", 'ý'),
    ("thorn", 'þ'),
    ("ydiaeresis", 'ÿ'),
    ("Aogonek", 'Ą'),
    ("aogonek", 'ą'),
    ("Cacute", 'Ć'),
    ("cacute", 'ć'),
    ("Ccaron", 'Č'),
    ("ccaron", 'č'),
    ("Dcaron", 'Ď'),
    ("dcaron", 'ď'),
    ("Dstroke", 'Đ'),
    ("dstroke", 'đ'),
    ("Eogonek", 'Ę'),
    ("eogonek", 'ę'),
    ("Ecaron", 'Ě'),
    ("ecaron", 'ě'),
    ("Lacute", 'Ĺ'),
    ("lacute", 'ĺ'),
    ("Lcaron", 'Ľ'),
    ("lcaron", 'ľ'),
    ("Lstroke", 'Ł'),
    ("lstroke", 'ł'),
    ("Nacute", 'Ń'),
    ("nacute", 'ń'),
    ("Ncaron", 'Ň'),
    ("ncaron", 'ň'),
    ("Odoubleacute", 'Ő'),
    ("odoubleacute", 'ő'),
    ("Racute", 'Ŕ'),
    ("racute", 'ŕ'),
    ("Rcaron", 'Ř'),
    ("rcaron", 'ř'),
    ("Sacute", 'Ś'),
    ("sacute", 'ś'),
    ("Scaron", 'Š'),
    ("scaron", 'š'),
    ("Tcaron", 'Ť'),
    ("tcaron", 'ť'),
    ("Uring", 'Ů'),
    ("uring", 'ů'),
    ("Udoubleacute", 'Ű'),
    ("udoubleacute", 'ű'),
    ("Zacute", 'Ź'),
    ("zacute", 'ź'),
    ("Zabovedot", 'Ż'),
    ("zabovedot", 'ż'),
    ("Zcaron", 'Ž'),
    ("zcaron", 'ž'),
];

/// One scanner's keys as they come, turned into what they type.
#[derive(Debug)]
pub struct Keyboard {
    keymap: Keymap,
    shift: u8,
    ctrl: u8,
    alt: bool,
    altgr: bool,
    caps: bool,
    /// The keypad digits typed while Alt is held.
    numpad: Option<u32>,
}

impl Keyboard {
    pub fn new(keymap: Keymap) -> Self {
        Self {
            keymap,
            shift: 0,
            ctrl: 0,
            alt: false,
            altgr: false,
            caps: false,
            numpad: None,
        }
    }

    /// One key event: `value` is 1 for a press, 0 for a release and 2 for
    /// a repeat, which a scanner never means.
    pub fn key(&mut self, code: u16, value: i32) -> Vec<Input> {
        let pressed = value == 1;
        let released = value == 0;
        match code {
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => {
                self.shift = held(self.shift, pressed, released);
                return Vec::new();
            }
            KEY_LEFTCTRL | KEY_RIGHTCTRL => {
                self.ctrl = held(self.ctrl, pressed, released);
                return Vec::new();
            }
            KEY_RIGHTALT => {
                if pressed || released {
                    self.altgr = pressed;
                }
                return Vec::new();
            }
            KEY_LEFTALT => {
                if pressed {
                    self.alt = true;
                    self.numpad = None;
                } else if released {
                    self.alt = false;
                    if let Some(number) = self.numpad.take() {
                        return number_char(number).map(typed).unwrap_or_default();
                    }
                }
                return Vec::new();
            }
            KEY_CAPSLOCK => {
                if pressed {
                    self.caps = !self.caps;
                }
                return Vec::new();
            }
            _ => {}
        }
        if !pressed {
            return Vec::new();
        }
        match code {
            KEY_ENTER | KEY_KPENTER => return vec![Input::Enter],
            KEY_TAB => return vec![Input::Tab],
            _ => {}
        }
        if let Some((_, ch)) = KEYPAD.iter().find(|(known, _)| *known == code) {
            if self.alt {
                if let Some(digit) = ch.to_digit(10) {
                    self.numpad = Some(self.numpad.unwrap_or(0).saturating_mul(10) + digit);
                }
                return Vec::new();
            }
            return typed(*ch);
        }
        if self.ctrl > 0 {
            let control = match code {
                KEY_LEFTBRACE => Some(0x1b),
                KEY_BACKSLASH => Some(0x1c),
                KEY_RIGHTBRACE => Some(0x1d),
                _ => self
                    .keymap
                    .char(code, 0)
                    .filter(|ch| ch.is_ascii_alphabetic())
                    .map(|ch| (ch.to_ascii_lowercase() as u8) & 0x1f),
            };
            return control
                .map(|byte| vec![Input::Byte(byte)])
                .unwrap_or_default();
        }
        let plain = self.keymap.char(code, if self.altgr { 2 } else { 0 });
        let mut shifted = self.shift > 0;
        if self.caps && plain.is_some_and(char::is_alphabetic) {
            shifted = !shifted;
        }
        let level = usize::from(self.altgr) * 2 + usize::from(shifted);
        self.keymap.char(code, level).map(typed).unwrap_or_default()
    }
}

fn held(count: u8, pressed: bool, released: bool) -> u8 {
    if pressed {
        count.saturating_add(1)
    } else if released {
        count.saturating_sub(1)
    } else {
        count
    }
}

/// What Alt with a number typed: the character with that code, Latin-1
/// above 127, as a scanner set to send it means it.
fn number_char(number: u32) -> Option<char> {
    (number <= 255).then(|| char::from(number as u8))
}

fn typed(ch: char) -> Vec<Input> {
    let mut buffer = [0u8; 4];
    ch.encode_utf8(&mut buffer)
        .bytes()
        .map(Input::Byte)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const US: &str = r#"
// Keyboard layouts for the United States of America.

default partial alphanumeric_keys modifier_keys
xkb_symbols "basic" {
    name[Group1]= "English (US)";
    key <AE01>	{[	 1,	 exclam		]};
    key <AE02>	{[	 2,	 at		]};
    key <AE04>	{[	 4,	 dollar		]};
    key <AE11>	{[   minus,	 underscore	]};
    key <AD01>	{[	 q,	 Q		]};
    key <AD06>	{[	 y,	 Y		]};
    key <AD12>	{[ bracketright, braceright	]};
    key <AC01>	{[	 a,	 A		]};
    key <AB06>	{[	 n,	 N		]};
    key <AB10>	{[   slash,	 question	]};
    key <SPCE>	{[ space ]};
};

partial alphanumeric_keys
xkb_symbols "dvp" {
    key <AE01> { [ ampersand, percent ] };
};
"#;

    const LATIN: &str = r#"
default partial
xkb_symbols "basic" {
    key <AE01>	{ [         1,     exclam,  onesuperior,   exclamdown ] };
    key <AD06>	{ [         y,          Y,    leftarrow,          yen ] };
    key <AB06>	{ [         n,          N,   dead_hook,   dead_horn ] };
};
partial
xkb_symbols "type4" {
    include "latin(basic)"
    key <AD06>	{ [         z,          Z,    leftarrow,          yen ] };
};
"#;

    const SK: &str = r#"
default partial alphanumeric_keys
xkb_symbols "basic" {
    // Slovak.
    include "latin"
    name[Group1] = "Slovak";
    key <AE01>  { [      plus,          1,       exclam,   dead_tilde ] };
    key <AE02>  { [    lcaron,          2,           at,   dead_caron ] };
    key <AD12>  { [adiaeresis,  parenleft, bracketright,     multiply ] };
    key <AC01>  { [         a,          A,   asciitilde,     NoSymbol ] };
    replace key <AB10> { type[Group1]="FOUR_LEVEL", symbols[Group1]= [ minus, underscore, asterisk, NoSymbol ] };
    include "level3(ralt_switch)"
};
"#;

    fn files(name: &str) -> Option<String> {
        match name {
            "us" => Some(US.to_string()),
            "latin" => Some(LATIN.to_string()),
            "sk" => Some(SK.to_string()),
            "level3" => Some("default partial xkb_symbols \"ralt_switch\" { key <RALT> { [ ISO_Level3_Shift ] }; };".to_string()),
            _ => None,
        }
    }

    fn keymap(layout: &str) -> Keymap {
        let mut keymap = Keymap::default();
        keymap.include(&files, layout, 0).unwrap();
        keymap
    }

    fn type_keys(keyboard: &mut Keyboard, keys: &[(u16, i32)]) -> String {
        let bytes: Vec<u8> = keys
            .iter()
            .flat_map(|(code, value)| keyboard.key(*code, *value))
            .map(|input| match input {
                Input::Byte(byte) => byte,
                Input::Enter => b'\n',
                Input::Tab => b'\t',
            })
            .collect();
        String::from_utf8(bytes).unwrap()
    }

    fn tap(code: u16) -> [(u16, i32); 2] {
        [(code, 1), (code, 0)]
    }

    #[test]
    fn a_layout_is_read_with_its_default_variant_and_includes() {
        let us = keymap("us");
        assert_eq!(us.char(2, 0), Some('1'));
        assert_eq!(us.char(2, 1), Some('!'));
        assert_eq!(us.char(57, 0), Some(' '));
        assert_eq!(keymap("us(dvp)").char(2, 0), Some('&'));
        // de-style: the include, then its own keys over it.
        let latin = keymap("latin(type4)");
        assert_eq!(latin.char(21, 0), Some('z'));
        assert_eq!(latin.char(2, 2), Some('¹'));
        // A dead key types nothing.
        assert_eq!(latin.char(49, 2), None);
    }

    #[test]
    fn a_slovak_scanner_types_its_digits_with_shift() {
        let sk = keymap("sk");
        assert_eq!(sk.char(2, 0), Some('+'));
        assert_eq!(sk.char(3, 0), Some('ľ'));
        assert_eq!(sk.char(3, 1), Some('2'));
        assert_eq!(sk.char(27, 2), Some(']'));
        // From latin, and replaced, symbols[Group1] spelled out.
        assert_eq!(sk.char(21, 0), Some('y'));
        assert_eq!(sk.char(53, 1), Some('_'));
        let mut keyboard = Keyboard::new(sk);
        let mut keys = vec![(KEY_LEFTSHIFT, 1)];
        keys.extend(tap(2));
        keys.extend(tap(3));
        keys.push((KEY_LEFTSHIFT, 0));
        keys.extend(tap(KEY_ENTER));
        assert_eq!(type_keys(&mut keyboard, &keys), "12\n");
    }

    #[test]
    fn shift_caps_lock_and_altgr_pick_the_level() {
        let mut keyboard = Keyboard::new(keymap("us"));
        let mut keys = vec![];
        keys.extend(tap(16));
        keys.push((KEY_LEFTSHIFT, 1));
        keys.extend(tap(16));
        keys.extend(tap(3));
        keys.push((KEY_LEFTSHIFT, 0));
        keys.extend(tap(KEY_CAPSLOCK));
        keys.extend(tap(16));
        keys.extend(tap(2));
        assert_eq!(type_keys(&mut keyboard, &keys), "qQ@Q1");

        let mut sk = Keyboard::new(keymap("sk"));
        let mut keys = vec![(KEY_RIGHTALT, 1)];
        keys.extend(tap(27));
        keys.push((KEY_RIGHTALT, 0));
        keys.extend(tap(27));
        assert_eq!(type_keys(&mut sk, &keys), "]ä");
    }

    #[test]
    fn ctrl_types_control_characters_and_gs_by_its_us_place() {
        let mut keyboard = Keyboard::new(keymap("sk"));
        let mut keys = vec![(KEY_LEFTCTRL, 1)];
        keys.extend(tap(KEY_RIGHTBRACE));
        keys.extend(tap(30));
        keys.push((KEY_LEFTCTRL, 0));
        keys.extend(tap(30));
        assert_eq!(type_keys(&mut keyboard, &keys), "\x1d\x01a");
    }

    #[test]
    fn alt_and_keypad_digits_type_a_character_by_its_number() {
        let mut keyboard = Keyboard::new(keymap("us"));
        let mut keys = vec![(KEY_LEFTALT, 1)];
        keys.extend(tap(82));
        keys.extend(tap(80));
        keys.extend(tap(73));
        keys.push((KEY_LEFTALT, 0));
        keys.extend(tap(79));
        keys.extend(tap(KEY_KPENTER));
        keys.extend(tap(KEY_TAB));
        assert_eq!(type_keys(&mut keyboard, &keys), "\x1d1\n\t");
    }

    #[test]
    fn a_repeat_or_a_release_types_nothing() {
        let mut keyboard = Keyboard::new(keymap("us"));
        assert!(keyboard.key(16, 2).is_empty());
        assert!(keyboard.key(16, 0).is_empty());
    }

    #[test]
    fn a_missing_layout_or_variant_says_so() {
        let mut keymap = Keymap::default();
        assert!(keymap
            .include(&files, "xx", 0)
            .unwrap_err()
            .contains("no keyboard layout"));
        assert!(keymap
            .include(&files, "us(nope)", 0)
            .unwrap_err()
            .contains("no variant"));
    }

    #[test]
    fn keysyms_are_read_by_name_letter_or_code_point() {
        assert_eq!(keysym("a"), Some('a'));
        assert_eq!(keysym("bracketright"), Some(']'));
        assert_eq!(keysym("U20AC"), Some('€'));
        assert_eq!(keysym("dead_acute"), None);
        assert_eq!(keysym("NoSymbol"), None);
    }

    /// The real layouts, where the host has them.
    #[test]
    fn the_installed_layouts_read_when_there_are_any() {
        let xkb = Path::new("/usr/share/X11/xkb");
        if !xkb.join("symbols/us").exists() {
            return;
        }
        let us = Keymap::load(xkb, "us").unwrap();
        assert_eq!(us.char(30, 0), Some('a'));
        let de = Keymap::load(xkb, "de").unwrap();
        assert_eq!(de.char(21, 0), Some('z'));
        assert_eq!(de.char(16, 2), Some('@'));
        let sk = Keymap::load(xkb, "sk(qwerty)").unwrap();
        assert_eq!(sk.char(3, 1), Some('2'));
    }
}
