//! UI Automation selection capture.
//!
//! Two dedicated threads (docs/DESIGN.md §3):
//!
//! * the **capture worker** initialises COM as `COINIT_MULTITHREADED`, caches a
//!   single `IUIAutomation`, and answers one job at a time;
//! * the **coordinator** waits at most [`CAPTURE_TIMEOUT`] for each job and
//!   reports [`SelectionError::TimedOut`] otherwise. It never tries to cancel a
//!   UIA call, so a hung provider cannot corrupt the worker; it only means the
//!   worker stays busy and further hotkeys are dropped until it returns.
//!
//! The host thread only enqueues and returns, so neither the tray icon nor the
//! global hotkey can be stalled by a slow or hung provider.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows::core::IUnknown;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationValuePattern,
    UIA_TextPatternId, UIA_ValuePatternId,
};

use crate::host::Event;
use crate::selection::{Selection, SelectionError};

/// Hard budget for one capture request (docs/DESIGN.md §3 / task spec).
pub(crate) const CAPTURE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Defensive cap on the number of selection ranges a provider may hand back.
const MAX_RANGES: i32 = 256;

/// Defensive cap on captured text length in UTF-8 bytes. A dictionary lookup
/// never needs more, and a misbehaving provider (e.g. a log viewer exposing its
/// whole buffer through ValuePattern) must not be able to make the app allocate
/// unbounded memory. Truncation happens on a character boundary.
const MAX_SELECTION_BYTES: usize = 96 * 1024;

/// Handle to the capture pipeline. Dropping it stops the worker after its
/// current job (a hung UIA call is left to die with the process).
pub(crate) struct Capture {
    requests: Option<Sender<()>>,
    busy: Arc<AtomicBool>,
}

impl Capture {
    /// Spawn the worker and coordinator. `deliver` is called from the
    /// coordinator thread, never from the host thread.
    pub(crate) fn start(deliver: impl Fn(Event) + Send + 'static) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<()>();
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let busy = Arc::new(AtomicBool::new(false));

        let worker_busy = Arc::clone(&busy);
        let worker_started = thread::Builder::new()
            .name("dict-input-uia".to_string())
            .spawn(move || worker_loop(job_rx, worker_busy));
        if worker_started.is_err() {
            return Self {
                requests: None,
                busy,
            };
        }

        let coordinator_busy = Arc::clone(&busy);
        let coordinator_started = thread::Builder::new()
            .name("dict-input-uia-timeout".to_string())
            .spawn(move || coordinator_loop(request_rx, job_tx, coordinator_busy, deliver));
        if coordinator_started.is_err() {
            return Self {
                requests: None,
                busy,
            };
        }

        Self {
            requests: Some(request_tx),
            busy,
        }
    }

    /// Ask for a capture. Non-blocking.
    ///
    /// * `Ok(())` — the request was queued; the result arrives later as
    ///   [`Event::Selection`] or [`Event::SelectionFailed`].
    /// * `Err(TimedOut)` — a previous capture is still running (busy-skip
    ///   policy). The caller reports it and drops this request.
    /// * `Err(Automation(..))` — the capture pipeline is not available.
    pub(crate) fn request(&self) -> Result<(), SelectionError> {
        let Some(sender) = &self.requests else {
            return Err(SelectionError::Automation("取词线程不可用".to_string()));
        };
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(SelectionError::TimedOut);
        }
        if sender.send(()).is_err() {
            self.busy.store(false, Ordering::Release);
            return Err(SelectionError::Automation("取词线程已停止".to_string()));
        }
        Ok(())
    }
}

struct Job {
    reply: Sender<Result<Selection, SelectionError>>,
}

fn coordinator_loop(
    requests: Receiver<()>,
    jobs: Sender<Job>,
    busy: Arc<AtomicBool>,
    deliver: impl Fn(Event),
) {
    while requests.recv().is_ok() {
        let (reply_tx, reply_rx) = mpsc::channel();
        if jobs.send(Job { reply: reply_tx }).is_err() {
            busy.store(false, Ordering::Release);
            deliver(Event::SelectionFailed(SelectionError::Automation(
                "取词线程已停止".to_string(),
            )));
            return;
        }

        match reply_rx.recv_timeout(CAPTURE_TIMEOUT) {
            Ok(Ok(selection)) => deliver(Event::Selection(selection)),
            Ok(Err(error)) => deliver(Event::SelectionFailed(error)),
            // Do not touch `busy`: the worker is still inside the UIA call and
            // clears the flag itself when (if ever) it returns. Until then every
            // hotkey is dropped as TimedOut, which is the documented policy.
            Err(RecvTimeoutError::Timeout) => {
                deliver(Event::SelectionFailed(SelectionError::TimedOut));
            }
            Err(RecvTimeoutError::Disconnected) => {
                busy.store(false, Ordering::Release);
                deliver(Event::SelectionFailed(SelectionError::Automation(
                    "取词线程已停止".to_string(),
                )));
                return;
            }
        }
    }
}

fn worker_loop(jobs: Receiver<Job>, busy: Arc<AtomicBool>) {
    // SAFETY: COM is initialised exactly once on this dedicated thread, before
    // any automation object exists, and an MTA is what docs/DESIGN.md §3 asks
    // for. CoUninitialize below balances this call on the same thread.
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let com_ready = init.is_ok();
    let mut automation_slot: Option<IUIAutomation> = None;

    while let Ok(job) = jobs.recv() {
        let outcome = if com_ready {
            match automation(&mut automation_slot) {
                Ok(automation) => {
                    // SAFETY: `automation` is a live IUIAutomation interface on
                    // this MTA thread, valid for the duration of the call.
                    unsafe { capture_with(automation) }
                }
                Err(error) => Err(error),
            }
        } else {
            Err(SelectionError::Automation(format!(
                "COM 初始化失败（HRESULT 0x{:08X}）",
                init.0
            )))
        };

        // The worker decides when it is free again, not the coordinator: a
        // timed-out request must leave the busy flag set while UIA is still
        // executing inside this thread.
        busy.store(false, Ordering::Release);
        let _ = job.reply.send(outcome);
    }

    drop(automation_slot);
    if com_ready {
        // SAFETY: balances the successful CoInitializeEx on this same thread.
        unsafe { CoUninitialize() };
    }
}

fn automation(slot: &mut Option<IUIAutomation>) -> Result<&IUIAutomation, SelectionError> {
    if slot.is_none() {
        // SAFETY: COM is initialised on this thread and CUIAutomation is the
        // in-process UI Automation coclass; no outer object is needed.
        let created: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
                .map_err(|error| {
                    SelectionError::Automation(format!(
                        "无法创建 UI Automation 实例：{}",
                        error.message()
                    ))
                })?;
        *slot = Some(created);
    }
    slot.as_ref()
        .ok_or_else(|| SelectionError::Automation("UI Automation 未初始化".to_string()))
}

/// One capture attempt against the focused element.
///
/// # Safety
///
/// Must be called on the capture worker thread with COM initialised as MTA, and
/// `automation` must be a live `IUIAutomation`.
unsafe fn capture_with(automation: &IUIAutomation) -> Result<Selection, SelectionError> {
    // SAFETY: live automation object on the owning thread.
    let element = unsafe { automation.GetFocusedElement() }
        .map_err(|error| SelectionError::Automation(format!("GetFocusedElement：{}", error.message())))?;

    // SAFETY: `element` is alive for the whole function; the pattern is queried
    // through the element's own vtable.
    match unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) } {
        Ok(pattern) => {
            // SAFETY: `pattern` was just obtained from a live element.
            let ranges = unsafe { pattern.GetSelection() }.map_err(|error| {
                SelectionError::Automation(format!("TextPattern.GetSelection：{}", error.message()))
            })?;
            // SAFETY: `ranges` is a live range array owned by the pattern call.
            let count = unsafe { ranges.Length() }
                .map_err(|error| SelectionError::Automation(format!("RangeArray.Length：{}", error.message())))?
                .clamp(0, MAX_RANGES);

            let mut pieces = Vec::with_capacity(count as usize);
            for index in 0..count {
                // SAFETY: index is within [0, count) as reported by the array.
                let range = unsafe { ranges.GetElement(index) }.map_err(|error| {
                    SelectionError::Automation(format!("RangeArray.GetElement：{}", error.message()))
                })?;
                // SAFETY: `range` is live; -1 means "the whole range".
                let text = unsafe { range.GetText(-1) }.map_err(|error| {
                    SelectionError::Automation(format!("TextRange.GetText：{}", error.message()))
                })?;
                pieces.push(text.to_string());
            }
            join_pieces(&pieces)
        }
        // No TextPattern: fall back to ValuePattern for plain edit controls.
        Err(_) => {
            // SAFETY: `element` is alive; ValuePattern is queried the same way.
            match unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
            {
                Ok(value) => {
                    // SAFETY: `value` is live; reading the current value has no
                    // side effects.
                    let text = unsafe { value.CurrentValue() }.map_err(|error| {
                        SelectionError::Automation(format!("ValuePattern.CurrentValue：{}", error.message()))
                    })?;
                    join_pieces(&[text.to_string()])
                }
                Err(_) => Err(SelectionError::NoTextPattern),
            }
        }
    }
}

/// Concatenate the selected ranges and reject an effectively empty selection.
///
/// Kept pure so the decision logic is unit-tested without UIA.
pub(crate) fn join_pieces(pieces: &[String]) -> Result<Selection, SelectionError> {
    let mut text = String::new();
    for piece in pieces {
        text.push_str(piece);
        if text.len() >= MAX_SELECTION_BYTES {
            break;
        }
    }
    if text.len() > MAX_SELECTION_BYTES {
        // Truncate on a character boundary so the string stays valid UTF-8.
        let mut end = MAX_SELECTION_BYTES;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    if text.trim().is_empty() {
        Err(SelectionError::EmptySelection)
    } else {
        Ok(Selection { text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_multiple_ranges_in_order() {
        let selection = join_pieces(&["Hello".to_string(), " world".to_string()]).unwrap();
        assert_eq!(selection.text, "Hello world");
    }

    #[test]
    fn empty_or_whitespace_selection_is_an_error() {
        assert_eq!(join_pieces(&[]), Err(SelectionError::EmptySelection));
        assert_eq!(
            join_pieces(&["   ".to_string(), "\r\n".to_string()]),
            Err(SelectionError::EmptySelection)
        );
    }

    #[test]
    fn selection_text_is_not_trimmed_here() {
        // Trimming is the UI/dict-core's job (`behavior.trim_selection`).
        let selection = join_pieces(&["  spaced  ".to_string()]).unwrap();
        assert_eq!(selection.text, "  spaced  ");
    }

    #[test]
    fn oversized_selection_is_capped() {
        let huge = "x".repeat(MAX_SELECTION_BYTES + 100);
        let selection = join_pieces(&[huge]).unwrap();
        assert!(selection.text.len() <= MAX_SELECTION_BYTES);
        assert!(!selection.text.is_empty());
    }

    #[test]
    fn cap_does_not_split_a_multi_byte_character() {
        let huge = "词".repeat(MAX_SELECTION_BYTES);
        let selection = join_pieces(&[huge]).unwrap();
        assert!(selection.text.len() <= MAX_SELECTION_BYTES);
        // If the cut had landed inside a character, this would not be a valid
        // &str; the function guarantees it by only truncating at boundaries.
        assert!(selection.text.chars().all(|c| c == '词'));
    }
}
