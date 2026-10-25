//! Best-effort description of the current process's Windows privileges.
//!
//! The capture path synthesises `Ctrl+C`. Windows silently drops synthetic
//! input when the sender's integrity level is lower than the target window's
//! (UIPI); the sanctioned fix is a token with UIAccess. This module only
//! **reports** what the current token has: it never requests or changes
//! anything, and a negative answer is informational, not an error. Capture
//! keeps working through its other strategies either way.
//!
//! [`current`] is the only Win32 entry point and is deliberately thin; the
//! classification and formatting helpers are pure and unit-tested.

use std::fmt;

use windows::Win32::Foundation::{BOOL, CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, PSID, TOKEN_ELEVATION,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation, TokenIntegrityLevel, TokenUIAccess,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Mandatory label RID for Low integrity (`S-1-16-4096`).
const LOW_RID: u32 = 0x1000;
/// Mandatory label RID for Medium integrity (`S-1-16-8192`).
const MEDIUM_RID: u32 = 0x2000;
/// Mandatory label RID for High integrity (`S-1-16-12288`).
const HIGH_RID: u32 = 0x3000;
/// Mandatory label RID for System integrity (`S-1-16-16384`).
const SYSTEM_RID: u32 = 0x4000;

/// The mandatory integrity level of a process token.
///
/// Only the four levels Windows documents as stable are named; anything else
/// (for example a custom SID, or a query that failed) is [`Integrity::Unknown`]
/// rather than a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integrity {
    /// Low: sandboxed processes, some browsers' renderers.
    Low,
    /// Medium: a normal unelevated user process (the shipped default).
    Medium,
    /// High: an elevated process.
    High,
    /// System: a service or the SYSTEM account.
    System,
    /// Could not be determined (no token access, unexpected SID, or a call
    /// failure). Never a reason to change behaviour.
    Unknown,
}

impl Integrity {
    /// Classify the last sub-authority of the mandatory label SID.
    ///
    /// Windows defines the four named levels by exact RID; values in between
    /// (for example `0x2100`, "medium plus") are not one of them and map to
    /// [`Integrity::Unknown`].
    pub fn from_rid(rid: u32) -> Self {
        match rid {
            LOW_RID => Self::Low,
            MEDIUM_RID => Self::Medium,
            HIGH_RID => Self::High,
            SYSTEM_RID => Self::System,
            _ => Self::Unknown,
        }
    }

    /// Short, stable English label used in logs and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::System => "System",
            Self::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for Integrity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// What the current process token says about UIAccess, elevation and integrity.
///
/// Plain data: cheap to copy, safe to carry through the UI, and never a
/// capability that this type can grant. All three fields are observations made
/// once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessPrivilege {
    /// The token carries UIAccess (`TokenUIAccess`).
    pub uiaccess: bool,
    /// The token is elevated (`TokenElevation`).
    pub elevated: bool,
    /// The token's mandatory integrity level.
    pub integrity: Integrity,
}

impl fmt::Display for ProcessPrivilege {
    /// The one-line startup/diagnostic form, e.g.
    /// `uiaccess=false elevated=false integrity=Medium`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "uiaccess={} elevated={} integrity={}",
            self.uiaccess, self.elevated, self.integrity
        )
    }
}

/// Query the current process token.
///
/// Best effort by design: every failure path yields `false`/`Unknown` instead
/// of an error or a panic, because this is a status line, not a gate.
pub fn current() -> ProcessPrivilege {
    let mut privilege = ProcessPrivilege {
        uiaccess: false,
        elevated: false,
        integrity: Integrity::Unknown,
    };

    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle for this process and
    // cannot fail; `token` is a valid, writable out-parameter that receives an
    // owned handle only when the call succeeds. TOKEN_QUERY is the access the
    // queries below need.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return privilege;
    }

    // SAFETY: `token` was opened by `OpenProcessToken` above with TOKEN_QUERY,
    // so it is a valid token handle for the duration of these calls. It is
    // closed exactly once, after the last query.
    unsafe {
        privilege.uiaccess = query_uiaccess(token).unwrap_or(false);
        privilege.elevated = query_elevated(token).unwrap_or(false);
        privilege.integrity = query_integrity(token).unwrap_or(Integrity::Unknown);
        let _ = CloseHandle(token);
    }

    privilege
}

/// Read `TokenUIAccess` (a `BOOL`).
///
/// # Safety
///
/// `token` must be a valid process token handle opened with `TOKEN_QUERY`.
unsafe fn query_uiaccess(token: HANDLE) -> Option<bool> {
    let mut value = BOOL::default();
    let mut returned = 0u32;

    // SAFETY: `token` satisfies the caller contract. `value` is a writable
    // BOOL, and the buffer length is exactly `TokenUIAccess`'s documented
    // size (a BOOL).
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenUIAccess,
            Some((&mut value as *mut BOOL).cast()),
            std::mem::size_of::<BOOL>() as u32,
            &mut returned,
        )
    };

    result.ok().map(|()| value.0 != 0)
}

/// Read `TokenElevation`.
///
/// # Safety
///
/// `token` must be a valid process token handle opened with `TOKEN_QUERY`.
unsafe fn query_elevated(token: HANDLE) -> Option<bool> {
    let mut elevation = TOKEN_ELEVATION {
        TokenIsElevated: 0,
    };
    let mut returned = 0u32;

    // SAFETY: `token` satisfies the caller contract. `elevation` is writable
    // and correctly sized/aligned for `TokenElevation`.
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };

    result.ok().map(|()| elevation.TokenIsElevated != 0)
}

/// Read `TokenIntegrityLevel` and classify its SID.
///
/// # Safety
///
/// `token` must be a valid process token handle opened with `TOKEN_QUERY`.
unsafe fn query_integrity(token: HANDLE) -> Option<Integrity> {
    // First call: ask for the required size only. It is expected to fail with
    // ERROR_INSUFFICIENT_BUFFER, so the result is deliberately ignored; a zero
    // size means there is nothing to read.
    let mut needed = 0u32;
    // SAFETY: `token` satisfies the caller contract; passing no buffer with
    // length 0 is the documented size-probe form of this call.
    let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed) };
    if needed == 0 {
        return None;
    }

    // TOKEN_MANDATORY_LABEL holds pointers, so the buffer must be
    // pointer-aligned; Vec<u64> provides that on every supported target.
    let word = std::mem::size_of::<u64>();
    let words = (needed as usize + word - 1) / word;
    let mut buffer = vec![0u64; words];
    let capacity = (words * word) as u32;

    // SAFETY: `token` satisfies the caller contract. The buffer is at least
    // `capacity` bytes and aligned for TOKEN_MANDATORY_LABEL; `capacity` is
    // the allocation's real length (it may exceed `needed`).
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            capacity,
            &mut needed,
        )
    };
    if result.is_err() {
        return None;
    }

    // SAFETY: on success Windows wrote a TOKEN_MANDATORY_LABEL at the start of
    // the buffer. The read is unaligned-safe, `Label.Sid` points into that
    // same live buffer, and the SID is only used by the call below.
    let label =
        unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()) };
    // SAFETY: `label.Label.Sid` points into `buffer`, which is still alive for
    // the duration of this call.
    unsafe { integrity_from_sid(label.Label.Sid) }
}

/// Read the last sub-authority (the mandatory label RID) of a SID.
///
/// # Safety
///
/// `sid` must point at a valid SID that stays alive for this call. It need not
/// have any sub-authorities.
unsafe fn integrity_from_sid(sid: PSID) -> Option<Integrity> {
    if sid.0.is_null() {
        return None;
    }

    // SAFETY: `sid` satisfies the caller contract, so the count pointer is a
    // valid byte inside the SID.
    let count = unsafe { GetSidSubAuthorityCount(sid) };
    if count.is_null() {
        return None;
    }
    // SAFETY: `count` is non-null and points into the live SID.
    let count = unsafe { *count };
    if count == 0 {
        return None;
    }

    // SAFETY: `sid` is valid and `count - 1` is a valid index below the SID's
    // sub-authority count, so the returned pointer points into the SID.
    let rid = unsafe { GetSidSubAuthority(sid, u32::from(count - 1)) };
    if rid.is_null() {
        return None;
    }
    // SAFETY: `rid` is non-null and points into the live SID buffer.
    Some(Integrity::from_rid(unsafe { *rid }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_four_named_integrity_rids() {
        assert_eq!(Integrity::from_rid(0x1000), Integrity::Low);
        assert_eq!(Integrity::from_rid(0x2000), Integrity::Medium);
        assert_eq!(Integrity::from_rid(0x3000), Integrity::High);
        assert_eq!(Integrity::from_rid(0x4000), Integrity::System);
    }

    #[test]
    fn any_other_rid_is_unknown() {
        for rid in [0, 1, 0x0FFF, 0x1001, 0x2100, 0x3001, 0x5000, u32::MAX] {
            assert_eq!(
                Integrity::from_rid(rid),
                Integrity::Unknown,
                "rid {rid:#x} must not be guessed"
            );
        }
    }

    #[test]
    fn integrity_labels_are_stable() {
        assert_eq!(Integrity::Low.label(), "Low");
        assert_eq!(Integrity::Medium.label(), "Medium");
        assert_eq!(Integrity::High.label(), "High");
        assert_eq!(Integrity::System.label(), "System");
        assert_eq!(Integrity::Unknown.label(), "Unknown");
        assert_eq!(Integrity::Medium.to_string(), "Medium");
    }

    #[test]
    fn privilege_formats_as_one_line() {
        let privilege = ProcessPrivilege {
            uiaccess: true,
            elevated: false,
            integrity: Integrity::High,
        };
        assert_eq!(
            privilege.to_string(),
            "uiaccess=true elevated=false integrity=High"
        );

        let plain = ProcessPrivilege {
            uiaccess: false,
            elevated: false,
            integrity: Integrity::Medium,
        };
        assert_eq!(
            plain.to_string(),
            "uiaccess=false elevated=false integrity=Medium"
        );
    }
}
