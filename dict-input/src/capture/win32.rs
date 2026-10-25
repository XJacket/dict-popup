//! The Win32 implementation of [`CaptureEnv`] plus the clipboard/keyboard
//! primitives it needs.
//!
//! This file is deliberately an adapter: no strategy order, no restore policy,
//! no error prose. The pipeline decides *when* to copy and *whether* to keep
//! the result; this module only performs one requested action and reports a
//! typed failure.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_TEXT, CF_UNICODETEXT};
use windows::Win32::UI::Accessibility::IUIAutomation;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CAPITAL, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN,
    VK_SHIFT, VK_SPACE, VK_TAB,
};

use super::env::CaptureEnv;
use super::failure::CaptureFailure;

/// `OpenClipboard` fails while another process holds it; retry briefly.
const OPEN_ATTEMPTS: usize = 8;
/// Delay between clipboard-open retries.
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(12);

/// Win32-backed capture environment for one job.
///
/// The UI Automation object is passed in by the caller so the capture worker
/// can cache it across jobs on its COM-initialised thread.
pub(crate) struct Win32Env<'a> {
    window: Option<isize>,
    automation: &'a mut Option<IUIAutomation>,
    com_ready: bool,
}

impl<'a> Win32Env<'a> {
    pub(crate) fn new(
        window: Option<isize>,
        automation: &'a mut Option<IUIAutomation>,
        com_ready: bool,
    ) -> Self {
        Self {
            window,
            automation,
            com_ready,
        }
    }
}

impl CaptureEnv for Win32Env<'_> {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn pot_selection(&mut self) -> Option<String> {
        // Pot's crate: releases modifier keys and restores the clipboard the
        // way Pot does. An empty result simply falls through to our own path
        // (the validating constructor rejects it).
        Some(selection::get_text())
    }

    fn press_copy(&mut self) -> Result<(), CaptureFailure> {
        // The hotkey is Ctrl+Alt+D: if those modifiers are still held the
        // target sees Ctrl+Alt+C and ignores it. Wait briefly for the user to
        // let go. This is part of the "press copy" edge, not pipeline policy.
        wait_for_modifiers_released();
        send_ctrl_c()
    }

    fn request_wm_copy(&mut self) -> bool {
        send_wm_copy()
    }

    fn clipboard_seq(&self) -> u32 {
        // SAFETY: GetClipboardSequenceNumber takes no arguments and returns a
        // plain counter.
        unsafe { GetClipboardSequenceNumber() }
    }

    fn read_clipboard(&mut self) -> Result<String, CaptureFailure> {
        read_text_detailed()
    }

    fn write_clipboard(&mut self, text: &str) {
        write_text(text);
    }

    fn read_uia(&mut self) -> Result<String, CaptureFailure> {
        super::uia::capture(self.automation, self.com_ready, self.window)
    }
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// Whether the clipboard currently holds text (diagnostics/tests only).
pub(crate) fn read_text() -> Option<String> {
    read_text_detailed().ok()
}

/// Read the clipboard text, or say why it is unavailable.
pub(crate) fn read_text_detailed() -> Result<String, CaptureFailure> {
    for _ in 0..OPEN_ATTEMPTS {
        // SAFETY: a null owner is allowed for clipboard access.
        if unsafe { OpenClipboard(None) }.is_ok() {
            // SAFETY: the clipboard is open on this thread; the returned text
            // handle is owned by it and read before the close below.
            let result = unsafe { read_clipboard_text() };
            // SAFETY: balances the successful OpenClipboard above.
            let _ = unsafe { CloseClipboard() };
            return result;
        }
        std::thread::sleep(OPEN_RETRY_DELAY);
    }

    Err(CaptureFailure::ClipboardBusy)
}

/// # Safety
///
/// The clipboard must be open on this thread.
unsafe fn read_clipboard_text() -> Result<String, CaptureFailure> {
    // Preferred: UTF-16 text.
    // SAFETY: the clipboard is open; GetClipboardData returns a borrowed
    // handle owned by the clipboard.
    if let Ok(handle) = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) } {
        // SAFETY: `handle` came from the open clipboard.
        if let Some(text) = unsafe { read_utf16(handle) } {
            return Ok(text);
        }
    }

    // Clipboard managers and a few older applications publish ANSI text only.
    // SAFETY: as above.
    if let Ok(handle) = unsafe { GetClipboardData(CF_TEXT.0 as u32) } {
        // SAFETY: `handle` came from the open clipboard.
        if let Some(bytes) = unsafe { read_bytes(handle) } {
            let text = decode_ansi(&bytes);
            if !text.is_empty() {
                return Ok(text);
            }
        }
    }

    // No text format at all: from the pipeline's point of view this is the
    // same as "nothing was selected".
    Err(CaptureFailure::NoSelection)
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
        MultiByteToWideChar(
            CP_ACP,
            MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0),
            bytes,
            Some(&mut wide),
        )
    };
    if written <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&wide[..written as usize])
}

/// Replace the clipboard text. Best effort: failures are ignored on purpose
/// (the lookup result matters more than a perfect restore).
pub(crate) fn write_text(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();

    for _ in 0..OPEN_ATTEMPTS {
        // SAFETY: null owner; the clipboard is opened for writing below.
        if unsafe { OpenClipboard(None) }.is_err() {
            std::thread::sleep(OPEN_RETRY_DELAY);
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
                    let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(handle.0));
                }
            }
            let _ = CloseClipboard();
        }
        return;
    }
}

// ---------------------------------------------------------------------------
// Synthetic input / WM_COPY
// ---------------------------------------------------------------------------

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

/// Send an explicit key-up for every modifier and for C, then a clean Ctrl+C.
///
/// This is exactly what Pot's crate does before copying, and the reason its
/// clipboard path works after a hotkey: no stale "Ctrl+Alt is still down" state
/// is left in the target's keyboard input.
pub(crate) fn send_ctrl_c() -> Result<(), CaptureFailure> {
    const RELEASE_FIRST: [VIRTUAL_KEY; 9] = [
        VK_CONTROL,
        VK_MENU,
        VK_SHIFT,
        VK_LWIN,
        VK_SPACE,
        VK_TAB,
        VK_ESCAPE,
        VK_CAPITAL,
        VIRTUAL_KEY(b'C' as u16),
    ];

    for key in RELEASE_FIRST {
        send_one(key, true)?;
        std::thread::sleep(Duration::from_millis(8));
    }

    send_one(VK_CONTROL, false)?;
    std::thread::sleep(Duration::from_millis(8));
    send_one(VIRTUAL_KEY(b'C' as u16), false)?;
    send_one(VIRTUAL_KEY(b'C' as u16), true)?;
    send_one(VK_CONTROL, true)?;

    Ok(())
}

fn send_one(key: VIRTUAL_KEY, up: bool) -> Result<(), CaptureFailure> {
    let inputs = [key_input(key, up)];
    // SAFETY: one fully initialised INPUT record of the declared size.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(CaptureFailure::InputBlocked);
    }
    Ok(())
}

/// Ask the focused control of the foreground window to copy itself
/// (`WM_COPY`). Returns false when there is no focus window to talk to.
fn send_wm_copy() -> bool {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, SendMessageW,
        GUITHREADINFO, WM_COPY,
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

// ---------------------------------------------------------------------------
// Window probes (diagnostics only)
// ---------------------------------------------------------------------------

/// The window that currently holds the foreground, or `None` when the desktop
/// has none (diagnostics only).
pub(crate) fn foreground_window() -> Option<isize> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    // SAFETY: GetForegroundWindow takes no arguments and only reads the
    // foreground window.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        None
    } else {
        Some(foreground.0 as isize)
    }
}

/// Class name of `hwnd`, or `None` when the window is gone.
///
/// Bounded on purpose: a diagnostics report is a log line, so the name is
/// capped at 48 characters.
pub(crate) fn class_name(hwnd: isize) -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

    /// Longest class name kept in a diagnostic report.
    const MAX_CLASS_CHARS: usize = 48;

    let target = HWND(hwnd as *mut core::ffi::c_void);
    if target.is_invalid() {
        return None;
    }
    let mut buffer = [0u16; 256];
    // SAFETY: `buffer` is valid for its declared length and `target` was just
    // checked; GetClassNameW only reads the window's class name.
    let length = unsafe { GetClassNameW(target, &mut buffer) };
    if length <= 0 {
        return None;
    }
    let length = (length as usize).min(buffer.len());
    let name = String::from_utf16_lossy(&buffer[..length]);
    Some(name.chars().take(MAX_CLASS_CHARS).collect())
}

// ---------------------------------------------------------------------------
// Diagnostics (probe examples / tests only)
// ---------------------------------------------------------------------------

/// Diagnostic: type text with `KEYEVENTF_UNICODE` (no virtual keys involved).
/// Returns how many characters the API accepted.
pub(crate) fn type_text(text: &str) -> usize {
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
pub(crate) fn press_combo(keys: &[u16]) {
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
