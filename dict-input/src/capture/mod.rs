//! Selection capture as one deep module.
//!
//! The rest of the crate (and `dict-popup`) only needs the entry point
//! [`capture`] and three types:
//!
//! * [`CaptureTarget`] - where and how to capture;
//! * [`CapturedText`] - validated text, never empty, already trimmed according
//!   to `behavior.trim_selection` and capped;
//! * [`CaptureFailure`] - a real failure enum whose `Display` is the only
//!   user-facing sentence.
//!
//! Everything else is private:
//!
//! ```text
//! capture/
//!   mod.rs       public entry point: capture(target) -> Result<CapturedText, CaptureFailure>
//!   target.rs    CaptureTarget (window + trim policy)
//!   text.rs      CapturedText + TrimSelection + the one length cap
//!   failure.rs   CaptureFailure + the one Display implementation
//!   env.rs       CaptureEnv trait: the testable seam
//!   pipeline.rs  ordered strategies, deadlines, clipboard restore, fake-env tests
//!   win32.rs     Win32 implementation of CaptureEnv (clipboard, SendInput, WM_COPY)
//!   uia.rs       UI Automation search (the last strategy)
//!   service.rs   worker/coordinator threads owned by the host
//! ```
//!
//! The strategy order is fixed here and by [`pipeline`]: Pot's crate, then
//! `Ctrl+C`, then `WM_COPY`, then UI Automation. There is no `prefer_clipboard`
//! flag and no `DICT_POPUP_CAPTURE_MODE` environment variable any more.

use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::IUIAutomation;

mod env;
mod failure;
pub(crate) mod pipeline;
mod service;
mod target;
mod text;
#[cfg(test)]
pub(crate) mod testing;
pub(crate) mod uia;
pub(crate) mod win32;

pub use failure::CaptureFailure;
pub use target::CaptureTarget;
pub use text::{CapturedText, TrimSelection};

pub(crate) use env::CaptureEnv;
pub(crate) use service::Capture;

/// The hard budget for one capture request (docs/DESIGN.md §3).
///
/// This is the single authoritative number for every deadline around a
/// capture: [`service`] enforces it around the worker, [`pipeline`] stops
/// starting new strategies once it is spent, and `dict-popup` derives its UI
/// watchdog from it instead of restating a duration of its own.
pub const CAPTURE_BUDGET: std::time::Duration = pipeline::CAPTURE_TIMEOUT;

/// Capture the current selection from `target`.
///
/// This is the synchronous entry point: it runs the ordered strategies,
/// restores the clipboard, and returns. The interactive app uses the capture
/// service (same pipeline on a dedicated worker) so a hung provider cannot
/// stall the message loop.
pub fn capture(target: CaptureTarget) -> Result<CapturedText, CaptureFailure> {
    with_com(|automation_slot, com_ready| {
        let mut environment = win32::Win32Env::new(target.window(), automation_slot, com_ready);
        pipeline::run(&mut environment, target)
    })
}

/// Run `body` inside the exact COM session a capture uses.
///
/// One MTA initialisation, one lazily-created UI Automation object, and a
/// teardown order that releases every COM object before `CoUninitialize`.
/// Diagnostics borrow this session instead of setting COM up on its own, so a
/// probe and the shipping app talk to the same apartment.
pub(crate) fn with_com<R>(body: impl FnOnce(&mut Option<IUIAutomation>, bool) -> R) -> R {
    let mut automation_slot = None;

    // SAFETY: COM is initialised once on this thread before any automation
    // object is created. If the caller already initialised the thread with a
    // different apartment the call fails, `com_ready` stays false, and the UIA
    // strategy reports a platform failure while the clipboard strategies still
    // work.
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

    let result = body(&mut automation_slot, init.is_ok());

    // Release every COM object before uninitialising the apartment.
    drop(automation_slot);
    if init.is_ok() {
        // SAFETY: balances the successful CoInitializeEx on this same thread.
        unsafe { CoUninitialize() };
    }
    result
}
