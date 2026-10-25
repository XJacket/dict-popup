//! Clipboard-based selection capture (the approach Pot uses).
//!
//! UI Automation only works when an application implements the text patterns
//! properly; in practice many do not (XAML islands, Electron, games, custom
//! toolkits). Simulating `Ctrl+C` and reading the clipboard works everywhere
//! the user can select text.
//!
//! The user's clipboard is not sacrificed: the previous *text* is saved first
//! and written back afterwards. Non-text formats (images, files) cannot be
//! restored this way - that limitation is documented in the README.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_TEXT, CF_UNICODETEXT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use crate::selection::{Selection, SelectionError};

/// How long to wait for the target application to answer the copy request.
const COPY_TIMEOUT: Duration = Duration::from_millis(450);
const POLL_INTERVAL: Duration = Duration::from_millis(15);
/// `OpenClipboard` fails while another process holds it; retry briefly.
const OPEN_ATTEMPTS: usize = 8;

/// Copy the current selection with `Ctrl+C` and return it, restoring the
/// previous clipboard text afterwards.
pub(crate) fn capture() -> Result<Selection, SelectionError> {
    let sequence_before = unsafe { GetClipboardSequenceNumber() };
    let previous = read_text();

    // The hotkey is Ctrl+Alt+D: if those modifiers are still held, the app sees
    // Ctrl+Alt+C and ignores it. Wait (briefly) for the user to let go.
    wait_for_modifiers_released();

    send_ctrl_c()?;

    let deadline = Instant::now() + COPY_TIMEOUT;
    let mut captured = None;
    let mut read_failure: Option<&'static str> = None;
    while Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        // The sequence number only changes when the clipboard is written, which
        // is exactly the signal that the copy happened (or that nothing did).
        if unsafe { GetClipboardSequenceNumber() } != sequence_before {
            match read_text_detailed() {
                Ok(text) => {
                    // The sequence can move for unrelated reasons (another app,
                    // a clipboard manager, our own earlier write). Text that is
                    // identical to what was already there is not a copy.
                    if previous.as_deref() != Some(text.as_str()) {
                        captured = Some(text);
                        break;
                    }
                    read_failure = Some("clipboard text unchanged by the copy");
                }
                Err(reason) => read_failure = Some(reason),
            }
        }
    }

    // Fallback that does not rely on synthetic input reaching the target: ask
    // the focused control to copy itself. Standard edit controls (Notepad,
    // dialogs, many IDE editors) honour WM_COPY across processes.
    if captured.is_none() && unsafe { GetClipboardSequenceNumber() } == sequence_before {
        if send_wm_copy() {
            let fallback_deadline = Instant::now() + Duration::from_millis(300);
            while Instant::now() < fallback_deadline {
                std::thread::sleep(POLL_INTERVAL);
                if unsafe { GetClipboardSequenceNumber() } != sequence_before {
                    match read_text_detailed() {
                        Ok(text) => {
                            if previous.as_deref() != Some(text.as_str()) {
                                captured = Some(text);
                                break;
                            }
                            read_failure = Some("clipboard text unchanged by the copy");
                        }
                        Err(reason) => read_failure = Some(reason),
                    }
                }
            }
        }
    }

    // Sample the sequence BEFORE restoring our own clipboard text: the restore
    // itself bumps the counter and would otherwise make a failed copy look like
    // a successful one.
    let sequence_after_copy = unsafe { GetClipboardSequenceNumber() };

    // Put the user's own clipboard back before doing anything else.
    if let Some(previous) = previous.as_deref() {
        if captured.as_deref() != Some(previous) {
            write_text(previous);
        }
    }

    match captured {
        Some(text) if !text.trim().is_empty() => Ok(Selection { text }),
        _ => {
            let mut message = format!(
                "剪贴板未更新：{}；注入{}",
                diagnostics(sequence_before, sequence_after_copy),
                if sequence_after_copy == sequence_before {
                    "未到达目标窗口（前台程序的控件没有收到 Ctrl+C）"
                } else {
                    "到达了，但读不到文本"
                }
            );
            if let Some(reason) = read_failure {
                message.push_str(&format!("；读取失败：{reason}"));
            }
            Err(SelectionError::Automation(message))
        }
    }
}

/// Human-readable state for a failed copy: which window was in front, and
/// whether the clipboard sequence moved at all. This is what turns "取词失败"
/// into something diagnosable from the log.
fn diagnostics(sequence_before: u32, sequence_after_copy: u32) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};

    // SAFETY: read-only queries with stack buffers of the declared size.
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut class = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        format!(
            "前台窗口 class='{class}' hwnd=0x{:X}；剪贴板序号 {sequence_before}→{sequence_after_copy}",
            hwnd.0 as isize
        )
    }
}

/// Ask the focused control of the foreground window to copy itself (`WM_COPY`).
/// Returns false when there is no focus window to talk to.
fn send_wm_copy() -> bool {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, SendMessageW, GUITHREADINFO,
        WM_COPY,
    };

    // SAFETY: read-only queries plus one documented message; all handles are
    // checked before use.
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        let thread = GetWindowThreadProcessId(foreground, Some(&mut pid));
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread, &mut info).is_err() || info.hwndFocus.is_invalid() {
            return false;
        }
        SendMessageW(info.hwndFocus, WM_COPY, WPARAM(0), LPARAM(0));
        true
    }
}

/// Whether the clipboard still holds the text we captured (used by the tests
/// and diagnostics only).
pub(crate) fn read_text() -> Option<String> {
    read_text_detailed().ok()
}

/// Like [`read_text`] but says *why* it failed. The failure reason is what makes
/// "剪贴板未更新" diagnosable.
pub(crate) fn read_text_detailed() -> Result<String, &'static str> {
    let mut last_error = "OpenClipboard 一直被占用";

    for _ in 0..OPEN_ATTEMPTS {
        // SAFETY: null owner is allowed for clipboard access.
        if unsafe { OpenClipboard(None) }.is_ok() {
            // SAFETY: the clipboard is open; handles are owned by it.
            let result = unsafe { read_clipboard_text() };
            // SAFETY: balances the successful OpenClipboard above.
            let _ = unsafe { CloseClipboard() };
            return result.map_err(|reason| match reason {
                ClipboardReadError::NoUnicodeText => "剪贴板里没有文本（CF_UNICODETEXT/CF_TEXT 都不可用）",
            });
        }
        last_error = "OpenClipboard 一直被占用";
        std::thread::sleep(Duration::from_millis(12));
    }

    Err(last_error)
}

enum ClipboardReadError {
    NoUnicodeText,
}

/// Replace the clipboard text. Best effort: failures are ignored on purpose
/// (the lookup result matters more than a perfect restore).
pub(crate) fn write_text(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();

    for _ in 0..OPEN_ATTEMPTS {
        // SAFETY: null owner; the clipboard is opened for writing below.
        if unsafe { OpenClipboard(None) }.is_err() {
            std::thread::sleep(Duration::from_millis(12));
            continue;
        }

        // SAFETY: the clipboard is open; we own its contents until CloseClipboard.
        unsafe {
            let _ = EmptyClipboard();
            // SAFETY: movable global block of exactly `bytes` bytes.
            if let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
                let pointer = GlobalLock(handle);
                if !pointer.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer as *mut u16, wide.len());
                    let _ = GlobalUnlock(handle);
                    // SAFETY: HGLOBAL and HANDLE are both raw pointers; the
                    // clipboard takes ownership of the block on success.
                    let _ = SetClipboardData(
                        CF_UNICODETEXT.0 as u32,
                        HANDLE(handle.0),
                    );
                }
            }
            let _ = CloseClipboard();
        }
        return;
    }
}

/// # Safety
///
/// The clipboard must be open on this thread.
unsafe fn read_clipboard_text() -> Result<String, ClipboardReadError> {
    // Preferred: UTF-16 text.
    if let Ok(handle) = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) } {
        if let Some(text) = unsafe { read_utf16(handle) } {
            return Ok(text);
        }
    }

    // Clipboard managers and a few older applications publish ANSI text only.
    // Accept it instead of reporting "no text".
    if let Ok(handle) = unsafe { GetClipboardData(CF_TEXT.0 as u32) } {
        if let Some(bytes) = unsafe { read_bytes(handle) } {
            let text = decode_ansi(&bytes);
            if !text.is_empty() {
                return Ok(text);
            }
        }
    }

    Err(ClipboardReadError::NoUnicodeText)
}

/// Copy a UTF-16 clipboard handle into a String, bounded by its allocation.
///
/// # Safety
///
/// The clipboard must be open; `handle` must come from it.
unsafe fn read_utf16(handle: HANDLE) -> Option<String> {
    let global = HGLOBAL(handle.0);
    // SAFETY: `global` is a text handle owned by the clipboard.
    let pointer = unsafe { GlobalLock(global) };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the scan is bounded by the real allocation; reading past it would
    // corrupt the heap (`GlobalSize` is the only trustworthy bound).
    unsafe {
        let capacity = GlobalSize(global) / std::mem::size_of::<u16>();
        let limit = capacity.min(60_000);
        let wide = pointer as *const u16;
        let mut length = 0usize;
        while length < limit && *wide.add(length) != 0 {
            length += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(wide, length));
        let _ = GlobalUnlock(global);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

/// Copy a raw clipboard handle into bytes, bounded and NUL-trimmed.
///
/// # Safety
///
/// The clipboard must be open; `handle` must come from it.
unsafe fn read_bytes(handle: HANDLE) -> Option<Vec<u8>> {
    let global = HGLOBAL(handle.0);
    // SAFETY: `global` is a data handle owned by the clipboard.
    let pointer = unsafe { GlobalLock(global) };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: bounded copy of the block.
    unsafe {
        let size = (GlobalSize(global) as usize).min(60_000);
        let bytes = std::slice::from_raw_parts(pointer as *const u8, size);
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let owned = bytes[..end].to_vec();
        let _ = GlobalUnlock(global);
        if owned.is_empty() {
            None
        } else {
            Some(owned)
        }
    }
}

fn decode_ansi(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{
        MultiByteToWideChar, CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS,
    };

    if bytes.is_empty() {
        return String::new();
    }
    let mut wide = vec![0u16; bytes.len() + 1];
    // SAFETY: both buffers are valid for their declared lengths.
    let written = unsafe {
        MultiByteToWideChar(CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), bytes, Some(&mut wide))
    };
    if written <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&wide[..written as usize])
}

/// Wait until Ctrl/Alt/Shift/Win are physically released (max 600 ms).
///
/// Without this the synthesized `Ctrl+C` arrives while the hotkey's own
/// modifiers are still down, and applications treat it as `Ctrl+Alt+C`.
pub(crate) fn wait_for_modifiers_released() {
    const KEYS: [u16; 5] = [VK_CONTROL.0, VK_MENU.0, VK_SHIFT.0, VK_LWIN.0, VK_RWIN.0];
    let deadline = Instant::now() + Duration::from_millis(600);

    while Instant::now() < deadline {
        // SAFETY: GetAsyncKeyState only reads the keyboard state for a key code.
        let held = KEYS
            .iter()
            .any(|code| (unsafe { GetAsyncKeyState(*code as i32) } as u16 & 0x8000) != 0);
        if !held {
            return;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

/// Diagnostic: type text with `KEYEVENTF_UNICODE` (no virtual keys involved).
/// Returns how many characters the API accepted.
pub(crate) fn debug_type_text(text: &str) -> usize {
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_UNICODE;

    let mut accepted = 0usize;
    for ch in text.chars() {
        for up in [false, true] {
            let mut encode = [0u16; 2];
            let units = ch.encode_utf16(&mut encode);
            for unit in units {
                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(0),
                            wScan: *unit,
                            dwFlags: if up {
                                KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0 | KEYEVENTF_UNICODE.0)
                            } else {
                                KEYBD_EVENT_FLAGS(KEYEVENTF_UNICODE.0)
                            },
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                // SAFETY: one fully initialised INPUT record of the declared size.
                let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
                accepted += sent as usize;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    accepted
}

/// Diagnostic: press a virtual-key chord through `SendInput` (tests only).
pub(crate) fn debug_press_combo(keys: &[u16]) {
    for key in keys {
        let input = [key_input(VIRTUAL_KEY(*key), false)];
        // SAFETY: one fully initialised INPUT record of the declared size.
        let _ = unsafe { SendInput(&input, std::mem::size_of::<INPUT>() as i32) };
        std::thread::sleep(Duration::from_millis(20));
    }
    for key in keys.iter().rev() {
        let input = [key_input(VIRTUAL_KEY(*key), true)];
        // SAFETY: as above.
        let _ = unsafe { SendInput(&input, std::mem::size_of::<INPUT>() as i32) };
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn send_ctrl_c() -> Result<(), SelectionError> {
    // One SendInput call with Ctrl-down, C-down, C-up, Ctrl-up in a single
    // array is acknowledged by the API but many applications coalesce it and
    // never see the chord. Injecting the four events separately with a small
    // gap is what a human typing the shortcut produces, and is recognised.
    const GAP: Duration = Duration::from_millis(25);

    for (key, up) in [
        (VK_CONTROL, false),
        (VIRTUAL_KEY(b'C' as u16), false),
        (VIRTUAL_KEY(b'C' as u16), true),
        (VK_CONTROL, true),
    ] {
        let inputs = [key_input(key, up)];
        // SAFETY: `inputs` is one fully initialised INPUT of the declared size.
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            return Err(SelectionError::Automation(format!(
                "无法发送 Ctrl+C（SendInput 返回 {sent}/1）"
            )));
        }
        std::thread::sleep(GAP);
    }

    Ok(())
}

fn key_input(key: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if up {
                    KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0)
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
