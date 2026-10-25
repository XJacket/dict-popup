//! UI Automation selection capture (the strategy, not the ordering).
//!
//! The pipeline calls [`capture`] last. This file searches for a text surface
//! and returns the raw selection text; it does not decide strategy order,
//! clipboard restore, or user-facing prose.
//!
//! Two UIA facts shape the code:
//!
//! * many toolkits report the top-level *window* as focused, so the search also
//!   walks the window the user was in when the hotkey fired;
//! * `ValuePattern` exposes a control's whole content, not its selection. A long
//!   value therefore means "this provider probably has no selection"
//!   ([`VALUE_PATTERN_MAX_CHARS`]), which is a **strategy heuristic**, not a
//!   general text-length limit. Real text selections are capped only by
//!   [`super::text::MAX_CAPTURED_BYTES`] inside [`super::CapturedText::new`].

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationValuePattern, TreeScope_Children, TreeScope_Descendants, UIA_TextPatternId,
    UIA_ValuePatternId,
};

use super::failure::CaptureFailure;
use super::text::{CapturedText, TrimSelection, MAX_CAPTURED_BYTES};

/// Defensive cap on the number of selection ranges a provider may hand back.
const MAX_RANGES: i32 = 256;

/// A `ValuePattern` value longer than this is almost certainly the whole
/// document rather than a user selection.
///
/// This is deliberately **not** a general text limit: it is the UIA strategy
/// saying "this provider probably has no selection". It lives next to the
/// strategy that applies it, not inside [`CapturedText`].
pub(crate) const VALUE_PATTERN_MAX_CHARS: usize = 200;

/// Run one UI Automation capture and return the raw selection text.
///
/// `com_ready` is the result of the worker thread's `CoInitializeEx`; when COM
/// is unavailable the strategy reports a platform failure and the pipeline
/// falls back to the clipboard strategies that already ran.
pub(crate) fn capture(
    automation_slot: &mut Option<IUIAutomation>,
    com_ready: bool,
    window: Option<isize>,
) -> Result<String, CaptureFailure> {
    if !com_ready {
        return Err(CaptureFailure::Platform(
            "COM was not initialised on the capture thread".to_string(),
        ));
    }
    let automation = automation(automation_slot)?;
    // SAFETY: COM is initialised on this thread and `automation` is the live
    // cached IUIAutomation created above.
    unsafe { capture_with(automation, window) }
}

fn automation(slot: &mut Option<IUIAutomation>) -> Result<&IUIAutomation, CaptureFailure> {
    if slot.is_none() {
        // SAFETY: COM is initialised on this thread and CUIAutomation is the
        // in-process UI Automation coclass; no outer object is needed.
        let created: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.map_err(
                |error| {
                    CaptureFailure::Platform(format!(
                        "CoCreateInstance(CUIAutomation): {}",
                        error.message()
                    ))
                },
            )?;
        *slot = Some(created);
    }
    slot.as_ref().ok_or_else(|| {
        CaptureFailure::Platform("UI Automation instance unavailable".to_string())
    })
}

/// One capture attempt.
///
/// # Safety
///
/// Must be called on the capture worker thread with COM initialised, and
/// `automation` must be a live `IUIAutomation`.
unsafe fn capture_with(
    automation: &IUIAutomation,
    window: Option<isize>,
) -> Result<String, CaptureFailure> {
    // Fast path: whatever UI Automation considers focused.
    // SAFETY: `automation` is live on this thread.
    if let Ok(element) = unsafe { automation.GetFocusedElement() } {
        // SAFETY: element is live on this thread.
        if let Ok(text) = unsafe { try_element(&element) } {
            return Ok(text);
        }
    }

    // Fallback: the window the user was in when the hotkey fired. WinUI/XAML
    // hosts (Windows 11 Notepad, anything with a XAML island) report the
    // *window* as focused; the control that owns the selection is a descendant
    // (e.g. `Document` / `RichEditD2DPT`).
    // A provider error (as opposed to "nothing found") and the number of
    // `Document`/`Edit` elements the provider reported. The latter is tracked
    // only to tell "a text surface exists but holds no selection" from "no text
    // surface at all"; it is not shown to the user.
    let mut search_error = None;
    let mut text_surfaces = 0usize;
    if let Some(hwnd) = window {
        let target = HWND(hwnd as *mut core::ffi::c_void);
        if target.is_invalid() {
            search_error = Some("target window handle is invalid".to_string());
        } else {
            // SAFETY: live automation object; any HWND yields an element or an
            // error.
            if let Ok(root) = unsafe { automation.ElementFromHandle(target) } {
                // SAFETY: element is live on this thread.
                if let Some(text) = unsafe {
                    search_text(automation, &root, &mut search_error, &mut text_surfaces)
                } {
                    return Ok(text);
                }
            }
        }
    }

    if let Some(error) = search_error {
        return Err(CaptureFailure::Platform(error));
    }
    if text_surfaces > 0 {
        // There was a text surface, but no selection in it.
        return Err(CaptureFailure::NoSelection);
    }
    Err(CaptureFailure::NoTextPattern)
}

/// Find a descendant whose text pattern yields a non-empty selection.
///
/// `*text_surfaces` counts the `Document`/`Edit` descendants the provider
/// actually returned, not the elements visited by the search.
///
/// # Safety
///
/// Same contract as [`capture_with`].
unsafe fn search_text(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    search_error: &mut Option<String>,
    text_surfaces: &mut usize,
) -> Option<String> {
    // Ask UI Automation for the text surfaces below this window instead of
    // walking children by hand: XAML islands (Windows 11 Notepad) return
    // nothing for FindAll(Children) but do answer a descendant query.
    // SAFETY: `automation` is live on this thread (caller contract).
    let condition = match unsafe { text_surface_condition(automation) } {
        Ok(condition) => condition,
        Err(message) => {
            *search_error = Some(message);
            return None;
        }
    };

    // SAFETY: root is live; descendants are resolved by the provider.
    let found = match unsafe { root.FindAll(TreeScope_Descendants, &condition) } {
        Ok(found) => found,
        Err(error) => {
            *search_error = Some(format!("FindAll(descendants): {}", error.message()));
            return None;
        }
    };
    // SAFETY: live array; the count is provider data and is clamped.
    let count = match unsafe { found.Length() } {
        Ok(count) => count.clamp(0, 64),
        Err(error) => {
            *search_error = Some(format!("RangeArray.Length: {}", error.message()));
            return None;
        }
    };
    *text_surfaces += count as usize;

    for index in 0..count {
        // SAFETY: index is within the reported range.
        let Ok(element) = (unsafe { found.GetElement(index) }) else {
            continue;
        };
        // SAFETY: element is live on this thread.
        if let Ok(text) = unsafe { try_element(&element) } {
            return Some(text);
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

    // SAFETY: `automation` is live on this thread (function contract).
    let document = unsafe {
        automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_DocumentControlTypeId.0),
        )
    }
    .map_err(|error| format!("CreatePropertyCondition(Document): {}", error.message()))?;
    // SAFETY: as above; the condition object is independent storage.
    let edit = unsafe {
        automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_EditControlTypeId.0),
        )
    }
    .map_err(|error| format!("CreatePropertyCondition(Edit): {}", error.message()))?;

    // SAFETY: both conditions are live on this thread.
    let text_surfaces: IUIAutomationCondition =
        unsafe { automation.CreateOrCondition(&document, &edit) }
            .map_err(|error| format!("CreateOrCondition: {}", error.message()))?;

    // FindAll walks the *raw* view by default. XAML providers (Windows 11
    // Notepad, WinUI) expose their real content only in the control view: the
    // raw view shows a single empty Pane. Intersecting with the control-view
    // condition is what makes those apps work.
    // SAFETY: `automation` is live on this thread.
    let control_view = unsafe { automation.ControlViewCondition() }
        .map_err(|error| format!("ControlViewCondition: {}", error.message()))?;

    // SAFETY: all three conditions are live on this thread.
    let condition: IUIAutomationCondition =
        unsafe { automation.CreateAndCondition(&text_surfaces, &control_view) }
            .map_err(|error| format!("CreateAndCondition: {}", error.message()))?;
    Ok(condition)
}

/// Try the text pattern, then the value pattern, on one element.
///
/// Returns raw text; emptiness is validated through [`CapturedText::new`] so
/// the "is this usable?" rule exists exactly once.
///
/// # Safety
///
/// `element` must be a live UI Automation element on this thread.
unsafe fn try_element(element: &IUIAutomationElement) -> Result<String, CaptureFailure> {
    // SAFETY: `element` is alive for the whole function; the pattern is queried
    // through the element's own vtable.
    match unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) } {
        Ok(pattern) => {
            // SAFETY: `pattern` was just obtained from a live element.
            let ranges = unsafe { pattern.GetSelection() }.map_err(|error| {
                CaptureFailure::Platform(format!("TextPattern.GetSelection: {}", error.message()))
            })?;
            // SAFETY: `ranges` is a live range array owned by the pattern call.
            let count = unsafe { ranges.Length() }
                .map_err(|error| {
                    CaptureFailure::Platform(format!(
                        "RangeArray.Length: {}",
                        error.message()
                    ))
                })?
                .clamp(0, MAX_RANGES);

            let mut pieces = Vec::with_capacity(count as usize);
            for index in 0..count {
                // SAFETY: index is within [0, count) as reported by the array.
                let range = unsafe { ranges.GetElement(index) }.map_err(|error| {
                    CaptureFailure::Platform(format!(
                        "RangeArray.GetElement: {}",
                        error.message()
                    ))
                })?;
                // SAFETY: `range` is live; -1 means "the whole range".
                let text = unsafe { range.GetText(-1) }.map_err(|error| {
                    CaptureFailure::Platform(format!("TextRange.GetText: {}", error.message()))
                })?;
                pieces.push(text.to_string());
            }
            usable(join_pieces(&pieces))
        }
        // No TextPattern: fall back to ValuePattern for plain edit controls.
        Err(_) => {
            // SAFETY: `element` is alive; ValuePattern is queried the same way.
            match unsafe {
                element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            } {
                Ok(value) => {
                    // SAFETY: `value` is live; reading the current value has no
                    // side effects.
                    let text = unsafe { value.CurrentValue() }.map_err(|error| {
                        CaptureFailure::Platform(format!(
                            "ValuePattern.CurrentValue: {}",
                            error.message()
                        ))
                    })?;
                    let text = text.to_string();
                    // Strategy-specific heuristic: a ValuePattern exposes the
                    // *whole* content, not the selection. Looking up a
                    // 4000-character document would be wrong (and noisy), so
                    // treat anything long as "no selection". This is not the
                    // general capture cap; see VALUE_PATTERN_MAX_CHARS.
                    if text.chars().count() > VALUE_PATTERN_MAX_CHARS {
                        return Err(CaptureFailure::NoSelection);
                    }
                    usable(text)
                }
                Err(_) => Err(CaptureFailure::NoTextPattern),
            }
        }
    }
}

/// Validate a strategy result through the one constructor.
fn usable(raw: String) -> Result<String, CaptureFailure> {
    CapturedText::new(&raw, TrimSelection::Preserve).map(CapturedText::into_string)
}

/// Concatenate the selected ranges in provider order.
///
/// The defensive byte cap is applied by [`CapturedText::new`]; stopping the
/// concatenation early only avoids building an unbounded string first.
pub(crate) fn join_pieces(pieces: &[String]) -> String {
    let mut text = String::new();
    for piece in pieces {
        text.push_str(piece);
        if text.len() >= MAX_CAPTURED_BYTES {
            break;
        }
    }
    text
}

// ---------------------------------------------------------------------------
// Diagnostics: bounded tree sample
// ---------------------------------------------------------------------------

/// One node of the diagnostics tree sample.
///
/// Plain owned data: every UI Automation object is released before this leaves
/// the capture session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SampleNode {
    pub(crate) depth: usize,
    pub(crate) control_type: i32,
    pub(crate) class: String,
    pub(crate) has_text_pattern: bool,
}

/// Hard bounds for [`tree_sample`].
///
/// A diagnostic is a log line, not a data structure: the sampler stops at
/// `max_depth`, returns at most `max_nodes`, and truncates class names to
/// `max_class_chars` characters.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SampleLimits {
    pub(crate) max_depth: usize,
    pub(crate) max_nodes: usize,
    pub(crate) max_children_per_node: i32,
    pub(crate) max_class_chars: usize,
}

/// Walk the control tree below `window` and return a bounded sample.
///
/// This reads the *same* `IUIAutomation` object the pipeline used (the caller
/// passes the session's cached instance); it never creates a second automation
/// object and never runs a capture.
///
/// Returns `Err` with a short reason when the tree cannot be reached.
pub(crate) fn tree_sample(
    automation: &IUIAutomation,
    window: Option<isize>,
    limits: SampleLimits,
) -> Result<(Vec<SampleNode>, bool), String> {
    let Some(hwnd) = window else {
        return Err("target window unknown".to_string());
    };
    let target = HWND(hwnd as *mut core::ffi::c_void);
    if target.is_invalid() {
        return Err("target window handle is invalid".to_string());
    }

    // SAFETY: `automation` is live on this thread (the caller runs inside the
    // capture session) and any HWND yields an element or an error.
    let root = unsafe { automation.ElementFromHandle(target) }
        .map_err(|error| format!("ElementFromHandle: {}", error.message()))?;

    let mut nodes = Vec::with_capacity(limits.max_nodes.min(32));
    let mut truncated = false;
    // SAFETY: `automation` and `root` are live on this thread; the walk only
    // reads properties and children.
    unsafe { walk_sample(automation, &root, 0, limits, &mut nodes, &mut truncated) };
    Ok((nodes, truncated))
}

/// # Safety
///
/// `automation` and `element` must be live on a thread with COM initialised.
unsafe fn walk_sample(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
    depth: usize,
    limits: SampleLimits,
    nodes: &mut Vec<SampleNode>,
    truncated: &mut bool,
) {
    if nodes.len() >= limits.max_nodes {
        *truncated = true;
        return;
    }

    // SAFETY: `element` is live (caller contract). Provider data is clamped
    // below and the class name goes through a character-bounded truncation.
    let (control_type, class, has_text_pattern) = unsafe {
        let control_type = element
            .CurrentControlType()
            .map(|value| value.0)
            .unwrap_or(-1);
        let class = element
            .CurrentClassName()
            .map(|value| truncate_wide(value.as_wide(), limits.max_class_chars))
            .unwrap_or_default();
        let has_text_pattern = element
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .is_ok();
        (control_type, class, has_text_pattern)
    };

    nodes.push(SampleNode {
        depth,
        control_type,
        class,
        has_text_pattern,
    });

    // SAFETY: both objects are live on this thread (caller contract).
    let Ok(condition) = (unsafe { automation.CreateTrueCondition() }) else {
        return;
    };
    // SAFETY: `element` is live; the child array is provider data.
    let Ok(children) = (unsafe { element.FindAll(TreeScope_Children, &condition) }) else {
        return;
    };
    // SAFETY: live array; the count is provider data and is clamped below.
    let Ok(reported) = (unsafe { children.Length() }) else {
        return;
    };
    let count = reported.clamp(0, limits.max_children_per_node);
    if reported > count {
        *truncated = true;
    }
    if depth >= limits.max_depth {
        // The depth budget stops here; say so only if there is something below.
        if count > 0 {
            *truncated = true;
        }
        return;
    }

    for index in 0..count {
        if nodes.len() >= limits.max_nodes {
            *truncated = true;
            return;
        }
        // SAFETY: index is within the reported range.
        let Ok(child) = (unsafe { children.GetElement(index) }) else {
            continue;
        };
        // SAFETY: child is live on this thread.
        unsafe { walk_sample(automation, &child, depth + 1, limits, nodes, truncated) };
    }
}

/// Decode at most `max_chars` characters of a UTF-16 class name.
///
/// Bounded in both directions: the string never grows past `max_chars`
/// characters, and an unpaired surrogate becomes U+FFFD instead of a panic.
fn truncate_wide(wide: &[u16], max_chars: usize) -> String {
    let mut text = String::new();
    for decoded in std::char::decode_utf16(wide.iter().copied()).take(max_chars) {
        text.push(decoded.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_multiple_ranges_in_order() {
        let text = join_pieces(&["Hello".to_string(), " world".to_string()]);
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn join_does_not_trim() {
        // Trimming belongs to CapturedText, where the trim policy is applied.
        let raw = join_pieces(&["  spaced  ".to_string()]);
        assert_eq!(raw, "  spaced  ");
        assert_eq!(
            CapturedText::new(&raw, TrimSelection::Preserve)
                .expect("non-empty")
                .text(),
            "  spaced  "
        );
    }

    #[test]
    fn empty_ranges_are_rejected_by_the_validator() {
        assert_eq!(join_pieces(&[]), "");
        assert_eq!(
            CapturedText::new(&join_pieces(&[]), TrimSelection::Preserve),
            Err(CaptureFailure::NoSelection)
        );
        assert_eq!(
            CapturedText::new(
                &join_pieces(&["   ".to_string(), "\r\n".to_string()]),
                TrimSelection::Preserve
            ),
            Err(CaptureFailure::NoSelection)
        );
    }

    #[test]
    fn value_pattern_cap_is_a_strategy_heuristic() {
        // Boundary documented next to the constant: <= 200 chars is a plausible
        // selection, > 200 is treated as "the whole document".
        assert_eq!(VALUE_PATTERN_MAX_CHARS, 200);
        assert_eq!(
            usable("x".repeat(VALUE_PATTERN_MAX_CHARS)).map(|text| text.len()),
            Ok(VALUE_PATTERN_MAX_CHARS)
        );
    }

    #[test]
    fn class_names_are_truncated_on_a_character_boundary() {
        let ascii: Vec<u16> = "Notepad".encode_utf16().collect();
        assert_eq!(truncate_wide(&ascii, 16), "Notepad");
        let cjk: Vec<u16> = "词典窗口".encode_utf16().collect();
        assert_eq!(truncate_wide(&cjk, 2), "词典");
        assert_eq!(truncate_wide(&ascii, 0), "");
    }
}
