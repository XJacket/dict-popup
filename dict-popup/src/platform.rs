//! Small Win32 helpers used by the UI layer that are not part of the host API:
//! opening a folder, a startup/panic message box, and returning the working set
//! to the OS once the card is closed.
//!
//! `docs/DESIGN.md` §3 says the UI thread must not call Win32 directly. These
//! are one-shot, non-blocking calls that the `dict-input` `Event`/`Command` API
//! does not expose, so they live here. The cursor / monitor / DPI queries that
//! used to live here are gone: the host samples them into the hotkey's
//! [`dict_input::host::Anchor`], which is the single source for the card's
//! position and scale.

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_OK, SW_SHOWNORMAL,
};

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
