//! Tray icon commands.

/// What the user picked from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenSettings,
    ReloadDictionaries,
    Quit,
}
