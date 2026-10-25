//! The threaded capture service: a worker that owns COM and a coordinator that
//! enforces the hard timeout.
//!
//! This is transport, not policy. It calls the same [`super::pipeline::run`] as
//! the synchronous entry point, one job at a time, and turns the result into
//! [`Event`]s for the host thread. Dropping the handle stops the worker after
//! its current job (a hung UIA call is left to die with the process).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;

use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::IUIAutomation;

use crate::host::Event;

use super::failure::CaptureFailure;
use super::pipeline::{self, CAPTURE_TIMEOUT};
use super::target::CaptureTarget;
use super::text::CapturedText;
use super::win32::Win32Env;

/// Handle to the capture pipeline.
pub(crate) struct Capture {
    requests: Option<Sender<CaptureTarget>>,
    busy: Arc<AtomicBool>,
}

impl Capture {
    /// Spawn the worker and coordinator. `deliver` is called from the
    /// coordinator thread, never from the host thread.
    pub(crate) fn start(deliver: impl Fn(Event) + Send + 'static) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<CaptureTarget>();
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let busy = Arc::new(AtomicBool::new(false));

        let worker_busy = Arc::clone(&busy);
        let worker_started = thread::Builder::new()
            .name("dict-input-capture".to_string())
            .spawn(move || worker_loop(job_rx, worker_busy));
        if worker_started.is_err() {
            return Self {
                requests: None,
                busy,
            };
        }

        let coordinator_busy = Arc::clone(&busy);
        let coordinator_started = thread::Builder::new()
            .name("dict-input-capture-timeout".to_string())
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
    /// * `Ok(())` - the request was queued; the result arrives later as
    ///   [`Event::Selection`] or [`Event::SelectionFailed`].
    /// * `Err(Busy)` - a previous capture is still running (busy-skip policy).
    /// * `Err(Platform(..))` - the capture pipeline is not available.
    pub(crate) fn request(&self, target: CaptureTarget) -> Result<(), CaptureFailure> {
        let Some(sender) = &self.requests else {
            return Err(CaptureFailure::Platform(
                "capture worker unavailable".to_string(),
            ));
        };
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(CaptureFailure::Busy);
        }
        if sender.send(target).is_err() {
            self.busy.store(false, Ordering::Release);
            return Err(CaptureFailure::Platform(
                "capture worker stopped".to_string(),
            ));
        }
        Ok(())
    }
}

struct Job {
    target: CaptureTarget,
    reply: Sender<Result<CapturedText, CaptureFailure>>,
}

fn coordinator_loop(
    requests: Receiver<CaptureTarget>,
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
            deliver(Event::SelectionFailed(CaptureFailure::Platform(
                "capture worker stopped".to_string(),
            )));
            return;
        }

        match reply_rx.recv_timeout(CAPTURE_TIMEOUT) {
            Ok(Ok(selection)) => deliver(Event::Selection(selection)),
            Ok(Err(error)) => deliver(Event::SelectionFailed(error)),
            // Do not touch `busy`: the worker is still inside the UIA call and
            // clears the flag itself when (if ever) it returns. Until then every
            // hotkey is dropped as Busy/TimedOut, which is the documented
            // policy.
            Err(RecvTimeoutError::Timeout) => {
                deliver(Event::SelectionFailed(CaptureFailure::TimedOut));
            }
            Err(RecvTimeoutError::Disconnected) => {
                busy.store(false, Ordering::Release);
                deliver(Event::SelectionFailed(CaptureFailure::Platform(
                    "capture worker stopped".to_string(),
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
        let mut env = Win32Env::new(job.target.window(), &mut automation_slot, com_ready);
        let outcome = pipeline::run(&mut env, job.target);

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
