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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
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
    /// `stable_window` cycles in every run ("Routes stable"; the plugin's Cancel then reads OK),
    /// and there is a pick to keep (user 2026-11-17: *Best so far* is not empty; with the TopoR
    /// loop the loops' predicted pick). Never goes back within a run.
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
    /// last-preview (user 2026-11-19, "I would like the last round result to be displayed"): the
    /// TopoR loop cycle that finished last (any run): run (1-based; 0 = none yet), cycle (0 = the
    /// initial plan), when it finished (seconds since the job started) and its own state's
    /// *predicted* criteria (the plan's measure, nothing drawn; not the archive's pick).
    #[serde(default)]
    pub loop_run: u32,
    #[serde(default)]
    pub loop_cycle: u32,
    #[serde(default)]
    pub loop_at_s: f64,
    #[serde(default)]
    pub loop_errors: usize,
    #[serde(default)]
    pub loop_open: usize,
    #[serde(default)]
    pub loop_violations: usize,
    #[serde(default)]
    pub loop_vias: usize,
    #[serde(default)]
    pub loop_length_mm: f64,
    #[serde(default)]
    pub loop_shrinkages: usize,
    /// last-preview (user 2026-11-19, the "Last run" row): the run whose own pick changed last
    /// (of the N seeded runs; a run's pick = the best of its own variants, predicted in the TopoR
    /// loop, as judged once drawn): run (1-based; 0 = none yet), the cycle its pick came from, when
    /// it changed (seconds since the job started), `"plan"` (predicted) or `"copper"` (drawn) and
    /// the pick's criteria.
    #[serde(default)]
    pub runbest_run: u32,
    #[serde(default)]
    pub runbest_cycle: u32,
    #[serde(default)]
    pub runbest_at_s: f64,
    #[serde(default)]
    pub runbest_kind: String,
    #[serde(default)]
    pub runbest_errors: usize,
    #[serde(default)]
    pub runbest_open: usize,
    #[serde(default)]
    pub runbest_violations: usize,
    #[serde(default)]
    pub runbest_vias: usize,
    #[serde(default)]
    pub runbest_length_mm: f64,
    #[serde(default)]
    pub runbest_shrinkages: usize,
    /// When the round judged last (`last_*`) ended, seconds since the job started (0 = none yet).
    #[serde(default)]
    pub last_at_s: f64,
    /// last-preview: how many sketches ([`Sketch`]) were published so far (0 = none; it grows by one
    /// per publish, so a change says the caller may fetch a new one).
    #[serde(default)]
    pub sketch_seq: u64,
}

/// last-preview: a picture of one result for the progress dialog's board image (user 2026-11-19):
/// `kind = "plan"`: the state of a TopoR loop cycle (per wire its layer stretches and planned vias,
/// [`crate::topo::pourplan::plan_items`]: the plan, not drawn copper); `kind = "copper"`: the items
/// of a drawn round. Items in the KiCad frame, as the result's `items`. Built only when the caller
/// enabled sketches ([`Sketches::enable`]); it never feeds back into the run.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Sketch {
    /// Run (1-based) and round / loop cycle (0 = the initial plan) it shows.
    pub run: u32,
    pub cycle: u32,
    /// `"plan"` or `"copper"`.
    pub kind: String,
    /// Its publish number ([`Progress::sketch_seq`] at the publish) and when (seconds since the job
    /// started).
    pub seq: u64,
    pub at_s: f64,
    /// Its criteria (predicted for a plan, as judged for copper).
    pub errors: usize,
    pub open: usize,
    pub violations: usize,
    pub vias: usize,
    pub length_mm: f64,
    pub items: Vec<crate::model::Item>,
}

/// last-preview: the sketches a run publishes for the caller ([`RunControl::sketches`]): `enabled`
/// (set once by the caller, the plugin's progress dialog; off = nothing is ever built: headless,
/// harness, MCP), `want` (the caller asks for the next last-round sketch; cleared when one is
/// published, so at most one is built per request), the last one and the best one per run. One
/// mutex for both slots: a reader gets a whole sketch (an `Arc`), never a torn one. O(1) per call.
#[derive(Debug, Default)]
pub struct Sketches {
    enabled: AtomicBool,
    want: AtomicBool,
    slot: Mutex<SketchSlot>,
}

#[derive(Debug, Default)]
struct SketchSlot {
    seq: u64,
    last: Option<Arc<Sketch>>,
    best: Vec<Option<Arc<Sketch>>>,
}

impl Sketches {
    /// Build sketches from now on (the caller shows them). O(1).
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Relaxed);
    }
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    /// Ask for a last-round sketch at the next cycle's end (only while enabled). O(1).
    pub fn request(&self) {
        if self.enabled() {
            self.want.store(true, Ordering::Relaxed);
        }
    }
    /// A last-round sketch is asked for (enabled and requested). One relaxed load when off. O(1).
    pub fn wanted(&self) -> bool {
        self.enabled() && self.want.load(Ordering::Relaxed)
    }
    /// The last sketch (`best = false`) or the best one of run `best_run` (1-based). O(1).
    pub fn get(&self, best: bool, best_run: u32) -> Option<Arc<Sketch>> {
        let s = self.slot.lock().ok()?;
        if best {
            s.best.get((best_run as usize).checked_sub(1)?).cloned().flatten()
        } else {
            s.last.clone()
        }
    }
}

/// Which slot a published sketch goes to ([`RunControl::publish_sketch`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SketchTo {
    pub last: bool,
    pub best: bool,
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
    /// The steps a stop test saw the limit cut, acting as hard after it passed ([`TimeLimit::cuts`],
    /// [`TimeLimit::note_cut_in`]): one bit per [`CutStep`] (the report then names what was cut,
    /// not only that the limit acted as hard).
    cut: AtomicU8,
    /// `[run] limit_grace_s`: nanoseconds past the limit that the TopoR loop step may still run
    /// (`LIMIT_NONE` = no grace bound here; `0` is a real value: stop at the next check).
    grace_ns: AtomicU64,
    /// A stop test of the TopoR loop saw the grace over ([`TimeLimit::grace_over`]).
    grace_hit: AtomicBool,
    /// The longest time (ns) the step that ran when the limit passed went on past it
    /// ([`TimeLimit::note_step_end`], at the first soft-step change after the limit).
    wait_ns: AtomicU64,
    /// One bit per run (run 0 and runs >= 63 share bit 63): its step that ran at the limit has ended.
    wait_runs: AtomicU64,
    /// A TopoR loop cycle was dropped (cut by the grace or the cap, not offered).
    cycle_dropped: AtomicBool,
    /// The soft step each run is in ([`SoftSlot`], index = run, 0 = the only run or the call's
    /// shared plane stubs / projection): its first change after the limit ends the wait.
    soft: Mutex<Vec<SoftSlot>>,
    /// `[optimize] loop = "cycles"`: the soft step is the whole-route cycle ([`soft_step_of`]).
    whole_cycle: AtomicBool,
    /// A stop test saw the limit, acting as hard, cut a step before the first run's soft step at
    /// the limit ended ([`TimeLimit::note_cut_in`]): that step was cut, it did not run to its end.
    cut_in_wait: AtomicBool,
    /// `[run] drawing_grace_s` (ns; `LIMIT_NONE` = no bound): how long the drawing may run past the
    /// later of the limit and its own start before its optional passes stop.
    draw_grace_ns: AtomicU64,
    /// An optional pass of the drawing stopped because `drawing_grace_s` ran out.
    draw_grace_hit: AtomicBool,
    /// Wires whose ERBS the drawing bound cut (drawn the sequential way instead).
    draw_grace_erbs: AtomicU64,
    /// Nanoseconds since `start` at which [`TimeLimit::harden`] was first called (the plugin's CPU-clock
    /// cap); `LIMIT_NONE` = never. The instant the wall-clock cap rules use (with the round's 1 s floor).
    harden_ns: AtomicU64,
}

/// What the soft time limit lets finish (operate.md, `timeout_mode`): with `[optimize] loop =
/// "topor"` the initial routing (plane stubs, projection, topological search, layer assignment and
/// rip-up), the TopoR loop and the drawing (traces, via reduction, fuse, pour stitching); with
/// `loop = "cycles"` the whole-route cycle. The wait TIME_LIMIT_WAIT reports is that of the soft
/// step the limit fell in (lead 2026-11-17), not of one progress stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoftStep {
    Initial,
    Loop,
    Drawing,
}

impl SoftStep {
    /// Its words in the report line.
    pub fn words(self) -> &'static str {
        match self {
            SoftStep::Initial => "the initial routing",
            SoftStep::Loop => "the TopoR loop",
            SoftStep::Drawing => "the drawing",
        }
    }
}

/// The progress stages of the topological core and the soft step each belongs to (prefixes of
/// the stage texts; a test checks every `ctl.stage` literal of core.rs is here).
pub const SOFT_STAGES: [(&str, SoftStep); 9] = [
    ("plane stubs", SoftStep::Initial),
    ("projection", SoftStep::Initial),
    ("topological search", SoftStep::Initial),
    ("layer assignment", SoftStep::Initial),
    ("TopoR loop", SoftStep::Loop),
    ("picking the variant to draw", SoftStep::Drawing),
    ("drawing the traces", SoftStep::Drawing),
    ("via reduction", SoftStep::Drawing),
    ("pour stitching", SoftStep::Drawing),
];

/// The soft step of a stage text (`None`: not a stage of the topological core). O(1).
pub fn soft_step_of(stage: &str) -> Option<SoftStep> {
    SOFT_STAGES.iter().find(|(p, _)| stage.starts_with(p)).map(|&(_, s)| s)
}

/// A run's soft step: none yet, open (round, key, when it began: ns since the limit's start), or
/// ended (idle, between steps).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SoftSlot {
    #[default]
    Never,
    Open(u32, u64, u64),
    Closed,
}

/// The key of a stage's soft step within its round: the class, or with `whole_cycle` one key for
/// the whole cycle; a stage outside [`SOFT_STAGES`] is a step of its own (its text). O(len).
fn soft_key(stage: &str, whole_cycle: bool) -> u64 {
    match soft_step_of(stage) {
        Some(_) if whole_cycle => 1,
        Some(s) => 2 + s as u64,
        None => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            stage.hash(&mut h);
            h.finish() | (1 << 63)
        }
    }
}

/// M2 fix 3 (verify-m2c 4): a step the time limit, acting as hard (`[run] timeout_mode = "hard"`,
/// or the soft limit past its overrun cap), can cut, as the report names it. The planning steps
/// (initial routing, its rip-up rounds, the TopoR loop cycle, P3/P4 moves) are [`CutStep::Plan`];
/// the drawing's own steps each have theirs. The octilinear polish and the fuse have no stop
/// test: the limit never cuts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutStep {
    /// A planning step: the kept plan is drawn in full after it.
    Plan,
    /// The plane stubs, placed before the topology (a planning step: the plan needs them).
    PlaneStubs,
    /// The drawing of the traces (vias, wires, the feedback redraw, never-drop).
    Drawing,
    /// The via reduction after the drawing.
    ViaReduction,
    /// The pour stitching (D4) of a drawn variant.
    Stitching,
}

impl CutStep {
    pub const ALL: [CutStep; 5] = [CutStep::Plan, CutStep::PlaneStubs, CutStep::Drawing, CutStep::ViaReduction, CutStep::Stitching];

    fn bit(self) -> u8 {
        1 << (self as u8)
    }

    /// Its name in `LimitReport::cut_steps` / `result.json` `time_limit.cut_steps`.
    pub fn name(self) -> &'static str {
        match self {
            CutStep::Plan => "plan",
            CutStep::PlaneStubs => "plane_stubs",
            CutStep::Drawing => "drawing",
            CutStep::ViaReduction => "via_reduction",
            CutStep::Stitching => "stitching",
        }
    }

    /// Its words in the report lines.
    pub fn words(self) -> &'static str {
        match self {
            CutStep::Plan => "a planning step",
            CutStep::PlaneStubs => "the plane stubs",
            CutStep::Drawing => "the drawing of the traces",
            CutStep::ViaReduction => "the via reduction",
            CutStep::Stitching => "the pour stitching",
        }
    }

    /// A step of the drawing (cut: the board is drawn only in part, or not finished).
    pub fn in_drawing(self) -> bool {
        matches!(self, CutStep::Drawing | CutStep::ViaReduction | CutStep::Stitching)
    }
}

const LIMIT_UNSET: u64 = u64::MAX;
const LIMIT_NONE: u64 = u64::MAX - 1;

impl Default for TimeLimit {
    fn default() -> TimeLimit {
        TimeLimit { start: Instant::now(), ns: AtomicU64::new(LIMIT_UNSET), changed: AtomicBool::new(false), hard: AtomicBool::new(false), cap_ns: AtomicU64::new(LIMIT_NONE), capped: AtomicBool::new(false), cut: AtomicU8::new(0), grace_ns: AtomicU64::new(LIMIT_NONE), grace_hit: AtomicBool::new(false), wait_ns: AtomicU64::new(0), wait_runs: AtomicU64::new(0), cycle_dropped: AtomicBool::new(false), soft: Mutex::new(Vec::new()), whole_cycle: AtomicBool::new(false), cut_in_wait: AtomicBool::new(false), draw_grace_ns: AtomicU64::new(LIMIT_NONE), draw_grace_hit: AtomicBool::new(false), draw_grace_erbs: AtomicU64::new(0), harden_ns: AtomicU64::new(LIMIT_NONE) }
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

    /// The overrun cap as an instant (limit + cap); `None` = no limit or no cap. O(1).
    pub fn cap_deadline(&self) -> Option<Instant> {
        Some(self.deadline()? + Duration::from_secs_f64(self.overrun_cap_s()?))
    }

    /// `[run] limit_grace_s`: seconds past the limit the TopoR loop step may still run (soft
    /// limit); `< 0` / non-finite = no bound, `0` = stop at the next check. O(1).
    pub fn set_grace(&self, seconds: f64) {
        let ns = if seconds.is_finite() && seconds >= 0.0 { ((seconds * 1e9) as u64).min(LIMIT_NONE - 1) } else { LIMIT_NONE };
        self.grace_ns.store(ns, Ordering::Relaxed);
    }

    /// The grace in seconds; `None` = unbounded. O(1).
    pub fn grace_s(&self) -> Option<f64> {
        match self.grace_ns.load(Ordering::Relaxed) {
            LIMIT_UNSET | LIMIT_NONE => None,
            ns => Some(ns as f64 * 1e-9),
        }
    }

    /// The limit passed and so did its grace: the stop test of the TopoR loop step ends the step
    /// now (a cycle it cuts is not offered). Noted for the report when true ([`TimeLimit::grace_hit`]).
    /// Only read inside the loop ([`in_loop_scope`]); the initial routing and the drawing never ask. O(1).
    pub fn grace_over(&self) -> bool {
        let over = match (self.seconds(), self.grace_s()) {
            (Some(l), Some(g)) => self.elapsed_s() >= l + g,
            _ => false,
        };
        if over {
            self.grace_hit.store(true, Ordering::Relaxed);
        }
        over
    }

    /// The soft step that ran when the limit passed has ended (the run went on to its next soft
    /// step or fell idle): its wait is the time since the limit. Noted once per run (`run` as in
    /// [`RunControl::for_run`], 0 = a single run); a run's later steps began after the limit and
    /// never count. O(1).
    fn note_step_end(&self, run: u32) {
        let bit = 1u64 << run.min(63);
        if self.passed() && self.wait_runs.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
            self.note_wait();
        }
    }

    /// `[optimize] loop = "cycles"`: the soft step is the whole-route cycle. O(1).
    pub fn set_whole_cycle(&self, on: bool) {
        self.whole_cycle.store(on, Ordering::Relaxed);
    }

    /// Run `run` enters the stage `stage` of round `round` (`""` = it computes nothing now): a
    /// change of its soft step ends the one before (noted once per run, [`TimeLimit::note_step_end`]).
    /// A run's first stage of a best-of-N call continues the call's shared step (plane stubs,
    /// projection: part of every run's initial routing). O(runs).
    /// Only a step that began before the limit counts: a run idle at the limit (waiting at the
    /// draw gate) whose drawing begins later has no wait of its own.
    fn soft_enter(&self, run: u32, round: u32, stage: &str) {
        let now = self.start.elapsed().as_nanos().min(u128::from(LIMIT_NONE - 1)) as u64;
        let key = (!stage.is_empty()).then(|| (round, soft_key(stage, self.whole_cycle.load(Ordering::Relaxed))));
        let Ok(mut v) = self.soft.lock() else { return };
        let i = run as usize;
        if v.len() <= i {
            v.resize(i + 1, SoftSlot::Never);
        }
        let mut cur = v[i];
        if cur == SoftSlot::Never && i > 0 {
            cur = v[0];
        }
        let next = match (key, cur) {
            (None, _) => SoftSlot::Closed,
            (Some((r, k)), SoftSlot::Open(r0, k0, t0)) if (r, k) == (r0, k0) => SoftSlot::Open(r, k, t0),
            (Some((r, k)), _) => SoftSlot::Open(r, k, now),
        };
        if let SoftSlot::Open(_, _, t0) = cur {
            let began_before = self.ns.load(Ordering::Relaxed) > t0; // the limit (ns since start) is after its start
            if cur != next && began_before {
                self.note_step_end(run);
            }
        }
        v[i] = next;
    }

    fn note_wait(&self) {
        if let Some(d) = self.deadline() {
            let ns = Instant::now().saturating_duration_since(d).as_nanos().min(u128::from(u64::MAX / 2)) as u64;
            self.wait_ns.fetch_max(ns, Ordering::Relaxed);
        }
    }

    /// Some run's step ended after the limit passed ([`TimeLimit::note_step_end`]). O(1).
    pub fn step_end_noted(&self) -> bool {
        self.wait_runs.load(Ordering::Relaxed) != 0
    }

    /// Seconds the step that ran when the limit passed went on past it (0 when it ended before). O(1).
    pub fn wait_s(&self) -> f64 {
        self.wait_ns.load(Ordering::Relaxed) as f64 * 1e-9
    }

    /// A TopoR loop cycle was cut by the limit (grace or cap) and not offered. O(1).
    pub fn note_cycle_dropped(&self) {
        self.cycle_dropped.store(true, Ordering::Relaxed);
    }

    /// A TopoR loop cycle was dropped ([`TimeLimit::note_cycle_dropped`]). O(1).
    pub fn cycle_dropped(&self) -> bool {
        self.cycle_dropped.load(Ordering::Relaxed)
    }

    /// A stop test saw the grace over. O(1).
    pub fn grace_hit(&self) -> bool {
        self.grace_hit.load(Ordering::Relaxed)
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

    /// The limit stops the step that runs now: it passed and acts as hard (`[run] timeout_mode =
    /// "hard"`, or the overrun cap fired, [`TimeLimit::is_hard`]). When true it is noted
    /// ([`TimeLimit::cut_any`]) as a cut of a planning step: the stop test that asked stops its
    /// step there. O(1).
    pub fn cuts(&self) -> bool {
        self.cuts_in(CutStep::Plan)
    }

    /// [`TimeLimit::cuts`] for the stop test of `step`. O(1).
    pub fn cuts_in(&self, step: CutStep) -> bool {
        self.is_hard() && self.passed() && self.note_cut_in(step)
    }

    /// A planning step's stop test with its own condition (the round's 1 s floor) found the limit
    /// cutting its step: noted for the report ([`TimeLimit::cut_any`]). Always true. O(1).
    pub fn note_cut(&self) -> bool {
        self.note_cut_in(CutStep::Plan)
    }

    /// The stop test of `step` found the limit cutting it: noted for the report. Always true. O(1).
    pub fn note_cut_in(&self, step: CutStep) -> bool {
        self.cut.fetch_or(step.bit(), Ordering::Relaxed);
        if self.wait_runs.load(Ordering::Relaxed) == 0 {
            self.cut_in_wait.store(true, Ordering::Relaxed); // the step the limit fell in is still open: it is the one cut
        }
        true
    }

    /// The limit, acting as hard, cut the soft step it fell in (not only a later one). O(1).
    pub fn cut_in_wait(&self) -> bool {
        self.cut_in_wait.load(Ordering::Relaxed)
    }

    /// `[run] drawing_grace_s`: seconds the drawing may run past the later of the limit and its own
    /// start before its optional passes (via reduction, ERBS) stop; `< 0` / non-finite = no bound,
    /// `0` = they stop at their first check past the limit. O(1).
    pub fn set_draw_grace(&self, seconds: f64) {
        let ns = if seconds.is_finite() && seconds >= 0.0 { ((seconds * 1e9) as u64).min(LIMIT_NONE - 1) } else { LIMIT_NONE };
        self.draw_grace_ns.store(ns, Ordering::Relaxed);
    }

    /// The drawing bound in seconds; `None` = unbounded. O(1).
    pub fn draw_grace_s(&self) -> Option<f64> {
        match self.draw_grace_ns.load(Ordering::Relaxed) {
            LIMIT_UNSET | LIMIT_NONE => None,
            ns => Some(ns as f64 * 1e-9),
        }
    }

    /// The instant the optional passes of a drawing that began at `draw_start` stop (soft limit):
    /// the later of the limit and the drawing's start, plus `drawing_grace_s`; `None` = no limit or
    /// no bound. A snapshot: a later change of the limit does not move it. O(1).
    pub fn draw_bound(&self, draw_start: Instant) -> Option<Instant> {
        let d = self.deadline()?;
        let g = self.draw_grace_s()?;
        Some(d.max(draw_start) + Duration::from_secs_f64(g))
    }

    /// The stop test of an optional pass of the drawing: the limit passed and the drawing bound
    /// ([`TimeLimit::draw_bound`]) with it; noted for the report when true. O(1).
    pub fn draw_grace_over(&self, draw_start: Instant) -> bool {
        let over = self.passed() && self.draw_bound(draw_start).is_some_and(|b| Instant::now() >= b);
        if over {
            self.draw_grace_hit.store(true, Ordering::Relaxed);
        }
        over
    }

    /// An optional pass of the drawing stopped at the drawing bound. O(1).
    pub fn draw_grace_hit(&self) -> bool {
        self.draw_grace_hit.load(Ordering::Relaxed)
    }

    /// `n` wires had their ERBS cut by the drawing bound (drawn the sequential way). O(1).
    pub fn note_draw_grace_erbs(&self, n: usize) {
        if n > 0 {
            self.draw_grace_hit.store(true, Ordering::Relaxed);
            self.draw_grace_erbs.fetch_add(n as u64, Ordering::Relaxed);
        }
    }

    /// Wires whose ERBS the drawing bound cut. O(1).
    pub fn draw_grace_erbs(&self) -> usize {
        self.draw_grace_erbs.load(Ordering::Relaxed) as usize
    }

    /// Whether the limit, acting as hard, cut a step of the run ([`TimeLimit::cuts`]). A limit
    /// (or overrun cap) that passed only after the steps it stops had ended cut nothing. O(1).
    pub fn cut_any(&self) -> bool {
        self.cut.load(Ordering::Relaxed) != 0
    }

    /// The steps the limit cut, in [`CutStep::ALL`] order. O(1).
    pub fn cut_steps(&self) -> Vec<CutStep> {
        let bits = self.cut.load(Ordering::Relaxed);
        CutStep::ALL.into_iter().filter(|s| bits & s.bit() != 0).collect()
    }

    /// `[run] timeout_mode = "hard"` was set ([`TimeLimit::set_hard`]), apart from the overrun cap. O(1).
    pub fn hard_mode(&self) -> bool {
        self.hard.load(Ordering::Relaxed)
    }

    /// The caller's overrun cap fired (the plugin's CPU clock: limit plus cap reached in CPU
    /// seconds): the limit acts as the hard one from now on ([`TimeLimit::is_hard`]). O(1).
    pub fn harden(&self) {
        self.capped.store(true, Ordering::Relaxed);
        let ns = (self.start.elapsed().as_nanos() as u64).min(LIMIT_NONE - 1);
        let _ = self.harden_ns.compare_exchange(LIMIT_NONE, ns, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// [`TimeLimit::harden`] was called (the plugin's CPU-clock cap). O(1).
    pub fn hardened(&self) -> bool {
        self.harden_ns.load(Ordering::Relaxed) != LIMIT_NONE
    }

    /// The instant the overrun cap took effect: the earlier of the wall-clock cap
    /// ([`TimeLimit::cap_deadline`]) and the moment of [`TimeLimit::harden`] (the CPU clock's cap).
    /// Both clocks are then alike: the round's 1 s floor applies to this instant (lead 2026-11-17,
    /// verifier r6 D1). `None` = neither. O(1).
    pub fn cap_instant(&self) -> Option<Instant> {
        let h = match self.harden_ns.load(Ordering::Relaxed) {
            LIMIT_NONE => None,
            ns => Some(self.start + Duration::from_nanos(ns)),
        };
        match (h, self.cap_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
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

/// [`RunControl::stop_kind`]: the first stop request was a Cancel.
pub const STOP_CANCEL: u8 = 1;
/// [`RunControl::stop_kind`]: the first stop request was OK at "Routes stable".
pub const STOP_OK: u8 = 2;

/// Cancel flag, time limit and progress of one run; cheap to clone (shared state).
#[derive(Clone, Debug, Default)]
pub struct RunControl {
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<Mutex<Progress>>,
    /// The live wall-clock limit ([`TimeLimit`]), shared by every run of a best-of-N call.
    pub limit: Arc<TimeLimit>,
    /// What the first stop request was ([`RunControl::request_stop`]): 0 none yet, [`STOP_CANCEL`]
    /// or [`STOP_OK`] (OK at "Routes stable"). Latched once, at the request, never changed after.
    pub stop_kind: Arc<AtomicU8>,
    /// M-B0: 0 = the only run; k >= 1 = run k of a best-of-N call (its stage text is prefixed
    /// with the run, and the round is the coordinator's).
    pub run: u32,
    /// Test-only injection point (see [`TestHook`]); `None` in every real run.
    pub test_hook: TestHook,
    /// last-preview: the sketches for the caller's board image, shared by every run of the call.
    pub sketches: Arc<Sketches>,
}

/// A test-only hook: the engine asks it at a few named points (`RunControl::test_point`); a test
/// installs a closure that acts there (reach the limit, cancel) or answers `true` to inject a fault.
/// Not part of any option or config; unset (the default) it costs one `Option` check.
#[derive(Clone, Default)]
pub struct TestHook(pub Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>);

impl std::fmt::Debug for TestHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "TestHook(set)" } else { "TestHook(none)" })
    }
}

impl RunControl {
    /// Install the test hook (tests only).
    pub fn set_test_hook(&mut self, f: impl Fn(&str) -> bool + Send + Sync + 'static) {
        self.test_hook = TestHook(Some(Arc::new(f)));
    }

    /// Ask the test hook (if any) at the named point; `false` without one.
    pub fn test_point(&self, point: &str) -> bool {
        self.test_hook.0.as_ref().is_some_and(|h| h(point))
    }

    pub fn new() -> RunControl {
        RunControl::default()
    }

    /// Ask the run to stop at its next deadline check: Cancel, or OK when the progress shows
    /// "Routes stable" at this moment ([`RunControl::request_stop`] with `None`). O(1).
    pub fn request_cancel(&self) {
        self.request_stop(None);
    }

    /// Ask the run to stop (user 2026-11-17): `ok` = the button read OK when it was pressed
    /// ("Routes stable" shown: keep the result, labelled stable, the pour stitching in full);
    /// `None` = the progress's own [`Progress::stable`] right now. Only the first request counts:
    /// what it was is latched here, before the flag is set, so every later reader
    /// ([`RunControl::ok_at_stable`]) sees the state at the press, not at the time it looks (a run
    /// that turns stable or ends after a Cancel does not make it an OK). A caller that sets
    /// [`RunControl::cancel`] directly gets a Cancel. O(1).
    pub fn request_stop(&self, ok: Option<bool>) {
        let ok = ok.unwrap_or_else(|| self.progress.lock().is_ok_and(|p| p.stable));
        let _ = self.stop_kind.compare_exchange(0, if ok { STOP_OK } else { STOP_CANCEL }, Ordering::SeqCst, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// The run was stopped by OK at "Routes stable" (the first stop request, latched by
    /// [`RunControl::request_stop`]); `false` without a stop request or for a Cancel. O(1).
    pub fn ok_at_stable(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) && self.stop_kind.load(Ordering::SeqCst) == STOP_OK
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
            p.notice = limit_notice(&open, self.limit.is_hard(), !self.limit.hard_mode() && self.limit.cap_hit());
        }
        p
    }

    /// Name the current stage and round (progress dialog wording); leaves the counters alone. O(1).
    pub(crate) fn stage(&self, round: u32, rounds: u32, stage: &str) {
        if let Ok(mut p) = self.progress.lock() {
            // M2.TL: the stage that ran when the limit passed is the step the soft limit waited for.
            Self::note_limit_step(&self.limit, &mut p);
            // the soft step that ran at the limit ends at the first change of soft step (lead 2026-11-17)
            self.limit.soft_enter(self.run, round, stage);
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
            self.limit.soft_enter(self.run, 0, ""); // computes nothing: its soft step has ended
            if let Some(s) = p.run_steps.get_mut(self.run as usize - 1) {
                s.clear();
            }
        }
    }

    /// The same cancel flag and progress, as run `run` (1-based) of a best-of-N call. O(1).
    pub(crate) fn for_run(&self, run: u32) -> RunControl {
        RunControl { cancel: self.cancel.clone(), progress: self.progress.clone(), limit: self.limit.clone(), stop_kind: self.stop_kind.clone(), run, test_hook: self.test_hook.clone(), sketches: self.sketches.clone() }
    }

    /// Change the progress under the lock (the M-B0 coordinator's fields). O(1) + `f`.
    pub(crate) fn with_progress(&self, f: impl FnOnce(&mut Progress)) {
        if let Ok(mut p) = self.progress.lock() {
            f(&mut p);
        }
    }

    /// Seconds since the job started (the time limit's clock). O(1).
    pub fn since_start_s(&self) -> f64 {
        self.limit.elapsed_s()
    }

    /// last-preview: publish `sk` (its `seq` and `at_s` are set here) to the slots `to` (the best
    /// slot of its own run). Clears the request when it goes to the last slot. Under the slot's
    /// lock, then the progress's `sketch_seq` (never both locks at once). O(1) + the move.
    pub(crate) fn publish_sketch(&self, mut sk: Sketch, to: SketchTo) {
        if !self.sketches.enabled() || !(to.last || to.best) {
            return;
        }
        sk.at_s = self.since_start_s();
        let seq = {
            let Ok(mut s) = self.sketches.slot.lock() else { return };
            s.seq += 1;
            sk.seq = s.seq;
            let k = (sk.run as usize).saturating_sub(1);
            let sk = Arc::new(sk);
            if to.best {
                if s.best.len() <= k {
                    s.best.resize(k + 1, None);
                }
                s.best[k] = Some(sk.clone());
            }
            if to.last {
                s.last = Some(sk);
                self.sketches.want.store(false, Ordering::Relaxed);
            }
            s.seq
        };
        self.with_progress(|p| p.sketch_seq = p.sketch_seq.max(seq));
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
    STOP.with(|c| c.borrow().as_ref().is_some_and(|(f, l, always)| f.load(Ordering::Relaxed) || ((*always || l.is_hard()) && l.passed() && l.note_cut()) || (LOOP_SCOPE.with(|s| s.get()) && l.grace_over())))
}

thread_local! {
    /// Inside the TopoR loop step ([`in_loop_scope`]): the `[run] limit_grace_s` bound stops its moves.
    static LOOP_SCOPE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` as the TopoR loop step: [`stop_requested`] then also answers true once the time limit
/// and its grace (`[run] limit_grace_s`) have passed, so the move-level stop tests end the step.
/// The flag before comes back after `f`, also when it panics. O(1) + `f`.
pub fn in_loop_scope<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            LOOP_SCOPE.with(|s| s.set(self.0));
        }
    }
    let _restore = Restore(LOOP_SCOPE.with(|s| s.replace(true)));
    f()
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

    /// last-preview: a request is honoured only while enabled and is cleared by the publish to the
    /// last slot (at most one build per request); a best-only publish keeps it; best slots are per
    /// run; every publish raises `sketch_seq` by one.
    #[test]
    fn sketch_request_publish_and_slots() {
        let ctl = RunControl::new();
        ctl.sketches.request();
        assert!(!ctl.sketches.wanted(), "a request before enable is ignored");
        ctl.publish_sketch(Sketch { run: 1, ..Default::default() }, SketchTo { last: true, best: true });
        assert!(ctl.sketches.get(false, 0).is_none() && ctl.snapshot().sketch_seq == 0, "published while disabled");
        ctl.sketches.enable();
        ctl.sketches.request();
        assert!(ctl.sketches.wanted());
        let r2 = ctl.for_run(2);
        r2.publish_sketch(Sketch { run: 2, cycle: 7, ..Default::default() }, SketchTo { last: false, best: true });
        assert!(ctl.sketches.wanted(), "a best-only publish answers no request");
        assert_eq!(ctl.snapshot().sketch_seq, 1);
        r2.publish_sketch(Sketch { run: 2, cycle: 8, kind: "plan".into(), ..Default::default() }, SketchTo { last: true, best: false });
        assert!(!ctl.sketches.wanted(), "the last-slot publish clears the request");
        let last = ctl.sketches.get(false, 0).expect("last");
        assert_eq!((last.run, last.cycle, last.seq, last.kind.as_str()), (2, 8, 2, "plan"));
        assert_eq!(ctl.sketches.get(true, 2).expect("run 2's best").cycle, 7);
        assert!(ctl.sketches.get(true, 1).is_none() && ctl.sketches.get(true, 0).is_none() && ctl.sketches.get(true, 9).is_none());
        ctl.publish_sketch(Sketch { run: 1, cycle: 3, ..Default::default() }, SketchTo { last: false, best: false });
        assert_eq!(ctl.snapshot().sketch_seq, 2, "a publish to no slot is no publish");
        assert!(ctl.sketches.get(false, 0).expect("last").at_s >= 0.0);
    }

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
    fn a_hardened_hard_limit_keeps_the_hard_notice() {
        // verify-m2d O3: in hard mode a caller's harden() (the plugin's CPU clock) is no "overrun cap":
        // the cap is a soft-mode feature, so the notice stays the hard mode's own line.
        let ctl = RunControl::new();
        ctl.limit.init(0.0);
        ctl.limit.set_hard(true);
        ctl.stage(1, 1, "drawing the traces");
        ctl.limit.reach();
        ctl.limit.harden();
        assert!(ctl.limit.cap_hit(), "harden() fired the cap flag");
        assert_eq!(ctl.snapshot().notice, "time limit reached (hard): the current step stops now (drawing the traces)");
    }

    #[test]
    fn the_cap_deadline_is_the_limit_plus_the_cap() {
        let ctl = RunControl::new();
        assert!(ctl.limit.cap_deadline().is_none(), "no limit");
        ctl.limit.init(100.0);
        assert!(ctl.limit.cap_deadline().is_none(), "no cap");
        ctl.limit.set_overrun_cap(20.0);
        let (d, c) = (ctl.limit.deadline().unwrap(), ctl.limit.cap_deadline().unwrap());
        assert_eq!(c.duration_since(d), std::time::Duration::from_secs(20));
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
    #[test]
    fn the_grace_ends_the_loop_step_only_after_limit_plus_grace_and_only_in_its_scope() {
        // limit_grace_s: read only inside the TopoR loop; 0 = the next check after the limit; none = never.
        let ctl = RunControl::new();
        ctl.limit.init(0.2);
        ctl.limit.set_grace(0.3);
        assert!(!ctl.limit.grace_over(), "before the limit");
        std::thread::sleep(Duration::from_millis(250));
        assert!(ctl.limit.passed() && !ctl.limit.grace_over(), "inside the grace");
        assert!(!in_loop_scope(|| with_control_mode(&ctl, false, stop_requested)), "inside the grace the loop runs on");
        std::thread::sleep(Duration::from_millis(300));
        assert!(ctl.limit.grace_over() && ctl.limit.grace_hit());
        assert!(in_loop_scope(|| with_control_mode(&ctl, false, stop_requested)), "grace over: the loop's moves stop");
        assert!(!with_control_mode(&ctl, false, stop_requested), "outside the loop (initial routing, drawing) nothing stops");
        assert!(!stop_requested(), "outside any control nothing is installed");
        // the scope is restored, also on a panic
        let _ = std::panic::catch_unwind(|| in_loop_scope(|| panic!("boom")));
        assert!(!with_control_mode(&ctl, false, stop_requested));
    }

    #[test]
    fn grace_zero_stops_at_the_limit_and_no_grace_never() {
        let ctl = RunControl::new();
        ctl.limit.init(0.05);
        ctl.limit.set_grace(0.0);
        assert!(!ctl.limit.grace_over());
        std::thread::sleep(Duration::from_millis(80));
        assert!(ctl.limit.grace_over(), "0 = stop at the next check");
        let none = RunControl::new();
        none.limit.init(0.05);
        none.limit.set_grace(-1.0);
        std::thread::sleep(Duration::from_millis(80));
        assert!(none.limit.passed() && !none.limit.grace_over() && none.limit.grace_s().is_none(), "negative = unbounded");
        let free = RunControl::new();
        free.limit.init(0.0);
        free.limit.set_grace(0.0);
        assert!(!free.limit.grace_over(), "no limit: the grace never fires");
        assert!(!in_loop_scope(|| with_control_mode(&free, false, stop_requested)));
    }

    #[test]
    fn every_stage_of_the_core_has_a_soft_step() {
        // every `ctl.stage(..)` text literal of the topological core maps to a soft step
        let src = concat!(include_str!("topo/core.rs"), include_str!("topo/optimise.rs"));
        let mut n = 0;
        for (i, _) in src.match_indices(".stage(") {
            let line = &src[i..src[i..].find('\n').map_or(src.len(), |e| i + e)];
            let Some(q) = line.find('"') else { continue };
            let lit = &line[q + 1..];
            let lit = &lit[..lit.find('"').unwrap_or(lit.len())];
            if line.starts_with(".stage(\"") || lit.starts_with("after ") {
                continue; // the cut report's own stage(name, ..), not the progress
            }
            assert!(soft_step_of(lit).is_some(), "stage {lit:?} has no soft step (SOFT_STAGES)");
            n += 1;
        }
        assert!(n >= 8, "found {n} stage literals");
        assert_eq!(soft_step_of("projection of the board"), Some(SoftStep::Initial));
        assert_eq!(soft_step_of("TopoR loop on the topology"), Some(SoftStep::Loop));
        assert_eq!(soft_step_of("via reduction and fuse"), Some(SoftStep::Drawing));
        assert_eq!(soft_step_of("routing"), None);
    }

    #[test]
    fn the_wait_counts_the_whole_soft_step_not_one_stage() {
        let ctl = RunControl::new();
        ctl.limit.init(0.1);
        ctl.stage(1, 1, "plane stubs (pads of plane nets onto their pour)");
        ctl.stage(1, 1, "projection of the board");
        std::thread::sleep(Duration::from_millis(200)); // the limit falls in the projection
        ctl.stage(1, 1, "topological search");
        ctl.stage(1, 1, "layer assignment, rip-up and reroute");
        assert!(!ctl.limit.step_end_noted(), "the initial routing goes on: no wait noted yet");
        std::thread::sleep(Duration::from_millis(150));
        ctl.stage(1, 1, "TopoR loop on the topology"); // the initial routing ends here
        let w = ctl.limit.wait_s();
        assert!(ctl.limit.step_end_noted() && (0.24..5.0).contains(&w), "{w}");
        assert_eq!(ctl.snapshot().limit_step, "projection of the board");
        std::thread::sleep(Duration::from_millis(100));
        ctl.stage(1, 1, "drawing the traces"); // later steps began after the limit: no wait of theirs
        assert_eq!(ctl.limit.wait_s(), w);
    }

    #[test]
    fn a_limit_before_any_step_change_notes_nothing_and_a_change_before_the_limit_neither() {
        let ctl = RunControl::new();
        ctl.limit.init(0.15);
        ctl.stage(1, 1, "projection of the board");
        ctl.stage(1, 1, "TopoR loop on the topology"); // before the limit
        assert!(!ctl.limit.step_end_noted() && ctl.limit.wait_s() == 0.0);
    }

    #[test]
    fn the_loop_step_ends_where_the_loop_does() {
        let ctl = RunControl::new();
        ctl.limit.init(0.05);
        ctl.stage(1, 1, "TopoR loop on the topology");
        std::thread::sleep(Duration::from_millis(150));
        ctl.stage(1, 1, "picking the variant to draw"); // the loop returned
        let w = ctl.limit.wait_s();
        assert!((0.09..5.0).contains(&w), "{w}");
        std::thread::sleep(Duration::from_millis(150));
        ctl.stage(1, 1, "drawing the traces");
        assert_eq!(ctl.limit.wait_s(), w, "the pick and the drawing are one step");
    }

    #[test]
    fn with_whole_route_cycles_the_cycle_is_the_step() {
        let ctl = RunControl::new();
        ctl.limit.set_whole_cycle(true);
        ctl.limit.init(0.05);
        ctl.stage(1, 2, "projection of the board");
        std::thread::sleep(Duration::from_millis(100));
        ctl.stage(1, 2, "drawing the traces");
        ctl.stage(1, 2, "via reduction and fuse");
        ctl.stage(1, 2, "pour stitching");
        assert!(!ctl.limit.step_end_noted(), "the cycle runs to its end");
        std::thread::sleep(Duration::from_millis(100));
        ctl.stage(2, 2, "projection of the board"); // the next cycle
        assert!(ctl.limit.step_end_noted() && ctl.limit.wait_s() > 0.14, "{}", ctl.limit.wait_s());
    }

    #[test]
    fn a_run_of_many_continues_the_shared_projection_and_ends_its_step_when_idle() {
        let ctl = RunControl::new();
        ctl.with_progress(|p| p.runs = 2);
        ctl.limit.init(0.05);
        ctl.stage(1, 1, "projection of the board"); // shared by the runs
        std::thread::sleep(Duration::from_millis(100));
        let (r1, r2) = (ctl.for_run(1), ctl.for_run(2));
        r1.stage(1, 1, "topological search"); // part of run 1's initial routing
        r2.stage(1, 1, "topological search");
        assert!(!ctl.limit.step_end_noted());
        std::thread::sleep(Duration::from_millis(100));
        r1.idle(); // run 1 waits at the draw gate
        let w = ctl.limit.wait_s();
        assert!(ctl.limit.step_end_noted() && w > 0.14, "{w}");
        std::thread::sleep(Duration::from_millis(100));
        r2.stage(1, 1, "TopoR loop on the topology");
        assert!(ctl.limit.wait_s() > w + 0.08, "the longest run counts");
    }

    #[test]
    fn a_run_idle_at_the_limit_whose_drawing_begins_later_has_no_wait() {
        // runs 2: run 1 waits at the draw gate when the limit passes, run 2 is in its loop; run 1 then
        // draws (began after the limit): only run 2's loop step is a wait
        let ctl = RunControl::new();
        ctl.with_progress(|p| p.runs = 2);
        ctl.limit.init(0.05);
        let (r1, r2) = (ctl.for_run(1), ctl.for_run(2));
        r1.stage(1, 1, "TopoR loop on the topology");
        r2.stage(1, 1, "TopoR loop on the topology");
        r1.stage(1, 1, "picking the variant to draw");
        r1.idle(); // before the limit
        std::thread::sleep(Duration::from_millis(80));
        r2.stage(1, 1, "picking the variant to draw"); // run 2's loop ends after the limit
        let w = ctl.limit.wait_s();
        assert!(ctl.limit.step_end_noted() && w > 0.02, "{w}");
        r1.stage(1, 1, "drawing the traces"); // run 1 draws: began after the limit
        std::thread::sleep(Duration::from_millis(150));
        r1.idle();
        assert_eq!(ctl.limit.wait_s(), w, "a step that began after the limit is no wait");
    }

    #[test]
    fn a_cut_while_the_step_at_the_limit_runs_is_a_cut_of_that_step() {
        let ctl = RunControl::new();
        ctl.limit.init(0.02);
        ctl.stage(1, 1, "TopoR loop on the topology");
        std::thread::sleep(Duration::from_millis(50));
        ctl.limit.note_cut_in(CutStep::Plan);
        assert!(ctl.limit.cut_in_wait());
        let late = RunControl::new();
        late.limit.init(0.02);
        late.stage(1, 1, "TopoR loop on the topology");
        std::thread::sleep(Duration::from_millis(50));
        late.stage(1, 1, "drawing the traces");
        late.limit.note_cut_in(CutStep::Drawing); // a later step's cut (the cap fired in the drawing)
        assert!(!late.limit.cut_in_wait());
    }

    #[test]
    fn the_drawing_bound_counts_from_the_later_of_the_limit_and_the_drawing_start() {
        let ctl = RunControl::new();
        ctl.limit.init(0.05);
        ctl.limit.set_draw_grace(0.1);
        let early = Instant::now(); // the drawing began before the limit
        assert!(!ctl.limit.draw_grace_over(early), "before the limit");
        std::thread::sleep(Duration::from_millis(80));
        assert!(!ctl.limit.draw_grace_over(early), "inside the bound");
        let late = Instant::now(); // a drawing that begins after the limit gets the whole bound
        std::thread::sleep(Duration::from_millis(90));
        assert!(ctl.limit.draw_grace_over(early) && ctl.limit.draw_grace_hit(), "limit + 0.1 s passed");
        assert!(!ctl.limit.draw_grace_over(late), "late start + 0.1 s not yet");
        std::thread::sleep(Duration::from_millis(40));
        assert!(ctl.limit.draw_grace_over(late));
        for neg in [-1.0, -5.0, f64::NAN] {
            let c = RunControl::new();
            c.limit.init(0.01);
            c.limit.set_draw_grace(neg);
            std::thread::sleep(Duration::from_millis(20));
            assert!(c.limit.draw_grace_s().is_none() && !c.limit.draw_grace_over(early) && !c.limit.draw_grace_hit(), "{neg}: no bound");
        }
        let free = RunControl::new();
        free.limit.init(0.0);
        free.limit.set_draw_grace(0.0);
        assert!(!free.limit.draw_grace_over(early), "no limit: no bound");
    }

    #[test]
    fn negative_graces_are_unbounded() {
        for neg in [-1.0, -5.0, -1e9] {
            let c = RunControl::new();
            c.limit.init(0.01);
            c.limit.set_grace(neg);
            std::thread::sleep(Duration::from_millis(20));
            assert!(c.limit.grace_s().is_none() && !c.limit.grace_over(), "{neg}");
        }
    }

    #[test]
    fn a_dropped_cycle_is_noted() {
        let ctl = RunControl::new();
        assert!(!ctl.limit.cycle_dropped());
        ctl.limit.note_cycle_dropped();
        assert!(ctl.limit.cycle_dropped());
    }
}
