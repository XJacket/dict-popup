//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` handling.

use std::io;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
};

use super::pure;
use super::win32_util::{error_text, wide};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Owned registry key; closed on drop.
struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW and is closed
        // exactly once, here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open(access: REG_SAM_FLAGS) -> io::Result<RegKey> {
    let path = wide(RUN_KEY);
    let mut key = HKEY(std::ptr::null_mut());
    // SAFETY: `path` is NUL-terminated and outlives the call; `key` is a valid
    // out-parameter.
    let status = unsafe {
        RegOpenKeyExW(
            windows::Win32::System::Registry::HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            access,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(registry_error("RegOpenKeyExW", status.0));
    }
    Ok(RegKey(key))
}

fn registry_error(operation: &str, code: u32) -> io::Error {
    io::Error::new(
        io::ErrorKind::Other,
        format!(
            "{operation} 失败：{}（错误码 {code}）",
            error_text(code)
        ),
    )
}

/// Decode a `REG_SZ` payload. Trailing bytes after the first NUL are ignored,
/// as is an odd trailing byte.
pub(crate) fn decode_reg_sz(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

fn encode_reg_sz(text: &str) -> Vec<u8> {
    wide(text)
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

/// Is the app registered to start with Windows for *this* executable?
///
/// "Registered" means the value exists and points at the running exe; a stale
/// value from a moved/renamed build reports `false` so the settings checkbox is
/// honest and re-enabling rewrites it.
pub(crate) fn query() -> io::Result<bool> {
    let exe = std::env::current_exe()?;
    let key = open(KEY_QUERY_VALUE)?;
    let name = wide(pure::RUN_VALUE_NAME);
    let name = PCWSTR::from_raw(name.as_ptr());

    let mut kind = REG_VALUE_TYPE(0);
    let mut size = 0u32;
    // SAFETY: `name` is NUL-terminated; all out-parameters are valid; a null
    // data pointer with a non-null size pointer asks only for the size.
    let status = unsafe {
        RegQueryValueExW(key.0, name, None, Some(&mut kind), None, Some(&mut size))
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if status != ERROR_SUCCESS {
        return Err(registry_error("RegQueryValueExW", status.0));
    }
    if size == 0 {
        return Ok(false);
    }

    let mut buffer = vec![0u8; size as usize];
    let mut read = size;
    // SAFETY: `buffer` is writable for `read` bytes and both out-parameters are
    // valid for the duration of the call.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name,
            None,
            Some(&mut kind),
            Some(buffer.as_mut_ptr()),
            Some(&mut read),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(registry_error("RegQueryValueExW", status.0));
    }
    buffer.truncate(read as usize);

    Ok(kind == REG_SZ && pure::run_value_matches(&decode_reg_sz(&buffer), &exe))
}

/// Enable or disable autostart, writing the quoted path of the running exe.
pub(crate) fn set(enabled: bool) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let key = open(KEY_SET_VALUE)?;
    let name = wide(pure::RUN_VALUE_NAME);
    let name = PCWSTR::from_raw(name.as_ptr());

    if enabled {
        let value = encode_reg_sz(&pure::run_value(&exe));
        // SAFETY: `name` is NUL-terminated and `value` is a valid REG_SZ payload
        // (UTF-16LE including the terminator); it outlives the call.
        let status = unsafe {
            RegSetValueExW(key.0, name, 0, REG_SZ, Some(value.as_slice()))
        };
        if status != ERROR_SUCCESS {
            return Err(registry_error("RegSetValueExW", status.0));
        }
    } else {
        // SAFETY: `name` is NUL-terminated and belongs to this key.
        let status = unsafe { RegDeleteValueW(key.0, name) };
        if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
            return Err(registry_error("RegDeleteValueW", status.0));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn decode_reg_sz_stops_at_the_terminator() {
        let bytes = encode_reg_sz("C:\\Apps\\dict-popup.exe");
        assert_eq!(decode_reg_sz(&bytes), "C:\\Apps\\dict-popup.exe");
        // Extra data after the NUL (some installers leave junk) is ignored.
        let mut padded = bytes.clone();
        padded.extend_from_slice(&[b'x', 0]);
        assert_eq!(decode_reg_sz(&padded), "C:\\Apps\\dict-popup.exe");
    }

    #[test]
    fn decode_reg_sz_handles_unicode_and_empty_payloads() {
        let bytes = encode_reg_sz(r"C:\词典\dict-popup.exe");
        assert_eq!(decode_reg_sz(&bytes), r"C:\词典\dict-popup.exe");
        assert_eq!(decode_reg_sz(&[]), "");
        assert_eq!(decode_reg_sz(&[0, 0]), "");
        // An odd trailing byte is dropped instead of panicking.
        assert_eq!(decode_reg_sz(&[b'a', 0, b'b']), "a");
    }

    #[test]
    fn encoded_value_round_trips_through_the_matcher() {
        let exe = Path::new(r"C:\Program Files\dict-popup\dict-popup.exe");
        let bytes = encode_reg_sz(&pure::run_value(exe));
        assert!(pure::run_value_matches(&decode_reg_sz(&bytes), exe));
        // REG_SZ payloads are NUL terminated.
        assert_eq!(bytes[bytes.len() - 2..], [0, 0]);
    }
}
