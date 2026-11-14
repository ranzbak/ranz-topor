//! M2 = shopping list 6.1 "draw once" and M2.TL "soft time limit" (user 2026-11-13).
//!
//! * Only the variant the TopoR loop picks is drawn: every run judges its plans on the topology,
//!   the best pick over all runs by its predicted criteria is drawn, the other runs draw nothing
//!   (`[drawing] candidates = 1`; TopoR 6.3 manual pp. 72-73: variants judged and saved before
//!   any copper; Polubasov 2010: the exact geometry is computed once, on the metric stage).
//! * On the TopoR-loop path the drawing never changes the plan (`drawing_feedback` has no effect).
//! * Soft limit: a limit reached during the initial routing (P2-P4) still draws its plan; Cancel
//!   before any plan was kept is a loud "nothing to draw yet" (fail fast), never a quiet empty run.

use std::time::{Duration, Instant};
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

/// The real-board crops of the P3/P4 tests, public boards only: qmtech-minimig's USB corner (4
/// layers) and esp-rs/esp-rust-board's (2 layers, CERN-OHL-P-2.0), the latter once branch
/// public-boards brings its fixture (it drops in by name; until then only qmtech runs).
fn p3_crops() -> Vec<&'static str> {
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    ["qmtech-usb-crop.json", "esp-rust-board-usb-crop.json"].into_iter().filter(|f| f.starts_with("qmtech") || std::path::Path::new(&format!("{dir}/{f}")).exists()).collect()
}

fn crop(name: &str) -> BoardModel {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses")
}

/// The TopoR method's settings (M0 defaults: loop topor, initial once, taut lengths), headless;
/// `extra` (`, "key": value ...`) adds keys or overrides these.
/// The wall limit is a day, not the product's 600 s: these tests reach, harden and cancel the limit
/// themselves where it is under test, and a debug `cargo test` (tests/test_scaffold.py) runs several
/// times slower than release, so a 600 s limit fired on its own there (verify-m2d). The product
/// default (`[run] timeout_s` 600) is untouched.
fn topor(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut"}"#).expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
}

/// Seconds a test waits for a stage before it calls the run hung. A debug build (tests/test_scaffold.py runs
/// `cargo test` unoptimised, several times slower, under load) needs far longer to reach the drawing than
/// release (verify-m2d: "never reached drawing the traces" at 900 s); this is a hang detector, not a bound.
fn watchdog_s() -> u64 {
    if cfg!(debug_assertions) { 14_400 } else { 900 }
}

fn run(m: &BoardModel, o: &Options) -> RouteResult {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    route_with(m, o, &ctl)
}

fn best_of_line(r: &RouteResult) -> &str {
    r.warnings.iter().find(|w| w.starts_with("best of ")).map(|s| s.as_str()).unwrap_or_else(|| panic!("best-of line: {:?}", r.warnings))
}

/// The plan handed to the drawing, as its machine-readable line (C0), without its timings (keys
/// ending in `_s`, starting with `ms_` / `us_`, and `layer_share_pct`, a share of the loop's
/// time: they differ between any two runs) and its work meters (`cut_split`, `move_cost`:
/// process-wide counters that other tests running at the same time add to).
fn plan_line(r: &RouteResult) -> serde_json::Value {
    fn untimed(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.retain(|k, _| !(k.ends_with("_s") || k.starts_with("ms_") || k.starts_with("us_") || k == "layer_share_pct" || k == "cut_split" || k == "move_cost"));
                m.values_mut().for_each(untimed);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(untimed),
            _ => {}
        }
    }
    let line = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).unwrap_or_else(|| panic!("plan metrics line: {:?}", r.warnings));
    let mut v: serde_json::Value = serde_json::from_str(line.trim_start_matches("plan metrics:").trim()).expect("plan metrics JSON");
    untimed(&mut v);
    v
}

#[test]
fn only_the_picked_variant_is_drawn() {
    let m = eprom();
    let one = run(&m, &topor(r#", "runs": 3"#));
    let line = best_of_line(&one);
    // two of the three runs drew nothing: their notes say whose pick was predicted better
    let not_drawn = line.matches("pick not drawn (another run's is: [drawing] candidates = 1").count();
    assert_eq!(not_drawn, 2, "{line}");
    let picked: usize = line.split("run ").nth(1).and_then(|t| t.split_whitespace().next()).and_then(|k| k.parse().ok()).expect("picked run");
    assert!(line.contains(&format!("the pick of run {picked} is predicted better")), "the drawn pick is the predicted best: {line}");
    assert_eq!(one.routed, one.connections_total, "the pick is drawn completely: {:?}", one.warnings);
    // the option keeps the path before M2: every run draws its own pick
    let all = run(&m, &topor(r#", "runs": 3, "draw_candidates": 3"#));
    assert!(!best_of_line(&all).contains("pick not drawn"), "{}", best_of_line(&all));
    assert_eq!(all.variants.is_empty(), one.variants.is_empty());
}

#[test]
fn regression_guard_drawing_feedback_leaves_the_topor_plan_alone() {
    // A regression guard, not a proof (verify-m2 D): with one run and the TopoR loop the plan is
    // handed to the drawing once, so a feedback path that changed the plan could not show here on
    // the base either. It guards the M2 claim that the drawing never reaches the plan: drawing
    // feedback asked for leaves the plan line, the one drawing and the planning-only plan alone.
    let m = eprom();
    let off = run(&m, &topor(r#", "runs": 1, "drawing_feedback": false"#));
    let on = run(&m, &topor(r#", "runs": 1, "drawing_feedback": true"#));
    assert_eq!(plan_line(&off), plan_line(&on));
    // the plan that was drawn is the loop's pick: one plan line, one drawing, nothing after it
    for r in [&off, &on] {
        assert_eq!(r.warnings.iter().filter(|w| w.starts_with("plan metrics:")).count(), 1);
        assert!(r.warnings.iter().any(|w| w.starts_with("TopoR loop ([optimize]")), "{:?}", r.warnings);
        assert_eq!(r.routed, r.connections_total, "{:?}", r.warnings);
    }
    // and the plan of a planning-only run (no copper) is the plan the full run drew
    let plan = run(&m, &topor(r#", "runs": 1, "drawing_feedback": true, "stop_after": "plan""#));
    assert!(plan.items.is_empty());
    assert_eq!(plan_line(&plan), plan_line(&on), "the drawing does not change the plan it was handed");
}

#[test]
fn a_limit_reached_before_the_run_draws_the_initial_plan() {
    // M2.TL: the limit has passed when the run begins: the initial routing (the step that runs)
    // finishes, no loop cycle begins, its plan (generation 0) is drawn.
    let m = eprom();
    let o = topor(r#", "runs": 2"#);
    let ctl = RunControl::new();
    ctl.limit.init(1e-6);
    let r = route_with(&m, &o, &ctl);
    assert_eq!(r.routed, r.connections_total, "the initial plan is drawn completely: {:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.starts_with("TopoR loop ([optimize]") && w.contains(": 0 cycles") && w.contains("ended by time limit")), "{:?}", r.warnings);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.mode == "soft" && l.overrun_s > 0.0, "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit (soft, [run] timeout_mode)")), "{:?}", r.warnings);
    assert_eq!(r.stop_reason, "timeout");
    // The drawing's own steps are not cut either (verify-m2 F2): their counts are those of the
    // same plan drawn without a limit (the plane stubs, placed first: tests/topo_44.rs).
    let ctl = RunControl::new();
    ctl.limit.init(1e-6);
    let r = route_with(&m, &topor(r#", "runs": 1"#), &ctl);
    let free = initial_plan_free(&m, r#", "runs": 1"#);
    assert_eq!(plan_core(&r), plan_core(&free), "the plan drawn is the initial plan");
    assert!(via_reduction(&free).1.is_empty(), "{:?}", via_reduction(&free));
    assert_eq!(via_reduction(&r), via_reduction(&free), "the soft limit does not cut the via reduction");
}

/// The via reduction's counts (wires with vias, relabels tried, sweeps, accepted, vias removed),
/// without its time; and how it ended (a stop by the time budget or Cancel / OK, or none).
fn via_reduction(r: &RouteResult) -> (String, String) {
    let w = r.warnings.iter().find(|w| w.starts_with("via reduction after drawing:")).unwrap_or_else(|| panic!("via reduction line: {:?}", r.warnings));
    let counts = w.split(" (").next().expect("counts").to_string();
    let stop = ["stopped by the time budget", "stopped by Cancel / OK"].into_iter().find(|s| w.contains(s)).unwrap_or("").to_string();
    (counts, stop)
}

/// The start of the planes line (stub vias, plane pads): "planes as terminals ([planes]
/// terminals)" on this branch; main renamed it "shared plane vias ([planes] share_vias)". MERGE:
/// both are matched, so the merge cannot turn [`plane_stubs`] into a silent `None == None`.
const PLANES_LINE: [&str; 2] = ["planes as terminals ([planes] terminals)", "shared plane vias ([planes] share_vias)"];

/// The plane stubs placed before the topology (stub vias, plane pads), from the planes line; a
/// board without a planes line must have no plane stub via (the via reduction's line counts them,
/// always written): else the line was renamed and this fails loudly (verify-m2c 2).
fn plane_stubs(r: &RouteResult) -> String {
    let red = r.warnings.iter().find(|w| w.starts_with("via reduction after drawing:")).unwrap_or_else(|| panic!("via reduction line: {:?}", r.warnings));
    let stub_vias: usize = red.split(" plane stub vias").next().and_then(|t| t.rsplit(' ').next()).and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("plane stub via count: {red}"));
    match r.warnings.iter().find(|w| PLANES_LINE.iter().any(|p| w.starts_with(p))) {
        Some(w) => w.rsplit("; ").next().expect("stub count").to_string(),
        None => {
            assert_eq!(stub_vias, 0, "{stub_vias} plane stub vias but no planes line starting with one of {PLANES_LINE:?} (renamed?): {:?}", r.warnings);
            "no plane stub vias".to_string()
        }
    }
}

/// [`plan_line`] without what differs between the TopoR loop's pick of the initial plan and the
/// same plan drawn by one whole-route cycle: the loop's own block and the cut sheets' cache hits.
fn plan_core(r: &RouteResult) -> serde_json::Value {
    let mut v = plan_line(r);
    if let Some(m) = v.as_object_mut() {
        m.remove("loop");
        m.remove("cut_sheets");
    }
    v
}

/// The initial plan drawn without a time limit: one whole-route cycle, no TopoR loop (the
/// reference of the limit tests: the drawing of the same plan, with nothing cut).
fn initial_plan_free(m: &BoardModel, extra: &str) -> RouteResult {
    run(m, &topor(&format!(r#"{extra}, "opt_loop": "cycles", "topo_rounds": 1"#)))
}

#[test]
fn a_limit_reached_in_p3_p4_still_draws_the_initial_plan() {
    // The limit is reached while the layer assignment and the rip-up rounds run (as the plugin's
    // CPU clock does it): they finish, and their plan is drawn, completely: the drawing's own
    // steps too (verify-m2 F2: the via reduction got 0 s past the limit, and the plane stubs
    // stopped at it): their counts are those of the same plan drawn without a limit.
    for fx in p3_crops() {
        let m = crop(fx);
        let o = topor(r#", "runs": 1, "capacity_model": "cuts""#);
        // The reference: the initial plan drawn with no limit.
        let free = initial_plan_free(&m, r#", "runs": 1, "capacity_model": "cuts""#);
        assert!(via_reduction(&free).1.is_empty(), "{fx}: {:?}", via_reduction(&free));
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        let c2 = ctl.clone();
        let m2 = m.clone();
        let o2 = o.clone();
        let h = std::thread::spawn(move || route_with(&m2, &o2, &c2));
        let t0 = Instant::now();
        while !ctl.snapshot().stage.contains("layer assignment") && !h.is_finished() {
            assert!(t0.elapsed().as_secs() < watchdog_s(), "{fx}: never reached P3");
            std::thread::sleep(Duration::from_millis(2));
        }
        ctl.limit.reach();
        assert!(ctl.snapshot().notice.starts_with("time limit reached, waiting for the current step to finish (layer assignment"), "{:?}", ctl.snapshot().notice);
        let r = h.join().expect("run thread");
        let open: Vec<_> = r.connections.iter().filter(|c| c.status != "routed" && c.reason.contains("time limit")).collect();
        assert!(open.is_empty(), "{fx}: nothing is cut by the soft limit: {open:?}");
        assert!(r.routed > 0 && !r.items.is_empty(), "{fx}: the initial plan is drawn");
        let l = r.limit.as_ref().expect("limit report");
        assert!(l.reached && l.step.contains("layer assignment") && l.cut == "none", "{fx}: {l:?}");
        assert!(r.warnings.iter().any(|w| w.contains(": 0 cycles") && w.contains("ended by time limit")), "{fx}: no loop cycle after the limit: {:?}", r.warnings);
        assert_eq!(plan_core(&r), plan_core(&free), "{fx}: the plan drawn is the initial plan");
        assert_eq!(via_reduction(&r), via_reduction(&free), "{fx}: the soft limit does not cut the via reduction");
        assert_eq!(plane_stubs(&r), plane_stubs(&free), "{fx}");
    }
}

#[test]
fn cancel_before_any_plan_is_a_loud_error() {
    // the TopoR loop, whole-route cycles, and the cut model (Cancel during its projection)
    for extra in [r#", "runs": 2"#, r#", "runs": 2, "opt_loop": "cycles""#, r#", "runs": 2, "capacity_model": "cuts""#] {
        let m = eprom();
        let o = topor(extra);
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        ctl.request_cancel();
        let r = route_with(&m, &o, &ctl);
        assert!(r.warnings.first().is_some_and(|w| w.starts_with("ERROR: nothing to draw yet")), "{extra}: {:?}", r.warnings);
        assert_eq!(r.routed, 0);
        assert!(r.items.is_empty());
        assert_eq!(r.stop_reason, "cancel");
        assert!(r.connections.iter().all(|c| c.reason.starts_with("nothing to draw yet")), "{extra}: every connection says why");
    }
}

#[test]
fn cancel_in_p3_p4_keeps_no_plan_and_says_so() {
    // Cancel while the layer assignment and the rip-up rounds run: the initial routing (P2-P4) is
    // one step and its plan counts as a variant only once P4 ends (user 2026-11-13, M2.TL); M0's
    // stop checks end P3/P4 within a move, so nothing is kept: the loud "nothing to draw yet",
    // no copper, quickly, never half a plan drawn.
    for fx in p3_crops() {
        let m = crop(fx);
        let o = topor(r#", "runs": 2, "capacity_model": "cuts""#);
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        let c2 = ctl.clone();
        let h = std::thread::spawn(move || route_with(&m, &o, &c2));
        let t0 = Instant::now();
        while !ctl.snapshot().stage.contains("layer assignment") && !h.is_finished() {
            assert!(t0.elapsed().as_secs() < watchdog_s(), "{fx}: never reached P3");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!h.is_finished(), "{fx}: the run ended before P3");
        let t = Instant::now();
        ctl.request_cancel();
        let r = h.join().expect("run thread");
        // Latency in seconds depends on the build profile: a debug build (tests/test_scaffold.py runs
        // `cargo test` unoptimised) is ~10x slower per move, so its bound is wider (measured 6.1 s on
        // the esp crop where release stays well under 1 s); the stop check is within one move either way.
        let bound = if cfg!(debug_assertions) { 60.0 } else { 5.0 };
        assert!(t.elapsed().as_secs_f64() < bound, "{fx}: Cancel in P3/P4 took {:.2} s (bound {bound} s)", t.elapsed().as_secs_f64());
        assert_eq!(r.stop_reason, "cancel", "{fx}: {:?}", r.warnings);
        assert!(r.warnings.first().is_some_and(|w| w.starts_with("ERROR: nothing to draw yet")), "{fx}: {:?}", r.warnings);
        assert!(r.items.is_empty() && r.routed == 0, "{fx}");
    }
}

/// The P3/P4 move counts of a run: the P4 lines (evaluations, moves tried / applied per kind) and
/// the P3 capacity refinement's move count. All counts, no timings.
fn move_counts(r: &RouteResult) -> Vec<String> {
    let mut v: Vec<String> = r.warnings.iter().filter(|w| w.starts_with("P4 moves on the")).cloned().collect();
    let p3 = r.warnings.iter().find(|w| w.starts_with("layers: P3 capacity refinement")).unwrap_or_else(|| panic!("P3 line: {:?}", r.warnings));
    v.push(p3.split(" moves)").next().and_then(|t| t.rsplit('(').next()).expect("P3 moves").to_string());
    assert_eq!(v.len(), 3, "P4 topology + layers lines and the P3 count: {v:?}");
    v
}

/// A run of `o` on `m` whose wall-clock limit is reached when P3 begins (as the plugin's CPU
/// clock does it), returned when it ends.
fn reached_in_p3(m: &BoardModel, o: &Options) -> RouteResult {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (m2, o2, c2) = (m.clone(), o.clone(), ctl.clone());
    let h = std::thread::spawn(move || route_with(&m2, &o2, &c2));
    let t0 = Instant::now();
    while !ctl.snapshot().stage.contains("layer assignment") && !h.is_finished() {
        assert!(t0.elapsed().as_secs() < watchdog_s(), "never reached P3");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!h.is_finished(), "the run ended before P3");
    ctl.limit.reach();
    h.join().expect("run thread")
}

#[test]
fn a_soft_limit_in_p3_p4_does_not_cut_their_moves_until_its_overrun_cap() {
    // M2.TL: with the soft limit only Cancel / OK ends the moves inside a step (verify-m2 C: the
    // limit stopping M0's move checks again was not caught; the plan still drew complete). The
    // moves of P3/P4 under a limit reached in P3 are the moves of the unlimited run, one by one.
    // The optional P4 moves are on ([optimize] moves; they skip net trees with branch points on
    // traces, so the net trees are fixed) so there are moves to count.
    let m = crop("qmtech-usb-crop.json");
    let o = topor(r#", "runs": 1, "capacity_model": "cuts", "moves": true, "net_tree": "fixed""#);
    let free = run(&m, &o);
    let want = move_counts(&free);
    assert!(want[0].contains(" evaluations") && !want[0].contains(": 0 evaluations"), "the fixture moves in P4: {want:?}");
    let r = reached_in_p3(&m, &o);
    let l = r.limit.as_ref().expect("limit report");
    // user 2026-11-13: a direct engine call has the overrun cap by default ("auto" = wall_clock_s)
    assert!(l.reached && l.mode == "soft" && !l.cap_hit && l.cap_s == Some(o.wall_clock_s) && l.cut == "none", "{l:?}");
    assert_eq!(move_counts(&r), want, "the soft limit cut P3/P4 moves");
    assert!(!r.warnings.iter().any(|w| w.contains("overrun cap reached")), "{:?}", r.warnings);
    // F3 `[run] timeout_overrun_s` (user 2026-11-13): past the cap the soft limit acts as the hard
    // one: the moves stop, what exists is drawn, and a loud warning says so.
    let capped = topor(r#", "runs": 1, "capacity_model": "cuts", "moves": true, "net_tree": "fixed", "timeout_overrun_s": 0.001"#);
    let r = reached_in_p3(&m, &capped);
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.mode == "soft" && l.cap_hit && l.cap_s.is_some_and(|c| (c - 0.001).abs() < 1e-9), "{l:?}");
    let p4: Vec<String> = r.warnings.iter().filter(|w| w.starts_with("P4 moves on the")).cloned().collect();
    assert_ne!(p4, want[..2].to_vec(), "past the overrun cap the P4 moves stop (or never begin)");
    assert!(r.warnings.iter().any(|w| w.starts_with("WARNING: time limit overrun cap reached ([run] timeout_overrun_s = 0 s")), "{:?}", r.warnings);
    // "draw what exists": the cap fired in P3, before the drawing began, so the plan is drawn to its end
    assert!(r.routed > 0 && !r.items.is_empty(), "what existed is drawn: {:?}", r.warnings);
    assert!(!r.connections.iter().any(|c| c.reason.contains("time limit")), "the drawing was not cut: {:?}", r.connections.iter().filter(|c| c.reason.contains("time limit")).collect::<Vec<_>>());
    assert_eq!(l.cut, "plan", "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("WARNING: time limit overrun cap reached") && w.contains("the step that ran then was cut and the kept plan")), "{:?}", r.warnings);
}

#[test]
fn a_hard_limit_before_the_drawing_draws_the_kept_plan_in_full() {
    // User 2026-11-13: `hard` takes the overrun cap's path: the limit cuts the planning step that
    // runs (here P3/P4, reached when P3 begins) and the kept plan is drawn in full (TopoR judges
    // the variants before any copper and draws the chosen one, 6.3 manual pp. 72-73, p. 237).
    // Before, the hard limit cut the drawing too and left a nearly empty board.
    let m = crop("qmtech-usb-crop.json");
    let soft = reached_in_p3(&m, &topor(r#", "runs": 1, "capacity_model": "cuts""#));
    let r = reached_in_p3(&m, &topor(r#", "runs": 1, "capacity_model": "cuts", "timeout_mode": "hard""#));
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.mode == "hard" && l.cut == "plan" && !l.cap_hit, "{l:?}");
    let undrawn: Vec<_> = r.connections.iter().filter(|c| c.reason.contains("before this connection was drawn")).collect();
    assert!(undrawn.is_empty(), "the hard limit did not cut the drawing of the kept plan: {undrawn:?}");
    assert!(r.routed > 0 && !r.items.is_empty(), "{:?}", r.warnings);
    assert_eq!(via_reduction(&r).1, "", "the via reduction of the kept plan runs to its end: {:?}", via_reduction(&r));
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit (hard, [run] timeout_mode)") && w.contains("the step that ran then was cut and the kept plan")), "{:?}", r.warnings);
    assert_eq!(plane_stubs(&r), plane_stubs(&soft), "the plane stubs were placed before the limit");
}

#[test]
fn more_runs_than_rayon_threads_never_hang_at_the_draw_gate() {
    // M2: 3 TopoR-loop runs on a 2-thread pool share 2 compute slots; a run waiting at the draw
    // gate must give its slot back, else the third run never computes and the call hangs
    // (verify-m2 A: caught only at RAYON_NUM_THREADS=2). A watchdog fails the test fast.
    let m = eprom();
    let o = topor(r#", "runs": 3"#);
    let h = std::thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().expect("2-thread pool");
        pool.install(|| run(&m, &o))
    });
    let t0 = Instant::now();
    while !h.is_finished() {
        assert!(t0.elapsed() < Duration::from_secs(120), "3 runs on 2 slots hung at the draw gate");
        std::thread::sleep(Duration::from_millis(20));
    }
    let r = h.join().expect("run thread");
    let line = best_of_line(&r);
    assert_eq!(line.matches("pick not drawn (another run's is: [drawing] candidates = 1").count(), 2, "all 3 runs offered: {line}");
    assert_eq!(r.routed, r.connections_total, "{:?}", r.warnings);
}

// ---- verify-m2c: what the limit, acting as hard, cut (LimitReport.cut / cut_steps), the drawing
// cut by it (decided behaviour, loud), a Cancel's own reason, the stitching's exemption.

/// The reason of a connection the time limit left undrawn (core.rs DRAW_CUT).
const DRAW_CUT: &str = "time limit reached before this connection was drawn";
/// The reason of a connection a Cancel / OK left undrawn (core.rs DRAW_CANCEL).
const DRAW_CANCEL: &str = "Cancel / OK came before this connection was drawn";
/// The loud line of a drawing the limit cut (core.rs DRAWING_CUT_LINE; plugin postrun.py).
const DRAWING_CUT_LINE: &str = "WARNING: the drawing was cut by the hard time limit";

/// Runs `o` on `m` (limit `o.wall_clock_s`); calls `act` once the stage names one of `at`;
/// returns the result and whether `act` came while the run still ran.
fn act_at(m: &BoardModel, o: &Options, at: &[&str], act: impl Fn(&RunControl)) -> (RouteResult, bool) {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (m2, o2, c2) = (m.clone(), o.clone(), ctl.clone());
    let h = std::thread::spawn(move || route_with(&m2, &o2, &c2));
    let t0 = Instant::now();
    while !at.iter().any(|a| ctl.snapshot().stage.contains(a)) && !h.is_finished() {
        assert!(t0.elapsed().as_secs() < watchdog_s(), "never reached {at:?}");
        std::thread::yield_now();
    }
    let hit = !h.is_finished();
    act(&ctl);
    (h.join().expect("run thread"), hit)
}

fn reasons(r: &RouteResult, why: &str) -> usize {
    r.connections.iter().filter(|c| c.reason == why).count()
}

#[test]
fn a_hard_limit_reached_in_the_drawing_cuts_it_loudly() {
    // Decided behaviour (verify-m2c 3, a change is the user's call): a limit that passes while the
    // drawing runs cuts it; the board is drawn only in part, and a WARNING line says how much and
    // what to change. The report names the step cut: the drawing (not "plan ... drawn in full").
    let m = crop("qmtech-usb-crop.json");
    let o = topor(r#", "runs": 1, "capacity_model": "cuts", "timeout_mode": "hard""#);
    let (r, hit) = act_at(&m, &o, &["drawing the traces"], |c| c.limit.reach());
    assert!(hit, "the run ended before the drawing");
    let l = r.limit.as_ref().expect("limit report");
    let undrawn = reasons(&r, DRAW_CUT);
    assert!(undrawn > 0 && reasons(&r, DRAW_CANCEL) == 0, "the hard limit cut the drawing: {:?}", r.connections.iter().map(|c| &c.reason).collect::<Vec<_>>());
    assert!(l.reached && l.mode == "hard" && !l.cap_hit, "{l:?}");
    assert_eq!((l.cut.as_str(), l.cut_steps.clone()), ("drawing", vec!["drawing".to_string()]), "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit (hard, [run] timeout_mode)") && w.contains("the drawing was the step that ran then: the drawing of the traces was cut")), "{:?}", r.warnings);
    let want = format!("{DRAWING_CUT_LINE} (the drawing of the traces): {} of {} connections drawn, {undrawn} left undrawn by the cut; use [run] timeout_mode = \"soft\" or a longer [run] timeout_s for a complete board", r.routed, r.connections_total);
    assert!(r.warnings.iter().any(|w| w.starts_with(&want)), "{want}\n{:?}", r.warnings);
}

#[test]
fn a_hard_limit_reached_in_the_via_reduction_names_that_step() {
    // verify-m2d O2: the step the limit cut is the via reduction (its own CutStep), not the drawing
    // that precedes it: cut_steps names it and the report line says the vias it had not removed stay.
    let m = crop("qmtech-usb-crop.json");
    let o = topor(r#", "runs": 1, "capacity_model": "cuts", "timeout_mode": "hard", "opt_loop": "cycles", "topo_rounds": 1"#);
    let (r, hit) = act_at(&m, &o, &["via reduction and fuse"], |c| c.limit.reach());
    assert!(hit, "the run ended before the via reduction");
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.mode == "hard", "{l:?}");
    assert!(l.cut_steps.contains(&"via_reduction".to_string()), "the via reduction is the step cut: {l:?}");
    assert!(r.warnings.iter().any(|w| w.contains("the via reduction was cut (the vias it had not removed yet stay)")), "{:?}", r.warnings);
}

#[test]
fn a_wire_never_drop_skips_because_of_the_cut_keeps_both_reasons() {
    // verify-m2e F3 (user 2026-11-14: keep both): the limit passes between a wire's failed drawing and
    // never drop; the skipped wire's reason names the cut AND why its own drawing had failed. The test
    // hook injects the failed drawing of the first wire and reaches the (hard) limit at never drop's start.
    let m = eprom();
    let o = topor(r#", "runs": 1, "timeout_mode": "hard", "opt_loop": "cycles", "topo_rounds": 1"#);
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (l2, f2) = (ctl.limit.clone(), failed.clone());
    ctl.set_test_hook(move |p| match p {
        "fail wire drawing" => !f2.swap(true, std::sync::atomic::Ordering::SeqCst),
        "never drop" => {
            l2.reach();
            // a round is cut only past its 1 s floor (core.rs `floor`)
            std::thread::sleep(Duration::from_millis(1200));
            false
        }
        _ => false,
    });
    let r = route_with(&m, &o, &ctl);
    let own = "its own drawing had failed: drawing: injected by the test hook";
    let both: Vec<&String> = r.connections.iter().map(|c| &c.reason).filter(|w| w.starts_with(DRAW_CUT) && w.contains(own)).collect();
    assert_eq!(both.len(), 1, "one wire: the cut reason and its own failure: {:?}\n{:?} {:?}", r.connections.iter().map(|c| &c.reason).filter(|w| !w.is_empty()).collect::<Vec<_>>(), r.limit, r.warnings.iter().filter(|w| w.contains("never drop") || w.contains("limit")).collect::<Vec<_>>());
}

#[test]
fn the_overrun_cap_firing_in_the_drawing_cuts_it() {
    // The soft limit passed in P3 (the step finishes, no new one begins); the cap fires (here: the
    // caller hardens the limit, as the plugin's CPU clock does) once the drawing runs: the drawing
    // began before it, so the cap cuts it (one path with `hard`).
    let m = crop("qmtech-usb-crop.json");
    let o = topor(r#", "runs": 1, "capacity_model": "cuts""#);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (m2, o2, c2) = (m.clone(), o.clone(), ctl.clone());
    let h = std::thread::spawn(move || route_with(&m2, &o2, &c2));
    let t0 = Instant::now();
    while !ctl.snapshot().stage.contains("layer assignment") && !h.is_finished() {
        assert!(t0.elapsed().as_secs() < watchdog_s(), "never reached P3");
        std::thread::sleep(Duration::from_millis(2));
    }
    ctl.limit.reach();
    while !ctl.snapshot().stage.contains("drawing the traces") && !h.is_finished() {
        std::thread::yield_now();
    }
    assert!(!h.is_finished(), "the run ended before the drawing");
    ctl.limit.harden();
    let r = h.join().expect("run thread");
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.mode == "soft" && l.cap_hit, "{l:?}");
    assert!(reasons(&r, DRAW_CUT) > 0, "the cap firing in the drawing cut it");
    assert_eq!(l.cut, "drawing", "{l:?}");
    assert!(l.cut_steps.contains(&"drawing".to_string()), "{l:?}");
    assert!(r.warnings.iter().any(|w| w.starts_with(&format!("{DRAWING_CUT_LINE} (the soft limit past its overrun cap acts as the hard one) (")) && w.contains("raise [run] timeout_overrun_s or timeout_s for a complete board")), "{:?}", r.warnings);
}

#[test]
fn a_cancel_is_no_limit_cut_and_leaves_its_own_reason() {
    // verify-m2c 4: Cancel / OK is no cut by the time limit (cut "none", whatever the mode), and a
    // whole-route cycle's drawing it ends leaves Cancel's reason, not the time limit's.
    let m = crop("qmtech-usb-crop.json");
    let o = topor(r#", "runs": 1, "capacity_model": "cuts", "timeout_mode": "hard", "opt_loop": "cycles", "topo_rounds": 3"#);
    let (r, hit) = act_at(&m, &o, &["drawing the traces"], |c| c.request_cancel());
    assert!(hit, "the run ended before the drawing");
    let l = r.limit.as_ref().expect("limit report");
    assert_eq!((l.cut.as_str(), l.cut_steps.len(), l.reached), ("none", 0, false), "a Cancel is no limit cut: {l:?}");
    assert!(reasons(&r, DRAW_CANCEL) > 0 && reasons(&r, DRAW_CUT) == 0, "Cancel's own reason: {:?}", r.connections.iter().map(|c| &c.reason).collect::<Vec<_>>());
    assert!(!r.warnings.iter().any(|w| w.starts_with(DRAWING_CUT_LINE)), "{:?}", r.warnings);
    // the TopoR loop, hard: a Cancel in P3 keeps no plan, and is no limit cut either
    let o = topor(r#", "runs": 1, "capacity_model": "cuts", "timeout_mode": "hard""#);
    let (r, hit) = act_at(&m, &o, &["layer assignment"], |c| c.request_cancel());
    assert!(hit, "the run ended before P3");
    let l = r.limit.as_ref().expect("limit report");
    assert_eq!((l.cut.as_str(), l.cut_steps.len()), ("none", 0), "{l:?}");
}

/// A 40 x 38 mm 2-layer board: GND (net 1) poured on F.Cu and B.Cu over [2, 32] x [3, 35] mm,
/// GND pads U1.1 (SMD, B.Cu, (4, 19)) and U3.1 (THT, (20, 19)); SIG (net 2) from J1.1 (7, 1.5) to
/// J2.1 (7, 36.5), both SMD on B.Cu: its straight trace cuts the B.Cu pour from edge to edge, so
/// U1.1's piece needs a stitch via (as tests/topo_d4.rs, here drawn by the router).
fn pour_cut_board() -> BoardModel {
    const MM: i64 = 1_000_000;
    let pad = |r: &str, net: i32, x: i64, y: i64, layers: &str, attr: &str| {
        let shapes: Vec<String> = layers.split(',').map(|l| format!(r#""{}": {{ "kind": "circle", "d": 1000000 }}"#, l.trim())).collect();
        format!(r#"{{ "uuid": "{r}", "ref": "{r}", "number": "1", "net": {net}, "attr": "{attr}", "center": [{x}, {y}], "layers": [{layers}], "shapes": {{ {} }} }}"#, shapes.join(","))
    };
    let pads = [pad("U1", 1, 4 * MM, 19 * MM, "31", "smd"), pad("U3", 1, 20 * MM, 19 * MM, "0, 31", "pth"), pad("J1", 2, 7 * MM, 1_500_000, "31", "smd"), pad("J2", 2, 7 * MM, 36_500_000, "31", "smd")];
    let fill = |l: u32| format!(r#"{{ "layer": {l}, "outer": [[2000000, 3000000], [32000000, 3000000], [32000000, 35000000], [2000000, 35000000]] }}"#);
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
          "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [40000000, 0] }},
            {{ "kind": "seg", "a": [40000000, 0], "b": [40000000, 38000000] }},
            {{ "kind": "seg", "a": [40000000, 38000000], "b": [0, 38000000] }},
            {{ "kind": "seg", "a": [0, 38000000], "b": [0, 0] }} ],
          "pads": [ {} ],
          "pours": [ {{ "uuid": "zz", "net": 1, "layers": [0, 31], "pads": ["U1.1", "U3.1"],
            "vias": [], "clearance": 200000, "min_thickness": 254000, "thermal_gap": 500000,
            "spoke_width": 500000, "pad_connection": 2,
            "fills": [ {}, {} ] }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0 }} }}"#,
        pads.join(","),
        fill(0),
        fill(31)
    );
    BoardModel::from_json(&s).expect("pour board")
}

/// The pour stitching line's counts ("N pour pieces cut off by the routed copper before, M after").
fn stitching(r: &RouteResult) -> String {
    let w = r.warnings.iter().find(|w| w.starts_with("pour stitching:")).unwrap_or_else(|| panic!("pour stitching line: {:?}", r.warnings));
    w.split(';').next().expect("counts").to_string()
}

#[test]
fn a_hard_limit_before_the_drawing_does_not_cut_the_pour_stitching() {
    // The limit acted as hard before the drawing began: the kept plan is drawn in full, its pour
    // stitching too (it carries the drawing's decision, verify-m2c 5): the piece is joined.
    let m = pour_cut_board();
    let free = run(&m, &topor(r#", "runs": 1"#));
    assert_eq!(free.routed, free.connections_total, "{:?}", free.warnings);
    assert_eq!(stitching(&free), "pour stitching: 1 pour pieces cut off by the routed copper before, 0 after", "the board needs its stitch via");
    let o = topor(r#", "runs": 1, "timeout_mode": "hard""#);
    let ctl = RunControl::new();
    ctl.limit.init(1e-6);
    let r = route_with(&m, &o, &ctl);
    let l = r.limit.as_ref().expect("limit report");
    // (the initial routing ends within its 1 s floor here, so the limit may have cut nothing)
    assert!(l.reached && l.mode == "hard" && l.cut != "drawing", "{l:?}");
    assert!(!l.cut_steps.iter().any(|s| s == "stitching" || s == "drawing" || s == "via_reduction"), "the drawing's steps were not cut: {l:?}");
    assert_eq!(stitching(&r), stitching(&free), "the stitching of the kept plan runs to its end");
    assert!(!r.warnings.iter().any(|w| w.starts_with(DRAWING_CUT_LINE)), "{:?}", r.warnings);
}

#[test]
fn the_pour_stitching_after_a_drawing_the_limit_cut_is_cut_too() {
    // verify-m2c 5: the stitching carries the drawing's decision: the limit passed after the
    // drawing began (the decision is made before the stage says "drawing the traces"), so the
    // drawing's steps are not exempt and the stitching that follows is cut (before, it re-decided
    // at its start and ran in full). The drawing itself is not cut: a round gets at least 1 s,
    // and this board's round takes less; the stitching has no such floor.
    // The board's drawing takes milliseconds: a loaded machine may not let this thread see it, so
    // a run whose limit came too late (not while the drawing or the via reduction ran) is tried again.
    let m = pour_cut_board();
    let o = topor(r#", "runs": 1, "timeout_mode": "hard""#);
    let in_time = std::sync::atomic::AtomicBool::new(false);
    let r = (0..40)
        .find_map(|_| {
            in_time.store(false, std::sync::atomic::Ordering::Relaxed);
            let (r, hit) = act_at(&m, &o, &["drawing the traces", "via reduction"], |c| {
                c.limit.reach();
                let st = c.snapshot().stage;
                in_time.store(st.contains("drawing the traces") || st.contains("via reduction"), std::sync::atomic::Ordering::Relaxed);
            });
            (hit && in_time.load(std::sync::atomic::Ordering::Relaxed)).then_some(r)
        })
        .expect("in 40 runs the limit never came while the drawing ran");
    let l = r.limit.as_ref().expect("limit report");
    assert!(l.reached && l.cut == "drawing" && l.cut_steps.contains(&"stitching".to_string()), "{l:?}");
    assert!(!l.cut_steps.contains(&"plan".to_string()), "{l:?}");
    assert_eq!(r.routed, r.connections_total, "the traces were drawn before the limit: {:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.starts_with(&format!("{DRAWING_CUT_LINE} (")) && w.contains("the pour stitching")), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.starts_with("time limit (hard") && w.contains("the pour stitching was cut")), "{:?}", r.warnings);
}
