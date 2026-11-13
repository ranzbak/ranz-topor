//! Run control for a routing call: cancellation and progress reporting (M4b, Task P1).
//!
//! A [`RunControl`] is shared between the caller (any thread) and the routing run. `cancel` is
//! polled at every deadline check of the engine through [`expired`] / [`cancelled`]: the run then
//! takes the same anytime path as a wall-clock timeout and returns the best result so far with
//! `stop_reason == "cancel"`. `progress` is a snapshot the run overwrites per iteration and per
//! pass.
//!
//! Passes that take a plain `Instant` deadline also poll [`cancelled`], a thread-local cancel flag;
//! nothing installs that flag any more (its installer went with the classic router), so it reads
//! false and Cancel reaches the core through [`RunControl::is_cancelled`].
//!
//! The wall-clock time limit is live ([`TimeLimit`]): the caller may raise or lower it while the run
//! goes on (the plugin's progress window), and every deadline check of the core reads it through
//! [`RunControl::over_limit`] / [`RunControl::deadline`]. Passes that take a plain `Instant`
//! deadline get the limit as it stands when they start.
//!
//! Complexity: [`expired`] and [`cancelled`] are O(1) (one clock read, one thread-local access,
//! one relaxed atomic load); the [`TimeLimit`] reads and writes are O(1) and lock-free (one atomic);
//! [`RunControl::update`] is O(1) under an uncontended mutex.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// Seconds spent on measurements outside the time limit so far ([`RunControl::measured`]).
    #[serde(default)]
    pub measure_s: f64,
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
    /// M-B1 never drop: rule violations of the best result's written copper.
    #[serde(default)]
    pub best_violations: usize,
    /// Score of the best result in the pick order (`[optimize] pick`, `Objective::better`): errors
    /// (open connections plus connections with violations), open connections, shrinkages, and the
    /// round (cycle, 1-based) of its run it came from. The best is exactly what the final pick
    /// takes now: the best over the runs' picks, ties to the lower run.
    #[serde(default)]
    pub best_errors: usize,
    #[serde(default)]
    pub best_open: usize,
    #[serde(default)]
    pub best_shrinkages: usize,
    #[serde(default)]
    pub best_round: u32,
    /// The round judged last (any run): run and round (1-based; 0 = none yet) and the criteria the
    /// variant table judges it by (the better of its variants, as drawn and after the via
    /// reduction, with the pour stitching counted).
    #[serde(default)]
    pub last_run: u32,
    #[serde(default)]
    pub last_round: u32,
    #[serde(default)]
    pub last_errors: usize,
    #[serde(default)]
    pub last_open: usize,
    #[serde(default)]
    pub last_violations: usize,
    #[serde(default)]
    pub last_vias: usize,
    #[serde(default)]
    pub last_length_mm: f64,
    #[serde(default)]
    pub last_shrinkages: usize,
    #[serde(default)]
    pub last_routed: usize,
    /// The last round against the best: `"new best"` (it is the best now; `last_decider` /
    /// `last_delta` against the best before it, none for the first), `"tie"` (the same score as the
    /// best before it, and the best now: ties go to the lower run, as in the final pick), `"worse"`,
    /// `"same"` (the same score: the earlier result stays) or `"better"` (better, but the variant
    /// table did not keep it); `""` before the first round.
    #[serde(default)]
    pub last_vs_best: String,
    /// The criterion that decides (`Objective::decider`: `"errors"`, `"open"`, `"vias"`,
    /// `"length"`, `"shrinkages"`, `"score"`, `"area"`; `""` = none) and last minus best on it
    /// (count, mm or mm²).
    #[serde(default)]
    pub last_decider: String,
    #[serde(default)]
    pub last_delta: f64,
    /// Rounds that ended so far over all runs (judged or not), and the log lines of the last
    /// [`ROUND_LINES`] of them (line `i` is round number `rounds_ended - len + i + 1`): the run
    /// flow writes each one once to log.txt.
    #[serde(default)]
    pub rounds_ended: u32,
    #[serde(default)]
    pub round_lines: Vec<String>,
    /// M2.TL soft time limit: the limit has passed while the run goes on (the step that runs
    /// finishes, then the best kept variant is drawn; Cancel stops now). Live in
    /// [`RunControl::snapshot`]; false after Cancel and once the run is done.
    #[serde(default)]
    pub limit_reached: bool,
    /// The step that ran when the limit passed (its stage text; "" before the limit).
    #[serde(default)]
    pub limit_step: String,
    /// The progress dialog's line for the soft limit ("time limit reached, waiting for the
    /// current step to finish (<step>); press Cancel to stop now"), "" when it does not apply.
    #[serde(default)]
    pub notice: String,
    /// The step each run of a best-of-N call computes now (index = run - 1; "" = the run computes
    /// nothing: between its cycles, waiting at the M2 draw gate, or done). The soft limit names
    /// these steps, not only the stage text the last run set (verify-m2 F6).
    #[serde(default)]
    pub run_steps: Vec<String>,
}

/// The soft time limit's line for the progress dialog and the run log ([`Progress::notice`]). O(len).
/// `hard` (`[run] timeout_mode = "hard"`): the limit stops the step that runs, so the line says
/// that instead of waiting (verify-m2 F7a).
/// `capped`: the overrun cap (`[run] timeout_overrun_s`) has fired: the soft limit acts as the
/// hard one from then on (F3, user 2026-11-13).
pub fn limit_notice(step: &str, hard: bool, capped: bool) -> String {
    let at = if step.is_empty() { String::new() } else { format!(" ({step})") };
    match (capped, hard) {
        (true, _) => format!("time limit overrun cap reached ([run] timeout_overrun_s): the current step stops now{at}"),
        (false, false) => format!("time limit reached, waiting for the current step to finish{at}; press Cancel to stop now"),
        (false, true) => format!("time limit reached (hard): the current step stops now{at}"),
    }
}

/// Round lines [`Progress`] keeps (the run flow polls every 0.25 s; a round takes longer).
pub const ROUND_LINES: usize = 32;

impl Progress {
    /// The steps that compute now: of a best-of-N call each run's that computes ("run k: step",
    /// joined by "; "), else the stage text. O(runs).
    pub fn open_steps(&self) -> String {
        let open: Vec<String> = self.run_steps.iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(i, s)| format!("run {}: {s}", i + 1)).collect();
        if open.is_empty() { self.stage.clone() } else { open.join("; ") }
    }

    /// Note an ended round's log line (the last [`ROUND_LINES`] stay). O(ROUND_LINES).
    pub(crate) fn push_round_line(&mut self, line: String) {
        self.rounds_ended += 1;
        if self.round_lines.len() >= ROUND_LINES {
            self.round_lines.remove(0);
        }
        self.round_lines.push(line);
    }
}

/// The wall-clock time limit of a run, changeable while it runs: nanoseconds since `start` in one
/// atomic, so a check costs one clock read and one relaxed load, without a lock. `start` is when the
/// control was made (the job's start for `route_async`); the limit is seconds since then.
///
/// States: unset (the run sets it from `Options::wall_clock_s` at its start, [`TimeLimit::init`]),
/// none (no limit: `0` or a negative / non-finite value), or a limit. A limit below the time
/// already spent stops the run at its next check, like the limit firing.
#[derive(Debug)]
pub struct TimeLimit {
    start: Instant,
    ns: AtomicU64,
    /// Set by [`TimeLimit::set`] (the caller changed the limit during the run): the report says so.
    changed: AtomicBool,
    /// `[run] timeout_mode = "hard"` (the limit stops the step that runs; [`limit_notice`]).
    hard: AtomicBool,
    /// F3 `[run] timeout_overrun_s`: nanoseconds past the limit after which the soft limit acts as
    /// the hard one (`LIMIT_NONE` = no cap), and whether that happened (or the caller said so,
    /// [`TimeLimit::harden`]: the plugin's CPU clock).
    cap_ns: AtomicU64,
    capped: AtomicBool,
}

const LIMIT_UNSET: u64 = u64::MAX;
const LIMIT_NONE: u64 = u64::MAX - 1;

impl Default for TimeLimit {
    fn default() -> TimeLimit {
        TimeLimit { start: Instant::now(), ns: AtomicU64::new(LIMIT_UNSET), changed: AtomicBool::new(false), hard: AtomicBool::new(false), cap_ns: AtomicU64::new(LIMIT_NONE), capped: AtomicBool::new(false) }
    }
}

impl TimeLimit {
    /// Seconds -> stored value: none for `<= 0` / non-finite, else nanoseconds (saturating below
    /// the two sentinels). O(1).
    fn encode(seconds: f64) -> u64 {
        if !seconds.is_finite() || seconds <= 0.0 {
            return LIMIT_NONE;
        }
        let ns = seconds * 1e9;
        if ns >= (LIMIT_NONE - 1) as f64 { LIMIT_NONE - 1 } else { (ns as u64).max(1) }
    }

    /// The run's own limit (`Options::wall_clock_s`, 0 = none), only when nobody set one yet. O(1).
    pub fn init(&self, seconds: f64) {
        let _ = self.ns.compare_exchange(LIMIT_UNSET, Self::encode(seconds), Ordering::Relaxed, Ordering::Relaxed);
    }

    /// The limit is hard (`[run] timeout_mode = "hard"`) or soft (default). O(1).
    pub fn set_hard(&self, hard: bool) {
        self.hard.store(hard, Ordering::Relaxed);
    }

    /// Whether the limit acts as hard now: `[run] timeout_mode = "hard"`, or the overrun cap fired
    /// ([`TimeLimit::cap_hit`]). Read live. O(1) (a clock read with a cap set).
    pub fn is_hard(&self) -> bool {
        self.hard.load(Ordering::Relaxed) || self.cap_hit()
    }

    /// F3 `[run] timeout_overrun_s`: seconds the soft limit may be overrun before it acts as the
    /// hard one (`<= 0` / non-finite = no cap; on this limit's clock, the wall clock). O(1).
    pub fn set_overrun_cap(&self, seconds: f64) {
        self.cap_ns.store(Self::encode(seconds), Ordering::Relaxed);
    }

    /// The overrun cap in seconds; `None` = no cap. O(1).
    pub fn overrun_cap_s(&self) -> Option<f64> {
        match self.cap_ns.load(Ordering::Relaxed) {
            LIMIT_UNSET | LIMIT_NONE => None,
            ns => Some(ns as f64 * 1e-9),
        }
    }

    /// The overrun cap fired (the limit plus the cap passed, noted once), or the caller hardened
    /// the limit ([`TimeLimit::harden`]). O(1).
    pub fn cap_hit(&self) -> bool {
        if self.capped.load(Ordering::Relaxed) {
            return true;
        }
        let hit = match (self.seconds(), self.overrun_cap_s()) {
            (Some(l), Some(c)) => self.elapsed_s() >= l + c,
            _ => false,
        };
        if hit {
            self.capped.store(true, Ordering::Relaxed);
        }
        hit
    }

    /// The caller's overrun cap fired (the plugin's CPU clock: limit plus cap reached in CPU
    /// seconds): the limit acts as the hard one from now on ([`TimeLimit::is_hard`]). O(1).
    pub fn harden(&self) {
        self.capped.store(true, Ordering::Relaxed);
    }

    /// Change the limit (seconds since the start; `0` = no limit). O(1), any thread.
    pub fn set(&self, seconds: f64) {
        self.ns.store(Self::encode(seconds), Ordering::Relaxed);
        self.changed.store(true, Ordering::Relaxed);
    }

    /// Whether [`TimeLimit::set`] was called. O(1).
    pub fn changed(&self) -> bool {
        self.changed.load(Ordering::Relaxed)
    }

    /// The limit in seconds since the start; `None` = no limit (or not set yet). O(1).
    pub fn seconds(&self) -> Option<f64> {
        match self.ns.load(Ordering::Relaxed) {
            LIMIT_UNSET | LIMIT_NONE => None,
            ns => Some(ns as f64 * 1e-9),
        }
    }

    /// The limit as an instant (a snapshot: a later change does not move it). O(1).
    pub fn deadline(&self) -> Option<Instant> {
        match self.ns.load(Ordering::Relaxed) {
            LIMIT_UNSET | LIMIT_NONE => None,
            ns => Some(self.start + Duration::from_nanos(ns)),
        }
    }

    /// Seconds spent since the start. O(1).
    pub fn elapsed_s(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Seconds left before the limit (0 when it passed); `None` = no limit. O(1).
    pub fn remaining_s(&self) -> Option<f64> {
        self.seconds().map(|l| (l - self.elapsed_s()).max(0.0))
    }

    /// Moves a set limit later by `seconds` (time spent on a measurement that must not count:
    /// the M1 compass's read-only cut test); not a change by the caller ([`TimeLimit::changed`]).
    /// No limit: nothing. O(1).
    pub fn extend(&self, seconds: f64) {
        if seconds > 0.0 && seconds.is_finite() {
            let add = (seconds * 1e9) as u64;
            let _ = self.ns.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |ns| (ns < LIMIT_NONE).then(|| ns.saturating_add(add).min(LIMIT_NONE - 1)));
        }
    }

    /// The limit has passed (as it stands now). O(1).
    pub fn passed(&self) -> bool {
        self.deadline().is_some_and(|d| Instant::now() >= d)
    }

    /// The limit is reached now (M2.TL, the plugin's CPU clock: it reached its own limit and the
    /// engine's soft limit follows): the limit becomes the time spent, not a change by the caller
    /// ([`TimeLimit::changed`]); a limit already passed stays. O(1).
    pub fn reach(&self) {
        let now = ((self.elapsed_s() * 1e9) as u64).clamp(1, LIMIT_NONE - 1);
        let _ = self.ns.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |ns| (ns == LIMIT_UNSET || ns == LIMIT_NONE || ns > now).then_some(now));
    }
}

/// Cancel flag, time limit and progress of one run; cheap to clone (shared state).
#[derive(Clone, Debug, Default)]
pub struct RunControl {
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<Mutex<Progress>>,
    /// The live wall-clock limit ([`TimeLimit`]), shared by every run of a best-of-N call.
    pub limit: Arc<TimeLimit>,
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

    /// Change the run's wall-clock limit while it runs (seconds since the start, `0` = none). O(1).
    pub fn set_time_limit(&self, seconds: f64) {
        self.limit.set(seconds);
    }

    /// The run's limit as an instant, as it stands now (`None` = no limit). O(1).
    pub fn deadline(&self) -> Option<Instant> {
        self.limit.deadline()
    }

    /// The run's wall-clock limit has passed (read live at every check). O(1).
    pub fn over_limit(&self) -> bool {
        self.limit.passed()
    }

    /// `seconds` spent on a measurement outside the time limit (the M1 compass's cut test): the
    /// wall-clock limit moves later by it and `Progress::measure_s` adds it up (the plugin's CPU
    /// clock leaves it out). O(1).
    pub(crate) fn measured(&self, seconds: f64) {
        self.limit.extend(seconds);
        self.with_progress(|p| p.measure_s += seconds.max(0.0));
    }

    /// The run's time limit has passed: the step that runs finishes and no new one starts (M2.TL
    /// soft limit). Records the step that ran when it passed ([`Progress::limit_step`]). O(1).
    pub fn limit_reached(&self) -> bool {
        let passed = self.limit.passed();
        if passed {
            self.with_progress(|p| {
                if p.limit_step.is_empty() {
                    let open = p.open_steps();
                    p.limit_step = if open.is_empty() { "routing".into() } else { open };
                }
            });
        }
        passed
    }

    /// Copy of the current progress; the soft limit's fields ([`Progress::limit_reached`],
    /// [`Progress::notice`]) are read live. O(1).
    pub fn snapshot(&self) -> Progress {
        let mut p = self.progress.lock().map(|p| p.clone()).unwrap_or_default();
        p.limit_reached = self.limit.passed() && !self.is_cancelled() && p.phase != "done";
        if p.limit_reached {
            let open = p.open_steps();
            if p.limit_step.is_empty() {
                p.limit_step = open.clone();
            }
            p.notice = limit_notice(&open, self.limit.is_hard(), self.limit.cap_hit());
        }
        p
    }

    /// Name the current stage and round (progress dialog wording); leaves the counters alone. O(1).
    pub(crate) fn stage(&self, round: u32, rounds: u32, stage: &str) {
        if let Ok(mut p) = self.progress.lock() {
            // M2.TL: the stage that ran when the limit passed is the step the soft limit waited for.
            Self::note_limit_step(&self.limit, &mut p);
            if self.run > 0 && p.runs > 1 {
                p.stage = if stage.is_empty() { String::new() } else { format!("run {}: {stage}", self.run) };
                let i = self.run as usize - 1;
                if p.run_steps.len() <= i {
                    p.run_steps.resize(i + 1, String::new());
                }
                p.run_steps[i] = stage.to_string();
                return;
            }
            p.stage = stage.to_string();
            p.round = round;
            p.rounds = rounds;
        }
    }

    /// M2.TL: the steps that ran when the limit passed are the steps the soft limit waits for
    /// (recorded once, before a step changes). O(runs).
    fn note_limit_step(limit: &TimeLimit, p: &mut Progress) {
        if p.limit_step.is_empty() && limit.passed() {
            let open = p.open_steps();
            if !open.is_empty() {
                p.limit_step = open;
            }
        }
    }

    /// Run `run` of a best-of-N call computes nothing now (between its cycles, waiting at the M2
    /// draw gate, or done): the soft limit no longer names its step. O(runs).
    pub(crate) fn idle(&self) {
        if self.run == 0 {
            return;
        }
        if let Ok(mut p) = self.progress.lock() {
            Self::note_limit_step(&self.limit, &mut p);
            if let Some(s) = p.run_steps.get_mut(self.run as usize - 1) {
                s.clear();
            }
        }
    }

    /// The same cancel flag and progress, as run `run` (1-based) of a best-of-N call. O(1).
    pub(crate) fn for_run(&self, run: u32) -> RunControl {
        RunControl { cancel: self.cancel.clone(), progress: self.progress.clone(), limit: self.limit.clone(), run }
    }

    /// Change the progress under the lock (the M-B0 coordinator's fields). O(1) + `f`.
    pub(crate) fn with_progress(&self, f: impl FnOnce(&mut Progress)) {
        if let Ok(mut p) = self.progress.lock() {
            f(&mut p);
        }
    }

    /// Update the progress; counters only ever grow (high-water marks). O(1).
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

/// Whether the run of the current thread was cancelled. O(1).
pub fn cancelled() -> bool {
    CANCEL.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    })
}

/// Runs `f` with `flag` as the current thread's cancel flag (what [`cancelled`] reads); the flag
/// before comes back after `f`, also when it panics. cancel-fix: the projection of the board
/// (`[capacity] model = "cuts"`: minutes on a large board) has no `RunControl` to ask. O(1) + `f`.
pub fn with_cancel<R>(flag: Arc<AtomicBool>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            CANCEL.with(|c| *c.borrow_mut() = prev);
        }
    }
    let _restore = Restore(CANCEL.with(|c| c.replace(Some(flag))));
    f()
}

thread_local! {
    /// M0 stop latency: the cancel flag and live time limit of the run on this thread
    /// ([`with_control`]), read only by [`stop_requested`] (not by [`cancelled`], whose readers,
    /// the drawing and the pour stitching, run on through a Cancel to draw the loop's pick).
    /// With the soft limit the time limit stops nothing until the overrun cap fires (M2.TL, F3).
    static STOP: RefCell<Option<StopState>> = const { RefCell::new(None) };
}

/// What [`stop_requested`] reads: the cancel flag, the time limit and whether it always stops the
/// steps (`hard`; else only once it acts as hard, [`TimeLimit::is_hard`]: the overrun cap, F3).
type StopState = (Arc<AtomicBool>, Arc<TimeLimit>, bool);

/// Runs `f` with `ctl`'s cancel flag and time limit as what [`stop_requested`] reads on this
/// thread; the state before comes back after `f`, also when it panics. O(1) + `f`.
pub fn with_control<R>(ctl: &RunControl, f: impl FnOnce() -> R) -> R {
    with_control_mode(ctl, true, f)
}

/// [`with_control`]; `limit_stops` = false (M2.TL soft limit, `[run] timeout_mode = "soft"`):
/// only Cancel / OK stops the work loops, the time limit lets the step that runs finish. O(1) + `f`.
pub fn with_control_mode<R>(ctl: &RunControl, limit_stops: bool, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<StopState>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            STOP.with(|c| *c.borrow_mut() = prev);
        }
    }
    let _restore = Restore(STOP.with(|c| c.replace(Some((ctl.cancel.clone(), ctl.limit.clone(), limit_stops)))));
    f()
}

/// The run of this thread ([`with_control`]) was cancelled or its time limit passed (the latter
/// only where the limit stops steps: `[run] timeout_mode = "hard"` or the overrun cap fired, M2.TL / F3): the stop test of the
/// work loops that have no `RunControl` (the topology and label moves of P3/P4 and their cut
/// prices, M0). Cheap: one atomic load, and a clock read only with a limit set. O(1).
pub fn stop_requested() -> bool {
    STOP.with(|c| c.borrow().as_ref().is_some_and(|(f, l, always)| f.load(Ordering::Relaxed) || ((*always || l.is_hard()) && l.passed())))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soft_limit_lets_the_moves_run_on_and_cancel_still_stops_them() {
        // M2.TL: with_control_mode(.., false) = the soft limit: a passed limit is no stop request
        // for the work loops (the step that runs finishes); Cancel is; hard (with_control) both.
        let ctl = RunControl::new();
        ctl.limit.init(600.0);
        ctl.limit.reach();
        assert!(with_control(&ctl, stop_requested), "hard: the passed limit stops the moves");
        assert!(!with_control_mode(&ctl, false, stop_requested), "soft: the step runs on");
        ctl.request_cancel();
        assert!(with_control_mode(&ctl, false, stop_requested), "soft: Cancel stops now");
        assert!(!stop_requested(), "outside the scope nothing is installed");
    }

    #[test]
    fn with_cancel_installs_the_flag_for_its_scope_only() {
        assert!(!cancelled());
        let flag = Arc::new(AtomicBool::new(false));
        with_cancel(flag.clone(), || {
            assert!(!cancelled());
            flag.store(true, Ordering::Relaxed);
            assert!(cancelled());
            // nested: the inner flag wins, the outer one comes back after
            with_cancel(Arc::new(AtomicBool::new(false)), || assert!(!cancelled()));
            assert!(cancelled());
        });
        assert!(!cancelled());
        let caught = std::panic::catch_unwind(|| with_cancel(flag.clone(), || panic!("boom")));
        assert!(caught.is_err());
        assert!(!cancelled(), "a panic restores the flag before");
    }

    #[test]
    fn the_run_sets_its_own_limit_only_when_nobody_did() {
        let t = TimeLimit::default();
        assert_eq!(t.seconds(), None);
        t.init(600.0);
        assert!((t.seconds().unwrap() - 600.0).abs() < 1e-6);
        t.init(5.0); // a second init (the core's own) never overrides
        assert!((t.seconds().unwrap() - 600.0).abs() < 1e-6);
        assert!(!t.changed());
        let u = TimeLimit::default();
        u.set(42.0); // the caller was first: the run's init keeps it
        u.init(600.0);
        assert!((u.seconds().unwrap() - 42.0).abs() < 1e-6);
        assert!(u.changed());
    }

    #[test]
    fn raise_and_lower_move_the_deadline() {
        let ctl = RunControl::new();
        ctl.limit.init(100.0);
        let d0 = ctl.deadline().expect("a limit");
        ctl.set_time_limit(400.0);
        let d1 = ctl.deadline().expect("a limit");
        assert!((d1 - d0).as_secs_f64() > 299.9);
        assert!(!ctl.over_limit());
        ctl.set_time_limit(50.0);
        assert!(ctl.deadline().unwrap() < d0);
        assert!(!ctl.over_limit());
        assert!(ctl.limit.remaining_s().unwrap() <= 50.0);
    }

    #[test]
    fn zero_or_negative_means_no_limit() {
        let ctl = RunControl::new();
        ctl.limit.init(0.0);
        assert_eq!(ctl.limit.seconds(), None);
        assert_eq!(ctl.deadline(), None);
        ctl.set_time_limit(10.0);
        assert!(ctl.limit.seconds().is_some());
        ctl.set_time_limit(0.0);
        assert_eq!(ctl.limit.seconds(), None);
        assert!(!ctl.over_limit());
        ctl.set_time_limit(-3.0);
        assert_eq!(ctl.limit.remaining_s(), None);
        ctl.set_time_limit(f64::NAN);
        assert_eq!(ctl.deadline(), None);
    }

    #[test]
    fn lowering_below_the_time_spent_stops_at_the_next_check() {
        let ctl = RunControl::new();
        ctl.limit.init(600.0);
        std::thread::sleep(Duration::from_millis(5));
        assert!(!ctl.over_limit());
        ctl.set_time_limit(0.001); // below the 5 ms already spent
        assert!(ctl.over_limit());
        assert_eq!(ctl.limit.remaining_s(), Some(0.0));
        assert!(!ctl.is_cancelled(), "the limit is not a Cancel: the stop reason stays the time limit");
    }

    #[test]
    fn every_run_of_a_best_of_n_call_sees_the_same_limit() {
        let ctl = RunControl::new();
        ctl.limit.init(600.0);
        let r2 = ctl.for_run(2);
        let other = std::thread::spawn(move || {
            r2.set_time_limit(1e-6);
        });
        other.join().expect("thread");
        assert!(ctl.over_limit());
    }

    #[test]
    fn reach_ends_the_limit_now_without_counting_as_a_change() {
        let ctl = RunControl::new();
        ctl.limit.init(600.0);
        ctl.stage(1, 1, "layer assignment, rip-up and reroute");
        assert!(!ctl.snapshot().limit_reached);
        ctl.limit.reach();
        assert!(ctl.over_limit());
        assert!(!ctl.limit.changed(), "the plugin's CPU clock reached its limit: no change by the user");
        let p = ctl.snapshot();
        assert!(p.limit_reached);
        assert_eq!(p.notice, "time limit reached, waiting for the current step to finish (layer assignment, rip-up and reroute); press Cancel to stop now");
        ctl.stage(1, 1, "drawing the traces");
        assert_eq!(ctl.snapshot().limit_step, "layer assignment, rip-up and reroute", "the step that ran when the limit passed");
        ctl.request_cancel();
        assert!(!ctl.snapshot().limit_reached, "after Cancel the dialog says Cancel, not the limit");
        let none = RunControl::new();
        none.limit.init(0.0);
        none.limit.reach();
        assert!(none.over_limit(), "no limit: reach() sets one at the time spent");
    }

    #[test]
    fn the_soft_limit_names_the_steps_of_the_runs_still_computing() {
        // verify-m2 F6: not only the stage text the last run set
        let ctl = RunControl::new();
        ctl.limit.init(600.0);
        ctl.with_progress(|p| p.runs = 3);
        let (r1, r2, r3) = (ctl.for_run(1), ctl.for_run(2), ctl.for_run(3));
        r2.stage(1, 1, "layer assignment, rip-up and reroute");
        r3.stage(1, 1, "topological search");
        r1.stage(1, 1, "TopoR loop on the topology");
        r1.idle(); // run 1 waits at the draw gate
        ctl.limit.reach();
        let p = ctl.snapshot();
        assert_eq!(p.limit_step, "run 2: layer assignment, rip-up and reroute; run 3: topological search");
        assert_eq!(p.notice, "time limit reached, waiting for the current step to finish (run 2: layer assignment, rip-up and reroute; run 3: topological search); press Cancel to stop now");
        r3.idle();
        assert_eq!(ctl.snapshot().limit_step, "run 2: layer assignment, rip-up and reroute; run 3: topological search", "recorded once");
        assert!(ctl.snapshot().notice.contains("(run 2: layer assignment, rip-up and reroute)"), "the notice follows the runs still computing");
    }

    #[test]
    fn the_hard_limit_has_its_own_notice() {
        // verify-m2 F7a: the hard limit stops the step, it does not wait for it
        let ctl = RunControl::new();
        ctl.limit.init(600.0);
        ctl.limit.set_hard(true);
        ctl.stage(1, 1, "drawing the traces");
        ctl.limit.reach();
        assert_eq!(ctl.snapshot().notice, "time limit reached (hard): the current step stops now (drawing the traces)");
    }

    #[test]
    fn the_overrun_cap_makes_the_soft_limit_hard() {
        // F3 `[run] timeout_overrun_s`: before the limit plus the cap the soft limit stops no
        // step; after it (or when the caller hardens it, the plugin's CPU clock) it acts as hard
        let ctl = RunControl::new();
        ctl.limit.init(1e-6);
        ctl.limit.set_overrun_cap(3600.0);
        std::thread::sleep(Duration::from_millis(2));
        assert!(ctl.over_limit() && !ctl.limit.is_hard() && !ctl.limit.cap_hit());
        assert!(!with_control_mode(&ctl, false, stop_requested), "soft: the limit stops no step");
        ctl.limit.set_overrun_cap(1e-6);
        assert!(ctl.limit.cap_hit() && ctl.limit.is_hard(), "limit + cap passed");
        assert!(with_control_mode(&ctl, false, stop_requested), "past the cap the soft limit stops the steps");
        ctl.limit.set_overrun_cap(0.0);
        assert!(ctl.limit.cap_hit(), "noted once: it stays hard");
        assert!(ctl.snapshot().notice.starts_with("time limit overrun cap reached ([run] timeout_overrun_s): the current step stops now"), "{}", ctl.snapshot().notice);
        let cpu = RunControl::new();
        cpu.limit.init(0.0);
        cpu.limit.reach();
        assert!(!cpu.limit.is_hard() && cpu.limit.overrun_cap_s().is_none());
        cpu.limit.harden();
        assert!(cpu.limit.is_hard() && cpu.limit.cap_hit(), "the caller's clock hardened it");
        assert!(with_control_mode(&cpu, false, stop_requested));
    }

    #[test]
    fn huge_limits_saturate() {
        let t = TimeLimit::default();
        t.set(1e30);
        assert!(t.seconds().unwrap() > 1e9);
        assert!(!t.passed());
    }
}
