//! Key numbers to characters (US layout) and the modifier bits KEY events carry. The numbers are Metal OS's input driver's
//! (virtio-input order; the same table as `gliese-config::keys`).

pub mod mods {
    pub const SHIFT: u32 = 1;
    pub const CTRL: u32 = 2;
    pub const ALT: u32 = 4;
    pub const SUPER: u32 = 8;
    pub const CAPS: u32 = 16;
}

/// The modifier a key is (left and right alike), if it is one.
pub const fn modifier_of(code: u16) -> Option<u32> {
    match code {
        42 | 54 => Some(mods::SHIFT),
        29 | 97 => Some(mods::CTRL),
        56 | 100 => Some(mods::ALT),
        125 | 126 => Some(mods::SUPER),
        _ => None,
    }
}

pub const CAPS_LOCK: u16 = 58;

/// (unshifted, shifted) for keys that make a character.
pub fn chars(code: u16) -> Option<(char, char)> {
    const DIGITS: &[u8; 10] = b"1234567890";
    const SHIFTED: &[u8; 10] = b"!@#$%^&*()";
    let p = |a: u8, b: u8| Some((a as char, b as char));
    match code {
        2..=11 => p(DIGITS[code as usize - 2], SHIFTED[code as usize - 2]),
        12 => p(b'-', b'_'),
        13 => p(b'=', b'+'),
        16..=25 => p(b"qwertyuiop"[code as usize - 16], b"QWERTYUIOP"[code as usize - 16]),
        26 => p(b'[', b'{'),
        27 => p(b']', b'}'),
        30..=38 => p(b"asdfghjkl"[code as usize - 30], b"ASDFGHJKL"[code as usize - 30]),
        39 => p(b';', b':'),
        40 => p(b'\'', b'"'),
        41 => p(b'`', b'~'),
        43 => p(b'\\', b'|'),
        44..=50 => p(b"zxcvbnm"[code as usize - 44], b"ZXCVBNM"[code as usize - 44]),
        51 => p(b',', b'<'),
        52 => p(b'.', b'>'),
        53 => p(b'/', b'?'),
        57 => p(b' ', b' '),
        _ => None,
    }
}

/// The character a key makes with these modifiers: none with Ctrl, Alt or Super held (those are commands); Caps Lock shifts letters.
pub fn char_for(code: u16, m: u32) -> Option<char> {
    if m & (mods::CTRL | mods::ALT | mods::SUPER) != 0 {
        return None;
    }
    let (lo, hi) = chars(code)?;
    let caps = m & mods::CAPS != 0 && lo.is_ascii_alphabetic();
    Some(if (m & mods::SHIFT != 0) != caps { hi } else { lo })
}
