//! The one capture entry point and its ordered strategy list.
//!
//! [`run`] is the whole policy: Pot's crate, then our own `Ctrl+C` clipboard
//! capture, then `WM_COPY`, then UI Automation. Callers cannot reorder,
//! disable, or even name a strategy - they pass a [`CaptureTarget`] and get
//! either a validated [`CapturedText`] or a [`CaptureFailure`].
//!
//! The clipboard restore also lives here, exactly once: the user's previous
//! clipboard text is snapshotted before the first strategy and written back
//! after the last one, whether the capture succeeded or failed.
//!
//! Deadlines are cooperative inside this module and hard at the service layer:
//! [`CAPTURE_TIMEOUT`] bounds one whole request, while the clipboard strategies
//! get shorter per-strategy budgets. A hung UI Automation provider cannot be
//! cancelled, so the worker/coordinator pair still enforces the total budget
//! around it.

use std::time::{Duration, Instant};

use super::env::CaptureEnv;
use super::failure::CaptureFailure;
use super::target::CaptureTarget;
use super::text::{CapturedText, TrimSelection};

/// Hard budget for one capture request (docs/DESIGN.md §3).
///
/// The service coordinator uses this as the hard timeout around the worker;
/// the pipeline uses it to stop starting new strategies once time is up.
pub(crate) const CAPTURE_TIMEOUT: Duration = Duration::from_millis(1500);

/// How long the target application gets to answer a synthetic `Ctrl+C`.
const CLIPBOARD_COPY_TIMEOUT: Duration = Duration::from_millis(450);
/// `WM_COPY` is synchronous, so a short window is enough.
const WM_COPY_TIMEOUT: Duration = Duration::from_millis(300);
/// Clipboard polling interval. The sequence number, not a content comparison,
/// is the primary "did anything happen?" signal.
const POLL_INTERVAL: Duration = Duration::from_millis(15);

/// Which ordered strategy an attempt belongs to.
///
/// Diagnostics only: the pipeline reports attempts through [`Observer`] and
/// nothing in the capture path branches on this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strategy {
    Pot,
    CtrlC,
    WmCopy,
    Uia,
}

/// What one strategy attempt produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StrategyOutcome {
    /// The strategy produced text the pipeline accepted.
    Text { chars: usize },
    /// The strategy produced nothing usable (or was not reached because the
    /// request ran out of time after it).
    Failure(CaptureFailure),
}

/// Observation seam for diagnostics.
///
/// The pipeline calls this after every strategy it actually runs. Production
/// passes [`NoObserver`], whose default methods do nothing, so the capture path
/// keeps its exact behaviour; `diagnostics` passes a recorder and turns these
/// events into its report.
pub(crate) trait Observer {
    fn strategy_started(&mut self, _strategy: Strategy) {}

    fn strategy_finished(&mut self, _strategy: Strategy, _outcome: &StrategyOutcome) {}
}

/// The observer production uses: it observes nothing.
pub(crate) struct NoObserver;

impl Observer for NoObserver {}

/// Run the ordered strategy list.
///
/// This is the only place that knows the order. The public
/// [`super::capture`] builds a Win32 environment and calls this; the capture
/// worker does the same on its own COM-initialised thread.
pub(crate) fn run<E: CaptureEnv>(
    env: &mut E,
    target: CaptureTarget,
) -> Result<CapturedText, CaptureFailure> {
    run_observed(env, target, &mut NoObserver)
}

/// [`run`] with an observer attached, for diagnostics.
///
/// This is the same code path production takes: the observer cannot reorder,
/// skip, or duplicate a strategy.
pub(crate) fn run_observed<E: CaptureEnv>(
    env: &mut E,
    target: CaptureTarget,
    observer: &mut dyn Observer,
) -> Result<CapturedText, CaptureFailure> {
    let deadline = env.now() + CAPTURE_TIMEOUT;

    // Snapshot the user's clipboard once. Besides being what we restore, it is
    // the baseline for "the clipboard did not actually change" - a copy that
    // reproduces the old text is not a new selection.
    let previous = match env.read_clipboard() {
        Ok(text) => Some(text),
        // Restoring is impossible if we cannot read; say why instead of
        // replacing the user's clipboard with something arbitrary.
        Err(CaptureFailure::ClipboardBusy) => return Err(CaptureFailure::ClipboardBusy),
        Err(_) => None,
    };

    let outcome = run_strategies(env, target, previous.as_deref(), deadline, observer);

    // Restore policy: exactly one place, exactly one write, success or failure.
    if let Some(previous) = previous.as_deref() {
        env.write_clipboard(previous);
    }
    outcome
}

fn run_strategies<E: CaptureEnv>(
    env: &mut E,
    target: CaptureTarget,
    previous: Option<&str>,
    deadline: Instant,
    observer: &mut dyn Observer,
) -> Result<CapturedText, CaptureFailure> {
    // 1. Pot's crate. It restores the clipboard itself and handles modifier
    //    release the way Pot does, so it is the best first try.
    observer.strategy_started(Strategy::Pot);
    if let Some(raw) = env.pot_selection() {
        if let Ok(text) = CapturedText::new(&raw, target.trim()) {
            observer.strategy_finished(
                Strategy::Pot,
                &StrategyOutcome::Text {
                    chars: text.char_count(),
                },
            );
            return Ok(text);
        }
    }
    observer.strategy_finished(
        Strategy::Pot,
        &StrategyOutcome::Failure(CaptureFailure::NoSelection),
    );
    if expired(env, deadline) {
        return Err(CaptureFailure::TimedOut);
    }

    let mut failure = CaptureFailure::NoSelection;

    // 2. Our own Ctrl+C capture.
    observer.strategy_started(Strategy::CtrlC);
    match copy_via_clipboard(
        env,
        CopyTrigger::SyntheticInput,
        previous,
        deadline,
        target.trim(),
    ) {
        Ok(text) => {
            observer.strategy_finished(
                Strategy::CtrlC,
                &StrategyOutcome::Text {
                    chars: text.char_count(),
                },
            );
            return Ok(text);
        }
        Err(error) => {
            observer.strategy_finished(Strategy::CtrlC, &StrategyOutcome::Failure(error.clone()));
            failure = failure.prefer(error);
        }
    }
    if expired(env, deadline) {
        return Err(CaptureFailure::TimedOut);
    }

    // 3. WM_COPY: reaches standard edit controls even when synthetic keyboard
    //    input does not.
    observer.strategy_started(Strategy::WmCopy);
    match copy_via_clipboard(
        env,
        CopyTrigger::WindowMessage,
        previous,
        deadline,
        target.trim(),
    ) {
        Ok(text) => {
            observer.strategy_finished(
                Strategy::WmCopy,
                &StrategyOutcome::Text {
                    chars: text.char_count(),
                },
            );
            return Ok(text);
        }
        Err(error) => {
            observer.strategy_finished(Strategy::WmCopy, &StrategyOutcome::Failure(error.clone()));
            failure = failure.prefer(error);
        }
    }
    if expired(env, deadline) {
        return Err(CaptureFailure::TimedOut);
    }

    // 4. UI Automation: no clipboard side effect, but many toolkits do not
    //    implement the text patterns properly.
    observer.strategy_started(Strategy::Uia);
    let outcome = match env.read_uia() {
        Ok(raw) => match CapturedText::new(&raw, target.trim()) {
            Ok(text) => Ok(text),
            Err(error) => Err(failure.prefer(error)),
        },
        Err(error) => Err(failure.prefer(error)),
    };
    match outcome {
        Ok(text) => {
            observer.strategy_finished(
                Strategy::Uia,
                &StrategyOutcome::Text {
                    chars: text.char_count(),
                },
            );
            Ok(text)
        }
        Err(error) => {
            observer.strategy_finished(Strategy::Uia, &StrategyOutcome::Failure(error.clone()));
            Err(error)
        }
    }
}

/// How a copy request reaches the application.
#[derive(Clone, Copy)]
enum CopyTrigger {
    /// Synthetic `Ctrl+C` through `SendInput`.
    SyntheticInput,
    /// `WM_COPY` to the focused control.
    WindowMessage,
}

impl CopyTrigger {
    fn budget(self) -> Duration {
        match self {
            CopyTrigger::SyntheticInput => CLIPBOARD_COPY_TIMEOUT,
            CopyTrigger::WindowMessage => WM_COPY_TIMEOUT,
        }
    }

    fn request<E: CaptureEnv>(self, env: &mut E) -> Result<(), CaptureFailure> {
        match self {
            CopyTrigger::SyntheticInput => env.press_copy(),
            CopyTrigger::WindowMessage => {
                if env.request_wm_copy() {
                    Ok(())
                } else {
                    // No focus window to ask: this strategy simply has no
                    // selection to offer.
                    Err(CaptureFailure::NoSelection)
                }
            }
        }
    }
}

/// One poll loop, shared by both copy triggers.
///
/// The sequence number is sampled once, before the request. A change means the
/// clipboard was written; text identical to the previous clipboard content is
/// *not* accepted as a new selection.
fn copy_via_clipboard<E: CaptureEnv>(
    env: &mut E,
    trigger: CopyTrigger,
    previous: Option<&str>,
    deadline: Instant,
    trim: TrimSelection,
) -> Result<CapturedText, CaptureFailure> {
    let sequence_before = env.clipboard_seq();
    trigger.request(env)?;

    let budget = trigger.budget();
    let strategy_deadline = deadline.min(env.now() + budget);
    // A frozen clock (or a broken `now()`) must not turn the loop into a hang.
    let max_attempts = (budget.as_millis() / POLL_INTERVAL.as_millis()).max(1) as usize + 4;
    let mut attempts = 0usize;
    let mut last_failure = CaptureFailure::NoSelection;

    loop {
        if attempts >= max_attempts || env.now() >= strategy_deadline {
            return Err(if last_failure == CaptureFailure::NoSelection {
                CaptureFailure::TimedOut
            } else {
                last_failure
            });
        }
        env.sleep(POLL_INTERVAL);
        attempts += 1;

        if env.clipboard_seq() == sequence_before {
            continue;
        }

        match env.read_clipboard() {
            Ok(text) => {
                if previous == Some(text.as_str()) {
                    // The sequence moved for an unrelated reason (clipboard
                    // manager, our own write, another application) but the
                    // payload is the same old text: not a copy.
                    return Err(CaptureFailure::NoSelection);
                }
                return CapturedText::new(&text, trim);
            }
            Err(error) => last_failure = error,
        }
    }
}

fn expired<E: CaptureEnv>(env: &E, deadline: Instant) -> bool {
    env.now() >= deadline
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::testing::FakeEnv;

    fn target() -> CaptureTarget {
        CaptureTarget::new(Some(0x1234), TrimSelection::Preserve)
    }

    #[test]
    fn first_strategy_that_yields_text_wins() {
        let mut env = FakeEnv::new(Some("OLD"));
        env.pot = Some("from pot".to_string());
        env.copy_bumps_sequence = true;
        env.copy_text = Some("from clipboard".to_string());
        env.uia = Ok("from uia".to_string());

        let text = run(&mut env, target()).expect("Pot should win");
        assert_eq!(text.text(), "from pot");
        assert!(!env.calls.contains(&"press_copy"), "{:?}", env.calls);
        assert!(!env.calls.contains(&"wm_copy"), "{:?}", env.calls);
        assert!(!env.calls.contains(&"read_uia"), "{:?}", env.calls);
    }

    #[test]
    fn clipboard_strategy_wins_before_wm_copy_and_uia() {
        let mut env = FakeEnv::new(Some("OLD"));
        env.copy_bumps_sequence = true;
        env.copy_text = Some("from clipboard".to_string());
        env.wm_copy_available = true;
        env.wm_copy_bumps_sequence = true;
        env.wm_copy_text = Some("from wm_copy".to_string());
        env.uia = Ok("from uia".to_string());

        let text = run(&mut env, target()).expect("clipboard should win");
        assert_eq!(text.text(), "from clipboard");
        assert!(env.calls.contains(&"press_copy"));
        assert!(!env.calls.contains(&"wm_copy"), "{:?}", env.calls);
        assert!(!env.calls.contains(&"read_uia"), "{:?}", env.calls);
    }

    #[test]
    fn wm_copy_runs_when_ctrl_c_produced_nothing() {
        let mut env = FakeEnv::new(Some("OLD"));
        env.wm_copy_available = true;
        env.wm_copy_bumps_sequence = true;
        env.wm_copy_text = Some("from wm_copy".to_string());
        env.uia = Ok("from uia".to_string());

        let text = run(&mut env, target()).expect("wm_copy should win");
        assert_eq!(text.text(), "from wm_copy");
        assert!(env.calls.contains(&"press_copy"));
        assert!(env.calls.contains(&"wm_copy"));
        assert!(!env.calls.contains(&"read_uia"), "{:?}", env.calls);
    }

    #[test]
    fn previous_clipboard_is_restored_exactly_once_when_every_strategy_fails() {
        let mut env = FakeEnv::new(Some("the user's clipboard"));
        env.pot = None;
        env.wm_copy_available = false;
        env.uia = Err(CaptureFailure::NoTextPattern);

        let error = run(&mut env, target()).expect_err("everything failed");
        assert_eq!(error, CaptureFailure::NoTextPattern, "{:?}", env.calls);
        assert_eq!(env.writes, vec!["the user's clipboard".to_string()]);
    }

    #[test]
    fn unchanged_previous_text_is_not_treated_as_a_copy() {
        let mut env = FakeEnv::new(Some("OLD"));
        // Ctrl+C bumps the sequence but leaves the same text in place.
        env.copy_bumps_sequence = true;
        env.copy_text = Some("OLD".to_string());
        env.wm_copy_available = false;
        env.uia = Ok("from uia".to_string());

        let text = run(&mut env, target()).expect("UIA should be reached");
        assert_eq!(text.text(), "from uia");
        assert_eq!(env.writes, vec!["OLD".to_string()]);
    }

    #[test]
    fn a_strategy_that_never_produces_text_ends_as_timed_out() {
        let mut env = FakeEnv::new(Some("OLD"));
        // Every clock read jumps past the whole capture budget, as a hung
        // provider would.
        env.step_clock_by(Duration::from_secs(2));
        env.copy_bumps_sequence = false;

        let error = run(&mut env, target()).expect_err("no strategy can succeed");
        assert_eq!(error, CaptureFailure::TimedOut);
    }

    #[test]
    fn a_frozen_clock_cannot_hang_the_poll_loop() {
        let mut env = FakeEnv::new(Some("OLD"));
        // `now()` never moves and the clipboard never changes: the attempt cap
        // must end the strategy instead of spinning forever.
        env.step_clock_by(Duration::ZERO);
        env.uia = Err(CaptureFailure::TimedOut);

        let error = run(&mut env, target()).expect_err("no strategy can succeed");
        assert_eq!(error, CaptureFailure::TimedOut);
    }

    #[test]
    fn the_target_trim_policy_is_applied_where_the_text_is_created() {
        let mut env = FakeEnv::new(None);
        env.pot = Some("  from pot  ".to_string());

        let trimmed = run(
            &mut env,
            CaptureTarget::new(None, TrimSelection::Trim),
        )
        .expect("Pot should win");
        assert_eq!(trimmed.text(), "from pot");

        let mut env = FakeEnv::new(None);
        env.pot = Some("  from pot  ".to_string());
        let preserved = run(
            &mut env,
            CaptureTarget::new(None, TrimSelection::Preserve),
        )
        .expect("Pot should win");
        assert_eq!(preserved.text(), "  from pot  ");
    }

    #[test]
    fn input_failure_is_reported_when_no_later_strategy_wins() {
        let mut env = FakeEnv::new(Some("OLD"));
        env.copy_failure = Some(CaptureFailure::InputBlocked);
        env.wm_copy_available = false;
        env.uia = Err(CaptureFailure::NoSelection);

        let error = run(&mut env, target()).expect_err("everything failed");
        assert_eq!(error, CaptureFailure::InputBlocked);
        assert_eq!(env.writes, vec!["OLD".to_string()]);
    }

    /// Collects the attempts the pipeline reports, in order.
    struct Collect(Vec<Strategy>);

    impl Observer for Collect {
        fn strategy_finished(&mut self, strategy: Strategy, _outcome: &StrategyOutcome) {
            self.0.push(strategy);
        }
    }

    #[test]
    fn observer_sees_every_attempt_in_the_documented_order() {
        let mut env = FakeEnv::new(Some("OLD"));
        env.uia = Err(CaptureFailure::NoTextPattern);
        let mut collect = Collect(Vec::new());

        let error = run_observed(&mut env, target(), &mut collect).expect_err("everything failed");
        assert_eq!(error, CaptureFailure::NoTextPattern);
        assert_eq!(
            collect.0,
            vec![
                Strategy::Pot,
                Strategy::CtrlC,
                Strategy::WmCopy,
                Strategy::Uia,
            ]
        );
    }

    #[test]
    fn observer_is_not_told_about_strategies_that_did_not_run() {
        let mut env = FakeEnv::new(Some("OLD"));
        env.pot = Some("from pot".to_string());
        let mut collect = Collect(Vec::new());

        let text = run_observed(&mut env, target(), &mut collect).expect("Pot should win");
        assert_eq!(text.text(), "from pot");
        assert_eq!(collect.0, vec![Strategy::Pot]);
    }
}
