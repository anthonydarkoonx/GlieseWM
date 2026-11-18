//! Key names and modifiers (`bind = SUPER SHIFT, Return, …`).
//!
//! Key numbers are what Metal OS's input driver reports (`metal-driver-rt` `proto::input`, `ev::KEY`): the virtio-input numbering
//! of the main block (PC/AT set 1 order, A = 30), with the navigation keys at 102..=111 and Super at 125/126. Mouse buttons come as
//! `ev::BUTTON` with 1 left, 2 right, 3 middle. The table is written from those numbers.

/// Modifier bits as binds use them. Caps Lock is never part of a bind.
pub mod mods {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
    pub const SUPER: u8 = 8;
}

/// The key of a bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A keyboard key number.
    Code(u16),
    /// A pointer button: 1 left, 2 right, 3 middle, 4.. others.
    Mouse(u8),
}

/// The modifier a key number is, if it is one (left and right are the same).
pub const fn modifier_of(code: u16) -> Option<u8> {
    match code {
        42 | 54 => Some(mods::SHIFT),
        29 | 97 => Some(mods::CTRL),
        56 | 100 => Some(mods::ALT),
        125 | 126 => Some(mods::SUPER),
        _ => None,
    }
}

/// The attention keys a config may never bind (Ctrl+Alt+Delete, Ctrl+Shift+Escape): the compositor handles them before any bind.
pub const fn reserved(m: u8, key: Key) -> bool {
    match key {
        Key::Code(111) => m & mods::CTRL != 0 && m & mods::ALT != 0,
        Key::Code(1) => m & mods::CTRL != 0 && m & mods::SHIFT != 0,
        _ => false,
    }
}

/// Modifier names (any case): `SUPER` (also `WIN`, `LOGO`, `MOD4`), `SHIFT`, `CTRL` (`CONTROL`), `ALT` (`MOD1`).
pub fn modifier(name: &str) -> Option<u8> {
    let mut buf = [0u8; 8];
    let n = upper(name, &mut buf)?;
    Some(match n {
        "SUPER" | "WIN" | "LOGO" | "MOD4" => mods::SUPER,
        "SHIFT" => mods::SHIFT,
        "CTRL" | "CONTROL" => mods::CTRL,
        "ALT" | "MOD1" => mods::ALT,
        _ => return None,
    })
}

/// Parse a modifier list: names separated by spaces or `_` (`SUPER SHIFT`, `SUPER_SHIFT`); empty is no modifier.
pub fn parse_mods(s: &str) -> Result<u8, &str> {
    let mut m = 0;
    for part in s.split([' ', '_', '\t']).filter(|p| !p.is_empty()) {
        m |= modifier(part).ok_or(part)?;
    }
    Ok(m)
}

fn upper<'a>(s: &str, buf: &'a mut [u8; 8]) -> Option<&'a str> {
    if s.len() > buf.len() {
        return None;
    }
    for (d, b) in buf.iter_mut().zip(s.bytes()) {
        *d = b.to_ascii_uppercase();
    }
    core::str::from_utf8(&buf[..s.len()]).ok()
}

/// Named keys (matched without regard to case). Letters and digits are handled in [`parse_key`].
const NAMES: &[(&str, u16)] = &[
    ("escape", 1),
    ("esc", 1),
    ("minus", 12),
    ("equal", 13),
    ("backspace", 14),
    ("tab", 15),
    ("bracketleft", 26),
    ("bracketright", 27),
    ("return", 28),
    ("enter", 28),
    ("semicolon", 39),
    ("apostrophe", 40),
    ("grave", 41),
    ("backslash", 43),
    ("comma", 51),
    ("period", 52),
    ("slash", 53),
    ("space", 57),
    ("capslock", 58),
    ("f1", 59),
    ("f2", 60),
    ("f3", 61),
    ("f4", 62),
    ("f5", 63),
    ("f6", 64),
    ("f7", 65),
    ("f8", 66),
    ("f9", 67),
    ("f10", 68),
    ("f11", 87),
    ("f12", 88),
    ("print", 99),
    ("home", 102),
    ("up", 103),
    ("pageup", 104),
    ("prior", 104),
    ("left", 105),
    ("right", 106),
    ("end", 107),
    ("down", 108),
    ("pagedown", 109),
    ("next", 109),
    ("insert", 110),
    ("delete", 111),
    ("super", 125),
];

const LETTERS: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";
/// The key numbers of a..z.
const LETTER_CODES: [u16; 26] = [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];

/// A key: a letter, a digit, a name from the table, `code:N` (any key number), or `mouse:left|right|middle|N`.
pub fn parse_key(s: &str) -> Option<Key> {
    let s = s.trim();
    if let Some(n) = s.strip_prefix("code:") {
        let c: u16 = n.trim().parse().ok()?;
        return (1..=0x2FF).contains(&c).then_some(Key::Code(c));
    }
    if let Some(b) = s.strip_prefix("mouse:") {
        return match b.trim() {
            "left" => Some(Key::Mouse(1)),
            "right" => Some(Key::Mouse(2)),
            "middle" => Some(Key::Mouse(3)),
            n => n.parse::<u8>().ok().filter(|b| (1..=16).contains(b)).map(Key::Mouse),
        };
    }
    let bytes = s.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0].to_ascii_lowercase();
        if let Some(i) = LETTERS.iter().position(|l| *l == c) {
            return Some(Key::Code(LETTER_CODES[i]));
        }
        if c.is_ascii_digit() {
            // 1..9 are 2..10, 0 is 11
            return Some(Key::Code(if c == b'0' { 11 } else { (c - b'1') as u16 + 2 }));
        }
        return match c {
            b'-' => Some(Key::Code(12)),
            b'=' => Some(Key::Code(13)),
            b',' => Some(Key::Code(51)),
            b'.' => Some(Key::Code(52)),
            b'/' => Some(Key::Code(53)),
            _ => None,
        };
    }
    NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)).map(|(_, c)| Key::Code(*c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(parse_key("A"), Some(Key::Code(30)));
        assert_eq!(parse_key("q"), Some(Key::Code(16)));
        assert_eq!(parse_key("M"), Some(Key::Code(50)));
        assert_eq!(parse_key("1"), Some(Key::Code(2)));
        assert_eq!(parse_key("0"), Some(Key::Code(11)));
        assert_eq!(parse_key("Return"), Some(Key::Code(28)));
        assert_eq!(parse_key("left"), Some(Key::Code(105)));
        assert_eq!(parse_key("F12"), Some(Key::Code(88)));
        assert_eq!(parse_key("code:183"), Some(Key::Code(183)));
        assert_eq!(parse_key("mouse:right"), Some(Key::Mouse(2)));
        assert_eq!(parse_key("mouse:0"), None);
        assert_eq!(parse_key("Nope"), None);
        assert_eq!(parse_key("code:0"), None);
        // every letter has its own number
        let mut v: alloc::vec::Vec<u16> = LETTER_CODES.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), 26);
    }

    #[test]
    fn modifiers() {
        assert_eq!(parse_mods("SUPER SHIFT"), Ok(mods::SUPER | mods::SHIFT));
        assert_eq!(parse_mods("super_ctrl"), Ok(mods::SUPER | mods::CTRL));
        assert_eq!(parse_mods(""), Ok(0));
        assert_eq!(parse_mods("SUPER HYPER"), Err("HYPER"));
        assert!(reserved(mods::CTRL | mods::ALT, Key::Code(111)));
        assert!(reserved(mods::CTRL | mods::SHIFT, Key::Code(1)));
        assert!(!reserved(mods::SUPER, Key::Code(111)));
    }
}
