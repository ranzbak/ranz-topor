//! Run control for a routing call: cancellation and progress reporting (M4b, Task P1).
//!
//! A [`RunControl`] is shared between the caller (any thread) and the routing run. `cancel` is
//! polled at every deadline check of the engine through [`expired`] / [`cancelled`]: the run then
//! takes the same anytime path as a wall-clock timeout and returns the best result so far with
//! `stop_reason == "cancel"`. `progress` is a snapshot the run overwrites per iteration and per
//! pass.
//!
//! The low-level searches and relaxations receive a plain `Instant` deadline; to keep their
//! signatures the cancel flag of the current run is installed in a thread-local on every thread
//! of the run's private rayon pool ([`install_cancel`]), so concurrent runs in one process never
//! cancel each other.
//!
//! Complexity: [`expired`] and [`cancelled`] are O(1) (one clock read, one thread-local access,
//! one relaxed atomic load); [`RunControl::update`] is O(1) under an uncontended mutex.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Progress snapshot of a run. Monotone over a run: `iteration` is cumulative over the
/// pour-completion passes, `routed` / `total` / `elapsed_s` never decrease (high-water marks).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Progress {
    /// `""` before the run starts, then `setup`, `route`, `pour completion`, `done`.
    pub phase: String,
    pub iteration: u32,
    pub routed: usize,
    pub total: usize,
    pub vias: usize,
    pub elapsed_s: f64,
    /// What the run does now, in words for the progress dialog (topological core: "topological search",
    /// "layer assignment", "drawing", ...); "" when the core does not say. Not a high-water mark.
    #[serde(default)]
    pub stage: String,
    /// Whole-route round now running (1-based) and the most rounds ([optimize] rounds); 0 = unknown.
    #[serde(default)]
    pub round: u32,
    #[serde(default)]
    pub rounds: u32,
    /// M-B0: the best result has gained less than `[optimize] stable_gain_pct` over the last
    /// `stable_window` cycles in every run ("Routes stable"; the plugin's Cancel then reads OK).
    #[serde(default)]
    pub stable: bool,
    /// M-B0: gain (%) of the best result over the last `stable_window` cycles (the largest over
    /// the runs still going); `None` before a full window.
    #[serde(default)]
    pub gain_pct: Option<f64>,
    /// M-B0: fluctuation width of the cycle that started last.
    #[serde(default)]
    pub width: f64,
    /// M-B0 best of N: runs in all (`[optimize] runs`), how many are stable, how many ended.
    #[serde(default)]
    pub runs: u32,
    #[serde(default)]
    pub runs_stable: u32,
    #[serde(default)]
    pub runs_done: u32,
    /// M-B0: the best result so far over all runs (what OK / Cancel returns): routed of total,
    /// length (mm), vias and its run (1-based; 0 = none yet).
    #[serde(default)]
    pub best_routed: usize,
    #[serde(default)]
    pub best_total: usize,
    #[serde(default)]
    pub best_length_mm: f64,
    #[serde(default)]
    pub best_vias: usize,
    #[serde(default)]
    pub best_run: u32,
}

/// Cancel flag and progress of one run; cheap to clone (shared state).
#[derive(Clone, Debug, Default)]
pub struct RunControl {
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<Mutex<Progress>>,
    /// M-B0: 0 = the only run; k >= 1 = run k of a best-of-N call (its stage text is prefixed
    /// with the run, and the round is the coordinator's).
    pub run: u32,
}

impl RunControl {
    pub fn new() -> RunControl {
        RunControl::default()
    }

    /// Ask the run to stop at its next deadline check. O(1).
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Copy of the current progress. O(1).
    pub fn snapshot(&self) -> Progress {
        self.progress
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default()
    }

    /// Name the current stage and round (progress dialog wording); leaves the counters alone. O(1).
    pub(crate) fn stage(&self, round: u32, rounds: u32, stage: &str) {
        if let Ok(mut p) = self.progress.lock() {
            if self.run > 0 && p.runs > 1 {
                p.stage = if stage.is_empty() { String::new() } else { format!("run {}: {stage}", self.run) };
                return;
            }
            p.stage = stage.to_string();
            p.round = round;
            p.rounds = rounds;
        }
    }

    /// The same cancel flag and progress, as run `run` (1-based) of a best-of-N call. O(1).
    pub(crate) fn for_run(&self, run: u32) -> RunControl {
        RunControl { cancel: self.cancel.clone(), progress: self.progress.clone(), run }
    }

    /// Change the progress under the lock (the M-B0 coordinator's fields). O(1) + `f`.
    pub(crate) fn with_progress(&self, f: impl FnOnce(&mut Progress)) {
        if let Ok(mut p) = self.progress.lock() {
            f(&mut p);
        }
    }

    /// Update the progress; counters only ever grow (high-water marks). O(1).
    #[allow(dead_code)] // progress hook for the routing core (green field)
    pub(crate) fn update(
        &self,
        phase: &str,
        iteration: u32,
        routed: usize,
        total: usize,
        vias: usize,
        elapsed_s: f64,
    ) {
        if let Ok(mut p) = self.progress.lock() {
            p.phase = phase.to_string();
            p.iteration = p.iteration.max(iteration);
            p.routed = p.routed.max(routed);
            p.total = p.total.max(total);
            p.vias = p.vias.max(vias);
            p.elapsed_s = p.elapsed_s.max(elapsed_s);
        }
    }
}

thread_local! {
    static CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Install (or clear) the cancel flag polled by [`cancelled`] on the current thread.
#[allow(dead_code)] // cancel hook for the routing core (green field)
pub(crate) fn install_cancel(flag: Option<Arc<AtomicBool>>) {
    CANCEL.with(|c| *c.borrow_mut() = flag);
}

/// Whether the run of the current thread was cancelled. O(1).
pub fn cancelled() -> bool {
    CANCEL.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    })
}

/// Deadline passed or run cancelled: the single stop test of every deadline site. O(1).
pub fn expired(deadline: Instant) -> bool {
    Instant::now() >= deadline || cancelled()
}

/// Error of a [`Job`]: the closure returned `Err` (`Failed`) or panicked (`Panicked`, with the panic
/// message).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobError {
    Failed(String),
    Panicked(String),
}

type JobResult = Result<String, JobError>;

/// Sets the job's `finished` flag when dropped: at the normal end of the closure AND while a panic
/// unwinds the thread, so [`Job::done`] can never stay false after the thread ended (gate review B-1).
struct DoneGuard(Arc<AtomicBool>);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Text of a panic payload (`&str` or `String`, else a fixed placeholder). O(len).
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// A computation on its own thread (the engine side of the Python `RouteJob`): [`Job::done`] becomes
/// true when the thread ends, normally or by a panic; [`Job::wait`] joins it and returns its result,
/// a panic as [`JobError::Panicked`] with the panic message (idempotent: later calls return the same).
///
/// Complexity: `spawn` O(1) plus the thread start; `done` O(1) (one atomic load); `wait` blocks for
/// the remaining run time, then O(len(result)) for the clone.
pub struct Job {
    handle: Mutex<Option<std::thread::JoinHandle<Result<String, String>>>>,
    result: Mutex<Option<JobResult>>,
    finished: Arc<AtomicBool>,
}

impl Job {
    /// Run `f` on a new thread.
    pub fn spawn<F>(f: F) -> Job
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        let finished = Arc::new(AtomicBool::new(false));
        let guard = DoneGuard(finished.clone());
        let handle = std::thread::spawn(move || {
            let _guard = guard; // dropped on return and on unwind
            f()
        });
        Job {
            handle: Mutex::new(Some(handle)),
            result: Mutex::new(None),
            finished,
        }
    }

    /// The thread has ended (result ready, or panicked). O(1).
    pub fn done(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Block until the thread ends; its result, or the panic as [`JobError::Panicked`].
    pub fn wait(&self) -> JobResult {
        let mut res = self
            .result
            .lock()
            .map_err(|e| JobError::Failed(e.to_string()))?;
        if res.is_none() {
            let h = self
                .handle
                .lock()
                .map_err(|e| JobError::Failed(e.to_string()))?
                .take();
            let r = match h {
                Some(h) => match h.join() {
                    Ok(r) => r.map_err(JobError::Failed),
                    Err(payload) => Err(JobError::Panicked(panic_text(payload.as_ref()))),
                },
                None => Err(JobError::Failed("job already consumed".into())),
            };
            *res = Some(r);
        }
        res.clone()
            .unwrap_or_else(|| Err(JobError::Failed("no result".into())))
    }
}
