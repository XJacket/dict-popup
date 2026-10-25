//! The environment edges behind the capture pipeline.
//!
//! The pipeline ([`super::pipeline`]) only ever talks to the outside world
//! through this trait. That is the testable seam: unit tests plug in an
//! in-memory fake and exercise strategy order, clipboard restore and deadlines
//! without a desktop, a clipboard, or UI Automation.

use std::time::{Duration, Instant};

use super::failure::CaptureFailure;

/// Everything a capture strategy needs from the operating system.
///
/// One implementation exists for Win32 (see `win32.rs`); tests use a fake that
/// records calls. Methods that perform an action take `&mut self` so the fake
/// can keep plain state; read-only observations take `&self`.
pub trait CaptureEnv {
    /// Monotonic clock used for every deadline decision.
    fn now(&self) -> Instant;

    /// Wait between clipboard polls. Tests make this advance their fake clock.
    fn sleep(&self, duration: Duration);

    /// Ask Pot's [`selection`] crate to capture. `None` means the crate could
    /// not produce text; the pipeline then falls through to the next strategy.
    fn pot_selection(&mut self) -> Option<String>;

    /// Inject `Ctrl+C` into the foreground application. Returns
    /// [`CaptureFailure::InputBlocked`] when the synthetic input is rejected.
    fn press_copy(&mut self) -> Result<(), CaptureFailure>;

    /// Ask the focused control to copy itself with `WM_COPY`. Returns `false`
    /// when there is no focus window to talk to.
    fn request_wm_copy(&mut self) -> bool;

    /// Current clipboard sequence number: it changes only when the clipboard is
    /// written, which is the signal that a copy actually happened.
    fn clipboard_seq(&self) -> u32;

    /// Current clipboard text, or a failure. "No text format present" is
    /// reported as [`CaptureFailure::NoSelection`].
    fn read_clipboard(&mut self) -> Result<String, CaptureFailure>;

    /// Replace the clipboard text. Best effort: failures are ignored.
    fn write_clipboard(&mut self, text: &str);

    /// Run one UI Automation capture and return the raw text of the selection.
    /// The pipeline validates it through [`super::CapturedText::new`].
    fn read_uia(&mut self) -> Result<String, CaptureFailure>;
}
