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

impl VirtualKey {
    /// Look up a key by name (case-insensitive).
    ///
    /// This is the one key-name table: the config parser and the settings UI
    /// adapter both resolve keys through it.
    pub fn from_name(name: &str) -> Option<Self> {
        let upper = name.to_ascii_uppercase();
        if let Some((_, code)) = NAMED.iter().find(|(named, _)| *named == upper) {
            return Some(Self(*code));
        }

        // Single letter or digit.
        let mut chars = upper.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(Self(0x41 + (c as u32 - 'A' as u32)));
            }
            if c.is_ascii_digit() {
                return Some(Self(0x30 + (c as u32 - '0' as u32)));
            }
        }

        // Function keys F1..F24.
        if let Some(rest) = upper.strip_prefix('F') {
            if let Ok(n) = rest.parse::<u32>() {
                if (1..=24).contains(&n) {
                    return Some(Self(0x70 + n - 1));
                }
            }
        }

        None
    }

    /// Whether this key has a canonical name, i.e. `dict-input` can register
    /// it. Everything else (media keys, F25.., ...) is rejected by the capture
    /// UI with [`HotkeyError::UnsupportedKey`].
    pub fn is_supported(self) -> bool {
        Self::from_name(&key_name(self)).is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: Modifiers,
    pub vk: VirtualKey,
}

impl Hotkey {
    /// Build a hotkey directly from its parts, without going through text.
    ///
    /// This is the validated constructor used by the settings capture UI: it
    /// enforces the same policy the UI used to implement itself (one of
    /// Ctrl/Alt/Shift/Win, and a key `dict-input` can register). [`parse`]
    /// remains the config-text entry point and stays more permissive: the
    /// shipped config tests require bare keys such as `F9` to parse.
    pub fn new(mods: Modifiers, vk: VirtualKey) -> Result<Self, HotkeyError> {
        if mods.is_empty() {
            return Err(HotkeyError::NoModifier);
        }
        if !vk.is_supported() {
            return Err(HotkeyError::UnsupportedKey);
        }
        Ok(Self { mods, vk })
    }

    /// Canonical spelling used by the settings UI and the config file.
    ///
    /// `Display` and this method share the same implementation, so modifier
    /// order is defined in exactly one place.
    pub fn canonical(&self) -> String {
        self.to_string()
    }
}

impl Default for Hotkey {
    /// `Ctrl+Alt+D`, the shipped default. `config::DEFAULT_HOTKEY` must keep
    /// rendering to the same text (the popup tests assert it).
    fn default() -> Self {
        Self {
            mods: Modifiers {
                ctrl: true,
                alt: true,
                ..Modifiers::NONE
            },
            vk: VirtualKey(b'D' as u32),
        }
    }
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
    /// The capture UI saw no modifier.
    NoModifier,
    /// The key has no virtual-key mapping `dict-input` can register.
    UnsupportedKey,
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HotkeyError::Empty => write!(f, "热键为空"),
            HotkeyError::MultipleKeys(s) => write!(f, "只能有一个主键，多余的：{s}"),
            HotkeyError::UnknownKey(s) => write!(f, "无法识别的按键：{s}"),
            HotkeyError::MissingKey => write!(f, "缺少主键（例如 D、F5、Space）"),
            HotkeyError::NoModifier => write!(f, "全局热键至少需要 Ctrl / Alt / Shift 之一"),
            HotkeyError::UnsupportedKey => write!(f, "不支持这个按键，请换一个"),
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
                let vk = VirtualKey::from_name(&upper)
                    .ok_or_else(|| HotkeyError::UnknownKey(token.to_string()))?;
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

    fn mods(ctrl: bool, alt: bool, shift: bool, win: bool) -> Modifiers {
        Modifiers {
            ctrl,
            alt,
            shift,
            win,
        }
    }

    #[test]
    fn default_is_ctrl_alt_d() {
        let hk = Hotkey::default();
        assert_eq!(hk.canonical(), "Ctrl+Alt+D");
        assert_eq!(parse("Ctrl+Alt+D").unwrap(), hk);
    }

    #[test]
    fn new_matches_parse_of_its_canonical_string() {
        let cases = [
            (mods(true, true, false, false), VirtualKey(0x44)),
            (mods(true, false, false, false), VirtualKey(0x78)),
            (mods(false, true, true, true), VirtualKey(0x20)),
            (mods(false, false, true, false), VirtualKey(0xC0)),
            (mods(true, false, false, false), VirtualKey(0x35)),
        ];

        for (modifiers, key) in cases {
            let hotkey = Hotkey::new(modifiers, key).expect("constructs a valid hotkey");
            assert_eq!(hotkey.canonical(), hotkey.to_string());
            let reparsed = parse(&hotkey.canonical()).expect("canonical text parses");
            assert_eq!(reparsed, hotkey, "round trip failed for {hotkey}");
        }
    }

    #[test]
    fn new_renders_modifiers_in_canonical_order() {
        let hotkey = Hotkey::new(mods(true, true, true, true), VirtualKey(0x44))
            .expect("constructs a valid hotkey");
        assert_eq!(hotkey.canonical(), "Ctrl+Alt+Shift+Win+D");
    }

    #[test]
    fn canonical_order_does_not_depend_on_input_order() {
        for input in ["Shift+Alt+Ctrl+D", "Alt+Shift+Ctrl+D", "Ctrl+Shift+Alt+D"] {
            assert_eq!(parse(input).unwrap().canonical(), "Ctrl+Alt+Shift+D");
        }
        assert_eq!(
            parse("Win+Shift+Alt+Ctrl+D").unwrap().canonical(),
            "Ctrl+Alt+Shift+Win+D"
        );
    }

    #[test]
    fn new_rejects_missing_modifier_and_unsupported_keys() {
        assert_eq!(
            Hotkey::new(Modifiers::NONE, VirtualKey(0x44)).unwrap_err(),
            HotkeyError::NoModifier
        );
        assert_eq!(
            Hotkey::new(mods(true, false, false, false), VirtualKey(0x88)).unwrap_err(),
            HotkeyError::UnsupportedKey
        );
        assert!(Hotkey::new(mods(false, true, false, false), VirtualKey(0x00)).is_err());
    }

    #[test]
    fn every_error_has_its_own_sentence() {
        let cases = [
            (HotkeyError::Empty, "热键为空"),
            (
                HotkeyError::MultipleKeys("D, E".to_string()),
                "只能有一个主键，多余的：D, E",
            ),
            (
                HotkeyError::UnknownKey("Foo".to_string()),
                "无法识别的按键：Foo",
            ),
            (HotkeyError::MissingKey, "缺少主键（例如 D、F5、Space）"),
            (
                HotkeyError::NoModifier,
                "全局热键至少需要 Ctrl / Alt / Shift 之一",
            ),
            (HotkeyError::UnsupportedKey, "不支持这个按键，请换一个"),
        ];

        for (error, sentence) in cases {
            assert_eq!(error.to_string(), sentence);
        }
    }

    #[test]
    fn key_names_resolve_through_one_table() {
        for (name, code) in [
            ("D", 0x44),
            ("f9", 0x78),
            ("Space", 0x20),
            ("PageDown", 0x22),
            ("-", 0xBD),
            ("`", 0xC0),
        ] {
            let key = VirtualKey::from_name(name).expect("known key");
            assert_eq!(key, VirtualKey(code));
            assert!(key.is_supported(), "{name} must be supported");
        }
        assert!(!VirtualKey(0x88).is_supported());
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
