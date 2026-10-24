//! The Win32 host: owns the message-loop thread, the global hotkey, the tray
//! icon, single-instance detection, and UIA capture.
//!
//! Everything here runs off the UI thread. The UI never blocks on Win32, and a
//! hung UIA provider can never freeze the tray icon or the message pump.

use std::fmt;

use crate::hotkey::{Hotkey, HotkeyError};
use crate::selection::{Selection, SelectionError};
use crate::tray::TrayAction;

/// Events pushed to the UI thread.
#[derive(Debug, Clone)]
pub enum Event {
    /// The global hotkey fired.
    Hotkey,
    /// A selection was captured successfully.
    Selection(Selection),
    /// Capture failed; the UI decides whether to beep or show a hint.
    SelectionFailed(SelectionError),
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
    /// The hotkey could not be registered (usually taken by another app).
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
}

impl Host {
    /// Start the host. The sink is called from the host thread.
    pub fn start(options: HostOptions, sink: EventSink) -> Result<Self, HostError> {
        crate::host_impl::start(options, sink)
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    /// Ask the host to stop and wait for it.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

pub(crate) fn channel() -> (
    std::sync::mpsc::Sender<Command>,
    std::sync::mpsc::Receiver<Command>,
) {
    std::sync::mpsc::channel()
}
