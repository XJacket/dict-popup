//! Autostart registration under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use std::io;

pub const VALUE_NAME: &str = "dict-popup";

/// Whether the app is registered to start with Windows.
pub fn is_enabled() -> bool {
    crate::host_impl::autostart_query().unwrap_or(false)
}

/// Enable or disable autostart for the current executable.
pub fn set_enabled(enabled: bool) -> io::Result<()> {
    crate::host_impl::autostart_set(enabled)
}

/// Path of the running executable (used for display and registration).
pub fn current_exe() -> io::Result<std::path::PathBuf> {
    std::env::current_exe()
}
