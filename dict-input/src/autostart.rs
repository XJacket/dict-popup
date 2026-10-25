//! Autostart registration under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use std::io;

pub const VALUE_NAME: &str = "dict-popup";

/// Enable or disable autostart for the current executable.
pub fn set_enabled(enabled: bool) -> io::Result<()> {
    crate::host_impl::autostart_set(enabled)
}
