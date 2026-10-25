//! The immutable input to one capture request.

use super::text::TrimSelection;

/// Where and how to capture.
///
/// Callers only describe the *target*, never the strategy order or the
/// deadlines: [`super::capture`] owns those. In particular there is no flag to
/// ask for "clipboard only" or "UIA only" - that plumbing
/// (`DICT_POPUP_CAPTURE_MODE`) has been removed on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureTarget {
    window: Option<isize>,
    trim: TrimSelection,
}

impl CaptureTarget {
    /// `window` is a native `HWND` value, or `None` when the caller has no
    /// foreground window to fall back to. `trim` comes from
    /// `behavior.trim_selection`.
    pub fn new(window: Option<isize>, trim: TrimSelection) -> Self {
        Self { window, trim }
    }

    /// The window the user was in when the hotkey fired (0/None = unknown).
    pub fn window(&self) -> Option<isize> {
        self.window
    }

    /// How the captured text should be trimmed before it leaves this module.
    pub fn trim(&self) -> TrimSelection {
        self.trim
    }
}

impl Default for CaptureTarget {
    fn default() -> Self {
        Self {
            window: None,
            trim: TrimSelection::default(),
        }
    }
}
