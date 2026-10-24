//! Hotkey parsing and formatting.
//!
//! Kept free of Win32 calls so it is unit-testable on any host: the caller
//! turns [`Hotkey`] into `RegisterHotKey` arguments.

use std::fmt;

/// Modifier flags, mirroring `MOD_ALT`/`MOD_CONTROL`/`MOD_SHIFT`/`MOD_WIN`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
    };

    /// `MOD_*` bitmask ready for `RegisterHotKey`.
    pub fn to_win32(self) -> u32 {
        let mut flags = 0;
        if self.alt {
            flags |= 0x0001; // MOD_ALT
        }
        if self.ctrl {
            flags |= 0x0002; // MOD_CONTROL
        }
        if self.shift {
            flags |= 0x0004; // MOD_SHIFT
        }
        if self.win {
            flags |= 0x0008; // MOD_WIN
        }
        // MOD_NOREPEAT: holding the key must not spray lookups.
        flags | 0x4000
    }

    pub fn is_empty(self) -> bool {
        !self.ctrl && !self.alt && !self.shift && !self.win
    }
}

/// A single virtual-key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VirtualKey(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: Modifiers,
    pub vk: VirtualKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyError {
    Empty,
    /// More than one non-modifier key was given.
    MultipleKeys(String),
    /// A token that is neither a modifier nor a known key.
    UnknownKey(String),
    /// Only modifiers, no key ("Ctrl+Alt").
    MissingKey,
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HotkeyError::Empty => write!(f, "热键为空"),
            HotkeyError::MultipleKeys(s) => write!(f, "只能有一个主键，多余的：{s}"),
            HotkeyError::UnknownKey(s) => write!(f, "无法识别的按键：{s}"),
            HotkeyError::MissingKey => write!(f, "缺少主键（例如 D、F5、Space）"),
        }
    }
}

impl std::error::Error for HotkeyError {}

/// Named keys, uppercase, with the virtual-key code they map to.
const NAMED: &[(&str, u32)] = &[
    ("SPACE", 0x20),
    ("TAB", 0x09),
    ("ENTER", 0x0D),
    ("RETURN", 0x0D),
    ("ESC", 0x1B),
    ("ESCAPE", 0x1B),
    ("BACKSPACE", 0x08),
    ("DELETE", 0x2E),
    ("DEL", 0x2E),
    ("INSERT", 0x2D),
    ("INS", 0x2D),
    ("HOME", 0x24),
    ("END", 0x23),
    ("PAGEUP", 0x21),
    ("PGUP", 0x21),
    ("PAGEDOWN", 0x22),
    ("PGDN", 0x22),
    ("UP", 0x26),
    ("DOWN", 0x28),
    ("LEFT", 0x25),
    ("RIGHT", 0x27),
    ("`", 0xC0),
    ("-", 0xBD),
    ("=", 0xBB),
    ("[", 0xDB),
    ("]", 0xDD),
    ("\\", 0xDC),
    (";", 0xBA),
    ("'", 0xDE),
    (",", 0xBC),
    (".", 0xBE),
    ("/", 0xBF),
];

pub fn parse(input: &str) -> Result<Hotkey, HotkeyError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(HotkeyError::Empty);
    }

    let mut mods = Modifiers::NONE;
    let mut key: Option<(VirtualKey, String)> = None;

    for raw in trimmed.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        let upper = token.to_ascii_uppercase();
        match upper.as_str() {
            "CTRL" | "CONTROL" => mods.ctrl = true,
            "ALT" => mods.alt = true,
            "SHIFT" => mods.shift = true,
            "WIN" | "SUPER" | "META" => mods.win = true,
            _ => {
                let vk = parse_key(&upper).ok_or_else(|| HotkeyError::UnknownKey(token.to_string()))?;
                if let Some((_, first)) = &key {
                    return Err(HotkeyError::MultipleKeys(format!("{first}, {token}")));
                }
                key = Some((vk, token.to_string()));
            }
        }
    }

    let (vk, _) = key.ok_or(HotkeyError::MissingKey)?;
    Ok(Hotkey { mods, vk })
}

fn parse_key(upper: &str) -> Option<VirtualKey> {
    if let Some((_, code)) = NAMED.iter().find(|(name, _)| *name == upper) {
        return Some(VirtualKey(*code));
    }

    // Single letter or digit.
    let mut chars = upper.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphabetic() {
            return Some(VirtualKey(0x41 + (c as u32 - 'A' as u32)));
        }
        if c.is_ascii_digit() {
            return Some(VirtualKey(0x30 + (c as u32 - '0' as u32)));
        }
    }

    // Function keys F1..F24.
    if let Some(rest) = upper.strip_prefix('F') {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(VirtualKey(0x70 + n - 1));
            }
        }
    }

    None
}

/// Canonical spelling, modifiers in Ctrl+Alt+Shift+Win order.
impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.mods.ctrl {
            parts.push("Ctrl".into());
        }
        if self.mods.alt {
            parts.push("Alt".into());
        }
        if self.mods.shift {
            parts.push("Shift".into());
        }
        if self.mods.win {
            parts.push("Win".into());
        }
        parts.push(key_name(self.vk));
        write!(f, "{}", parts.join("+"))
    }
}

fn key_name(vk: VirtualKey) -> String {
    let code = vk.0;
    if let Some((name, _)) = NAMED
        .iter()
        .find(|(name, c)| *c == code && *name != "RETURN" && *name != "ESCAPE")
    {
        return prettify(name);
    }
    if (0x41..=0x5A).contains(&code) {
        return char::from_u32(code).unwrap_or('?').to_string();
    }
    if (0x30..=0x39).contains(&code) {
        return char::from_u32(code).unwrap_or('?').to_string();
    }
    if (0x70..=0x87).contains(&code) {
        return format!("F{}", code - 0x70 + 1);
    }
    format!("VK_{code:02X}")
}

fn prettify(name: &str) -> String {
    match name {
        "PGUP" | "PAGEUP" => "PageUp".into(),
        "PGDN" | "PAGEDOWN" => "PageDown".into(),
        "INS" | "INSERT" => "Insert".into(),
        "DEL" | "DELETE" => "Delete".into(),
        "ESC" => "Esc".into(),
        "ENTER" | "RETURN" => "Enter".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_default() {
        let hk = parse("Ctrl+Alt+D").unwrap();
        assert!(hk.mods.ctrl && hk.mods.alt && !hk.mods.shift && !hk.mods.win);
        assert_eq!(hk.vk, VirtualKey(0x44));
    }

    #[test]
    fn is_case_and_space_insensitive() {
        assert_eq!(parse("ctrl+alt+d").unwrap(), parse("CTRL + ALT + D").unwrap());
        assert_eq!(parse("  Ctrl+Alt+D  ").unwrap(), parse("Ctrl+Alt+D").unwrap());
    }

    #[test]
    fn supports_named_and_function_keys() {
        assert_eq!(parse("F9").unwrap().vk, VirtualKey(0x78));
        assert_eq!(parse("F24").unwrap().vk, VirtualKey(0x87));
        assert_eq!(parse("Alt+Space").unwrap().vk, VirtualKey(0x20));
        assert_eq!(parse("Ctrl+`").unwrap().vk, VirtualKey(0xC0));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(parse("").unwrap_err(), HotkeyError::Empty);
        assert_eq!(parse("   ").unwrap_err(), HotkeyError::Empty);
        assert!(matches!(parse("Ctrl+Alt"), Err(HotkeyError::MissingKey)));
        assert!(matches!(
            parse("Ctrl+Foo"),
            Err(HotkeyError::UnknownKey(_))
        ));
        assert!(matches!(
            parse("Ctrl+D+E"),
            Err(HotkeyError::MultipleKeys(_))
        ));
        assert!(matches!(parse("F25"), Err(HotkeyError::UnknownKey(_))));
    }

    #[test]
    fn round_trips_through_display() {
        for input in [
            "Ctrl+Alt+D",
            "Alt+Shift+Q",
            "Win+Space",
            "F9",
            "Ctrl+`",
            "Ctrl+PageDown",
        ] {
            let hk = parse(input).unwrap();
            let rendered = hk.to_string();
            let reparsed = parse(&rendered).unwrap();
            assert_eq!(hk.mods, reparsed.mods, "modifiers lost for {input}");
            assert_eq!(hk.vk, reparsed.vk, "vk lost for {input}");
        }
    }

    #[test]
    fn display_uses_canonical_modifier_order() {
        let hk = parse("Shift+Alt+Ctrl+D").unwrap();
        assert_eq!(hk.to_string(), "Ctrl+Alt+Shift+D");
        assert_eq!(parse("Ctrl+Alt+D").unwrap().to_string(), "Ctrl+Alt+D");
    }

    #[test]
    fn win32_flags_include_norepeat() {
        let hk = parse("Ctrl+D").unwrap();
        let flags = hk.mods.to_win32();
        assert_eq!(flags & 0x0002, 0x0002, "MOD_CONTROL");
        assert_eq!(flags & 0x4000, 0x4000, "MOD_NOREPEAT");
        assert_eq!(flags & 0x0001, 0, "MOD_ALT must be clear");
    }
}
