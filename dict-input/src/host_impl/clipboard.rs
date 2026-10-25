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
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
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
    while Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        // The sequence number only changes when the clipboard is written, which
        // is exactly the signal that the copy happened (or that nothing did).
        if unsafe { GetClipboardSequenceNumber() } != sequence_before {
            if let Some(text) = read_text() {
                captured = Some(text);
                break;
            }
        }
    }

    // Put the user's own clipboard back before doing anything else.
    if let Some(previous) = previous.as_deref() {
        if captured.as_deref() != Some(previous) {
            write_text(previous);
        }
    }

    match captured {
        Some(text) if !text.trim().is_empty() => Ok(Selection { text }),
        _ => Err(SelectionError::Automation(format!(
            "剪贴板未更新：{}",
            diagnostics(sequence_before)
        ))),
    }
}

/// Human-readable state for a failed copy: which window was in front, and
/// whether the clipboard sequence moved at all. This is what turns "取词失败"
/// into something diagnosable from the log.
fn diagnostics(sequence_before: u32) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};

    // SAFETY: read-only queries with stack buffers of the declared size.
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut class = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        let after = GetClipboardSequenceNumber();
        format!(
            "前台窗口 class='{class}' hwnd=0x{:X}；剪贴板序号 {sequence_before}→{after}（{}）",
            hwnd.0 as isize,
            if after == sequence_before {
                "Ctrl+C 未生效"
            } else {
                "剪贴板变了但没有文本"
            }
        )
    }
}

/// Whether the clipboard still holds the text we captured (used by the tests
/// and diagnostics only).
pub(crate) fn read_text() -> Option<String> {
    for _ in 0..OPEN_ATTEMPTS {
        // SAFETY: null owner is allowed for clipboard access.
        if unsafe { OpenClipboard(None) }.is_ok() {
            // SAFETY: the clipboard is open; the handle is owned by it and must
            // not be freed by us.
            let text = unsafe { read_clipboard_text() };
            // SAFETY: balances the successful OpenClipboard above.
            let _ = unsafe { CloseClipboard() };
            return text;
        }
        std::thread::sleep(Duration::from_millis(12));
    }
    None
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
unsafe fn read_clipboard_text() -> Option<String> {
    // SAFETY: the clipboard is open, so this returns the current text handle.
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok()?;
    let global = HGLOBAL(handle.0);

    // SAFETY: `global` is a text handle owned by the clipboard.
    let pointer = unsafe { GlobalLock(global) };
    if pointer.is_null() {
        return None;
    }

    // SAFETY: the block is a NUL-terminated UTF-16 string owned by the clipboard.
    let mut length = 0usize;
    unsafe {
        let wide = pointer as *const u16;
        while *wide.add(length) != 0 {
            length += 1;
            if length > 60_000 {
                break;
            }
        }
        let slice = std::slice::from_raw_parts(wide, length);
        let text = String::from_utf16_lossy(slice);
        let _ = GlobalUnlock(global);
        Some(text)
    }
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
