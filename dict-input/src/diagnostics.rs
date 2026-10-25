//! Structured diagnostics for the capture pipeline.
//!
//! This module is the **only** diagnostics entry point:
//!
//! ```text
//! cargo run --release --example capture_probe -- [hwnd]
//! ```
//!
//! [`run`] does not re-enact a capture. It wraps the real capture environment
//! in a recording adapter, runs the production capture pipeline through that
//! wrapper inside the production COM session, and turns what it observed into a
//! [`Report`]. The pipeline cannot be reordered, a second `Ctrl+C` cannot be
//! sent, and a second clipboard read path cannot be used: there is exactly one
//! of each, in the production module.
//!
//! Formatting lives here, at the edge: [`Report`] implements [`Display`] and
//! the probe examples print it. Nothing in this module writes to stdout.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::time::{Duration, Instant};

use windows::Win32::UI::Accessibility::IUIAutomation;

use crate::capture::pipeline::{self, Observer, Strategy as PipelineStrategy, StrategyOutcome};
use crate::capture::uia::{self, SampleLimits};
use crate::capture::win32::{self, Win32Env};
use crate::capture::{CaptureEnv, CaptureFailure, CaptureTarget, CapturedText, TrimSelection};
use crate::privilege::{self, Integrity};

/// Upper bound for the UIA tree sample's depth.
const TREE_MAX_DEPTH: usize = 4;
/// Upper bound for the number of nodes in the UIA tree sample.
const TREE_MAX_NODES: usize = 24;
/// Upper bound for the children requested from one provider node.
const TREE_MAX_CHILDREN_PER_NODE: i32 = 16;
/// Class names longer than this are truncated in a report.
const TREE_MAX_CLASS_CHARS: usize = 48;
/// A note explaining why the tree sample is empty is a log line, not prose.
const MAX_TREE_NOTE_CHARS: usize = 96;
/// Upper bound for entries in the environment trace.
const MAX_TRACE_ENTRIES: usize = 64;
/// Upper bound for one trace result string.
const MAX_TRACE_DETAIL_CHARS: usize = 60;

/// One ordered capture strategy.
///
/// The order is [`Strategy::ALL`] and it mirrors the production pipeline
/// exactly; a report cannot invent a strategy the pipeline does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Pot's selection crate.
    Pot,
    /// Our synthetic `Ctrl+C` plus clipboard poll.
    CtrlC,
    /// `WM_COPY` to the focused control.
    WmCopy,
    /// UI Automation.
    Uia,
}

impl Strategy {
    /// The strategies in pipeline order. The pipeline itself is authoritative;
    /// this is the reporting copy of the same list.
    pub const ALL: [Strategy; 4] = [
        Strategy::Pot,
        Strategy::CtrlC,
        Strategy::WmCopy,
        Strategy::Uia,
    ];

    /// Short label used in a report.
    pub fn label(self) -> &'static str {
        match self {
            Strategy::Pot => "Pot",
            Strategy::CtrlC => "Ctrl+C",
            Strategy::WmCopy => "WM_COPY",
            Strategy::Uia => "UIA",
        }
    }
}

impl From<PipelineStrategy> for Strategy {
    fn from(strategy: PipelineStrategy) -> Self {
        match strategy {
            PipelineStrategy::Pot => Strategy::Pot,
            PipelineStrategy::CtrlC => Strategy::CtrlC,
            PipelineStrategy::WmCopy => Strategy::WmCopy,
            PipelineStrategy::Uia => Strategy::Uia,
        }
    }
}

impl fmt::Display for Strategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// What one run (or one strategy attempt) produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The strategy produced text the pipeline accepted. The report keeps the
    /// character count, never the captured text itself.
    Captured { chars: usize },
    /// The strategy produced nothing usable.
    Failed(CaptureFailure),
}

impl Outcome {
    /// Whether this outcome carries captured text.
    pub fn is_captured(&self) -> bool {
        matches!(self, Outcome::Captured { .. })
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Captured { chars } => write!(f, "captured {chars} chars"),
            Outcome::Failed(error) => write!(f, "failed ({error:?})"),
        }
    }
}

/// One strategy the pipeline actually ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// Which strategy.
    pub strategy: Strategy,
    /// What it returned.
    pub outcome: Outcome,
    /// Wall-clock time the pipeline spent in this strategy.
    pub elapsed: Duration,
}

/// One `CaptureEnv` method the pipeline called.
///
/// The high-frequency clock and sleep calls are not listed individually; they
/// are counted in [`PollingCounts`] so a long poll loop cannot push the
/// interesting calls out of a bounded report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvCall {
    /// `pot_selection()`.
    PotSelection,
    /// `press_copy()`.
    PressCopy,
    /// `request_wm_copy()`.
    RequestWmCopy,
    /// `clipboard_seq()`, logged only when the value changes.
    ClipboardSequence,
    /// `read_clipboard()`.
    ReadClipboard,
    /// `write_clipboard()`.
    WriteClipboard,
    /// `read_uia()`.
    ReadUia,
}

impl EnvCall {
    /// Short label used in a report.
    pub fn label(self) -> &'static str {
        match self {
            EnvCall::PotSelection => "pot_selection",
            EnvCall::PressCopy => "press_copy",
            EnvCall::RequestWmCopy => "request_wm_copy",
            EnvCall::ClipboardSequence => "clipboard_seq",
            EnvCall::ReadClipboard => "read_clipboard",
            EnvCall::WriteClipboard => "write_clipboard",
            EnvCall::ReadUia => "read_uia",
        }
    }
}

impl fmt::Display for EnvCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Counts of the polling calls the pipeline made.
///
/// The recorder sees every `now()`, `sleep()` and `clipboard_seq()` call; the
/// first two are aggregated here, and a sequence read is logged separately only
/// when the number changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PollingCounts {
    /// `CaptureEnv::now()` calls.
    pub clock_reads: usize,
    /// `CaptureEnv::sleep()` calls.
    pub sleeps: usize,
    /// `clipboard_seq()` calls that returned the value already seen last.
    pub unchanged_sequence_polls: usize,
}

/// One recorded environment call.
///
/// Consecutive identical calls are coalesced into one entry with
/// [`RecordedCall::repeats`], so a repeated read stays readable without losing
/// the fact that the call happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    /// Which method was called.
    pub call: EnvCall,
    /// A bounded description of the result (`Ok(12 chars)`, `Err(NoSelection)`,
    /// `false`, the sequence number, ...).
    pub result: String,
    /// How many times this exact call and result occurred in a row.
    pub repeats: usize,
}

/// A bounded sample of the UI Automation tree.
///
/// Plain owned data; the UIA objects are released before the report leaves the
/// capture session. `note` explains an empty sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSample {
    /// The sampled nodes, depth-first from the target window's root.
    pub nodes: Vec<TreeSampleNode>,
    /// Whether a depth, node, or child limit cut the walk short.
    pub truncated: bool,
    /// Why nothing could be sampled, when that is the case.
    pub note: Option<String>,
}

/// One node of [`TreeSample`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSampleNode {
    /// Depth below the target window (0 = the window itself).
    pub depth: usize,
    /// UI Automation control type id (`-1` when the provider failed).
    pub control_type: i32,
    /// Class name, truncated to a bounded number of characters.
    pub class: String,
    /// Whether the node exposes `TextPattern`.
    pub has_text_pattern: bool,
}

/// The structured result of one diagnostic run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The window the pipeline was pointed at.
    pub target_window: Option<isize>,
    /// Class name of the window that held the foreground when the run started.
    pub foreground_class: Option<String>,
    /// Whether the process token carries UIAccess. Informational: capture works
    /// without it, but synthetic `Ctrl+C` cannot cross integrity levels.
    pub uiaccess: bool,
    /// Whether the process token is elevated.
    pub elevated: bool,
    /// The process token's mandatory integrity level.
    pub integrity: Integrity,
    /// The strategy that produced the captured text, if any.
    pub chosen: Option<Strategy>,
    /// The run's final result.
    pub outcome: Outcome,
    /// Strategies the pipeline actually ran, in pipeline order.
    pub attempts: Vec<Attempt>,
    /// Strategies the pipeline never reached.
    pub not_attempted: Vec<Strategy>,
    /// Clipboard sequence number sampled before the pipeline ran.
    pub clipboard_sequence_before: u32,
    /// Clipboard sequence number sampled after the pipeline restored it.
    pub clipboard_sequence_after: u32,
    /// Bounded UIA tree sample, present only when the run failed at UIA.
    pub tree_sample: Option<TreeSample>,
    /// Every environment call the pipeline made that produced an interesting
    /// result, coalesced and bounded.
    pub env_trace: Vec<RecordedCall>,
    /// Calls that did not fit into [`Report::env_trace`].
    pub env_trace_suppressed: usize,
    /// High-frequency polling calls, accounted for instead of listed.
    pub polling: PollingCounts,
}

impl Report {
    /// Whether the clipboard sequence changed over the whole run (the copy
    /// itself and the restore both count).
    pub fn clipboard_changed(&self) -> bool {
        self.clipboard_sequence_before != self.clipboard_sequence_after
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "dict-input capture diagnostics")?;
        match self.target_window {
            Some(window) => writeln!(f, "  target window: 0x{window:X}")?,
            None => writeln!(f, "  target window: <none>")?,
        }
        match &self.foreground_class {
            Some(class) => writeln!(f, "  foreground class: '{class}'")?,
            None => writeln!(f, "  foreground class: <unknown>")?,
        }
        writeln!(
            f,
            "  privilege: uiaccess={} elevated={} integrity={}",
            self.uiaccess, self.elevated, self.integrity
        )?;
        writeln!(
            f,
            "  clipboard sequence: {} -> {} ({})",
            self.clipboard_sequence_before,
            self.clipboard_sequence_after,
            if self.clipboard_changed() {
                "changed"
            } else {
                "unchanged"
            }
        )?;

        writeln!(f, "  attempts:")?;
        if self.attempts.is_empty() {
            writeln!(f, "    <none>")?;
        }
        for (index, attempt) in self.attempts.iter().enumerate() {
            writeln!(
                f,
                "    {}. {}: {} in {:.1} ms",
                index + 1,
                attempt.strategy,
                attempt.outcome,
                attempt.elapsed.as_secs_f64() * 1000.0
            )?;
        }

        match self.chosen {
            Some(strategy) => writeln!(f, "  chosen: {strategy}")?,
            None => writeln!(f, "  chosen: <none>")?,
        }
        writeln!(f, "  result: {}", self.outcome)?;
        writeln!(
            f,
            "  not attempted: {}",
            strategy_list(&self.not_attempted)
        )?;

        if let Some(tree) = &self.tree_sample {
            write!(f, "  uia tree sample: ")?;
            match &tree.note {
                Some(note) => writeln!(f, "unavailable ({note})")?,
                None => {
                    writeln!(
                        f,
                        "{} nodes{}",
                        tree.nodes.len(),
                        if tree.truncated { ", truncated" } else { "" }
                    )?;
                    for node in &tree.nodes {
                        writeln!(
                            f,
                            "    [{}] type={} class='{}' text-pattern={}",
                            node.depth, node.control_type, node.class, node.has_text_pattern
                        )?;
                    }
                }
            }
        }

        writeln!(f, "  env trace: {} entries", self.env_trace.len())?;
        for call in &self.env_trace {
            if call.repeats > 1 {
                writeln!(f, "    {}() -> {} x{}", call.call, call.result, call.repeats)?;
            } else {
                writeln!(f, "    {}() -> {}", call.call, call.result)?;
            }
        }
        if self.env_trace_suppressed > 0 {
            writeln!(f, "    ({} calls suppressed)", self.env_trace_suppressed)?;
        }
        if self.polling != PollingCounts::default() {
            writeln!(
                f,
                "  env polling: clock reads={}, sleeps={}, unchanged sequence polls={}",
                self.polling.clock_reads,
                self.polling.sleeps,
                self.polling.unchanged_sequence_polls
            )?;
        }
        Ok(())
    }
}

/// Run one capture through the production pipeline and report what happened.
///
/// `target` describes where to capture. `None` uses the current foreground
/// window, exactly like the old probe did; an explicit target is used as
/// given. The pipeline's own order, deadlines, and clipboard restore are
/// untouched.
pub fn run(target: Option<CaptureTarget>) -> Report {
    let privilege = privilege::current();
    let foreground = win32::foreground_window();
    let foreground_class = foreground.and_then(win32::class_name);
    let capture_target = target
        .unwrap_or_else(|| CaptureTarget::new(foreground, TrimSelection::Preserve));
    let window = capture_target.window();

    let ((pipeline_run, env_trace, env_trace_suppressed, polling), tree_sample) =
        crate::capture::with_com(|automation, com_ready| {
            let (run, env_trace, env_trace_suppressed, polling) = {
                let mut environment = Win32Env::new(window, &mut *automation, com_ready);
                let mut env = Recorder::new(&mut environment);
                let run = run_pipeline(&mut env, capture_target);
                let (env_trace, env_trace_suppressed, polling) = env.into_trace();
                (run, env_trace, env_trace_suppressed, polling)
            };

            // The recorder (and the Win32Env inside it) is gone, so the
            // session's automation object can be inspected without a second
            // COM initialisation or a second automation instance.
            let tree_sample = if run.uia_failed() {
                Some(sample_tree(automation.as_ref(), window))
            } else {
                None
            };
            ((run, env_trace, env_trace_suppressed, polling), tree_sample)
        });

    Report {
        target_window: window,
        foreground_class,
        uiaccess: privilege.uiaccess,
        elevated: privilege.elevated,
        integrity: privilege.integrity,
        chosen: pipeline_run.chosen,
        outcome: pipeline_run.outcome,
        attempts: pipeline_run.attempts,
        not_attempted: pipeline_run.not_attempted,
        clipboard_sequence_before: pipeline_run.sequence_before,
        clipboard_sequence_after: pipeline_run.sequence_after,
        tree_sample,
        env_trace,
        env_trace_suppressed,
        polling,
    }
}

// ---------------------------------------------------------------------------
// Probe helpers (probe examples and regression tests only)
// ---------------------------------------------------------------------------

/// Read the clipboard text (no capture involved).
pub fn clipboard_text() -> Option<String> {
    win32::read_text()
}

/// Replace the clipboard text (no capture involved).
pub fn write_clipboard(text: &str) {
    win32::write_text(text);
}

/// Type text with `KEYEVENTF_UNICODE` and report how many input events the API
/// accepted. Used by `capture_probe` to separate "synthetic input never
/// arrives" from "the copy shortcut was ignored".
pub fn type_text(text: &str) -> usize {
    win32::type_text(text)
}

/// Press a virtual-key chord through `SendInput`: keys are pressed in order
/// and released in reverse. Used by `tests/clipboard_loop.rs`.
pub fn press_combo(keys: &[u16]) {
    win32::press_combo(keys)
}

/// Run the production capture entry point and flatten its result to strings,
/// for the regression test that drives a real edit control.
pub fn capture_selection() -> Result<String, String> {
    crate::capture::capture(CaptureTarget::new(None, TrimSelection::Preserve))
        .map(CapturedText::into_string)
        .map_err(|failure| failure.to_string())
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Everything the pipeline part of a run learned.
struct PipelineRun {
    outcome: Outcome,
    chosen: Option<Strategy>,
    attempts: Vec<Attempt>,
    not_attempted: Vec<Strategy>,
    sequence_before: u32,
    sequence_after: u32,
}

impl PipelineRun {
    /// Whether the run ended in a UIA failure, which is when a tree sample is
    /// worth collecting.
    fn uia_failed(&self) -> bool {
        matches!(self.outcome, Outcome::Failed(_))
            && self.attempts.iter().any(|attempt| {
                attempt.strategy == Strategy::Uia && matches!(attempt.outcome, Outcome::Failed(_))
            })
    }
}

/// Run the production pipeline through `env` and collect the observer events.
///
/// Kept generic over [`CaptureEnv`] so the recorder can be tested against the
/// in-memory fake without Win32.
fn run_pipeline<E: CaptureEnv>(env: &mut E, target: CaptureTarget) -> PipelineRun {
    // Sampled through the same seam, and therefore visible in the trace.
    let sequence_before = env.clipboard_seq();
    let mut observer = AttemptObserver::default();
    let result = pipeline::run_observed(env, target, &mut observer);
    let sequence_after = env.clipboard_seq();

    let attempts = observer.attempts;
    let outcome = match &result {
        Ok(text) => Outcome::Captured {
            chars: text.char_count(),
        },
        Err(error) => Outcome::Failed(error.clone()),
    };
    let chosen = attempts.iter().rev().find_map(|attempt| match attempt.outcome {
        Outcome::Captured { .. } => Some(attempt.strategy),
        Outcome::Failed(_) => None,
    });
    let not_attempted = Strategy::ALL
        .iter()
        .copied()
        .filter(|strategy| {
            !attempts
                .iter()
                .any(|attempt| attempt.strategy == *strategy)
        })
        .collect();

    PipelineRun {
        outcome,
        chosen,
        attempts,
        not_attempted,
        sequence_before,
        sequence_after,
    }
}

/// Turns pipeline observer events into [`Attempt`]s, with wall-clock timing.
#[derive(Default)]
struct AttemptObserver {
    started: Option<(Strategy, Instant)>,
    attempts: Vec<Attempt>,
}

impl Observer for AttemptObserver {
    fn strategy_started(&mut self, strategy: PipelineStrategy) {
        self.started = Some((strategy.into(), Instant::now()));
    }

    fn strategy_finished(&mut self, strategy: PipelineStrategy, outcome: &StrategyOutcome) {
        let strategy = Strategy::from(strategy);
        let elapsed = match self.started.take() {
            Some((started, at)) if started == strategy => at.elapsed(),
            // Defensive: never let a missing start turn into a panic in a
            // diagnostic. The pipeline always pairs the events.
            _ => Duration::ZERO,
        };
        let outcome = match outcome {
            StrategyOutcome::Text { chars } => Outcome::Captured { chars: *chars },
            StrategyOutcome::Failure(error) => Outcome::Failed(error.clone()),
        };
        self.attempts.push(Attempt {
            strategy,
            outcome,
            elapsed,
        });
    }
}

/// Sample the tree through the session's existing automation object.
fn sample_tree(automation: Option<&IUIAutomation>, window: Option<isize>) -> TreeSample {
    let Some(automation) = automation else {
        return TreeSample {
            nodes: Vec::new(),
            truncated: false,
            note: Some("no UI Automation object in the capture session".to_string()),
        };
    };
    let limits = SampleLimits {
        max_depth: TREE_MAX_DEPTH,
        max_nodes: TREE_MAX_NODES,
        max_children_per_node: TREE_MAX_CHILDREN_PER_NODE,
        max_class_chars: TREE_MAX_CLASS_CHARS,
    };
    match uia::tree_sample(automation, window, limits) {
        Ok((nodes, truncated)) => TreeSample {
            nodes: nodes
                .into_iter()
                .map(|node| TreeSampleNode {
                    depth: node.depth,
                    control_type: node.control_type,
                    class: node.class,
                    has_text_pattern: node.has_text_pattern,
                })
                .collect(),
            truncated,
            note: None,
        },
        Err(note) => TreeSample {
            nodes: Vec::new(),
            truncated: false,
            note: Some(truncate_chars(&note, MAX_TREE_NOTE_CHARS)),
        },
    }
}

/// Recording decorator around a real [`CaptureEnv`].
///
/// Every call the pipeline makes goes through this wrapper exactly as it would
/// go straight to the Win32 adapter; the wrapper only writes it down. Calls
/// that produced a result worth reading are kept in order; the high-frequency
/// clock, sleep, and unchanged-sequence polls are counted in
/// [`PollingCounts`] so a long poll loop cannot crowd out the strategy calls
/// that matter.
struct Recorder<'a, E: CaptureEnv> {
    inner: &'a mut E,
    trace: RefCell<Trace>,
    last_sequence: Cell<Option<u32>>,
}

#[derive(Default)]
struct Trace {
    entries: Vec<RecordedCall>,
    suppressed: usize,
    polling: PollingCounts,
}

impl<'a, E: CaptureEnv> Recorder<'a, E> {
    fn new(inner: &'a mut E) -> Self {
        Self {
            inner,
            trace: RefCell::new(Trace::default()),
            last_sequence: Cell::new(None),
        }
    }

    /// Append one call, coalescing an identical consecutive entry.
    ///
    /// `try_borrow_mut` instead of `borrow_mut`: a diagnostic must never panic,
    /// and a nested borrow (which cannot happen today) would simply drop the
    /// entry.
    fn record(&self, call: EnvCall, result: String) {
        let Ok(mut trace) = self.trace.try_borrow_mut() else {
            return;
        };
        if let Some(last) = trace.entries.last_mut() {
            if last.call == call && last.result == result {
                last.repeats = last.repeats.saturating_add(1);
                return;
            }
        }
        if trace.entries.len() >= MAX_TRACE_ENTRIES {
            trace.suppressed = trace.suppressed.saturating_add(1);
            return;
        }
        trace.entries.push(RecordedCall {
            call,
            result,
            repeats: 1,
        });
    }

    /// Account for one high-frequency polling call.
    fn count_poll(&self, update: impl FnOnce(&mut PollingCounts)) {
        let Ok(mut trace) = self.trace.try_borrow_mut() else {
            return;
        };
        update(&mut trace.polling);
    }

    fn into_trace(self) -> (Vec<RecordedCall>, usize, PollingCounts) {
        let trace = self.trace.into_inner();
        (trace.entries, trace.suppressed, trace.polling)
    }
}

impl<E: CaptureEnv> CaptureEnv for Recorder<'_, E> {
    fn now(&self) -> Instant {
        self.count_poll(|polling| {
            polling.clock_reads = polling.clock_reads.saturating_add(1);
        });
        self.inner.now()
    }

    fn sleep(&self, duration: Duration) {
        self.count_poll(|polling| {
            polling.sleeps = polling.sleeps.saturating_add(1);
        });
        self.inner.sleep(duration);
    }

    fn pot_selection(&mut self) -> Option<String> {
        let selection = self.inner.pot_selection();
        let result = match &selection {
            Some(text) => format!("Some({} chars)", text.chars().count()),
            None => "None".to_string(),
        };
        self.record(EnvCall::PotSelection, result);
        selection
    }

    fn press_copy(&mut self) -> Result<(), CaptureFailure> {
        let result = self.inner.press_copy();
        let detail = match &result {
            Ok(()) => "Ok".to_string(),
            Err(error) => format!("Err({})", short_failure(error)),
        };
        self.record(EnvCall::PressCopy, detail);
        result
    }

    fn request_wm_copy(&mut self) -> bool {
        let accepted = self.inner.request_wm_copy();
        self.record(EnvCall::RequestWmCopy, accepted.to_string());
        accepted
    }

    fn clipboard_seq(&self) -> u32 {
        let sequence = self.inner.clipboard_seq();
        if self.last_sequence.get() == Some(sequence) {
            self.count_poll(|polling| {
                polling.unchanged_sequence_polls =
                    polling.unchanged_sequence_polls.saturating_add(1);
            });
        } else {
            self.last_sequence.set(Some(sequence));
            self.record(EnvCall::ClipboardSequence, sequence.to_string());
        }
        sequence
    }

    fn read_clipboard(&mut self) -> Result<String, CaptureFailure> {
        let result = self.inner.read_clipboard();
        self.record(EnvCall::ReadClipboard, describe_text(&result));
        result
    }

    fn write_clipboard(&mut self, text: &str) {
        self.inner.write_clipboard(text);
        self.record(
            EnvCall::WriteClipboard,
            format!("{} chars", text.chars().count()),
        );
    }

    fn read_uia(&mut self) -> Result<String, CaptureFailure> {
        let result = self.inner.read_uia();
        self.record(EnvCall::ReadUia, describe_text(&result));
        result
    }
}

/// Bounded description of a text-producing environment call.
fn describe_text(result: &Result<String, CaptureFailure>) -> String {
    match result {
        Ok(text) => format!("Ok({} chars)", text.chars().count()),
        Err(error) => format!("Err({})", short_failure(error)),
    }
}

/// Bounded debug name of a failure, for the environment trace.
fn short_failure(error: &CaptureFailure) -> String {
    truncate_chars(&format!("{error:?}"), MAX_TRACE_DETAIL_CHARS)
}

/// Concatenate the labels, or `none`.
fn strategy_list(strategies: &[Strategy]) -> String {
    if strategies.is_empty() {
        return "none".to_string();
    }
    strategies
        .iter()
        .map(|strategy| strategy.label())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Copy at most `max_chars` characters of `text`.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::testing::FakeEnv;

    fn target() -> CaptureTarget {
        CaptureTarget::new(Some(0x1234), TrimSelection::Preserve)
    }

    fn has_call(trace: &[RecordedCall], call: EnvCall) -> bool {
        trace.iter().any(|entry| entry.call == call)
    }

    #[test]
    fn report_renders_every_section() {
        let report = Report {
            target_window: Some(0x1234),
            foreground_class: Some("Notepad".to_string()),
            uiaccess: false,
            elevated: true,
            integrity: Integrity::High,
            chosen: Some(Strategy::CtrlC),
            outcome: Outcome::Captured { chars: 5 },
            attempts: vec![
                Attempt {
                    strategy: Strategy::Pot,
                    outcome: Outcome::Failed(CaptureFailure::NoSelection),
                    elapsed: Duration::from_millis(2),
                },
                Attempt {
                    strategy: Strategy::CtrlC,
                    outcome: Outcome::Captured { chars: 5 },
                    elapsed: Duration::from_millis(450),
                },
            ],
            not_attempted: vec![Strategy::WmCopy, Strategy::Uia],
            clipboard_sequence_before: 10,
            clipboard_sequence_after: 12,
            tree_sample: None,
            env_trace: vec![RecordedCall {
                call: EnvCall::PressCopy,
                result: "Ok".to_string(),
                repeats: 1,
            }],
            env_trace_suppressed: 0,
            polling: PollingCounts::default(),
        };

        let text = report.to_string();
        assert!(text.contains("target window: 0x1234"), "{text}");
        assert!(text.contains("foreground class: 'Notepad'"), "{text}");
        assert!(
            text.contains("privilege: uiaccess=false elevated=true integrity=High"),
            "{text}"
        );
        assert!(text.contains("clipboard sequence: 10 -> 12 (changed)"), "{text}");
        assert!(text.contains("1. Pot: failed (NoSelection)"), "{text}");
        assert!(text.contains("2. Ctrl+C: captured 5 chars"), "{text}");
        assert!(text.contains("chosen: Ctrl+C"), "{text}");
        assert!(text.contains("result: captured 5 chars"), "{text}");
        assert!(text.contains("not attempted: WM_COPY, UIA"), "{text}");
        assert!(text.contains("press_copy() -> Ok"), "{text}");
    }

    #[test]
    fn report_renders_a_tree_sample_and_coalesced_calls() {
        let report = Report {
            target_window: None,
            foreground_class: None,
            uiaccess: false,
            elevated: false,
            integrity: Integrity::Unknown,
            chosen: None,
            outcome: Outcome::Failed(CaptureFailure::NoTextPattern),
            attempts: vec![Attempt {
                strategy: Strategy::Uia,
                outcome: Outcome::Failed(CaptureFailure::NoTextPattern),
                elapsed: Duration::from_millis(1),
            }],
            not_attempted: vec![Strategy::Pot, Strategy::CtrlC, Strategy::WmCopy],
            clipboard_sequence_before: 7,
            clipboard_sequence_after: 7,
            tree_sample: Some(TreeSample {
                nodes: vec![TreeSampleNode {
                    depth: 0,
                    control_type: 50032,
                    class: "Notepad".to_string(),
                    has_text_pattern: false,
                }],
                truncated: true,
                note: None,
            }),
            env_trace: vec![
                RecordedCall {
                    call: EnvCall::ClipboardSequence,
                    result: "7".to_string(),
                    repeats: 1,
                },
                RecordedCall {
                    call: EnvCall::ReadUia,
                    result: "Err(NoTextPattern)".to_string(),
                    repeats: 1,
                },
            ],
            env_trace_suppressed: 2,
            polling: PollingCounts {
                clock_reads: 3,
                sleeps: 2,
                unchanged_sequence_polls: 4,
            },
        };

        let text = report.to_string();
        assert!(text.contains("target window: <none>"), "{text}");
        assert!(text.contains("foreground class: <unknown>"), "{text}");
        assert!(
            text.contains("privilege: uiaccess=false elevated=false integrity=Unknown"),
            "{text}"
        );
        assert!(text.contains("clipboard sequence: 7 -> 7 (unchanged)"), "{text}");
        assert!(text.contains("chosen: <none>"), "{text}");
        assert!(text.contains("uia tree sample: 1 nodes, truncated"), "{text}");
        assert!(
            text.contains("[0] type=50032 class='Notepad' text-pattern=false"),
            "{text}"
        );
        assert!(text.contains("clipboard_seq() -> 7"), "{text}");
        assert!(text.contains("read_uia() -> Err(NoTextPattern)"), "{text}");
        assert!(
            text.contains("env polling: clock reads=3, sleeps=2, unchanged sequence polls=4"),
            "{text}"
        );
        assert!(text.contains("(2 calls suppressed)"), "{text}");
    }

    #[test]
    fn recorder_wraps_the_fake_env_and_records_what_the_pipeline_did() {
        let mut fake = FakeEnv::new(Some("OLD"));
        fake.copy_bumps_sequence = true;
        fake.copy_text = Some("from clipboard".to_string());

        let (run, (trace, suppressed, polling)) = {
            let mut recorder = Recorder::new(&mut fake);
            let run = run_pipeline(&mut recorder, target());
            let trace = recorder.into_trace();
            (run, trace)
        };

        assert_eq!(run.outcome, Outcome::Captured { chars: 14 });
        assert_eq!(run.chosen, Some(Strategy::CtrlC));
        assert_eq!(
            run.attempts
                .iter()
                .map(|attempt| attempt.strategy)
                .collect::<Vec<_>>(),
            vec![Strategy::Pot, Strategy::CtrlC]
        );
        assert_eq!(run.not_attempted, vec![Strategy::WmCopy, Strategy::Uia]);
        assert_eq!(run.sequence_before, 1);
        assert_eq!(run.sequence_after, 2);

        assert!(has_call(&trace, EnvCall::PotSelection));
        assert!(has_call(&trace, EnvCall::PressCopy));
        assert!(has_call(&trace, EnvCall::ReadClipboard));
        assert!(has_call(&trace, EnvCall::WriteClipboard));
        assert_eq!(suppressed, 0);
        assert!(polling.clock_reads > 0);
        assert!(polling.sleeps > 0);
        assert!(trace
            .iter()
            .any(|entry| entry.call == EnvCall::ReadClipboard
                && entry.result == "Ok(14 chars)"));

        // The recorder forwarded the production restore write unchanged.
        assert_eq!(fake.writes, vec!["OLD".to_string()]);
    }

    #[test]
    fn recorder_coalesces_repeated_calls() {
        let mut fake = FakeEnv::new(Some("OLD"));
        fake.uia = Err(CaptureFailure::NoTextPattern);
        let mut recorder = Recorder::new(&mut fake);
        let _ = recorder.read_uia();
        let _ = recorder.read_uia();
        let (trace, suppressed, polling) = recorder.into_trace();

        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0].call, EnvCall::ReadUia);
        assert_eq!(trace[0].repeats, 2);
        assert_eq!(suppressed, 0);
        assert_eq!(polling, PollingCounts::default());
    }

    #[test]
    fn polling_is_counted_so_later_strategies_stay_visible() {
        let mut fake = FakeEnv::new(Some("OLD"));
        // Nothing produces text, so the Ctrl+C poll loop runs to its attempt
        // cap. The trace must stay bounded while WM_COPY and UIA remain visible.
        let (run, (trace, _suppressed, polling)) = {
            let mut recorder = Recorder::new(&mut fake);
            let run = run_pipeline(&mut recorder, target());
            let trace = recorder.into_trace();
            (run, trace)
        };

        assert!(matches!(run.outcome, Outcome::Failed(_)));
        assert!(trace.len() < MAX_TRACE_ENTRIES);
        assert!(has_call(&trace, EnvCall::PotSelection));
        assert!(has_call(&trace, EnvCall::PressCopy));
        assert!(has_call(&trace, EnvCall::RequestWmCopy));
        assert!(has_call(&trace, EnvCall::ReadUia));
        assert!(polling.clock_reads > 0);
        assert!(polling.sleeps > 0);
        assert!(polling.unchanged_sequence_polls > 0);
    }

    #[test]
    fn a_failed_uia_run_gets_a_tree_sample_slot() {
        let mut fake = FakeEnv::new(Some("OLD"));
        fake.uia = Err(CaptureFailure::NoTextPattern);
        let mut recorder = Recorder::new(&mut fake);

        let run = run_pipeline(&mut recorder, target());

        assert_eq!(run.outcome, Outcome::Failed(CaptureFailure::NoTextPattern));
        assert!(run.uia_failed());
        assert_eq!(run.chosen, None);
        assert_eq!(
            run.attempts
                .iter()
                .map(|attempt| attempt.strategy)
                .collect::<Vec<_>>(),
            vec![
                Strategy::Pot,
                Strategy::CtrlC,
                Strategy::WmCopy,
                Strategy::Uia,
            ]
        );
    }
}
