//! last-preview (docs/dev/design/last-preview.md, user 2026-11-19 "I would like the last round
//! result to be displayed"): the run publishes sketches of its last round, of each run's pick and
//! the loop cycle's own predicted score for the progress dialog. The hooks only read the state:
//! the copper and the connections are the same bits with sketches enabled (and asked for at any
//! moment, from another thread) as without, at every thread count. Without `enable` nothing is
//! built. A plan sketch is in the KiCad frame, as the result's items.
//!
//! Board: eprom-to-kernal (2 layers), two runs, six TopoR loop cycles, headless.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use topor_engine::control::{RunControl, Sketch};
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

/// A run's pick sketch with the Last run row's (errors, vias, length) at the same moment.
type PickAt = (Sketch, (usize, usize, f64));
/// A last sketch with the hook point it was read at and the Last run row's numbers then.
type MidSeen = (Sketch, String, (usize, usize, f64));

const EPROM: &str = "eprom-to-kernal-ripped.json";

fn fixture_text(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).expect("fixture")
}

/// Headless, two runs, six loop cycles; the wall limit is a day (nothing here may be cut by the clock).
fn opts() -> Options {
    Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 6, "runs": 2}"#).expect("options parse")
}

/// The copper and the connections.
fn key(r: &RouteResult) -> String {
    format!("{}\n{}", serde_json::to_string(&r.items).expect("items"), serde_json::to_string(&r.connections).expect("connections"))
}

/// The route on `threads` threads; `sketches` = enabled, with a thread asking for a last-round
/// sketch every 200 us while it runs (the plugin asks every progress_preview_interval_s).
fn run(m: &BoardModel, o: &Options, threads: usize, sketches: bool) -> (RouteResult, RunControl) {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    if sketches {
        ctl.sketches.enable();
    }
    let done = Arc::new(AtomicBool::new(false));
    let asker = {
        let (c, d) = (ctl.clone(), done.clone());
        std::thread::spawn(move || {
            while !d.load(Ordering::Relaxed) {
                c.sketches.request();
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        })
    };
    let r = pool.install(|| route_with(m, o, &ctl));
    done.store(true, Ordering::Relaxed);
    asker.join().expect("asker");
    (r, ctl)
}

/// Every point of the sketch's items (KiCad frame, nm).
fn points(sk: &Sketch) -> Vec<(i64, i64)> {
    fn pt(v: &serde_json::Value) -> Option<(i64, i64)> {
        match v {
            serde_json::Value::Array(a) => Some((a.first()?.as_i64()?, a.get(1)?.as_i64()?)),
            serde_json::Value::Object(o) => Some((o.get("x")?.as_i64()?, o.get("y")?.as_i64()?)),
            _ => None,
        }
    }
    let mut out = Vec::new();
    for it in serde_json::to_value(&sk.items).expect("items").as_array().expect("array") {
        if let Some(p) = it.get("at").and_then(pt) {
            out.push(p);
        }
        if let Some(prim) = it.get("prim") {
            for k in ["a", "b", "start", "mid", "end"] {
                if let Some(p) = prim.get(k).and_then(pt) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// The bounding box of the pad centres in the model JSON as written (the KiCad frame).
fn pad_box(text: &str) -> (i64, i64, i64, i64) {
    let v: serde_json::Value = serde_json::from_str(text).expect("json");
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for p in v["pads"].as_array().expect("pads") {
        let c = &p["center"];
        let (x, y) = match c {
            serde_json::Value::Array(a) => (a[0].as_i64().expect("x"), a[1].as_i64().expect("y")),
            _ => (c["x"].as_i64().expect("x"), c["y"].as_i64().expect("y")),
        };
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    (x0, y0, x1, y1)
}

#[test]
fn copper_is_the_same_with_sketches_enabled_at_every_thread_count() {
    let text = fixture_text(EPROM);
    let m = BoardModel::from_json(&text).expect("fixture parses");
    let o = opts();
    let (r0, c0) = run(&m, &o, 1, false);
    let reference = key(&r0);
    // never enabled: nothing is built, but the loop cycles still reach the progress
    let p0 = c0.snapshot();
    assert_eq!(p0.sketch_seq, 0, "sketches built without enable");
    assert!(c0.sketches.get(false, 0).is_none() && c0.sketches.get(true, 1).is_none());
    assert!(p0.loop_cycle > 0 && p0.loop_run > 0 && p0.loop_at_s > 0.0, "loop cycle not reported: {p0:?}");
    assert!(p0.runbest_run > 0 && !p0.runbest_kind.is_empty(), "run pick not reported");
    for threads in [1usize, 2, 4] {
        let (r, ctl) = run(&m, &o, threads, true);
        assert!(key(&r) == reference, "{threads} threads with sketches: copper differs from the run without");
        let p = ctl.snapshot();
        assert!(p.sketch_seq > 0, "{threads} threads: no sketch published");
        let last = ctl.sketches.get(false, 0).expect("a last sketch");
        assert!(!last.items.is_empty(), "the last sketch is empty");
        // the run ends with the drawing: the last round is drawn copper
        assert_eq!(last.kind, "copper", "{threads} threads: the last sketch after the drawing");
        // the best's sketch is the best's own result (the pick of the best run)
        let best = ctl.sketches.get(true, p.best_run).expect("a best sketch");
        assert_eq!((best.errors, best.vias, best.run), (p.best_errors, p.best_vias, p.best_run), "best sketch vs progress best");
        // the Last run row's sketch is that run's pick
        let rb = ctl.sketches.get(true, p.runbest_run).expect("a run sketch");
        assert_eq!((rb.errors, rb.vias), (p.runbest_errors, p.runbest_vias), "run sketch vs progress runbest");
        assert!(last.seq <= p.sketch_seq && best.seq <= p.sketch_seq);
        // the last sketch is the copper the last round was judged by (its Last round row)
        if last.run == p.last_run {
            assert_eq!((last.errors, last.vias, last.length_mm), (p.last_errors, p.last_vias, p.last_length_mm), "{threads} threads: last sketch vs the Last round row");
        }
    }
}

#[test]
fn plan_sketches_are_in_the_kicad_frame_and_follow_the_loop() {
    let text = fixture_text(EPROM);
    let m = BoardModel::from_json(&text).expect("fixture parses");
    let mut o = opts();
    o.runs = 1;
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    ctl.sketches.enable();
    // the test hook stops at the loop's end: the sketches of the loop, before the drawing replaces them
    let seen: Arc<std::sync::Mutex<Vec<Sketch>>> = Arc::default();
    // the run's pick sketch at the loop's end and the Last run row then
    let picks: Arc<std::sync::Mutex<Vec<PickAt>>> = Arc::default();
    let mut c2 = ctl.clone();
    {
        let (c3, s2, p2) = (ctl.clone(), seen.clone(), picks.clone());
        c2.set_test_hook(move |point| {
            if point == "loop ended" {
                if let Some(sk) = c3.sketches.get(false, 0) {
                    s2.lock().expect("seen").push((*sk).clone());
                }
                if let Some(sk) = c3.sketches.get(true, 1) {
                    let p = c3.snapshot();
                    p2.lock().expect("picks").push(((*sk).clone(), (p.runbest_errors, p.runbest_vias, p.runbest_length_mm)));
                }
            } else {
                c3.sketches.request();
            }
            false
        });
    }
    let _ = route_with(&m, &o, &c2);
    let seen = seen.lock().expect("seen");
    let sk = seen.first().expect("a last sketch at the loop's end");
    assert_eq!(sk.kind, "plan", "in the loop the last sketch is the plan");
    assert!(!sk.items.is_empty(), "the plan sketch has no items");
    let (x0, y0, x1, y1) = pad_box(&text);
    let pad = 15_000_000; // the plan stays within 15 mm of the pads
    for (x, y) in points(sk) {
        assert!(x >= x0 - pad && x <= x1 + pad && y >= y0 - pad && y <= y1 + pad, "plan point ({x}, {y}) outside the pads' box ({x0}, {y0})-({x1}, {y1}): wrong frame?");
    }
    let p = ctl.snapshot();
    assert!(sk.cycle >= 1 && sk.cycle <= p.loop_cycle.max(6), "plan sketch cycle {} (loop cycle {})", sk.cycle, p.loop_cycle);
    // the Last run / best sketch is the run's pick (not its end state when an earlier state is the pick)
    let picks = picks.lock().expect("picks");
    let (pk, row) = picks.first().expect("a pick sketch at the loop's end");
    assert_eq!(pk.kind, "plan");
    assert_eq!((pk.errors, pk.vias, pk.length_mm), *row, "pick sketch vs the Last run row");
}

/// User 2026-11-19 ("the run/round displayed goes back to 1 / 1 after a while"): with the TopoR
/// loop the whole-route round is 1 for every run and the best's run flips between runs, so the
/// dialog needs a count that never goes back: `cycles_total` grows by one per finished loop cycle
/// of any run, the per-run pick cycles and the best's kind are reported.
#[test]
fn the_cycle_count_never_goes_back() {
    let text = fixture_text(EPROM);
    let m = BoardModel::from_json(&text).expect("fixture parses");
    let o = opts();
    let seen: Arc<std::sync::Mutex<Vec<u64>>> = Arc::default();
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let mut c2 = ctl.clone();
    {
        let (c3, s2) = (ctl.clone(), seen.clone());
        c2.set_test_hook(move |_| {
            s2.lock().expect("seen").push(c3.snapshot().cycles_total);
            false
        });
    }
    let _ = route_with(&m, &o, &c2);
    let seen = seen.lock().expect("seen");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "cycles_total went back");
    let p = ctl.snapshot();
    // two runs of six loop cycles each (headless: the cap), no cycle cut
    assert!(p.cycles_total >= 2 && p.cycles_total <= 12, "cycles_total {}", p.cycles_total);
    assert_eq!(p.pick_cycles.len(), 2, "pick cycles per run: {:?}", p.pick_cycles);
    // the Last run row's round is its run's pick cycle (the plugin shows pick_cycles for the best's round)
    assert!(p.runbest_cycle > 0 && p.pick_cycles[(p.runbest_run as usize).saturating_sub(1)] == p.runbest_cycle, "pick cycles {:?} vs Last run {}:{}", p.pick_cycles, p.runbest_run, p.runbest_cycle);
    assert_eq!(p.best_kind, "copper", "after the drawing the best is drawn");
}

/// The Last run / best sketch is the run's pick also when the pick is a state from inside a cycle
/// (TW5 escape offers: `escape = "layering"`, `loop_p4`), not the cycle's end state: at every
/// hook point of the loop the run's pick sketch has the Last run row's score.
#[test]
fn the_pick_sketch_is_the_pick_also_for_mid_cycle_states() {
    for fixture in [EPROM, "esp-rust-board-usb-crop.json"] {
        let text = fixture_text(fixture);
        let m = BoardModel::from_json(&text).expect("fixture parses");
        let o = Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 12, "runs": 1, "refine_lanes": 1, "escape": "layering", "loop_p4": true}"#).expect("options parse");
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        ctl.sketches.enable();
        let seen: Arc<std::sync::Mutex<Vec<MidSeen>>> = Arc::default();
        let mut c2 = ctl.clone();
        {
            let (c3, s2) = (ctl.clone(), seen.clone());
            c2.set_test_hook(move |_| {
                if let Some(sk) = c3.sketches.get(true, 1) {
                    let p = c3.snapshot();
                    s2.lock().expect("seen").push(((*sk).clone(), p.runbest_kind.clone(), (p.runbest_errors, p.runbest_vias, p.runbest_length_mm)));
                }
                false
            });
        }
        let _ = route_with(&m, &o, &c2);
        let seen = seen.lock().expect("seen");
        let plan: Vec<_> = seen.iter().filter(|(sk, kind, _)| sk.kind == "plan" && kind == "plan").collect();
        assert!(!plan.is_empty(), "{fixture}: no plan pick sketch seen");
        for (sk, _, row) in plan {
            assert_eq!((sk.errors, sk.vias, sk.length_mm), *row, "{fixture}: pick sketch of cycle {} vs the Last run row", sk.cycle);
        }
    }
}

/// The last round's sketch after the drawing is the copper variant the round was judged by (the
/// better of "as drawn" and "reduced + fused"), with the Last round row's score; on the esp USB
/// crop the variants differ.
#[test]
fn the_last_sketch_is_the_judged_copper() {
    let text = fixture_text("esp-rust-board-usb-crop.json");
    let m = BoardModel::from_json(&text).expect("fixture parses");
    let o = Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "runs": 1, "refine_lanes": 1}"#).expect("options parse");
    let (_, ctl) = run(&m, &o, 2, true);
    let p = ctl.snapshot();
    let last = ctl.sketches.get(false, 0).expect("a last sketch");
    assert_eq!(last.kind, "copper", "the last sketch after the drawing");
    assert_eq!((last.errors, last.vias, last.length_mm), (p.last_errors, p.last_vias, p.last_length_mm), "last sketch vs the Last round row");
}

/// verify-last-preview F4: the dialog's restart count (`Progress::loop_restarts`, shown in the round
/// context "run 2, restart 3, round 17") is the count the loop's own report names ("N restarts from
/// the archive"); a stall restart config (`restart = "stall"`, `loop_restart = 2`) makes it > 0.
#[test]
fn the_progress_restart_count_is_the_loops_restart_count() {
    let m = BoardModel::from_json(&fixture_text(EPROM)).expect("fixture parses");
    let o = Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 30, "runs": 1, "restart": "stall", "loop_restart": 2}"#).expect("options parse");
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let seen: Arc<std::sync::Mutex<Vec<u32>>> = Arc::default();
    let mut c2 = ctl.clone();
    {
        let (c3, s2) = (ctl.clone(), seen.clone());
        c2.set_test_hook(move |_| {
            s2.lock().expect("seen").push(c3.snapshot().loop_restarts);
            false
        });
    }
    let r = route_with(&m, &o, &c2);
    let line = r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).expect("the loop line");
    let head = line.split(" restarts from the archive").next().expect("head");
    let n: u32 = head.rsplit("; ").next().expect("count").parse().unwrap_or_else(|_| panic!("restarts in: {line}"));
    assert!(n > 0, "the config must restart at least once: {line}");
    let seen = seen.lock().expect("seen");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "loop_restarts went back: {seen:?}");
    assert_eq!(seen.iter().copied().max().unwrap_or(0), n, "progress {seen:?} vs the loop report: {line}");
    assert_eq!(ctl.snapshot().loop_restarts, n);
}

/// preview-errors (E12): a drawn round's sketch carries the places of that round's rule violations (the yellow dots of
/// the progress dialog). `channel_overfull` (4 wires through a slot that fits 2.00 mm, short by 0.40) is drawn with
/// violations (never drop); the run's best sketch is the result's copper, so its `errors_at` are the result's
/// `violations[].at`, in order; the last sketch is a drawn round's, and a plan sketch has none.
#[test]
fn drawn_sketches_carry_the_places_of_their_violations() {
    let path = format!("{}/tests/fixtures/fit/channel_overfull.json", env!("CARGO_MANIFEST_DIR"));
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
    let m = BoardModel::from_json(&doc["model"].to_string()).expect("model");
    let o = Options::from_json(r#"{"runs": 1, "wall_clock_s": 120.0, "topo_rounds": 1, "capacity_model": "cuts"}"#).expect("options");
    let (r, ctl) = run(&m, &o, 1, true);
    let at: Vec<[i64; 2]> = r.violations.iter().map(|v| v.at).collect();
    assert!(!at.is_empty(), "the fixture must be drawn with violations, else this test proves nothing");
    let p = ctl.snapshot();
    let best = ctl.sketches.get(true, p.best_run).expect("a best sketch");
    assert_eq!(best.kind, "copper");
    assert_eq!(best.errors_at, at, "best sketch vs the result's violations");
    let last = ctl.sketches.get(false, 0).expect("a last sketch");
    assert_eq!(last.kind, "copper");
    assert_eq!(last.errors_at.len(), last.violations, "last sketch: one place per violation of its round");
    assert!(!last.errors_at.is_empty());
}

/// plan-dots (user 2026-11-21: "I only see the errors when the routing is rendered to copper"): the
/// loop's plan sketches seen at the loop's end, with sketches asked for at every other hook point.
fn plan_sketches(m: &BoardModel, o: &Options) -> Vec<Sketch> {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    ctl.sketches.enable();
    let seen: Arc<std::sync::Mutex<Vec<Sketch>>> = Arc::default();
    let mut c2 = ctl.clone();
    {
        let (c3, s2) = (ctl.clone(), seen.clone());
        c2.set_test_hook(move |_| {
            for sk in [c3.sketches.get(false, 0), c3.sketches.get(true, 1)].into_iter().flatten() {
                let mut s = s2.lock().expect("seen");
                if sk.kind == "plan" && !s.iter().any(|x| x.seq == sk.seq) {
                    s.push((*sk).clone());
                }
            }
            c3.sketches.request();
            false
        });
    }
    let _ = route_with(m, o, &c2);
    let v = seen.lock().expect("seen").clone();
    v
}

fn channel_overfull() -> BoardModel {
    let path = format!("{}/tests/fixtures/fit/channel_overfull.json", env!("CARGO_MANIFEST_DIR"));
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("json");
    BoardModel::from_json(&doc["model"].to_string()).expect("model")
}

/// plan-dots: a plan sketch carries one place per predicted violation its plan counts. On
/// `channel_overfull` (4 wires through a slot between the pads at x = 4.15 and 6.05 mm, y = 6 mm
/// that fits 2) every model (per-edge edges, per-layer cuts) predicts the slot over-full, and a
/// dot lies in the slot's middle (x 5.1 +- 0.3 mm, y 6 +- 0.5 mm; the pads' facing sides are at
/// x 4.30 and 5.90, their corners at y 5.6 / 6.4: an edge or cut across it has its middle there).
#[test]
fn plan_sketches_carry_the_places_of_their_predicted_violations() {
    let m = channel_overfull();
    for model in ["edges", "cuts"] {
        let o = Options::from_json(&format!(r#"{{"runs": 1, "wall_clock_s": 120.0, "topo_rounds": 2, "opt_loop": "topor", "initial": "once", "capacity_model": "{model}"}}"#)).expect("options");
        let seen = plan_sketches(&m, &o);
        assert!(!seen.is_empty(), "{model}: no plan sketch");
        for sk in &seen {
            assert_eq!(sk.errors_at.len(), sk.violations, "{model}: cycle {}: one dot per predicted violation (no forced path here)", sk.cycle);
            assert!(sk.violations > 0, "{model}: cycle {}: the slot must be predicted over-full, else this test proves nothing", sk.cycle);
            let in_slot = sk.errors_at.iter().filter(|p| (p[0] - 5_100_000).abs() <= 300_000 && (p[1] - 6_000_000).abs() <= 500_000).count();
            assert!(in_slot > 0, "{model}: cycle {}: no dot in the slot: {:?}", sk.cycle, sk.errors_at);
        }
    }
}

/// plan-dots: on real crops (2 layers, vias, crossings, via cuts) every plan sketch of the loop
/// has as many dots as its plan's predicted violations (less the connections placed without a
/// legal path, which have no place: none on these crops with `initial = "once"` at this cap).
#[test]
fn every_plan_sketch_has_one_dot_per_predicted_violation() {
    let mut any = 0;
    for fixture in [EPROM, "esp-rust-board-usb-crop.json"] {
        let m = BoardModel::from_json(&fixture_text(fixture)).expect("fixture parses");
        let o = Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 4, "runs": 1, "refine_lanes": 1}"#).expect("options parse");
        let seen = plan_sketches(&m, &o);
        assert!(!seen.is_empty(), "{fixture}: no plan sketch");
        for sk in &seen {
            assert_eq!(sk.errors_at.len(), sk.violations, "{fixture}: cycle {}: dots vs predicted violations", sk.cycle);
            any += sk.violations;
        }
    }
    assert!(any > 0, "no plan predicted any violation: the test proves nothing");
}

/// plan-dots: a small board (KiCad frame, mm): `w` x `h` outline, `layers` signal layers (F.Cu,
/// then B.Cu), SMD rect pads `(net, x, y, w, h, layer id)`; nets 1..=3.
fn board(w: f64, h: f64, layers: usize, pads: &[(i32, f64, f64, f64, f64, i32)]) -> BoardModel {
    let p = |v: f64| (v * 1e6).round() as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4).map(|i| format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(c[i].0), p(c[i].1), p(c[(i + 1) % 4].0), p(c[(i + 1) % 4].1))).collect();
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y, pw, ph, l))| format!(r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,"layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#, p(x), p(y), p(pw), p(ph)))
        .collect();
    let ly = [r#"{"id":0,"name":"F.Cu","kind":"signal"}"#, r#"{"id":2,"name":"B.Cu","kind":"signal"}"#][..layers].join(",");
    let s = format!(
        r#"{{"units":"nm","board":"plandots","layers":[{ly}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}}],
            "pads":[{}],"keepouts":[],"outline":[{}],
            "rules":{{"min_clearance":200000,"min_track_width":250000,"min_via_diameter":600000,"min_via_drill":300000,
                     "min_annular":50000,"hole_clearance":250000,"hole_to_hole":250000,"edge_clearance":500000,
                     "default":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000}}}}}}"#,
        pj.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).expect("board")
}

fn loop_opts() -> Options {
    Options::from_json(r#"{"wall_clock_s": 120.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "runs": 1, "refine_lanes": 1}"#).expect("options parse")
}

/// plan-dots: a wall of another net's copper (net B, x 4.5..5.5 mm, the board's whole height, on
/// both layers) between A's two pads: A is placed through it (never drop: a connection without a legal
/// path, `forced`) and its traversal of B's copper is a predicted violation with its dot in the
/// wall. The forced connection itself is counted with no place: exactly one dot fewer than the
/// predicted violations.
#[test]
fn a_traversal_of_foreign_copper_has_its_dot_in_the_copper_and_the_forced_path_none() {
    let m = board(10.0, 10.0, 2, &[(1, 1.5, 5.0, 1.0, 1.0, 0), (1, 8.5, 5.0, 1.0, 1.0, 0), (2, 5.0, 5.0, 1.0, 10.0, 0), (2, 5.0, 5.0, 1.0, 10.0, 2)]);
    let seen = plan_sketches(&m, &loop_opts());
    assert!(!seen.is_empty(), "no plan sketch");
    for sk in &seen {
        assert_eq!(sk.errors_at.len() + 1, sk.violations, "cycle {}: one dot per predicted violation but the forced connection", sk.cycle);
        assert!(sk.errors_at.iter().any(|p| (p[0] - 5_000_000).abs() <= 500_000), "cycle {}: no dot in the wall: {:?}", sk.cycle, sk.errors_at);
    }
}

/// plan-dots: `board`'s connections planned by hand (each its shortest path, the loop's layer
/// assignment), the per-edge capacity model of it and the places of its predicted violations,
/// with the parts the measure counts: `(places, over-full of the model, unlayerable, foreign)`.
fn hand_plan(m: &BoardModel) -> (topor_engine::topo::compass::PlanPlaces, usize, usize, usize) {
    hand_plan_with(m, |_| {})
}

/// [`hand_plan`] with the layer report changed by `edit` before the places are taken.
fn hand_plan_with(m: &BoardModel, edit: impl FnOnce(&mut topor_engine::topo::layers::LayerReport)) -> (topor_engine::topo::compass::PlanPlaces, usize, usize, usize) {
    use topor_engine::topo::board::{self as tb, BuildOpts};
    use topor_engine::topo::init::InitWeights;
    use topor_engine::topo::layers::{self, LayerParams};
    use topor_engine::topo::optimise::{foreign_copper, layers_now};
    use topor_engine::topo::pack::Packer;
    use topor_engine::topo::refine::CapModel;
    use topor_engine::topo::state::State;
    let o = Options::default();
    let b = tb::build(m, &BuildOpts::default()).expect("build");
    let conns = b.connections(m);
    let mut st = State::new(&b.proj);
    for c in &conns {
        st.route_conn(c, &InitWeights::default(), (c.w + c.s) / 400_000.0).expect("routes");
    }
    let packer = Packer::for_model(m, &o, &b.proj);
    let smd = |ob: usize| b.smd_layer(ob);
    let mut lr = layers::assign_with(&st, &smd, &LayerParams::default(), Some(&packer));
    edit(&mut lr);
    let dirs = packer.taut_dirs(&st);
    let bottom = 1;
    let cm = CapModel::new(&st, &packer, &dirs, &lr.events, bottom, 1.0);
    let pl = topor_engine::topo::compass::plan_places(&st, &lr, &cm, bottom, 0, &[]);
    let fc = foreign_copper(&st, &layers_now(&st, &lr.events, bottom)).0;
    (pl, cm.overfull(), lr.unlayerable.len(), fc)
}

/// plan-dots: each crossing no layer separates has its dot at the crossing: the hand plan of a
/// board with two over-full edges (B's pads on B.Cu over net C's B.Cu cover), and the same plan
/// with one more such crossing at (3, 2) mm (KiCad frame) in its layer report: one dot more,
/// there; the rest unchanged.
#[test]
fn each_unlayerable_crossing_of_the_layer_report_has_a_dot_there() {
    let m = board(10.0, 6.0, 2, &[(1, 1.0, 3.0, 1.0, 1.0, 0), (1, 9.0, 3.0, 1.0, 1.0, 0), (2, 5.0, 1.0, 1.0, 1.0, 2), (2, 5.0, 5.0, 1.0, 1.0, 2), (3, 5.0, 3.0, 9.6, 5.6, 2)]);
    let (pl, over, unl, fc) = hand_plan(&m);
    assert_eq!(pl.at.len(), over + unl + fc, "one dot per counted violation: {pl:?}");
    assert!(over > 0, "the case shows: over-full edges");
    let (pl2, ..) = hand_plan_with(&m, |lr| lr.unlayerable.push(topor_engine::geom::Vec2 { x: 3e6, y: -2e6 }));
    assert_eq!(pl2.at.len(), pl.at.len() + 1);
    assert!(pl2.at.contains(&[3_000_000, 2_000_000]), "the crossing's dot at (3, 2) mm: {:?}", pl2.at);
    assert!(pl.at.iter().all(|p| pl2.at.contains(p)));
}
