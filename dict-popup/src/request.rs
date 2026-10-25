//! The lifecycle of one capture request.
//!
//! A hotkey starts a request; the host captures on its own thread and the
//! result arrives as [`dict_input::Event::Selection`] or
//! [`dict_input::Event::SelectionFailed`] up to one capture budget later.
//! Everything the UI remembers about that request lives here:
//!
//! * the anchor the card is positioned from,
//! * the one deadline ([`WATCHDOG`], derived from `dict-input`'s real budget),
//! * the content the card renders ([`State::Done`]),
//! * the captured query kept while the dictionaries are still loading,
//! * and the answer to "is this result still wanted?".
//!
//! Dismissing the card ([`Request::dismiss`]) replaces the whole state with
//! [`State::Idle`]. A late capture result, and a deferred lookup for a query
//! that was waiting for the registry, then have nowhere to land: they are
//! dropped by construction instead of by the old `ignore_result` /
//! `card.pending` pair. The card has no phase of its own - [`State`] is what
//! the UI renders.

use std::time::{Duration, Instant};

use dict_input::capture::CAPTURE_BUDGET;
use dict_input::host::Anchor;
use dict_input::CaptureFailure;

use crate::dictionaries::LookupReport;

/// Extra time the UI gives the host to deliver a result after its own budget.
pub const DELIVERY_GRACE: Duration = Duration::from_millis(300);

/// The one UI deadline around a capture: the host gives up at
/// [`CAPTURE_BUDGET`], so the card does too - plus [`DELIVERY_GRACE`] so a
/// result that is already on its way wins the race.
pub const WATCHDOG: Duration = CAPTURE_BUDGET.saturating_add(DELIVERY_GRACE);

/// The sentence the card shows when no result made it back in time.
pub const TIMEOUT_TEXT: &str = "取词超时，请重试。若一直失败，当前程序可能不支持 UIA 取词。";

/// What a finished request renders in the card.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A finished lookup (may still have zero hits).
    Report(LookupReport),
    /// A sentence instead of a report.
    Info(Info),
}

/// An informational card, optionally with a query to run later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub text: String,
    /// The captured query to look up once the dictionaries finish loading.
    /// It lives inside the request state, so dismissing the request drops it.
    pub retry: Option<String>,
}

impl Info {
    /// A plain message (dictionaries missing / misconfigured).
    pub fn message(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            retry: None,
        }
    }

    /// "The dictionaries are loading; look `query` up when they are ready."
    pub fn waiting(text: impl Into<String>, query: String) -> Self {
        Self {
            text: text.into(),
            retry: Some(query),
        }
    }
}

/// One request's state. The card renders this and nothing else.
#[derive(Debug, Clone)]
pub enum State {
    /// Nothing in flight, nothing shown.
    Idle,
    /// The hotkey fired and the result has not arrived yet. The card stays
    /// hidden (a visible card takes focus, and the capture would then read our
    /// own window) until the result or the deadline.
    Running { deadline: Instant },
    /// The result arrived; the card shows `Outcome`.
    Done(Outcome),
    /// The capture failed. A timeout is *not* in here (see [`State::TimedOut`]),
    /// so `Busy` and friends keep their own sentence.
    Failed(CaptureFailure),
    /// Nothing arrived before the deadline, or the host reported its own
    /// timeout. A terminal state: late results are dropped.
    TimedOut,
}

impl State {
    pub fn is_running(&self) -> bool {
        matches!(self, State::Running { .. })
    }

    /// The card header: headword of the first hit, or the query, or a label.
    pub fn headline(&self) -> String {
        match self {
            State::Running { .. } => "查询中".to_string(),
            State::Done(Outcome::Report(report)) => report
                .groups
                .first()
                .map(|group| group.entry.headword.clone())
                .filter(|headword| !headword.is_empty())
                .unwrap_or_else(|| report.query.clone()),
            State::Done(Outcome::Info(_)) => "dict-popup".to_string(),
            State::Failed(_) | State::TimedOut => "取词失败".to_string(),
            State::Idle => String::new(),
        }
    }

    /// The failure sentence, when this state is a failure.
    ///
    /// `Busy` and timeout therefore read differently: the host saying "a
    /// capture is already running" is not the same event as "nothing came
    /// back", and the card must not conflate them.
    pub fn failure_text(&self) -> Option<String> {
        match self {
            State::Failed(failure) => Some(failure.to_string()),
            State::TimedOut => Some(TIMEOUT_TEXT.to_string()),
            _ => None,
        }
    }
}

/// One capture request. `App` owns exactly one of these instead of a
/// `pending_hotkey` timestamp, an `ignore_result` flag and a pending query on
/// the card.
#[derive(Debug, Clone)]
pub struct Request {
    state: State,
    anchor: Anchor,
    /// Bumped on every visible transition; the card uses it for widget ids so
    /// new content starts a fresh scroll/collapse state (and the fade uses
    /// `CardState::open_generation`, which is separate).
    revision: u64,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            state: State::Idle,
            anchor: Anchor::default(),
            revision: 0,
        }
    }
}

impl Request {
    pub fn state(&self) -> &State {
        &self.state
    }

    /// The anchor of the current request (or the last one, until the next
    /// `start`/`dismiss`). The card is placed from it.
    pub fn anchor(&self) -> Anchor {
        self.anchor
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }

    /// The running request's deadline, if a request is running.
    ///
    /// The UI re-arms its wake-up from this every pass, so a second hotkey that
    /// replaces the deadline cannot leave the app sleeping past it.
    pub fn deadline(&self) -> Option<Instant> {
        match self.state {
            State::Running { deadline } => Some(deadline),
            _ => None,
        }
    }

    /// Begin a request for a hotkey pressed at `anchor`.
    pub fn start(&mut self, anchor: Anchor, now: Instant) {
        self.anchor = anchor;
        self.state = State::Running {
            deadline: now + WATCHDOG,
        };
        self.bump();
    }

    /// Finish a running request with the content its result produced.
    ///
    /// Returns `true` when the request accepted the result. A result for a
    /// dismissed, timed-out or already-finished request is dropped here.
    pub fn complete(&mut self, outcome: Outcome) -> bool {
        if !self.is_running() {
            return false;
        }
        self.state = State::Done(outcome);
        self.bump();
        true
    }

    /// A failed capture. The host's own timeout becomes [`State::TimedOut`];
    /// every other failure - including [`CaptureFailure::Busy`] - keeps its own
    /// sentence in [`State::Failed`].
    ///
    /// Returns `true` when the request accepted the failure.
    pub fn fail(&mut self, failure: CaptureFailure) -> bool {
        if !self.is_running() {
            return false;
        }
        self.state = match failure {
            CaptureFailure::TimedOut => State::TimedOut,
            failure => State::Failed(failure),
        };
        self.bump();
        true
    }

    /// Apply the deadline. Returns `true` when the request just expired, so the
    /// caller can reveal the timeout card.
    pub fn expire(&mut self, now: Instant) -> bool {
        match self.state {
            State::Running { deadline } if now >= deadline => {
                self.state = State::TimedOut;
                self.bump();
                true
            }
            _ => false,
        }
    }

    /// Replace the content of a finished request with the deferred lookup
    /// result (the dictionaries finished loading).
    ///
    /// Returns `false` when the request is gone - the card was dismissed while
    /// the dictionaries loaded - which is what stops a dismissed card from
    /// reopening.
    pub fn resolve(&mut self, outcome: Outcome) -> bool {
        match self.state {
            State::Done(_) => {
                self.state = State::Done(outcome);
                self.bump();
                true
            }
            _ => false,
        }
    }

    /// End the request and forget everything about it.
    pub fn dismiss(&mut self) {
        if matches!(self.state, State::Idle) {
            return;
        }
        self.state = State::Idle;
        self.anchor = Anchor::default();
        self.bump();
    }

    /// The captured query waiting for the dictionaries, if any.
    pub fn take_pending_query(&mut self) -> Option<String> {
        match &mut self.state {
            State::Done(Outcome::Info(info)) => info.retry.take(),
            _ => None,
        }
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::dictionaries::QueryScript;

    fn anchor() -> Anchor {
        Anchor {
            cursor: Some((100, 200)),
            work_area: Some(dict_input::host::WorkArea {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }),
            scale: 1.0,
        }
    }

    fn report(query: &str) -> Outcome {
        Outcome::Report(LookupReport {
            query: query.to_string(),
            script: QueryScript::Latin,
            candidates: Vec::new(),
            groups: Vec::new(),
            missed: Vec::new(),
            searched: 0,
            filtered: 0,
        })
    }

    fn waiting(query: &str) -> Outcome {
        Outcome::Info(Info::waiting("词典加载中，请稍候…", query.to_string()))
    }

    #[test]
    fn idle_to_running_to_done() {
        let mut request = Request::default();
        assert!(matches!(request.state(), State::Idle));
        assert!(!request.is_running());

        let now = Instant::now();
        request.start(anchor(), now);
        assert!(request.is_running());
        assert_eq!(request.anchor(), anchor());

        let State::Running { deadline } = request.state() else {
            panic!("start must run the request");
        };
        assert_eq!(*deadline, now + WATCHDOG);

        let revealed = request.complete(report("apple"));
        assert!(revealed);
        assert_eq!(request.anchor(), anchor());
        assert!(matches!(
            request.state(),
            State::Done(Outcome::Report(report)) if report.query == "apple"
        ));
        assert_eq!(request.state().headline(), "apple");

        // A duplicate result for the finished request is dropped.
        assert!(!request.complete(report("banana")));
    }

    #[test]
    fn dismiss_drops_a_late_result() {
        let mut request = Request::default();
        request.start(anchor(), Instant::now());

        request.dismiss();
        assert!(matches!(request.state(), State::Idle));
        assert_eq!(request.anchor(), Anchor::default());

        // The host cannot cancel a capture, so both kinds of result can arrive
        // after the card was closed. Neither may reopen it.
        assert!(!request.complete(report("apple")));
        assert!(!request.fail(CaptureFailure::NoSelection));
        assert!(!request.expire(Instant::now()));
        assert!(matches!(request.state(), State::Idle));
    }

    #[test]
    fn the_deadline_times_out_exactly_once() {
        let mut request = Request::default();
        let now = Instant::now();
        request.start(anchor(), now);

        assert!(!request.expire(now + WATCHDOG - Duration::from_millis(1)));
        assert!(request.is_running());

        assert!(request.expire(now + WATCHDOG));
        assert!(matches!(request.state(), State::TimedOut));
        // A late result after the timeout is dropped too.
        assert!(!request.complete(report("apple")));
        assert!(!request.expire(now + WATCHDOG * 2));
    }

    #[test]
    fn busy_is_not_a_timeout() {
        let mut busy = Request::default();
        busy.start(anchor(), Instant::now());
        assert!(busy.fail(CaptureFailure::Busy));
        assert!(matches!(
            busy.state(),
            State::Failed(CaptureFailure::Busy)
        ));

        let mut timed_out = Request::default();
        let now = Instant::now();
        timed_out.start(anchor(), now);
        timed_out.expire(now + WATCHDOG);
        assert!(matches!(timed_out.state(), State::TimedOut));

        let busy_text = busy.state().failure_text().unwrap_or_default();
        let timeout_text = timed_out.state().failure_text().unwrap_or_default();
        assert_eq!(busy_text, CaptureFailure::Busy.to_string());
        assert!(busy_text.contains("上一次取词"));
        assert_ne!(busy_text, timeout_text);
        assert_ne!(busy.state().headline(), "dict-popup");
    }

    #[test]
    fn the_hosts_own_timeout_becomes_the_timeout_state() {
        let mut request = Request::default();
        request.start(anchor(), Instant::now());
        assert!(request.fail(CaptureFailure::TimedOut));
        assert!(matches!(request.state(), State::TimedOut));
        assert_eq!(
            request.state().failure_text().as_deref(),
            Some(TIMEOUT_TEXT)
        );
    }

    #[test]
    fn a_pending_query_is_kept_until_the_registry_loads() {
        let mut request = Request::default();
        request.start(anchor(), Instant::now());
        assert!(request.complete(waiting("apple")));

        assert_eq!(request.take_pending_query().as_deref(), Some("apple"));
        // Taken once only.
        assert_eq!(request.take_pending_query(), None);

        // The deferred lookup replaces the "loading" sentence with the report
        // and keeps the card where it is.
        assert!(request.resolve(report("apple")));
        assert_eq!(request.anchor(), anchor());
        assert!(matches!(
            request.state(),
            State::Done(Outcome::Report(report)) if report.query == "apple"
        ));
    }

    #[test]
    fn dismissing_forgets_the_pending_query() {
        let mut request = Request::default();
        request.start(anchor(), Instant::now());
        request.complete(waiting("apple"));

        request.dismiss();

        // This is the old Esc bug: the registry finishes loading and replays a
        // query for a card the user already closed. The query died with the
        // request, and a late lookup cannot resolve an Idle request.
        assert_eq!(request.take_pending_query(), None);
        assert!(!request.resolve(report("apple")));
        assert!(matches!(request.state(), State::Idle));
    }

    #[test]
    fn a_second_hotkey_restarts_the_request_at_the_new_anchor() {
        let mut request = Request::default();
        let first = Instant::now();
        request.start(anchor(), first);

        let mut second_anchor = anchor();
        second_anchor.cursor = Some((600, 700));
        request.start(second_anchor, first + Duration::from_millis(50));

        assert!(request.is_running());
        assert_eq!(request.anchor(), second_anchor);
        let State::Running { deadline } = request.state() else {
            panic!("the second start must run the request");
        };
        assert_eq!(*deadline, first + Duration::from_millis(50) + WATCHDOG);
    }

    #[test]
    fn the_watchdog_is_derived_from_the_capture_budget() {
        // One authoritative number: dict-input's budget. The UI adds only the
        // delivery margin; it never restates the budget as its own duration.
        assert_eq!(WATCHDOG, CAPTURE_BUDGET + DELIVERY_GRACE);
        assert!(WATCHDOG > CAPTURE_BUDGET);
    }

    #[test]
    fn transitions_bump_the_revision() {
        let mut request = Request::default();
        let mut seen = request.revision();
        request.start(anchor(), Instant::now());
        assert_ne!(request.revision(), seen);
        seen = request.revision();
        request.complete(report("apple"));
        assert_ne!(request.revision(), seen);
        seen = request.revision();
        request.dismiss();
        assert_ne!(request.revision(), seen);
    }
}
