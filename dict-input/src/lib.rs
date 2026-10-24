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
