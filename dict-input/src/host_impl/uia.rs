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
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationValuePattern, TreeScope_Children, TreeScope_Descendants, UIA_TextPatternId,
    UIA_ValuePatternId,
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
    requests: Option<Sender<isize>>,
    busy: Arc<AtomicBool>,
}

impl Capture {
    /// Spawn the worker and coordinator. `deliver` is called from the
    /// coordinator thread, never from the host thread.
    pub(crate) fn start(deliver: impl Fn(Event) + Send + 'static) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<isize>();
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
    pub(crate) fn request(&self, target: HWND) -> Result<(), SelectionError> {
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
        if sender.send(target.0 as isize).is_err() {
            self.busy.store(false, Ordering::Release);
            return Err(SelectionError::Automation("取词线程已停止".to_string()));
        }
        Ok(())
    }
}

struct Job {
    /// Window the user was in when the hotkey fired (0 = unknown).
    target: isize,
    reply: Sender<Result<Selection, SelectionError>>,
}

fn coordinator_loop(
    requests: Receiver<isize>,
    jobs: Sender<Job>,
    busy: Arc<AtomicBool>,
    deliver: impl Fn(Event),
) {
    while let Ok(target) = requests.recv() {
        let (reply_tx, reply_rx) = mpsc::channel();
        if jobs
            .send(Job {
                target,
                reply: reply_tx,
            })
            .is_err()
        {
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
        let target = HWND(job.target as *mut core::ffi::c_void);
        let outcome = if com_ready {
            match automation(&mut automation_slot) {
                Ok(automation) => {
                    // SAFETY: `automation` is a live IUIAutomation interface on
                    // this MTA thread, valid for the duration of the call.
                    unsafe { capture_with(automation, target) }
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

/// One capture attempt.
///
/// # Safety
///
/// Must be called on the capture worker thread with COM initialised as MTA, and
/// `automation` must be a live `IUIAutomation`.
unsafe fn capture_with(
    automation: &IUIAutomation,
    target: HWND,
) -> Result<Selection, SelectionError> {
    // Fast path: whatever UI Automation considers focused.
    let last_error = match unsafe { automation.GetFocusedElement() } {
        Ok(element) => match unsafe { try_element(&element) } {
            Ok(selection) => return Ok(selection),
            Err(error) => Some(error),
        },
        Err(error) => Some(SelectionError::Automation(format!(
            "GetFocusedElement：{}",
            error.message()
        ))),
    };

    // Fallback: the window the user was in when the hotkey fired. WinUI/XAML
    // hosts (Windows 11 Notepad, anything with a XAML island) report the
    // *window* as focused; the control that owns the selection is a descendant
    // (e.g. `Document` / `RichEditD2DPT`).
    let mut stats = SearchStats::default();
    if !target.is_invalid() {
        // SAFETY: live automation object; any HWND yields an element or an error.
        if let Ok(root) = unsafe { automation.ElementFromHandle(target) } {
            // SAFETY: element is live on this thread.
            if let Some(selection) = unsafe { search_text(automation, &root, &mut stats) } {
                return Ok(selection);
            }
        }
    } else {
        stats.error = Some("没有可用的前台窗口句柄".to_string());
    }

    // Make the failure actionable instead of a bare "unsupported": the numbers
    // say whether the tree was searched at all and how many text controls it had.
    if let Some(error) = stats.error {
        return Err(SelectionError::Automation(format!(
            "已扫描 {} 个元素、{} 个文本控件：{error}",
            stats.nodes, stats.text_elements
        )));
    }
    if stats.text_elements > 0 {
        return Err(SelectionError::EmptySelection);
    }
    if let Some(SelectionError::Automation(message)) = last_error {
        return Err(SelectionError::Automation(format!(
            "未找到可读取的文本控件（扫描 {} 个元素）：{message}",
            stats.nodes
        )));
    }
    Err(SelectionError::Automation(format!(
        "未找到可读取的文本控件（扫描 {} 个元素）：该程序可能不通过 UI Automation 暴露选区",
        stats.nodes
    )))
}

#[derive(Default)]
struct SearchStats {
    nodes: usize,
    text_elements: usize,
    error: Option<String>,
}

/// The subtree search asks the provider for `Document`/`Edit` descendants only,
/// so no walk budget is needed; the coordinator's 1.5 s timeout still bounds it.

/// Diagnostic report for `examples/uia_probe.rs`: walks `hwnd` and describes
/// every element it sees. Not used by the app.
pub(crate) fn debug_capture_report(hwnd: isize) -> String {
    use windows::Win32::UI::Accessibility::UIA_TextPatternId;

    // SAFETY: dedicated thread, MTA initialised below; all calls are read-only.
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);

        // Every COM object must be released *before* CoUninitialize, hence the
        // inner scope: dropping `automation` after uninitialising the apartment
        // is an access violation.
        let report = {
            let automation: IUIAutomation = match CoCreateInstance(
                &CUIAutomation,
                None::<&IUnknown>,
                CLSCTX_INPROC_SERVER,
            ) {
                Ok(automation) => automation,
                Err(error) => {
                    CoUninitialize();
                    return format!("CoInitializeEx: {init:?}\nCoCreateInstance failed: {error}\n");
                }
            };

            let target = HWND(hwnd as *mut core::ffi::c_void);
            let root = match automation.ElementFromHandle(target) {
                Ok(root) => root,
                Err(error) => {
                    CoUninitialize();
                    return format!("CoInitializeEx: {init:?}\nElementFromHandle failed: {error}\n");
                }
            };

            fn walk(
                automation: &IUIAutomation,
                element: &IUIAutomationElement,
                depth: usize,
                budget: &mut usize,
                out: &mut String,
            ) {
                if depth > 6 || *budget == 0 {
                    return;
                }
                *budget -= 1;
                // SAFETY: live element on this thread.
                unsafe {
                    let control_type = element.CurrentControlType().map(|t| t.0).unwrap_or(-1);
                    let class = element
                        .CurrentClassName()
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let has_text =
                        element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId);
                    let mut sel = String::new();
                    if let Ok(pattern) = &has_text {
                        if let Ok(ranges) = pattern.GetSelection() {
                            if let Ok(n) = ranges.Length() {
                                for i in 0..n.min(3) {
                                    if let Ok(r) = ranges.GetElement(i) {
                                        if let Ok(t) = r.GetText(40) {
                                            sel.push_str(&t.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    out.push_str(&format!(
                        "{:indent$}type={control_type} class='{class}' text={} sel={:?}\n",
                        "",
                        has_text.is_ok(),
                        sel.replace('\r', "\\r").replace('\n', "\\n"),
                        indent = depth * 2
                    ));
                    if let Ok(condition) = automation.CreateTrueCondition() {
                        if let Ok(children) = element.FindAll(TreeScope_Children, &condition) {
                            if let Ok(count) = children.Length() {
                                for i in 0..count.clamp(0, 64) {
                                    if let Ok(child) = children.GetElement(i) {
                                        walk(automation, &child, depth + 1, budget, out);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let mut out = format!("CoInitializeEx: {init:?}\n");
            let mut budget = 400usize;
            walk(&automation, &root, 0, &mut budget, &mut out);
            out
        };

        CoUninitialize();
        report
    }
}

/// Find a descendant whose text pattern yields a non-empty selection.
///
/// # Safety
///
/// Same contract as [`capture_with`].
unsafe fn search_text(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    stats: &mut SearchStats,
) -> Option<Selection> {
    // Ask UI Automation for the text surfaces below this window instead of
    // walking children by hand: XAML islands (Windows 11 Notepad) return
    // nothing for FindAll(Children) but do answer a descendant query.
    let condition = match unsafe { text_surface_condition(automation) } {
        Ok(condition) => condition,
        Err(message) => {
            stats.error = Some(message);
            return None;
        }
    };

    // SAFETY: root is live; descendants are resolved by the provider.
    let found = match unsafe { root.FindAll(TreeScope_Descendants, &condition) } {
        Ok(found) => found,
        Err(error) => {
            stats.error = Some(format!("FindAll：{}", error.message()));
            return None;
        }
    };
    // SAFETY: live array; the count is provider data and is clamped.
    let count = match unsafe { found.Length() } {
        Ok(count) => count.clamp(0, 64),
        Err(error) => {
            stats.error = Some(format!("Length：{}", error.message()));
            return None;
        }
    };
    stats.nodes += count as usize;
    stats.text_elements += count as usize;

    for index in 0..count {
        // SAFETY: index is within the reported range.
        let Ok(element) = (unsafe { found.GetElement(index) }) else {
            continue;
        };
        // SAFETY: element is live on this thread.
        if let Ok(selection) = unsafe { try_element(&element) } {
            return Some(selection);
        }
    }
    None
}

/// Condition matching `Document` and `Edit` controls.
///
/// Only those can hold a *user* selection. A static `Text` element (a label,
/// the window title) also implements TextPattern; accepting it looked up the
/// title bar instead of the selected word.
///
/// # Safety
///
/// `automation` must be live on this thread.
unsafe fn text_surface_condition(
    automation: &IUIAutomation,
) -> Result<windows::Win32::UI::Accessibility::IUIAutomationCondition, String> {
    use windows::core::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationCondition, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
        UIA_EditControlTypeId,
    };

    let document = unsafe {
        automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_DocumentControlTypeId.0),
        )
    }
    .map_err(|error| format!("CreatePropertyCondition(Document)：{}", error.message()))?;
    let edit = unsafe {
        automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_EditControlTypeId.0),
        )
    }
    .map_err(|error| format!("CreatePropertyCondition(Edit)：{}", error.message()))?;

    let condition: IUIAutomationCondition =
        unsafe { automation.CreateOrCondition(&document, &edit) }
            .map_err(|error| format!("CreateOrCondition：{}", error.message()))?;
    Ok(condition)
}

/// Try the text pattern, then the value pattern, on one element.
///
/// # Safety
///
/// `element` must be a live UI Automation element on this thread.
unsafe fn try_element(element: &IUIAutomationElement) -> Result<Selection, SelectionError> {
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
                .map_err(|error| {
                    SelectionError::Automation(format!("RangeArray.Length：{}", error.message()))
                })?
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
                    let text = text.to_string();
                    // A ValuePattern exposes the *whole* content, not the
                    // selection. Looking up a 4000-character document would be
                    // wrong (and noisy), so treat anything long as "no selection".
                    if text.chars().count() > 200 {
                        return Err(SelectionError::EmptySelection);
                    }
                    join_pieces(&[text])
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
