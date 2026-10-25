//! In-memory [`CaptureEnv`] shared by the unit tests of the capture pipeline
//! and of the diagnostics recorder.
//!
//! Test-only: no Win32, no clipboard, no COM, and every call is recorded so a
//! test can assert what the pipeline actually asked for.

use std::cell::Cell;
use std::time::{Duration, Instant};

use super::env::CaptureEnv;
use super::failure::CaptureFailure;

/// Scriptable capture environment. Fields are public inside the crate so each
/// test can describe the machine it wants and inspect what happened.
pub(crate) struct FakeEnv {
    pub(crate) clock: Cell<Instant>,
    /// Every clock read advances the fake clock by this much.
    pub(crate) step: Cell<Duration>,
    pub(crate) pot: Option<String>,
    pub(crate) copy_failure: Option<CaptureFailure>,
    pub(crate) copy_bumps_sequence: bool,
    pub(crate) copy_text: Option<String>,
    pub(crate) wm_copy_available: bool,
    pub(crate) wm_copy_bumps_sequence: bool,
    pub(crate) wm_copy_text: Option<String>,
    pub(crate) sequence: Cell<u32>,
    pub(crate) clipboard: Option<String>,
    pub(crate) uia: Result<String, CaptureFailure>,
    pub(crate) calls: Vec<&'static str>,
    pub(crate) writes: Vec<String>,
}

impl FakeEnv {
    pub(crate) fn new(previous: Option<&str>) -> Self {
        Self {
            clock: Cell::new(Instant::now()),
            step: Cell::new(Duration::ZERO),
            pot: None,
            copy_failure: None,
            copy_bumps_sequence: false,
            copy_text: None,
            wm_copy_available: false,
            wm_copy_bumps_sequence: false,
            wm_copy_text: None,
            sequence: Cell::new(1),
            clipboard: previous.map(str::to_string),
            uia: Err(CaptureFailure::NoTextPattern),
            calls: Vec::new(),
            writes: Vec::new(),
        }
    }

    pub(crate) fn step_clock_by(&self, step: Duration) {
        self.step.set(step);
    }

    fn advance(&self) {
        let step = self.step.get();
        if !step.is_zero() {
            self.clock.set(self.clock.get() + step);
        }
    }
}

impl CaptureEnv for FakeEnv {
    fn now(&self) -> Instant {
        let now = self.clock.get();
        self.advance();
        now
    }

    fn sleep(&self, _duration: Duration) {
        self.advance();
    }

    fn pot_selection(&mut self) -> Option<String> {
        self.calls.push("pot");
        self.pot.clone()
    }

    fn press_copy(&mut self) -> Result<(), CaptureFailure> {
        self.calls.push("press_copy");
        if let Some(error) = &self.copy_failure {
            return Err(error.clone());
        }
        if self.copy_bumps_sequence {
            self.sequence.set(self.sequence.get() + 1);
            self.clipboard = self.copy_text.clone();
        }
        Ok(())
    }

    fn request_wm_copy(&mut self) -> bool {
        self.calls.push("wm_copy");
        if !self.wm_copy_available {
            return false;
        }
        if self.wm_copy_bumps_sequence {
            self.sequence.set(self.sequence.get() + 1);
            self.clipboard = self.wm_copy_text.clone();
        }
        true
    }

    fn clipboard_seq(&self) -> u32 {
        self.sequence.get()
    }

    fn read_clipboard(&mut self) -> Result<String, CaptureFailure> {
        self.calls.push("read_clipboard");
        self.clipboard
            .clone()
            .ok_or(CaptureFailure::NoSelection)
    }

    fn write_clipboard(&mut self, text: &str) {
        self.writes.push(text.to_string());
    }

    fn read_uia(&mut self) -> Result<String, CaptureFailure> {
        self.calls.push("read_uia");
        self.uia.clone()
    }
}
