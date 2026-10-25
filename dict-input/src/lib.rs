//! `dict-input` - the Win32 side of dict-popup.
//!
//! Responsibilities:
//! - global hotkey registration and change notification
//! - selected-text capture via one ordered pipeline (Pot crate, clipboard,
//!   `WM_COPY`, UI Automation), with a hard timeout
//! - tray icon with a menu, resilient to Explorer restarts
//! - single-instance enforcement
//! - optional autostart registration
//! - structured capture diagnostics behind [`diagnostics`]
//! - a read-only report of the process's UIAccess / elevation / integrity
//!   level behind [`privilege`]
//!
//! The UI thread never calls Win32 directly: it sends [`Command`]s and receives
//! [`Event`]s.

pub mod autostart;
pub mod capture;
pub mod diagnostics;
pub mod hotkey;
pub mod host;
pub mod privilege;
pub mod tray;

pub(crate) mod host_impl;

pub use capture::{CaptureFailure, CaptureTarget, CapturedText, TrimSelection};
pub use host::{Command, Event, EventSink, Host, HostError, HostOptions};
pub use hotkey::{Hotkey, HotkeyError, Modifiers, VirtualKey};
pub use privilege::{Integrity, ProcessPrivilege};
pub use tray::TrayAction;
