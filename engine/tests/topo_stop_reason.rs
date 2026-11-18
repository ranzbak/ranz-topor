//! stop_reason of the topological core ([optimize] loop = "topor", the default): every value the
//! result's `stop_reason` can take, with the dialog (`stop_when_stable` false) and headless, under
//! the wall clock (the engine's own limit, `wall_clock_s`) and the CPU clock (the plugin's own limit:
//! the engine gets `wall_clock_s` 0 and is told `RouteJob.reach_limit` = `limit.reach()`).
//! Definition of done 13 (stable-label round 2). eprom-to-kernal, one run, cheap.
//!
//! User 2026-11-17 "Report stable live": the TopoR loop shows "Routes stable" while it still runs,
//! so OK (= Cancel) at the live stable ends the run before its cycle cap with stop_reason "stable".
//! A dialog run never ends BY ITSELF at stable (only OK / Cancel / cap / time limit); headless runs
//! keep stopping at stable.

use std::time::{Duration, Instant};
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Clock {
    Wall,
    Cpu,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Dialog,
    Headless,
}

/// What acts on the running loop (from a watcher thread, once the loop runs).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Nothing,
    /// OK / Cancel the moment the progress shows "Routes stable".
    OkAtStable,
    /// Cancel while the loop runs, before stable (the loop never gets stable here).
    CancelInLoop,
    /// The limit is reached while the loop runs (wall: the limit set below the time spent; cpu: the
    /// plugin's `reach_limit`).
    LimitInLoop,
}

fn opts(mode: Mode, clock: Clock, extra: &str) -> Options {
    let wall = if clock == Clock::Wall { 300.0 } else { 0.0 };
    let headless = mode == Mode::Headless;
    let json = format!(r#"{{"runs": 1, "wall_clock_s": {wall}, "stop_when_stable": {headless}{extra}}}"#);
    Options::from_json(&json).expect("options parse")
}

/// The TopoR loop's report line: (cycles, stable_at, the first word of "ended by").
fn loop_line(r: &RouteResult) -> (usize, Option<usize>, String) {
    let l = r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).expect("TopoR loop line");
    let n = l.split("): ").nth(1).and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok()).expect("cycles");
    let by = l.split("ended by ").nth(1).and_then(|s| s.split([' ', ';', ',']).next()).unwrap_or("").to_string();
    let st = l.split("(stable after cycle ").nth(1).and_then(|s| s.split(')').next()).and_then(|s| s.parse().ok());
    (n, st, by)
}

/// Runs the eprom with `o`; `act` from a watcher once the TopoR loop runs. Returns the result and
/// whether the run was still going when the act came (premise of the mid-loop acts).
fn run(o: &Options, act: Act, clock: Clock) -> (RouteResult, bool) {
    run_on(eprom(), o, act, clock)
}

fn run_on(m: BoardModel, o: &Options, act: Act, clock: Clock) -> (RouteResult, bool) {
    let ctl = RunControl::new();
    let (c2, o2) = (ctl.clone(), o.clone());
    let h = std::thread::spawn(move || route_with(&m, &o2, &c2));
    let t = Instant::now();
    let mut running = true;
    if act != Act::Nothing {
        loop {
            let p = ctl.snapshot();
            let ready = match act {
                Act::OkAtStable => p.stable,
                _ => p.stage.contains("TopoR loop"),
            };
            if ready {
                break;
            }
            assert!(!h.is_finished(), "the run ended before {act:?}: {p:?}");
            // eprom is stable within a second (release); the bound scales with debug builds (DoD 9)
            let bound = if cfg!(debug_assertions) { 300 } else { 60 };
            assert!(t.elapsed() < Duration::from_secs(bound), "never ready for {act:?}: {p:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
        if act != Act::OkAtStable {
            std::thread::sleep(Duration::from_millis(100)); // a few loop cycles
        }
        running = !h.is_finished();
        match act {
            Act::OkAtStable | Act::CancelInLoop => ctl.request_cancel(),
            Act::LimitInLoop if clock == Clock::Wall => ctl.set_time_limit(0.001), // below the time spent
            Act::LimitInLoop => ctl.limit.reach(),
            Act::Nothing => {}
        }
    }
    (h.join().expect("run thread"), running)
}

/// Never stable inside the cap (no gain is below 0 %): the loop runs until something acts.
const NEVER_STABLE: &str = r#", "stable_gain_pct": 0.0, "topo_rounds": 100000"#;

fn no_budget_warning(r: &RouteResult) {
    assert!(!r.warnings.iter().any(|w| w.starts_with("time budget hit")), "{:?}", r.warnings);
}

fn check(mode: Mode, clock: Clock) {
    let tag = format!("{mode:?}/{clock:?}");
    // "stable": headless = the loop's own stop at stable; dialog = finish_when_stable (its own stop)
    // and OK at the live "Routes stable" (the loop ends mid-way, before the cap).
    let fin = if mode == Mode::Dialog { r#", "finish_when_stable": true, "topo_rounds": 1000"# } else { r#", "topo_rounds": 1000"# };
    let (r, _) = run(&opts(mode, clock, fin), Act::Nothing, clock);
    let (n, st, by) = loop_line(&r);
    assert_eq!((r.stop_reason.as_str(), by.as_str()), ("stable", "stable"), "{tag}: {:?}", r.warnings);
    assert!(st.is_some_and(|s| s == n) && !r.timed_out, "{tag}: {n} {st:?}");
    no_budget_warning(&r);
    if mode == Mode::Dialog {
        // TW3: the dialog's loop has no cycle cap (rounds = 3 is not obeyed); stable needs the
        // four bests unchanged for loop_stable_window = 8 cycles, so it comes after cycle 3
        let (r, running) = run(&opts(mode, clock, r#", "topo_rounds": 3"#), Act::OkAtStable, clock);
        let (n, st, by) = loop_line(&r);
        assert!(running, "{tag}: OK reached a running engine");
        assert_eq!(r.stop_reason, "stable", "{tag}: {:?}", r.warnings);
        assert_eq!(by, "Cancel", "{tag}: the loop ended by the OK ({n} cycles, stable at {st:?})");
        assert!(st.is_some_and(|s| s <= n) && n > 3, "{tag}: no cycle cap in the dialog: {n} {st:?}");
        assert!(!r.timed_out && r.converged, "{tag}");
        no_budget_warning(&r);
        assert!(r.routed > 0 && r.routed == r.connections_total, "{tag}: the stable board, drawn in full: {} of {}", r.routed, r.connections_total);
    }
    // "round cap": the cap before stable (headless; the dialog's loop has no cycle cap since TW3,
    // above). With restart = "stall" (the rule before TW3) the dialog still obeys rounds.
    let cap = if mode == Mode::Dialog { r#", "topo_rounds": 30, "restart": "stall""# } else { r#", "topo_rounds": 2, "stable_gain_pct": 0.0"# };
    let (r, _) = run(&opts(mode, clock, cap), Act::Nothing, clock);
    let (n, st, by) = loop_line(&r);
    assert_eq!((r.stop_reason.as_str(), by.as_str()), ("round cap", "cycle"), "{tag}: {:?}", r.warnings);
    assert_eq!(st.is_some(), mode == Mode::Dialog, "{tag}: {n} {st:?}");
    assert!(!r.timed_out, "{tag}");
    no_budget_warning(&r);
    // "timeout": the limit (soft, the default; and hard) reached while the loop runs
    for hard in [false, true] {
        let extra = format!("{NEVER_STABLE}{}", if hard { r#", "timeout_mode": "hard""# } else { "" });
        let (r, running) = run(&opts(mode, clock, &extra), Act::LimitInLoop, clock);
        let (_, st, by) = loop_line(&r);
        assert!(running, "{tag}");
        assert_eq!(r.stop_reason, "timeout", "{tag} hard {hard}: {:?}", r.warnings);
        assert!(by == "time" && st.is_none() && r.timed_out, "{tag} hard {hard}: {by} {st:?}");
        assert!(r.warnings.iter().any(|w| w.starts_with("time budget hit")), "{tag} hard {hard}: {:?}", r.warnings);
    }
    // "cancel": Cancel before stable
    let (r, running) = run(&opts(mode, clock, NEVER_STABLE), Act::CancelInLoop, clock);
    let (_, st, by) = loop_line(&r);
    assert!(running, "{tag}");
    assert_eq!((r.stop_reason.as_str(), by.as_str()), ("cancel", "Cancel"), "{tag}: {:?}", r.warnings);
    assert!(st.is_none(), "{tag}");
    // "stop_after plan"
    let (r, _) = run(&opts(mode, clock, r#", "stop_after": "plan", "topo_rounds": 30"#), Act::Nothing, clock);
    assert_eq!(r.stop_reason, "stop_after plan", "{tag}");
}

#[test]
fn stop_reason_dialog_wall_clock() {
    check(Mode::Dialog, Clock::Wall);
}

#[test]
fn stop_reason_dialog_cpu_clock() {
    check(Mode::Dialog, Clock::Cpu);
}

#[test]
fn stop_reason_headless_wall_clock() {
    check(Mode::Headless, Clock::Wall);
}

#[test]
fn stop_reason_headless_cpu_clock() {
    check(Mode::Headless, Clock::Cpu);
}

/// The pour board of tests/topo_m2_draw_once.rs (copy): A 40 x 38 mm 2-layer board: GND (net 1) poured on F.Cu and B.Cu over [2, 32] x [3, 35] mm,
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

/// The pour stitching line: (pieces cut off before, after), when the board had any.
fn stitching(r: &RouteResult) -> Option<(usize, usize)> {
    let l = r.warnings.iter().find(|w| w.starts_with("pour stitching: "))?;
    let n: Vec<usize> = l.split(' ').filter_map(|w| w.parse().ok()).take(2).collect();
    Some((n[0], n[1]))
}

/// User/lead 2026-11-17: OK at the live "Routes stable" = the same post-loop work as a run that ended
/// at stable by itself, so the pour stitching runs to its end (and says so); a Cancel before stable
/// still ends it (after the pass in progress: none had begun, so nothing is stitched).
#[test]
fn ok_at_live_stable_stitches_in_full_cancel_before_stable_does_not() {
    let o = opts(Mode::Dialog, Clock::Wall, r#", "topo_rounds": 100000"#);
    let (ok, running) = run_on(pour_cut_board(), &o, Act::OkAtStable, Clock::Wall);
    assert!(running && ok.stop_reason == "stable", "{:?}", ok.warnings);
    let (before, after) = stitching(&ok).expect("the routed SIG trace cuts off U1's pour piece (premise)");
    assert!(before > 0 && after < before, "the stitching ran: {before} -> {after}");
    assert!(ok.warnings.iter().any(|w| w.starts_with("OK at Routes stable: the pour stitching ran to its end")), "{:?}", ok.warnings);
    let (c, running) = run_on(pour_cut_board(), &opts(Mode::Dialog, Clock::Wall, NEVER_STABLE), Act::CancelInLoop, Clock::Wall);
    assert!(running && c.stop_reason == "cancel", "{:?}", c.warnings);
    assert!(!c.warnings.iter().any(|w| w.starts_with("OK at Routes stable")), "{:?}", c.warnings);
    let (b, a) = stitching(&c).expect("cancel: stitching line (cut before its first pass)");
    assert_eq!(a, b, "a Cancel before stable ends the stitching: nothing stitched");
}

/// Runs `o` on the eprom in a rayon pool of `threads` (the engine's share: its compute slots are
/// `min(runs, threads)`); the watcher polls the progress every millisecond and sends one stop
/// request (`request_cancel`: OK when the progress shows "Routes stable" at that moment, else
/// Cancel) the first time `when` holds; when "Routes stable" comes first it stops anyway and says
/// so. Returns the result and the progress at the request (`None`: `when` never held).
fn run_threads(o: &Options, threads: usize, when: fn(&topor_engine::control::Progress) -> bool) -> (RouteResult, Option<topor_engine::control::Progress>) {
    let ctl = RunControl::new();
    let (c2, o2) = (ctl.clone(), o.clone());
    let h = std::thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
        pool.install(|| route_with(&eprom(), &o2, &c2))
    });
    let t = Instant::now();
    let bound = Duration::from_secs(if cfg!(debug_assertions) { 300 } else { 60 });
    let mut at = None;
    loop {
        assert!(!h.is_finished(), "the run ended before the stop request");
        let p = ctl.snapshot();
        if when(&p) {
            ctl.request_cancel();
            at = Some(p);
            break;
        }
        if p.stable || t.elapsed() > bound {
            ctl.request_cancel(); // missed the moment (or never): end it anyway
            assert!(t.elapsed() <= bound, "never ready in {bound:?}: {p:?}");
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    (h.join().expect("run thread"), at)
}

/// verify-sl2 V1 (user 2026-11-17): Cancel at "1 of 2 stable" (the button still read Cancel) is a
/// Cancel, whichever run's pick is drawn and however the other run ends after it: stop_reason
/// "cancel", no "OK at Routes stable" and the pour stitching cut. Before the latch these seeds
/// drew the stable run's pick and ended "stable" with the stitching in full (probe P3).
/// TW3: `restart = "stall"` keeps each run's own stable meter. With the hybrid (default) stable is
/// the shared table's, which both runs read at their sync points over the same committed cycles,
/// so "1 of 2 stable" lasts only a few eprom cycles (ms) and the watcher misses it on a busy machine
/// (VM r5: 2 of 4 seeds). The latch under test is the dialog's, not the restart's.
#[test]
fn cancel_at_one_of_two_runs_stable_is_a_cancel() {
    let mut hit = 0;
    for seed in [2u64, 3, 4, 8] {
        let o = Options::from_json(&format!(r#"{{"runs": 2, "seed": {seed}, "wall_clock_s": 300, "stop_when_stable": false, "topo_rounds": 100000, "restart": "stall"}}"#)).expect("options");
        let (r, at) = run_threads(&o, 2, |p| p.runs_stable >= 1 && !p.stable);
        let Some(p) = at else { continue };
        hit += 1;
        assert!(p.runs == 2 && p.runs_stable == 1 && !p.stable, "seed {seed}: premise {p:?}");
        assert_eq!(r.stop_reason, "cancel", "seed {seed}: {:?}", r.warnings);
        assert!(!r.warnings.iter().any(|w| w.starts_with("OK at Routes stable")), "seed {seed}: {:?}", r.warnings);
        if let Some((before, after)) = stitching(&r) {
            assert_eq!(after, before, "seed {seed}: a Cancel before stable cuts the stitching");
        }
    }
    assert!(hit >= 3, "the moment 1 of 2 stable was seen for only {hit} of 4 seeds");
}

/// User 2026-11-17 "free the slot" (verify-sl2 V2): with fewer engine threads than runs a run whose
/// TopoR loop is stable gives its compute slot to a run not yet begun, so "Routes stable" (every
/// run stable) comes while the loops run, far before any cap, and Best so far is not empty then
/// (the loops' predicted pick); OK there ends it "stable". Before, the first runs held their slots
/// up to the cycle cap and the last run never began.
#[test]
fn routes_stable_comes_with_fewer_threads_than_runs() {
    let o = Options::from_json(r#"{"runs": 3, "wall_clock_s": 300, "stop_when_stable": false, "topo_rounds": 100000}"#).expect("options");
    let (r, at) = run_threads(&o, 2, |p| p.stable);
    let p = at.expect("Routes stable while the loops run");
    assert!(p.runs == 3 && p.runs_stable == 3 && p.runs_done == 0, "every run stable, none ended (no cap): {p:?}");
    assert!(p.best_total > 0 && p.best_round > 0, "a pick to keep at Routes stable: {p:?}");
    assert_eq!(r.stop_reason, "stable", "{:?}", r.warnings);
    let (n, st, by) = loop_line(&r);
    assert!(by == "Cancel" && st.is_some_and(|s| s <= n) && n < 100000, "{n} {st:?} {by}");
}

/// User 2026-11-17: the first progress that shows "Routes stable" has a pick to keep (Best so far
/// not empty), with one run and with several (the TopoR loop's predicted pick, nothing drawn yet).
#[test]
fn the_first_routes_stable_has_a_best_so_far() {
    for runs in [1usize, 3] {
        let o = Options::from_json(&format!(r#"{{"runs": {runs}, "wall_clock_s": 300, "stop_when_stable": false, "topo_rounds": 100000}}"#)).expect("options");
        let (r, at) = run_threads(&o, runs, |p| p.stable);
        let p = at.expect("Routes stable");
        assert!(p.best_total == r.connections_total && p.best_routed > 0 && p.best_round == 1 && p.best_run >= 1, "runs {runs}: {p:?}");
        // the predicted pick counts the planned connections, and every one of them is drawn (never drop)
        assert_eq!(p.best_routed, r.routed, "runs {runs}");
        assert_eq!(r.stop_reason, "stable", "runs {runs}");
    }
}

/// verify-sl3 F1 (probe P2c): OK at Routes stable (dialog), or the stop at stable (headless), then a
/// hard time limit that passes INSIDE the one drawing, past the round's 1 s floor (the test hook at
/// the first drawn wire sets the limit just ahead and sleeps past it). The limit, not the OK / the
/// stable stop, ended what is drawn: the result says timeout (stop_reason "timeout", timed_out) with
/// the TIME_HIT line naming the stable plan, never "stable"; on the pour board the limit cuts the
/// pour stitching, so no "ran to its end" line stands next to "the pour stitching was cut".
/// Before the fix the OK cases ended "stable", timed_out false (eprom routed 1 of 27), and the
/// headless pour case (only its stitching cut) "stable" too.
#[test]
fn ok_at_stable_then_a_hard_limit_cuts_the_drawing_is_a_timeout() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    for mode in [Mode::Dialog, Mode::Headless] {
        for (board, m) in [("eprom", eprom()), ("pour", pour_cut_board())] {
            let o = opts(mode, Clock::Wall, r#", "topo_rounds": 100000, "timeout_mode": "hard""#);
            let mut ctl = RunControl::new();
            let (lim, c4) = (ctl.limit.clone(), ctl.clone());
            let fired = Arc::new(AtomicUsize::new(0));
            let f2 = fired.clone();
            let dialog = mode == Mode::Dialog;
            ctl.set_test_hook(move |pt| {
                // the one drawing comes after the loop (draw once): after the OK, or after the headless stop
                if (!dialog || c4.ok_at_stable()) && pt == "fail wire drawing" && f2.fetch_add(1, Ordering::SeqCst) == 0 {
                    lim.set(lim.elapsed_s() + 0.05);
                    std::thread::sleep(Duration::from_millis(1300));
                }
                false
            });
            let (c2, o2) = (ctl.clone(), o.clone());
            let h = std::thread::spawn(move || route_with(&m, &o2, &c2));
            let t = Instant::now();
            let bound = Duration::from_secs(if cfg!(debug_assertions) { 300 } else { 60 });
            if dialog {
                loop {
                    assert!(!h.is_finished(), "{board}: the run ended before Routes stable");
                    if ctl.snapshot().stable {
                        ctl.request_stop(Some(true));
                        break;
                    }
                    assert!(t.elapsed() < bound, "{board}: never stable in {bound:?}");
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            let r = h.join().expect("run thread");
            let case = format!("{mode:?} {board}");
            assert_eq!(fired.load(Ordering::SeqCst), 1, "{case}: the hook reached the drawing (premise)");
            let (_, st, _) = loop_line(&r);
            assert!(st.is_some(), "{case}: the loop was stable (premise): {:?}", r.warnings);
            let cut_line = |s: &str| r.warnings.iter().any(|w| w.starts_with("time limit (hard") && w.contains(s));
            assert!(cut_line("the drawing of the traces was cut") || cut_line("the pour stitching was cut"), "{case}: premise, the limit cut the drawing: {:?}", r.warnings);
            assert_eq!(r.stop_reason, "timeout", "{case}: routed {}/{}: {:?}", r.routed, r.connections_total, r.warnings);
            assert!(r.timed_out && !r.converged, "{case}");
            let why = if dialog { "OK came at Routes stable, but the time limit then cut the drawing of the stable plan" } else { "the routes were stable, but the time limit then cut the drawing of the stable plan" };
            assert!(r.warnings.iter().any(|w| w.contains(why)), "{case}: {:?}", r.warnings);
            if cut_line("the pour stitching was cut") {
                // (eprom's one pour has no fill: its stitching has nothing to do and does run to its end)
                assert!(!r.warnings.iter().any(|w| w.contains("the pour stitching ran to its end")), "{case}: {:?}", r.warnings);
            }
            if board == "eprom" {
                assert!(r.routed < r.connections_total, "{case}: the cut leaves connections undrawn (premise)");
            } else {
                assert!(cut_line("the pour stitching was cut"), "{case}: the stitching is what the limit cut (premise): {:?}", r.warnings);
            }
        }
    }
}

/// Lead 2026-11-18 (limit-wait's bounded waits meet stable-label's "free the slot"): a stable run
/// past the time limit never gives its compute slot away (the yield waits for a slot again,
/// unbounded by `[run] limit_grace_s`). Two engine threads, three runs: run 3 waits for its first
/// slot while a loop is stable; the limit is reached at that run's yield check, so no yield.
#[test]
fn a_stable_run_past_the_limit_keeps_its_slot() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    let o = Options::from_json(r#"{"runs": 3, "wall_clock_s": 300, "stop_when_stable": false, "topo_rounds": 100000}"#).expect("options");
    let mut ctl = RunControl::new();
    let (l2, once) = (ctl.limit.clone(), Arc::new(AtomicBool::new(false)));
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    ctl.set_test_hook(move |p| {
        if p == "stable yield check" && !once.swap(true, Ordering::SeqCst) {
            l2.reach(); // the limit passes while the run is stable and run 3 waits for a slot
        }
        if p.starts_with("stable ") {
            s2.lock().unwrap().push(p.to_string());
        }
        false
    });
    let r = {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().expect("pool");
        pool.install(|| route_with(&eprom(), &o, &ctl))
    };
    let seen = seen.lock().unwrap().clone();
    assert!(seen.first().is_some_and(|p| p == "stable yield check"), "premise: a stable run reached its yield check: {seen:?}");
    assert!(!seen.iter().any(|p| p == "stable slot given"), "a yield past the limit: {seen:?}");
    assert!(!r.warnings.iter().any(|w| w.contains("gave its compute slot")), "{:?}", r.warnings);
    assert_eq!(r.stop_reason, "timeout", "{:?}", r.warnings);
}
