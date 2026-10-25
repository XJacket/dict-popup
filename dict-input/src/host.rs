//! The Win32 host: owns the message-loop thread, the global hotkey, the tray
//! icon, single-instance detection, and the capture service.
//!
//! `Host::start` performs the two one-off process-level Win32 calls (DPI
//! awareness and the single-instance mutex), then all Win32 work happens on the
//! dedicated host thread. The UI never blocks on Win32 after `start` returns,
//! and a hung UI Automation provider can never freeze the tray icon or the
//! message pump.

use std::fmt;

use crate::capture::{CaptureFailure, CapturedText};
use crate::hotkey::{Hotkey, HotkeyError};
use crate::tray::TrayAction;

pub use crate::host_impl::anchor::{Anchor, WorkArea};

/// Events pushed to the UI thread.
///
/// All events are delivered from the host thread, one at a time, so the sink
/// never needs its own locking.
#[derive(Debug, Clone)]
pub enum Event {
    /// The global hotkey fired.
    ///
    /// `anchor` is sampled on the host thread in the instant the key was
    /// handled (cursor position, that monitor's work area, and its scale
    /// factor). The UI places the card from it, so the position does not
    /// depend on where the mouse or the focus is when the capture result
    /// arrives up to one [`crate::capture::CAPTURE_BUDGET`] later.
    Hotkey { anchor: Anchor },
    /// A validated, non-empty selection was captured successfully.
    Selection(CapturedText),
    /// Capture failed; the UI decides whether to beep or show a hint.
    SelectionFailed(CaptureFailure),
    /// A hotkey could not be registered (initial registration or
    /// [`Command::SetHotkey`]).
    ///
    /// The payload is a complete, user-facing Chinese message that names the
    /// hotkey and the reason, e.g. `"热键 Ctrl+Alt+E 注册失败：… ；继续使用
    /// Ctrl+Alt+D"`. When the previous hotkey still works it stays active; see
    /// [`Command::SetHotkey`].
    HotkeyFailed(String),
    /// The user chose something from the tray menu.
    Tray(TrayAction),
    /// A second copy of the app was started; the running instance should
    /// surface its settings window.
    SecondLaunch,
}

/// Commands the UI sends to the host.
#[derive(Debug, Clone)]
pub enum Command {
    /// Replace the hotkey (None unregisters it).
    ///
    /// The new hotkey is registered under a second id before the old one is
    /// released. On failure the old hotkey remains active and a
    /// [`Event::HotkeyFailed`] carries the reason.
    SetHotkey(Option<Hotkey>),
    /// Update the tray tooltip (usually "dict-popup · N 本词典").
    SetTooltip(String),
    /// Show a tray balloon when something needs the user's attention.
    Notify { title: String, body: String },
    /// Apply the UI's `behavior.trim_selection` preference to future captures.
    ///
    /// Trimming happens when the text is created (see
    /// [`crate::CapturedText`]), so the UI never has to trim on its side.
    SetTrimSelection(bool),
    /// Stop the message loop and remove the tray icon.
    Shutdown,
}

#[derive(Debug)]
pub enum HostError {
    /// Another instance already owns the single-instance mutex.
    AlreadyRunning,
    /// Window class / message window creation failed.
    Win32(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::AlreadyRunning => write!(f, "已经有一个实例在运行"),
            HostError::Win32(m) => write!(f, "Win32 初始化失败：{m}"),
        }
    }
}

impl std::error::Error for HostError {}

/// Callback invoked from the host thread for every [`Event`].
pub type EventSink = Box<dyn Fn(Event) + Send + 'static>;

pub struct HostOptions {
    pub hotkey: Result<Hotkey, HotkeyError>,
    pub tooltip: String,
    /// Apply `behavior.trim_selection` to captured text (see
    /// [`Command::SetTrimSelection`] for changes after startup).
    pub trim_selection: bool,
}

/// Handle to the running host thread. Dropping it does not stop the thread;
/// call [`Host::shutdown`] or send [`Command::Shutdown`].
pub use crate::host_impl::Host;
