//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` handling.

use std::io;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, KEY_SET_VALUE,
    REG_SAM_FLAGS, REG_SZ,
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

fn encode_reg_sz(text: &str) -> Vec<u8> {
    wide(text)
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
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
