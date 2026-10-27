//! Version comparison for dictionary packages.
//!
//! Dictionary versions are not always semver: a data dump often labels itself
//! `2024.03`, and the next one `2024.10`. The comparison in this module is a
//! total order that handles both semver-ish and date-ish versions:
//!
//! 1. Split the version at the **first** `-` into a *release* part and an
//!    optional *pre-release* part (`1.2.0-rc.1` → `1.2.0` / `rc.1`).
//! 2. Compare the release parts dot-segment by dot-segment:
//!    - if both segments are all ASCII digits they are compared as numbers
//!      (`03` and `3` are equal, `10` is greater than `9`, and arbitrarily
//!      long numbers do not overflow);
//!    - otherwise the segments are compared case-insensitively, byte by byte
//!      (so `2024.b` > `2024.a`, and non-ASCII bytes compare by their byte
//!      value);
//!    - a missing segment counts as `0`, so `1.0` equals `1.0.0` and is less
//!      than `1.0.1`.
//! 3. If the release parts are equal, a version without a pre-release part is
//!    greater than one with it (`1.0.0` > `1.0.0-rc.1`). Two pre-release parts
//!    are compared dot-segment by dot-segment: numeric segments compare
//!    numerically, a numeric segment is less than a non-numeric one
//!    (`1.0.0-2` < `1.0.0-rc`), non-numeric segments compare case-insensitively,
//!    and a shorter set of segments is less when it is a prefix of the longer
//!    (`1.0.0-rc` < `1.0.0-rc.1`).
//! 4. If every part is equal the versions are [`Ordering::Equal`].
//!
//! The comparison is pure: it allocates a few strings for the case-insensitive
//! branch but touches no file system or clock.

use std::cmp::Ordering;

use crate::error::StoreError;

/// Compare two version strings, returning the order of `left` relative to
/// `right`.
///
/// The exact rule is documented at the [module level](self). The result is a
/// total order: for any two versions one of `Less`, `Equal` or `Greater` is
/// returned consistently, which is what makes "newest installed version"
/// queries deterministic.
pub fn compare(left: &str, right: &str) -> Ordering {
    let (left_release, left_pre) = split_prerelease(left);
    let (right_release, right_pre) = split_prerelease(right);

    let release = compare_release(left_release, right_release);
    if release != Ordering::Equal {
        return release;
    }

    match (left_pre, right_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => compare_prerelease(left, right),
    }
}

/// Return `true` when `candidate` sorts strictly after `current`.
///
/// This is the check an upgrade performs: `is_newer("2.4.2", "2.4.1")` and
/// `is_newer("2024.10", "2024.03")` are both `true`, while equal versions and
/// older versions are `false`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    compare(candidate, current) == Ordering::Greater
}

/// Check that a version string can be used as one component of a version
/// directory name (`<id>-<version>`).
///
/// A valid version is 1 to 64 bytes, does not start with `.` or `-`, and only
/// contains ASCII letters, digits, `.`, `-`, `+` and `_`. It does not have to
/// parse as semver: `2024.03` and `2024-03-01` are both accepted.
pub fn validate(version: &str) -> Result<(), StoreError> {
    let reject = |reason: &'static str| StoreError::InvalidVersion {
        raw: version.to_string(),
        reason,
    };
    if version.trim().is_empty() {
        return Err(reject("不能为空"));
    }
    if version.len() > 64 {
        return Err(reject("最长 64 个字节"));
    }
    if version.starts_with('.') || version.starts_with('-') {
        return Err(reject("不能以 '.' 或 '-' 开头"));
    }
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
    {
        return Err(reject("只能使用字母、数字和 '.', '-', '+', '_'"));
    }
    Ok(())
}

/// Split a version at the first `-` into its release and pre-release parts.
fn split_prerelease(version: &str) -> (&str, Option<&str>) {
    match version.find('-') {
        Some(index) => (&version[..index], Some(&version[index + 1..])),
        None => (version, None),
    }
}

/// Compare the dotted release parts of two versions.
fn compare_release(left: &str, right: &str) -> Ordering {
    let left: Vec<&str> = left.split('.').collect();
    let right: Vec<&str> = right.split('.').collect();
    let len = left.len().max(right.len());
    for index in 0..len {
        let left_segment = left.get(index).copied().unwrap_or("0");
        let right_segment = right.get(index).copied().unwrap_or("0");
        let ordering = compare_segment(left_segment, right_segment);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// Compare one release segment: numerically when both are digits, otherwise
/// case-insensitively.
fn compare_segment(left: &str, right: &str) -> Ordering {
    match (is_numeric(left), is_numeric(right)) {
        (true, true) => compare_numeric(left, right),
        _ => compare_ascii_case_insensitive(left, right),
    }
}

/// Compare two all-digit segments without parsing them into integers, so that
/// arbitrarily long numbers (dates, build ids) cannot overflow.
fn compare_numeric(left: &str, right: &str) -> Ordering {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    let left = if left.is_empty() { "0" } else { left };
    let right = if right.is_empty() { "0" } else { right };
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

/// Compare dot-separated pre-release identifiers.
fn compare_prerelease(left: &str, right: &str) -> Ordering {
    let left: Vec<&str> = left.split('.').collect();
    let right: Vec<&str> = right.split('.').collect();
    let len = left.len().min(right.len());
    for index in 0..len {
        let left_segment = left[index];
        let right_segment = right[index];
        let ordering = match (is_numeric(left_segment), is_numeric(right_segment)) {
            (true, true) => compare_numeric(left_segment, right_segment),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => compare_ascii_case_insensitive(left_segment, right_segment),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    // A shorter identifier list is lower when it is a prefix of the longer one.
    left.len().cmp(&right.len())
}

/// `true` when the whole string is one or more ASCII digits.
fn is_numeric(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit())
}

/// Compare two strings ignoring ASCII case, without allocating.
fn compare_ascii_case_insensitive(left: &str, right: &str) -> Ordering {
    let mut left = left.bytes().map(|byte| byte.to_ascii_lowercase());
    let mut right = right.bytes().map(|byte| byte.to_ascii_lowercase());
    loop {
        match (left.next(), right.next()) {
            (Some(left), Some(right)) => match left.cmp(&right) {
                Ordering::Equal => continue,
                ordering => return ordering,
            },
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering() {
        assert_eq!(compare("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare("1.2.3", "1.2.2"), Ordering::Greater);
        assert_eq!(compare("1.2.2", "1.2.3"), Ordering::Less);
        assert_eq!(compare("1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare("2", "10"), Ordering::Less);
        assert_eq!(compare("1.0.1", "1.0"), Ordering::Greater);
    }

    #[test]
    fn date_style_versions() {
        assert_eq!(compare("2024.03", "2024.10"), Ordering::Less);
        assert_eq!(compare("2024.10", "2024.03"), Ordering::Greater);
        assert_eq!(compare("2024.3", "2024.03"), Ordering::Equal);
        assert_eq!(compare("2024.03.01", "2024.03"), Ordering::Greater);
        assert_eq!(compare("2023.12", "2024.01"), Ordering::Less);
        assert_eq!(compare("2024-03-01", "2024-03-02"), Ordering::Less);
        assert_eq!(compare("2024-03", "2024-03-01"), Ordering::Less);
    }

    #[test]
    fn prerelease_ordering() {
        assert_eq!(compare("1.0.0-beta", "1.0.0"), Ordering::Less);
        assert_eq!(compare("1.0.0", "1.0.0-beta"), Ordering::Greater);
        assert_eq!(compare("1.0.0-alpha", "1.0.0-beta"), Ordering::Less);
        assert_eq!(compare("1.0.0-alpha", "1.0.0-alpha.1"), Ordering::Less);
        assert_eq!(compare("1.0.0-rc.2", "1.0.0-rc.10"), Ordering::Less);
        assert_eq!(compare("1.0.0-1", "1.0.0-alpha"), Ordering::Less);
        assert_eq!(compare("1.0.0-Beta", "1.0.0-beta"), Ordering::Equal);
        assert_eq!(compare("1.0.0-rc.1", "1.0.1-rc.1"), Ordering::Less);
    }

    #[test]
    fn numeric_segments_do_not_overflow() {
        assert_eq!(
            compare("99999999999999999999", "99999999999999999998"),
            Ordering::Greater
        );
        assert_eq!(compare("00012", "12"), Ordering::Equal);
        assert_eq!(compare("0", "00"), Ordering::Equal);
    }

    #[test]
    fn comparison_is_antisymmetric_and_transitive() {
        let versions = [
            "0.1",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-beta",
            "1.0.0",
            "1.0.1",
            "1.2",
            "1.10",
            "2024.03",
            "2024.10",
            "2024.10.1-rc.2",
        ];
        for left in versions {
            for right in versions {
                assert_eq!(
                    compare(left, right),
                    compare(right, left).reverse(),
                    "{left} vs {right}"
                );
            }
        }
        for first in versions {
            for second in versions {
                for third in versions {
                    if compare(first, second) == Ordering::Less
                        && compare(second, third) == Ordering::Less
                    {
                        assert_eq!(
                            compare(first, third),
                            Ordering::Less,
                            "{first} < {second} < {third}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn is_newer_only_for_strictly_greater() {
        assert!(is_newer("2.4.2", "2.4.1"));
        assert!(is_newer("2024.10", "2024.03"));
        assert!(!is_newer("2.4.1", "2.4.1"));
        assert!(!is_newer("2.4.0", "2.4.1"));
    }

    #[test]
    fn validate_rejects_unsafe_versions() {
        assert!(validate("2024.03").is_ok());
        assert!(validate("1.0.0-beta+1").is_ok());
        assert!(validate("2024-03-01").is_ok());
        assert!(validate("").is_err());
        assert!(validate("  ").is_err());
        assert!(validate("1/2").is_err());
        assert!(validate("1\\2").is_err());
        assert!(validate(".hidden").is_err());
        assert!(validate("-dash").is_err());
        assert!(validate(&"9".repeat(65)).is_err());
    }
}
