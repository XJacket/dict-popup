//! `dict-input` - the Win32 side of dict-popup.
//!
//! Responsibilities:
//! - global hotkey registration and change notification
//! - selected-text capture via UI Automation, with a hard timeout
//! - tray icon with a menu, resilient to Explorer restarts
//! - single-instance enforcement
//! - optional autostart registration
//!
//! The UI thread never calls Win32 directly: it sends [`Command`]s and receives
//! [`Event`]s.

pub mod autostart;
pub mod hotkey;
pub mod host;
pub mod selection;
pub mod tray;

pub(crate) mod host_impl;

pub use host::{Command, Event, EventSink, Host, HostError, HostOptions};
pub use hotkey::{Hotkey, HotkeyError, Modifiers, VirtualKey};
pub use selection::{Selection, SelectionError};
pub use tray::TrayAction;

/// Dump the UI Automation tree of a window and what each element exposes.
///
/// Diagnostics only (used by `examples/uia_probe.rs` when a capture fails):
///
/// ```text
/// cargo run --release --example uia_probe -- <hwnd>
/// ```
#[doc(hidden)]
pub fn debug_capture_report(hwnd: isize) -> String {
    host_impl::debug_capture_report(hwnd)
}

/// Read the clipboard text (diagnostics only).
#[doc(hidden)]
pub fn debug_clipboard_read() -> Option<String> {
    host_impl::debug_clipboard_read()
}

/// Write the clipboard text (diagnostics only; replaces the current text).
#[doc(hidden)]
pub fn debug_clipboard_write(text: &str) {
    host_impl::debug_clipboard_write(text)
}
