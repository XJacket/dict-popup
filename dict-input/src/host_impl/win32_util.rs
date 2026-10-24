//! Small Win32 helpers shared by the host implementation.

/// Encode `text` as a NUL-terminated UTF-16 string for a `PCWSTR` parameter.
///
/// The caller must keep the returned buffer alive for the duration of the Win32
/// call; `Vec::as_ptr` alone does not extend the lifetime.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Human-readable text for a Win32 error code (the same text the OS would give).
pub(crate) fn error_text(code: u32) -> String {
    std::io::Error::from_raw_os_error(code as i32).to_string()
}
