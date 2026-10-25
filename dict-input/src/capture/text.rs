//! Validated captured text.
//!
//! [`CapturedText`] is the *only* representation of "a selection the app may
//! look up". Its field is private and [`CapturedText::new`] enforces every
//! invariant the rest of the app relies on:
//!
//! * the configured [`TrimSelection`] policy is applied **here**, where the text
//!   is created, not in the UI;
//! * empty and whitespace-only captures are rejected with
//!   [`CaptureFailure::NoSelection`];
//! * the defensive UTF-8 length cap is applied on a character boundary.
//!
//! ## What is *not* decided here
//!
//! UI Automation's `ValuePattern` exposes a control's *whole* content, so the
//! UIA strategy treats a value longer than `VALUE_PATTERN_MAX_CHARS` as "this
//! provider has no selection". That is a strategy-specific heuristic, not a
//! general text limit: a real `TextPattern` selection of a long paragraph is
//! perfectly valid and is capped only by [`MAX_CAPTURED_BYTES`].

use std::fmt;

use super::failure::CaptureFailure;

/// Defensive cap on captured text length in UTF-8 bytes.
///
/// A dictionary lookup never needs more, and a misbehaving provider (e.g. a log
/// viewer exposing its whole buffer) must not be able to make the app allocate
/// unbounded memory. Truncation keeps the string on a character boundary.
pub(crate) const MAX_CAPTURED_BYTES: usize = 96 * 1024;

/// Whether the capture layer trims the text before handing it to the UI.
///
/// Mirrors `behavior.trim_selection` from the popup settings. It is applied
/// when the text is created so every caller gets the same answer and no
/// downstream layer has to trim (or re-check emptiness) itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrimSelection {
    /// Trim leading and trailing whitespace (`trim_selection = true`).
    #[default]
    Trim,
    /// Keep the selection exactly as the application exposed it.
    Preserve,
}

impl TrimSelection {
    /// Convert the persisted `behavior.trim_selection` boolean.
    pub fn from_enabled(enabled: bool) -> Self {
        if enabled {
            TrimSelection::Trim
        } else {
            TrimSelection::Preserve
        }
    }
}

/// Text captured from the focused control, validated and ready to look up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedText {
    text: String,
}

impl CapturedText {
    /// Validate a raw capture.
    ///
    /// The only constructor: downstream code can never observe an empty or
    /// whitespace-only `CapturedText`. The length cap is applied last, after
    /// trimming, so the retained text is the longest valid prefix.
    pub fn new(raw: &str, trim: TrimSelection) -> Result<Self, CaptureFailure> {
        let mut text = match trim {
            TrimSelection::Trim => raw.trim().to_string(),
            TrimSelection::Preserve => raw.to_string(),
        };

        if text.len() > MAX_CAPTURED_BYTES {
            let mut end = MAX_CAPTURED_BYTES;
            // Never cut through a multi-byte character: the result must stay
            // valid UTF-8.
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }

        // The single "is this text usable?" decision in the capture path.
        if text.trim().is_empty() {
            return Err(CaptureFailure::NoSelection);
        }
        Ok(Self { text })
    }

    /// The validated text. Never empty, never whitespace-only.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Number of `char`s (what the UI logs, never the text itself).
    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// Consume the wrapper, yielding the owned string.
    pub fn into_string(self) -> String {
        self.text
    }
}

impl fmt::Display for CapturedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl AsRef<str> for CapturedText {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_whitespace_only() {
        for raw in ["", " ", "\r\n", "\t  \n"] {
            assert_eq!(
                CapturedText::new(raw, TrimSelection::Preserve),
                Err(CaptureFailure::NoSelection),
                "raw={raw:?}"
            );
            assert_eq!(
                CapturedText::new(raw, TrimSelection::Trim),
                Err(CaptureFailure::NoSelection),
                "raw={raw:?}"
            );
        }
    }

    #[test]
    fn trims_when_asked() {
        let text = CapturedText::new("  spaced  ", TrimSelection::Trim).unwrap();
        assert_eq!(text.text(), "spaced");
    }

    #[test]
    fn preserves_whitespace_when_asked() {
        let text = CapturedText::new("  spaced  ", TrimSelection::Preserve).unwrap();
        assert_eq!(text.text(), "  spaced  ");
    }

    #[test]
    fn trims_before_applying_the_length_cap() {
        let raw = format!("  {}  ", "x".repeat(MAX_CAPTURED_BYTES + 100));
        let text = CapturedText::new(&raw, TrimSelection::Trim).unwrap();
        assert_eq!(text.text().len(), MAX_CAPTURED_BYTES);
    }

    #[test]
    fn oversized_text_is_capped() {
        let huge = "x".repeat(MAX_CAPTURED_BYTES + 100);
        let text = CapturedText::new(&huge, TrimSelection::Preserve).unwrap();
        assert!(text.text().len() <= MAX_CAPTURED_BYTES);
        assert!(!text.text().is_empty());
    }

    #[test]
    fn cap_does_not_split_a_multi_byte_character() {
        let huge = "词".repeat(MAX_CAPTURED_BYTES);
        let text = CapturedText::new(&huge, TrimSelection::Preserve).unwrap();
        assert!(text.text().len() <= MAX_CAPTURED_BYTES);
        assert!(text.text().chars().all(|c| c == '词'));
    }

    #[test]
    fn trim_policy_maps_from_the_config_boolean() {
        assert_eq!(TrimSelection::from_enabled(true), TrimSelection::Trim);
        assert_eq!(TrimSelection::from_enabled(false), TrimSelection::Preserve);
    }
}
