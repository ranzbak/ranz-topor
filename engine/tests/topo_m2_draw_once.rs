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

fn crop(name: &str) -> BoardModel {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses")
}

/// The TopoR method's settings (M0 defaults: loop topor, initial once, taut lengths), headless;
/// `extra` (`, "key": value ...`) adds keys or overrides these.
fn topor(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"wall_clock_s": 600.0, "opt_loop": "topor", "initial": "once", "search_length": "taut"}"#).expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
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
}

#[test]
fn a_limit_reached_in_p3_p4_still_draws_the_initial_plan() {
    // The limit is reached while the layer assignment and the rip-up rounds run (as the plugin's
    // CPU clock does it): they finish, and their plan is drawn, completely.
    for fx in ["qmtech-usb-crop.json", "dcsgone-usb-crop.json"] {
        let m = crop(fx);
        let o = topor(r#", "runs": 1, "capacity_model": "cuts""#);
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        let c2 = ctl.clone();
        let h = std::thread::spawn(move || route_with(&m, &o, &c2));
        let t0 = Instant::now();
        while !ctl.snapshot().stage.contains("layer assignment") && !h.is_finished() {
            assert!(t0.elapsed().as_secs() < 600, "{fx}: never reached P3");
            std::thread::sleep(Duration::from_millis(2));
        }
        ctl.limit.reach();
        assert!(ctl.snapshot().notice.starts_with("time limit reached, waiting for the current step to finish (layer assignment"), "{:?}", ctl.snapshot().notice);
        let r = h.join().expect("run thread");
        let open: Vec<_> = r.connections.iter().filter(|c| c.status != "routed" && c.reason.contains("time limit")).collect();
        assert!(open.is_empty(), "{fx}: nothing is cut by the soft limit: {open:?}");
        assert!(r.routed > 0 && !r.items.is_empty(), "{fx}: the initial plan is drawn");
        let l = r.limit.as_ref().expect("limit report");
        assert!(l.reached && l.step.contains("layer assignment"), "{fx}: {l:?}");
        assert!(r.warnings.iter().any(|w| w.contains(": 0 cycles") && w.contains("ended by time limit")), "{fx}: no loop cycle after the limit: {:?}", r.warnings);
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
    let m = crop("qmtech-usb-crop.json");
    let o = topor(r#", "runs": 2, "capacity_model": "cuts""#);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    let h = std::thread::spawn(move || route_with(&m, &o, &c2));
    let t0 = Instant::now();
    while !ctl.snapshot().stage.contains("layer assignment") && !h.is_finished() {
        assert!(t0.elapsed().as_secs() < 600, "never reached P3");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!h.is_finished(), "the run ended before P3");
    let t = Instant::now();
    ctl.request_cancel();
    let r = h.join().expect("run thread");
    assert!(t.elapsed().as_secs_f64() < 5.0, "Cancel in P3/P4 took {:.2} s", t.elapsed().as_secs_f64());
    assert_eq!(r.stop_reason, "cancel", "{:?}", r.warnings);
    assert!(r.warnings.first().is_some_and(|w| w.starts_with("ERROR: nothing to draw yet")), "{:?}", r.warnings);
    assert!(r.items.is_empty() && r.routed == 0);
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

#[test]
fn a_soft_limit_in_p3_p4_does_not_cut_their_moves() {
    // M2.TL: with the soft limit only Cancel / OK ends the moves inside a step (verify-m2 C: the
    // limit stopping M0's move checks again was not caught; the plan still drew complete). The
    // moves of P3/P4 under a limit reached in P3 are the moves of the unlimited run, one by one.
    let m = crop("qmtech-usb-crop.json");
    let o = topor(r#", "runs": 1, "capacity_model": "cuts""#);
    let free = run(&m, &o);
    let want = move_counts(&free);
    assert!(want[0].contains(" evaluations") && !want[0].contains(": 0 evaluations"), "the fixture moves in P4: {want:?}");
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (m2, o2, c2) = (m.clone(), o.clone(), ctl.clone());
    let h = std::thread::spawn(move || route_with(&m2, &o2, &c2));
    let t0 = Instant::now();
    while !ctl.snapshot().stage.contains("layer assignment") && !h.is_finished() {
        assert!(t0.elapsed().as_secs() < 600, "never reached P3");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!h.is_finished(), "the run ended before P3");
    ctl.limit.reach();
    let r = h.join().expect("run thread");
    assert!(r.limit.as_ref().is_some_and(|l| l.reached && l.mode == "soft"), "{:?}", r.limit);
    assert_eq!(move_counts(&r), want, "the soft limit cut P3/P4 moves");
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
