//! Small Win32 helpers used by the UI layer: cursor position, monitor work
//! area + DPI, opening a folder, and a startup/panic message box.
//!
//! `docs/DESIGN.md` §3 says the UI thread must not call Win32 directly. These
//! are read-only, non-blocking `user32`/`shcore` queries that the `dict-input`
//! `Event` API does not expose (an `Event::Hotkey` carries no cursor position
//! and the crate is frozen for this change), so they live here instead of on the
//! host thread. Every call checks its return value and degrades to `None`.

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, MessageBoxW, MB_ICONERROR, MB_OK, SW_SHOWNORMAL,
};

/// Physical-pixel position of the mouse cursor.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: `point` is a valid, writable POINT for the duration of the call.
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some((point.x, point.y))
}

/// Return this process's resident pages to the OS once the card is closed.
///
/// A tray app that sits idle for hours should not hold the pages it touched
/// while rendering a card: the dictionary stays memory-mapped, so a later
/// lookup faults them back in cheaply. Peak working set while the card is
/// *open* is measured in docs/QA.md; this keeps the idle figure low.
pub fn trim_working_set() -> bool {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no cleanup,
    // and K32EmptyWorkingSet only trims this process's own working set.
    unsafe { K32EmptyWorkingSet(GetCurrentProcess()).as_bool() }
}

/// A monitor's usable rectangle (taskbar excluded), in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl WorkArea {
    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }
}

/// Work area of the monitor containing `(x, y)`, plus its scale factor
/// (1.0 = 96 DPI).
pub fn work_area_at(x: i32, y: i32) -> Option<(WorkArea, f32)> {
    // SAFETY: both calls only read the monitor list; the MONITORINFO struct is
    // fully initialised and `cbSize` is set as the API requires.
    unsafe {
        let monitor = MonitorFromPoint(
            POINT { x, y },
            MONITOR_DEFAULTTONEAREST,
        );
        if monitor.is_invalid() {
            return None;
        }

        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }

        let area = WorkArea {
            left: info.rcWork.left,
            top: info.rcWork.top,
            right: info.rcWork.right,
            bottom: info.rcWork.bottom,
        };

        let mut dpi_x = 0u32;
        let mut dpi_y = 0u32;
        let scale = match GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) {
            Ok(()) if dpi_x > 0 => dpi_x as f32 / 96.0,
            _ => 1.0,
        };

        if area.width() <= 0 || area.height() <= 0 {
            return None;
        }

        Some((area, scale))
    }
}

/// Open a file or folder with the shell. Returns `false` (with no panic) when
/// the shell refuses.
pub fn open_path(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    let operation = wide("open");
    let file = wide(&path.to_string_lossy());

    // SAFETY: both strings are NUL-terminated and outlive the call; the HWND is
    // intentionally null so the shell picks the foreground window.
    let result = unsafe {
        ShellExecuteW(
            HWND::default(),
            PCWSTR(operation.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };

    // ShellExecuteW returns a value > 32 on success (the old, documented rule).
    result.0 as isize > 32
}

/// Blocking message box, used for startup failures and the panic hook only.
pub fn message_box(title: &str, text: &str) {
    let title = wide(title);
    let text = wide(text);
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            HWND::default(),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
