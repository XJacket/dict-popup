//! The Win32 host: owns the message-loop thread, the global hotkey, the tray
//! icon, single-instance detection, and UIA capture.
//!
//! `Host::start` performs the two one-off process-level Win32 calls (DPI
//! awareness and the single-instance mutex), then all Win32 work happens on the
//! dedicated host thread. The UI never blocks on Win32 after `start` returns,
//! and a hung UIA provider can never freeze the tray icon or the message pump.

use std::fmt;

use crate::hotkey::{Hotkey, HotkeyError};
use crate::selection::{Selection, SelectionError};
use crate::tray::TrayAction;

/// Events pushed to the UI thread.
///
/// All events are delivered from the host thread, one at a time, so the sink
/// never needs its own locking.
#[derive(Debug, Clone)]
pub enum Event {
    /// The global hotkey fired.
    Hotkey,
    /// A selection was captured successfully.
    Selection(Selection),
    /// Capture failed; the UI decides whether to beep or show a hint.
    SelectionFailed(SelectionError),
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
    /// Stop the message loop and remove the tray icon.
    Shutdown,
}

#[derive(Debug)]
pub enum HostError {
    /// Another instance already owns the single-instance mutex.
    AlreadyRunning,
    /// Kept for API compatibility. The host currently reports hotkey failures
    /// through [`Event::HotkeyFailed`] instead, so a taken hotkey does not
    /// prevent the app (and its tray icon) from starting.
    HotkeyUnavailable(HotkeyError),
    /// Window class / message window creation failed.
    Win32(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::AlreadyRunning => write!(f, "已经有一个实例在运行"),
            HostError::HotkeyUnavailable(e) => write!(f, "热键注册失败：{e}"),
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
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            hotkey: crate::hotkey::parse("Ctrl+Alt+D"),
            tooltip: "dict-popup".to_string(),
        }
    }
}

/// Handle to the running host thread. Dropping it does not stop the thread;
/// call [`Host::shutdown`] or send [`Command::Shutdown`].
pub struct Host {
    tx: std::sync::mpsc::Sender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
    waker: crate::host_impl::Waker,
}

impl Host {
    pub(crate) fn from_parts(
        tx: std::sync::mpsc::Sender<Command>,
        thread: std::thread::JoinHandle<()>,
        waker: crate::host_impl::Waker,
    ) -> Self {
        Self {
            tx,
            thread: Some(thread),
            waker,
        }
    }

    /// Start the host. The sink is called from the host thread.
    ///
    /// Side effects, in order:
    ///
    /// 1. `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` — call `start`
    ///    **before** creating any window so the UI gets correct coordinates.
    ///    If a window already exists the call fails and is ignored.
    /// 2. Single-instance check (`Local\dict-popup-single`). When another
    ///    instance is running it is asked to show its settings window and
    ///    [`HostError::AlreadyRunning`] is returned.
    /// 3. A dedicated thread is spawned and the hidden host window is created;
    ///    `start` returns once that thread is ready. Initial hotkey/tray
    ///    problems arrive as events ([`Event::HotkeyFailed`] can be the first
    ///    event, possibly just before `start` returns).
    pub fn start(options: HostOptions, sink: EventSink) -> Result<Self, HostError> {
        crate::host_impl::start(options, sink)
    }

    /// Queue a command and wake the message loop. Never blocks.
    pub fn send(&self, command: Command) {
        if self.tx.send(command).is_ok() {
            self.waker.wake();
        }
    }

    /// Ask the host to stop and wait for it.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(Command::Shutdown);
        self.waker.wake();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        self.waker.wake();
    }
}

pub(crate) fn channel() -> (
    std::sync::mpsc::Sender<Command>,
    std::sync::mpsc::Receiver<Command>,
) {
    std::sync::mpsc::channel()
}
