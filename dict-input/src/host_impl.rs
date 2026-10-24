//! Win32 implementation details. Agents: see `docs/DESIGN.md` for the contract.
//!
//! This file is the only place in the crate that touches Win32 directly.

use std::io;

use crate::host::{EventSink, Host, HostError, HostOptions};

pub(crate) fn start(_options: HostOptions, _sink: EventSink) -> Result<Host, HostError> {
    Err(HostError::Win32(
        "host layer not implemented yet (see docs/DESIGN.md)".to_string(),
    ))
}

pub(crate) fn autostart_query() -> io::Result<bool> {
    Ok(false)
}

pub(crate) fn autostart_set(_enabled: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "autostart not implemented yet",
    ))
}
