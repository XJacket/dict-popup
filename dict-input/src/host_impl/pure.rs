//! Pure helpers lifted out of the Win32 plumbing.
//!
//! Everything in this module is free of Win32 calls (only `const`s are borrowed
//! from the `windows` crate), so it can be unit-tested without a desktop
//! session. See `docs/DESIGN.md` §10.

use std::path::Path;

use windows::Win32::UI::Shell::NIN_SELECT;
use windows::Win32::UI::WindowsAndMessaging::{
    WM_APP, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_RBUTTONUP,
};

use crate::hotkey::Hotkey;
use crate::tray::TrayAction;

// ---------------------------------------------------------------------------
// Names handed to Win32
// ---------------------------------------------------------------------------

/// Window class of the hidden host window. The second instance also uses it to
/// find the first instance (`FindWindowW`).
pub(crate) const WINDOW_CLASS: &str = "DictPopupInputHost";
pub(crate) const WINDOW_TITLE: &str = "dict-popup";

/// Named mutex that makes the app single-instance (`Local\` = per logon session).
pub(crate) const MUTEX_NAME: &str = "Local\\dict-popup-single";

/// Session-scoped message id the second instance posts to the first one.
pub(crate) const SHOW_SETTINGS_MESSAGE: &str = "dict-popup-show-settings";

/// Broadcast Explorer sends after it restarts.
pub(crate) const TASKBAR_CREATED_MESSAGE: &str = "TaskbarCreated";

/// `HKCU\...\Run` value name.
pub(crate) const RUN_VALUE_NAME: &str = "dict-popup";

// ---------------------------------------------------------------------------
// Private window messages, tray ids, hotkey ids
// ---------------------------------------------------------------------------

/// Tray callback (`NOTIFYICON_VERSION_4`: LOWORD(lParam) = event message).
pub(crate) const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 1;
/// Posted by [`crate::host::Host::send`] to wake the blocked message loop.
pub(crate) const WAKE_MESSAGE: u32 = WM_APP + 2;
/// Posted by the capture threads once a result (or timeout) is queued.
pub(crate) const CAPTURE_READY_MESSAGE: u32 = WM_APP + 3;

pub(crate) const TRAY_ICON_ID: u32 = 1;
pub(crate) const TRAY_RETRY_TIMER: usize = 1;
pub(crate) const TRAY_RETRY_INTERVAL_MS: u32 = 500;
/// "a few attempts": 10 × 500 ms covers a slow Explorer start-up.
pub(crate) const TRAY_MAX_RETRIES: u32 = 10;

/// Hotkey ids are separate from menu ids; `RegisterHotKey` only needs them to be
/// unique per window. Two ids let us register the replacement before releasing
/// the old hotkey (docs/DESIGN.md §9.9).
pub(crate) const HOTKEY_ID_PRIMARY: i32 = 1;
pub(crate) const HOTKEY_ID_ALTERNATE: i32 = 2;

// ---------------------------------------------------------------------------
// Tray menu
// ---------------------------------------------------------------------------

pub(crate) const MENU_ID_OPEN_SETTINGS: usize = 0x1001;
pub(crate) const MENU_ID_RELOAD_DICTIONARIES: usize = 0x1002;
pub(crate) const MENU_ID_QUIT: usize = 0x1003;

pub(crate) fn menu_id_to_action(id: usize) -> Option<TrayAction> {
    match id {
        MENU_ID_OPEN_SETTINGS => Some(TrayAction::OpenSettings),
        MENU_ID_RELOAD_DICTIONARIES => Some(TrayAction::ReloadDictionaries),
        MENU_ID_QUIT => Some(TrayAction::Quit),
        _ => None,
    }
}

pub(crate) fn action_to_menu_id(action: TrayAction) -> usize {
    match action {
        TrayAction::OpenSettings => MENU_ID_OPEN_SETTINGS,
        TrayAction::ReloadDictionaries => MENU_ID_RELOAD_DICTIONARIES,
        TrayAction::Quit => MENU_ID_QUIT,
    }
}

/// LOWORD(lParam) of the tray callback under `NOTIFYICON_VERSION_4`.
pub(crate) fn tray_event_message(lparam: isize) -> u32 {
    (lparam as u32) & 0xffff
}

/// HIWORD(lParam) of the tray callback under `NOTIFYICON_VERSION_4`.
pub(crate) fn tray_event_icon_id(lparam: isize) -> u32 {
    ((lparam as u32) >> 16) & 0xffff
}

/// Which tray notifications mean "the user wants the menu".
///
/// `NOTIFYICON_VERSION_4` sends `NIN_SELECT` for a left click and
/// `WM_CONTEXTMENU` for a right click; older shells sent `WM_RBUTTONUP` and
/// `WM_LBUTTONDBLCLK`. Accept all of them so the menu is reachable whichever
/// shell version is in play.
pub(crate) fn tray_opens_menu(message: u32) -> bool {
    matches!(
        message,
        WM_CONTEXTMENU | WM_RBUTTONUP | WM_LBUTTONDBLCLK | NIN_SELECT
    )
}

// ---------------------------------------------------------------------------
// Tooltip / balloon string handling
// ---------------------------------------------------------------------------

/// `szTip` is `[u16; 128]`, so at most 127 UTF-16 units plus the terminator.
pub(crate) const SZ_TIP_UNITS: usize = 127;
/// `szInfo` is `[u16; 256]`.
pub(crate) const SZ_INFO_UNITS: usize = 255;
/// `szInfoTitle` is `[u16; 64]`.
pub(crate) const SZ_INFO_TITLE_UNITS: usize = 63;

fn is_high_surrogate(unit: u16) -> bool {
    (0xd800..=0xdbff).contains(&unit)
}

/// Encode `text` as UTF-16 for a fixed-size Win32 buffer.
///
/// Rules: stop at the first embedded NUL (Win32 would stop there anyway), cap at
/// `max_units`, and never leave half of a surrogate pair behind.
pub(crate) fn truncate_utf16(text: &str, max_units: usize) -> Vec<u16> {
    if max_units == 0 {
        return Vec::new();
    }
    let mut units: Vec<u16> = text.encode_utf16().collect();
    if let Some(nul) = units.iter().position(|&unit| unit == 0) {
        units.truncate(nul);
    }
    if units.len() > max_units {
        units.truncate(max_units);
        if units.last().copied().map(is_high_surrogate).unwrap_or(false) {
            units.pop();
        }
    }
    units
}

/// Copy `units` into a NUL-terminated Win32 string buffer, zero-filling the rest.
pub(crate) fn write_sz(destination: &mut [u16], units: &[u16]) {
    if destination.is_empty() {
        return;
    }
    let copy_len = destination.len().saturating_sub(1).min(units.len());
    destination[..copy_len].copy_from_slice(&units[..copy_len]);
    for slot in &mut destination[copy_len..] {
        *slot = 0;
    }
}

// ---------------------------------------------------------------------------
// Autostart value formatting
// ---------------------------------------------------------------------------

/// Value written to `HKCU\...\Run`: the exe path in quotes so spaces survive.
pub(crate) fn run_value(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Whether a stored `Run` value points at `exe`.
///
/// Windows paths are case-insensitive, so the comparison is ASCII-case
/// insensitive. A moved/renamed exe therefore reports "not enabled", which is
/// what the settings checkbox should show.
pub(crate) fn run_value_matches(stored: &str, exe: &Path) -> bool {
    let stored = stored.trim();
    let unquoted = stored
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(stored);
    unquoted.eq_ignore_ascii_case(&exe.to_string_lossy())
}

// ---------------------------------------------------------------------------
// Hotkey bookkeeping / messages
// ---------------------------------------------------------------------------

/// Pick the second hotkey id: the one that is currently free.
pub(crate) fn next_hotkey_id(active: Option<i32>) -> i32 {
    match active {
        Some(HOTKEY_ID_PRIMARY) => HOTKEY_ID_ALTERNATE,
        _ => HOTKEY_ID_PRIMARY,
    }
}

/// User-facing message for a failed (re-)registration. The old hotkey stays
/// active, so say so.
pub(crate) fn hotkey_failure_message(
    attempted: &Hotkey,
    previous: Option<&Hotkey>,
    reason: &str,
) -> String {
    match previous {
        Some(previous) => format!("热键 {attempted} 注册失败：{reason}；继续使用 {previous}"),
        None => format!("热键 {attempted} 注册失败：{reason}；当前没有可用热键"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn tooltip_keeps_short_text() {
        assert_eq!(truncate_utf16("dict-popup", SZ_TIP_UNITS), units("dict-popup"));
        assert_eq!(truncate_utf16("", SZ_TIP_UNITS), Vec::<u16>::new());
    }

    #[test]
    fn tooltip_is_capped_to_sz_tip() {
        let long = "a".repeat(500);
        let out = truncate_utf16(&long, SZ_TIP_UNITS);
        assert_eq!(out.len(), SZ_TIP_UNITS);
        assert!(out.iter().all(|&unit| unit == b'a' as u16));
    }

    #[test]
    fn tooltip_keeps_bmp_characters_whole() {
        // Every CJK character is one UTF-16 unit, so 7 units == 7 characters.
        let out = truncate_utf16(&"词典工具".repeat(40), 7);
        assert_eq!(out.len(), 7);
        assert_eq!(String::from_utf16(&out).unwrap(), "词典工具词典工");
    }

    #[test]
    fn tooltip_never_splits_a_surrogate_pair() {
        // "a😀" is [a, D83D, DE00]; a cap of 2 would end on the high surrogate.
        let out = truncate_utf16("a😀😀", 2);
        assert_eq!(out, vec![b'a' as u16]);
        assert!(String::from_utf16(&out).is_ok());
    }

    #[test]
    fn tooltip_stops_at_an_embedded_nul() {
        assert_eq!(truncate_utf16("abc\0def", SZ_TIP_UNITS), units("abc"));
    }

    #[test]
    fn write_sz_always_terminates() {
        let mut buffer = [0xffffu16; 4];
        write_sz(&mut buffer, &[1, 2, 3, 4, 5]);
        assert_eq!(buffer, [1, 2, 3, 0]);

        let mut buffer = [0xffffu16; 4];
        write_sz(&mut buffer, &[7, 8]);
        assert_eq!(buffer, [7, 8, 0, 0]);
    }

    #[test]
    fn menu_ids_round_trip() {
        for action in [
            TrayAction::OpenSettings,
            TrayAction::ReloadDictionaries,
            TrayAction::Quit,
        ] {
            assert_eq!(
                menu_id_to_action(action_to_menu_id(action)),
                Some(action),
                "menu id round trip failed for {action:?}"
            );
        }
    }

    #[test]
    fn unknown_menu_id_is_ignored() {
        assert_eq!(menu_id_to_action(0), None);
        assert_eq!(menu_id_to_action(0x1004), None);
    }

    #[test]
    fn tray_callback_halves_are_parsed() {
        // LOWORD = message (0x007b), HIWORD = icon id (1).
        let lparam = 0x0001_007b_isize;
        assert_eq!(tray_event_message(lparam), 0x007b);
        assert_eq!(tray_event_icon_id(lparam), 1);
    }

    #[test]
    fn tray_menu_triggers() {
        assert!(tray_opens_menu(WM_CONTEXTMENU));
        assert!(tray_opens_menu(WM_RBUTTONUP));
        assert!(tray_opens_menu(WM_LBUTTONDBLCLK));
        assert!(tray_opens_menu(NIN_SELECT));
        assert!(!tray_opens_menu(0));
        assert!(!tray_opens_menu(WM_APP + 1));
    }

    #[test]
    fn host_messages_do_not_collide() {
        let messages = [TRAY_CALLBACK_MESSAGE, WAKE_MESSAGE, CAPTURE_READY_MESSAGE];
        for (index, left) in messages.iter().enumerate() {
            assert!(*left >= WM_APP, "private messages must live above WM_APP");
            for right in &messages[index + 1..] {
                assert_ne!(left, right, "private message ids must be distinct");
            }
        }
    }

    #[test]
    fn hotkey_ids_alternate() {
        assert_eq!(next_hotkey_id(None), HOTKEY_ID_PRIMARY);
        assert_eq!(next_hotkey_id(Some(HOTKEY_ID_PRIMARY)), HOTKEY_ID_ALTERNATE);
        assert_eq!(next_hotkey_id(Some(HOTKEY_ID_ALTERNATE)), HOTKEY_ID_PRIMARY);
        // An id we do not know about still yields a usable slot.
        assert_eq!(next_hotkey_id(Some(99)), HOTKEY_ID_PRIMARY);
    }

    #[test]
    fn autostart_value_is_quoted() {
        assert_eq!(
            run_value(Path::new(r"C:\Program Files\dict-popup\dict-popup.exe")),
            r#""C:\Program Files\dict-popup\dict-popup.exe""#
        );
        assert_eq!(run_value(Path::new(r"C:\x\app.exe")), r#""C:\x\app.exe""#);
    }

    #[test]
    fn autostart_value_matches_case_insensitively() {
        let exe = Path::new(r"C:\Apps\DictPopup\dict-popup.exe");
        assert!(run_value_matches(&run_value(exe), exe));
        assert!(run_value_matches(&run_value(exe).to_uppercase(), exe));
        assert!(run_value_matches(
            r#"  "C:\Apps\DictPopup\dict-popup.exe"  "#,
            exe
        ));
        // Unquoted legacy values still match.
        assert!(run_value_matches(r"C:\Apps\DictPopup\dict-popup.exe", exe));
    }

    #[test]
    fn autostart_value_rejects_a_stale_path() {
        let exe = Path::new(r"C:\Apps\DictPopup\dict-popup.exe");
        assert!(!run_value_matches(
            r#""D:\Old\dict-popup.exe""#,
            exe
        ));
        assert!(!run_value_matches("", exe));
        assert!(!run_value_matches(
            r#""C:\Apps\DictPopup\dict-popup.exe" --autostart"#,
            exe
        ));
    }

    #[test]
    fn hotkey_failure_message_keeps_the_old_hotkey_visible() {
        let old = crate::hotkey::parse("Ctrl+Alt+D").unwrap();
        let new = crate::hotkey::parse("Ctrl+Alt+E").unwrap();

        let with_old = hotkey_failure_message(&new, Some(&old), "已被占用");
        assert!(with_old.contains("Ctrl+Alt+E"));
        assert!(with_old.contains("Ctrl+Alt+D"));
        assert!(with_old.contains("已被占用"));

        let without_old = hotkey_failure_message(&new, None, "已被占用");
        assert!(without_old.contains("Ctrl+Alt+E"));
        assert!(without_old.contains("没有可用热键"));
    }
}
