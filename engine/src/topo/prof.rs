//! TW0 profiling counters: where the time of one run goes (design: docs/dev/profiling.md).
//!
//! Switch: the environment variable `TOPO_PROF` (any value; read once) or [`set_on`] (tests).
//! Off, every hook is one relaxed load of a static flag and nothing else (no clock read, no
//! allocation, no lock): the copper never depends on it, and the overhead stays under 3 %.
//! Arguments of a hook are evaluated before the call: a costly argument (an O(wires) count) is
//! computed under `if prof::on()` at the call site, so off costs nothing beyond the load.
//! On, a run records
//!   * inclusive time, call and unit counts per [`K`] key (guards `g(K::..)`, `units`, `lap`),
//!     summed over all threads (a key inside the refine lanes can exceed the wall time);
//!   * stage windows: [`mark`] ends the window the label names, with the wall seconds since the
//!     last mark and the key deltas of that window; consecutive windows of one stage merge;
//!   * one row per TopoR loop cycle ([`cycle`]): wall seconds, re-lays tried / kept, ...
//!
//! [`report`] (the Python binding adds it to the result as `profile`) turns this into JSON; the
//! table, shares and flame-graph stage folding are `tools/prof_run.py` / `tools/prof_table.py`.
//! The windows are a wall-clock partition of one run, so they only make sense with
//! `optimize.runs = 1` (several parallel runs interleave their marks).
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Counter keys. The names are the JSON keys (`K::name`).
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum K {
    AssignWith,
    Refine,
    CapmodelBuild,
    Measure,
    TautDirs,
    ForeignCopper,
    LayersNow,
    TopologyClone,
    StateRestore,
    Reroute,
    RrPrice,
    RrPlace,
    RrViolAfter,
    SearchTree,
    UncrossAll,
    UncrossTouched,
    Relay,
    DescendLabels,
    CrossingPass,
    ViaCounts,
    Criteria,
    Snapshot,
    SpRestore,
    Offer,
    Potential,
    TryLayers,
    PairMove,
    Relabel,
    FixLayers,
    Junctions,
    StSweep,
    StUncrossReopt,
    StViarose,
    StFloat,
    StCross,
    StPairrelay,
    StPairmove,
    Cycle,
    CapmodelOptimise,
    RemoveWire,
    Dependents,
    RrSetLr,
    PathPrice,
    AnyPath,
    TopoSetup,
    Escape,
    FloatWire,
    LoopP4,
    TableSync,
    Fingerprint,
    XrDetect,
    XrReplan,
    XrJudge,
    PatternMove,
}

const NAMES: &[&str] = &[
    "assign_with",       // layers::assign_with (whole-board layering, incl. refine)
    "refine",            // refine::refine (P3 capacity refinement inside assign)
    "capmodel_build",    // CapModel::build (any caller)
    "measure",           // the loop's measure closure (dirs + CapModel + foreign + necks)
    "taut_dirs",         // Packer::taut_dirs
    "foreign_copper",    // optimise::foreign_copper
    "layers_now",        // optimise::layers_now
    "topology_clone",    // State::topology
    "state_restore",     // State::restore
    "reroute",           // Sp::reroute (one single-wire re-lay, everything)
    "rr_price",          // Rr::price (path_price of a wire, old + new)
    "rr_place",          // Rr::place (search + uncross + relay)
    "rr_viol_after",     // Rr::violations(after) = assign_t + measure: the loop's judge
    "search_tree",       // init::State::search_tree
    "uncross_all",
    "uncross_touched",
    "relay",             // State::relay (4.3c)
    "descend_labels",
    "crossing_pass",
    "via_counts",
    "criteria",          // Sp::criteria
    "snapshot",          // Sp::snapshot
    "sp_restore",        // Sp::restore
    "offer",             // optimise offer (criteria + covers + snapshot + archive)
    "potential",         // Topo::potential (sweep order)
    "try_layers",        // Sp::try_layers (measure inside)
    "pair_move",         // Sp::pair_move
    "relabel",           // layers::relabel
    "fix_layers",        // tree::fix_layers
    "junctions",         // tree::junctions
    "st_sweep",          // cycle lap: the sweep (re-lays one by one)
    "st_uncross_reopt",  // cycle lap: uncross_all + layer re-optimisation + via-rose candidates
    "st_viarose",        // cycle lap: via-rose relay + 2nd re-optimisation
    "st_float",          // cycle lap: floating up (labels)
    "st_cross",          // cycle lap: crossing rules
    "st_pairrelay",      // cycle lap: 5.4 pair_min relay
    "st_pairmove",       // cycle lap: 5.8 pair_move
    "cycle",             // Sp::cycle total (units = alive wires)
    "capmodel_optimise", // CapModel::optimise (inside refine)
    "remove_wire",       // tree::remove_wire
    "dependents",        // tree::dependents
    "rr_set_lr",         // set_lr after a kept re-lay
    "path_price",        // Topo::path_price
    "any_path",          // State::any_path
    "topo_setup",        // cycle lap: Topo clone + potentials + sort
    "escape",            // TW5 Sp::escape: one layering escape (whole-board layering, history, via-rose front)
    "float_wire",        // TW5 Sp::float_up: floating up of one wire (inside the sweep)
    "loop_p4",           // TW5 (f) Sp::loop_p4: one in-loop P4 rip-up round (victims, placing, layering)
    "table_sync",        // TW3: a run's sync with the shared variant table (wait for the other runs + commit + view)
    "fingerprint",       // TW3: Sp::fingerprint (the state hash a restart compares)
    "xr_detect",         // TW7 A: crossing-pass detection (xrules detectors + plain filter)
    "xr_replan",         // TW7 A: crossing-pass re-plans (Topo setup + try_joint / try_reroute)
    "xr_judge",          // TW7 A: crossing-pass judge (state clone, uncross_all, assign, score)
    "pattern_move",      // TW7 B: Sp::pattern_move (one targeted move: remove, place in rule order, layer, judge)
];
const N: usize = NAMES.len();

#[allow(clippy::declare_interior_mutable_const)]
const Z: AtomicU64 = AtomicU64::new(0);
static CALLS: [AtomicU64; N] = [Z; N];
static NS: [AtomicU64; N] = [Z; N];
static UNITS: [AtomicU64; N] = [Z; N];

static ON: AtomicBool = AtomicBool::new(false);
static INIT: OnceLock<()> = OnceLock::new();

/// Is profiling on? One relaxed load (after the first call, which reads `TOPO_PROF`).
#[inline]
pub fn on() -> bool {
    if INIT.get().is_none() {
        INIT.get_or_init(|| {
            if std::env::var_os("TOPO_PROF").is_some() {
                ON.store(true, Relaxed);
            }
        });
    }
    ON.load(Relaxed)
}

/// Switch profiling on or off for this process (tests); overrides the environment.
pub fn set_on(v: bool) {
    INIT.get_or_init(|| ());
    ON.store(v, Relaxed);
}

/// Time guard: adds its lifetime to the key when dropped (inclusive time, 1 call).
pub struct G(K, Option<Instant>);
#[inline]
pub fn g(k: K) -> G {
    G(k, if on() { Some(Instant::now()) } else { None })
}
impl Drop for G {
    #[inline]
    fn drop(&mut self) {
        if let Some(t) = self.1 {
            CALLS[self.0 as usize].fetch_add(1, Relaxed);
            NS[self.0 as usize].fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
        }
    }
}
/// Add `n` units (sizes: items built, events, wires) to a key.
#[inline]
pub fn units(k: K, n: usize) {
    if on() {
        UNITS[k as usize].fetch_add(n as u64, Relaxed);
    }
}
/// Lap timer: adds the time since `*t` to the key (1 call) and restarts `*t`. The clock is read only
/// when profiling is on (off: `*t` is left alone).
#[inline]
pub fn lap(t: &mut Instant, k: K) {
    if on() {
        CALLS[k as usize].fetch_add(1, Relaxed);
        NS[k as usize].fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
        *t = Instant::now();
    }
}

/// One stage of the run in the report: merged consecutive windows.
struct Row {
    stage: &'static str,
    labels: Vec<String>,
    wall_s: f64,
    mono0: f64,
    mono1: f64,
    keys: [[u64; 3]; N],
}
struct Cyc {
    n: usize,
    wall_s: f64,
    tried: usize,
    kept: usize,
    relaid: usize,
    pair_moved: usize,
    pair_relaid: usize,
    archived: bool,
}
struct St {
    last: Option<Instant>,
    mono_last: f64,
    rows: Vec<Row>,
    cycles: Vec<Cyc>,
}
static ST: Mutex<St> = Mutex::new(St { last: None, mono_last: 0.0, rows: Vec::new(), cycles: Vec::new() });

/// Set when [`mono_now`] could not read the clock (the `Instant` Debug format changed): the report then
/// says `"mono_ok": false` and the flame-graph stage folding must not trust `mono0` / `mono1`.
static MONO_BAD: AtomicBool = AtomicBool::new(false);

/// CLOCK_MONOTONIC seconds (Instant's clock on Linux), to match `perf record -k mono` samples.
/// Parsed from the `Instant` Debug text (no libc dependency); `None` when the text has no
/// `tv_sec` / `tv_nsec` numbers (the unit test `mono_now_reads_the_instant_debug_format` fails first).
fn mono_parse(dbg: &str) -> Option<f64> {
    let num = |key: &str| dbg.split(key).nth(1).and_then(|r| r.trim_start_matches([':', ' ']).split(|c: char| !c.is_ascii_digit()).next()).and_then(|x| x.parse::<f64>().ok());
    Some(num("tv_sec")? + num("tv_nsec")? * 1e-9)
}
fn mono_now() -> f64 {
    mono_parse(&format!("{:?}", Instant::now())).unwrap_or_else(|| {
        MONO_BAD.store(true, Relaxed);
        0.0
    })
}

/// Stage of the window a mark label ends (the windows partition the engine's time).
fn stage_of(label: &str) -> &'static str {
    const T: &[(&str, &str)] = &[
        ("stubs", "stubs"),
        ("projection", "projection"),
        ("free radius", "p2_p4"),
        ("P2", "p2_p4"),
        ("P4", "p2_p4"),
        ("pre-loop", "loop"),
        ("offer0", "loop"),
        ("cycle", "loop"),
        ("loop-end", "loop"),
        ("TopoR loop", "loop"),
        ("via relocation", "relocation"),
        ("branch relocation", "relocation"),
        ("drawing", "drawing"),
        ("via reduction", "via_reduction"),
        ("diff pairs", "tail"),
        ("round tail", "tail"),
        ("finish", "tail"),
    ];
    T.iter().find(|(p, _)| label.starts_with(p)).map_or("other", |x| x.1)
}

/// Start a run's record (route_topo entry): clears counters and rows, starts the first window.
pub fn begin() {
    if !on() {
        return;
    }
    for k in 0..N {
        CALLS[k].store(0, Relaxed);
        NS[k].store(0, Relaxed);
        UNITS[k].store(0, Relaxed);
    }
    let mut s = ST.lock().unwrap();
    s.rows.clear();
    s.cycles.clear();
    s.last = Some(Instant::now());
    s.mono_last = mono_now();
}

/// End the window `label` names: wall seconds and key deltas since the last mark.
pub fn mark(label: &str) {
    if !on() {
        return;
    }
    let mut s = ST.lock().unwrap();
    let Some(last) = s.last else { return };
    let wall = last.elapsed().as_secs_f64();
    let mono1 = mono_now();
    let mut keys = [[0u64; 3]; N];
    for (k, row) in keys.iter_mut().enumerate() {
        *row = [CALLS[k].swap(0, Relaxed), NS[k].swap(0, Relaxed), UNITS[k].swap(0, Relaxed)];
    }
    let (stage, mono0) = (stage_of(label), s.mono_last);
    s.last = Some(Instant::now());
    s.mono_last = mono1;
    match s.rows.last_mut().filter(|r| r.stage == stage) {
        Some(r) => {
            r.wall_s += wall;
            r.mono1 = mono1;
            if !r.labels.iter().any(|l| l == label) && r.labels.len() < 12 {
                r.labels.push(label.to_string());
            }
            for (a, b) in r.keys.iter_mut().zip(keys.iter()) {
                for i in 0..3 {
                    a[i] += b[i];
                }
            }
        }
        None => s.rows.push(Row { stage, labels: vec![label.to_string()], wall_s: wall, mono0, mono1, keys }),
    }
}

/// End of one TopoR loop cycle: its row (wall seconds since the last mark, re-lays tried / kept, ...)
/// and the mark that closes its window.
pub fn cycle(n: usize, tried: usize, kept: usize, relaid: usize, pair_moved: usize, pair_relaid: usize, archived: bool) {
    if !on() {
        return;
    }
    {
        let mut s = ST.lock().unwrap();
        let wall_s = s.last.map_or(0.0, |t| t.elapsed().as_secs_f64());
        s.cycles.push(Cyc { n, wall_s, tried, kept, relaid, pair_moved, pair_relaid, archived });
    }
    mark("cycle");
}

fn keys_json(k: &[[u64; 3]; N]) -> Value {
    let mut m = Map::new();
    for (i, v) in k.iter().enumerate() {
        if v[0] != 0 || v[1] != 0 || v[2] != 0 {
            m.insert(NAMES[i].to_string(), json!([v[0], v[1] as f64 / 1e9, v[2]]));
        }
    }
    Value::Object(m)
}

/// The record as JSON (`None` when profiling is off or no run began): `stages` in run order with
/// wall seconds and keys `[calls, inclusive seconds, units]`, `cycles` (one row per loop cycle),
/// `engine_s` (sum of the stage windows).
pub fn report() -> Option<Value> {
    if !on() {
        return None;
    }
    mark("finish");
    let s = ST.lock().unwrap();
    if s.rows.is_empty() {
        return None;
    }
    let stages: Vec<Value> = s.rows.iter().map(|r| json!({"stage": r.stage, "labels": r.labels, "wall_s": r.wall_s, "mono0": r.mono0, "mono1": r.mono1, "keys": keys_json(&r.keys)})).collect();
    let cycles: Vec<Value> = s.cycles.iter().map(|c| json!({"n": c.n, "wall_s": c.wall_s, "tried": c.tried, "kept": c.kept, "relaid": c.relaid, "pair_moved": c.pair_moved, "pair_relaid": c.pair_relaid, "archived": c.archived})).collect();
    let engine_s: f64 = s.rows.iter().map(|r| r.wall_s).sum();
    Some(json!({"version": 1, "mono_ok": !MONO_BAD.load(Relaxed), "engine_s": engine_s, "stages": stages, "cycles": cycles}))
}

#[cfg(test)]
mod tests {
    use super::{mono_parse, stage_of};
    use std::time::Instant;

    /// The monotone clock is read from `Instant`'s Debug text: it parses today (Linux), and a bad text is None
    /// (reported as `mono_ok: false`, not a silent 0).
    #[test]
    fn mono_now_reads_the_instant_debug_format() {
        if cfg!(target_os = "linux") {
            let a = mono_parse(&format!("{:?}", Instant::now())).expect("Instant Debug format changed: fix mono_parse");
            let b = mono_parse(&format!("{:?}", Instant::now())).unwrap();
            assert!(a > 0.0 && b >= a, "{a} {b}");
        }
        assert_eq!(mono_parse("Instant { t: 5 }"), None);
        assert_eq!(mono_parse("Instant { tv_sec: 12, tv_nsec: 500000000 }"), Some(12.5));
    }

    /// Every mark label the engine uses has a named stage (an unnamed window would hide time in "other").
    #[test]
    fn every_engine_label_has_a_stage() {
        for (label, stage) in [
            ("stubs", "stubs"),
            ("projection built once", "projection"),
            ("projection (shared, built once per call; feedback rooms applied)", "projection"),
            ("free radius of the junction candidates", "p2_p4"),
            ("P2 + P3", "p2_p4"),
            ("P4 rip-up and reroute", "p2_p4"),
            ("P4 moves", "p2_p4"),
            ("pre-loop", "loop"),
            ("offer0", "loop"),
            ("cycle", "loop"),
            ("loop-end", "loop"),
            ("TopoR loop", "loop"),
            ("via relocation", "relocation"),
            ("branch relocation", "relocation"),
            ("drawing + feedback", "drawing"),
            ("via reduction", "via_reduction"),
            ("diff pairs", "tail"),
            ("round tail", "tail"),
            ("finish", "tail"),
            ("something new", "other"),
        ] {
            assert_eq!(stage_of(label), stage, "{label}");
        }
    }
}
