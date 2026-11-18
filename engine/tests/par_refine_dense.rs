//! par-refine (docs/design/47-parallel-refine.md §6): a dense generated board on which every
//! layer refinement of the route is checked against the serial reference (`refine::shadow`: the
//! labels, the model arrays, the end-site memo, the evaluations and the candidate stream with
//! every price change and keep decision, bit for bit), for one lane with a speculation window,
//! for several lanes, and for several lanes with a window; then a Cancel inside a long epoch
//! must end the refinement at the next candidate (`refine_par::test_cancel`).
//!
//! The board: 4 signal layers, `N` two-pad nets from a left to a right pad column in reversed
//! order (every pair of nets crosses), squeezed through a gap between two rows of foreign pads
//! in the middle, so the edges of the gap are over-full on every layer, the events have several
//! layer moves, and the refinement keeps and rejects many moves around shared via sites.

use std::sync::atomic::Ordering::Relaxed;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};
use topor_engine::topo::refine::shadow;
use topor_engine::topo::refine_par::{stats, test_cancel};

const MM: f64 = 1e6;

/// The generated board (see the module doc): `n` nets, a gap of `gap` mm.
fn dense(n: usize, gap: f64) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let ids = [0u32, 4, 6, 2];
    let (w, h) = (14.0, 2.0 + n as f64 * 0.7);
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let lj: Vec<String> = ids.iter().map(|id| format!(r#"{{"id":{id},"name":"L{id}","kind":"signal"}}"#)).collect();
    let idl = format!("[{}]", ids.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
    let shapes = format!("{{{}}}", ids.iter().map(|id| format!(r#""{id}":{{"kind":"circle","d":{}}}"#, p(0.5))).collect::<Vec<_>>().join(","));
    let mut pads: Vec<(usize, f64, f64)> = Vec::new();
    for i in 0..n {
        let y = 1.0 + i as f64 * 0.7;
        pads.push((i + 1, 1.0 + (i % 2) as f64 * 0.6, y));
        pads.push((i + 1, w - 1.0 - (i % 2) as f64 * 0.6, h - y));
    }
    // two rows of foreign pads (one net each: obstacles, nothing to route) leave a gap of `gap`
    // mm around the middle
    let (cy, xm) = (h / 2.0, w / 2.0);
    let mut y = cy - gap / 2.0 - 0.25;
    while y > 0.4 {
        pads.push((n + 1, xm, y));
        pads.push((n + 1, xm + 1.2, y + 0.3));
        y -= 0.6;
    }
    let mut y = cy + gap / 2.0 + 0.25;
    while y < h - 0.4 {
        pads.push((n + 1, xm, y));
        pads.push((n + 1, xm + 1.2, y - 0.3));
        y += 0.6;
    }
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| {
            let net = if net > n { n + 1 + i } else { net };
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth","center":[{cx},{cy}],
                    "rotation_deg":0.0,"layers":{idl},"shapes":{shapes},"drill":{{"w":{dr},"h":{dr}}}}}"#,
                cx = p(x),
                cy = p(y),
                dr = p(0.3)
            )
        })
        .collect();
    let nets: Vec<String> = (1..=n + 1 + pads.len()).map(|k| format!(r#"{{"code":{k},"name":"N{k}"}}"#)).collect();
    let s = format!(
        r#"{{"units":"nm","board":"dense","layers":[{}],"nets":[{}],"pads":[{}],"outline":[{}],"rules":{{}}}}"#,
        lj.join(","),
        nets.join(","),
        pj.join(","),
        segs.join(",")
    );
    let mut m = BoardModel::from_json(&s).expect("generated board parses");
    m.rules.edge_clearance = 0;
    m
}

/// The TopoR method's settings, headless (as tests/par_refine.rs), ending after
/// the plan (the refinements are what is tested, the drawing is not); `extra` adds keys.
fn opts(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "runs": 1, "stop_after": "plan"}"#)
            .expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
}

fn run(m: &BoardModel, o: &Options) -> RouteResult {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    route_with(m, o, &ctl)
}

fn counts() -> [u64; 5] {
    [shadow::CHECKED.load(Relaxed), shadow::CHECKED_MOVED.load(Relaxed), stats::EPOCHS.load(Relaxed), stats::WASTED.load(Relaxed), stats::REDONE.load(Relaxed)]
}

#[test]
fn every_refinement_of_a_dense_board_equals_the_serial_one() {
    shadow::ON.store(true, Relaxed);
    let c_start = counts();
    // one lane with a window (speculation past kept moves, without the timing) and three lanes
    // with a window (the pool's races on top); tests/par_refine.rs has the plain lane counts.
    // The narrow gap (0.8 mm) makes kept moves that empty an edge-layer while passes of it are
    // left (the cursor's over-full test); the wider one (1.2 mm) long epochs and many memo fills.
    let w1 = r#", "refine_lanes": 1, "refine_test_window": 64"#;
    let w3 = r#", "refine_lanes": 3, "refine_test_window": 7"#;
    let cases: &[(usize, f64, &str)] = if cfg!(debug_assertions) { &[(10, 0.8, w1), (12, 1.2, w1)] } else { &[(10, 0.8, w1), (12, 1.2, w1), (12, 1.2, w3)] };
    for &(n, gap, extra) in cases {
        let m = dense(n, gap);
        let c0 = counts();
        let t0 = std::time::Instant::now();
        let r = run(&m, &opts(extra));
        let c1 = counts();
        let d: Vec<u64> = c1.iter().zip(&c0).map(|(a, b)| a - b).collect();
        eprintln!("{n} nets, gap {gap}{extra}: {:.1} s, routed {}/{}, vias {}; refinements checked {} (with kept moves {}), epochs {}, wasted {}, redone {}", t0.elapsed().as_secs_f64(), r.routed, r.routed + r.failed, r.vias, d[0], d[1], d[2], d[3], d[4]);
        assert!(d[1] > 0, "{n} nets, gap {gap}{extra}: no checked refinement kept a move");
        assert!(d[3] > 0, "{n} nets, gap {gap}{extra}: no candidate was evaluated past a kept one");
    }
    shadow::ON.store(false, Relaxed);
    let c_end = counts();
    // the board makes the end-site memo matter: candidates of one epoch filled a key differently
    assert!(c_end[4] > c_start[4], "no end-site fill was evaluated again: the board tests no memo order");

    // A Cancel inside an epoch (one lane, no window: an epoch runs to the first kept move, here
    // hundreds of candidates) ends the refinement at the next candidate: the run thread's lane
    // evaluated exactly the candidates up to the Cancel, none after it.
    let m = dense(12, 1.2);
    const AT: u64 = 40;
    let ctl = RunControl::new();
    ctl.limit.init(86400.0);
    *test_cancel::FLAG.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ctl.cancel.clone());
    test_cancel::SEEN.store(0, Relaxed);
    test_cancel::AT.store(AT, Relaxed);
    let t0 = std::time::Instant::now();
    let r = route_with(&m, &opts(r#", "refine_lanes": 1"#), &ctl);
    eprintln!("cancel at candidate {AT}: {:.1} s", t0.elapsed().as_secs_f64());
    test_cancel::AT.store(0, Relaxed);
    *test_cancel::FLAG.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    assert!(ctl.cancel.load(Relaxed), "the hook did not fire: fewer than {AT} candidates");
    assert_eq!(test_cancel::SEEN.load(Relaxed), AT, "candidates evaluated after the Cancel (routed {})", r.routed);

    // The shadow's own negative test: a change of any compared item in the reference's result
    // (`shadow::FAULT`) makes the comparison panic naming that item. A panic hook keeps the
    // messages (a panic in a lane thread reaches the caller with another payload).
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keep = seen.clone();
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.payload().downcast_ref::<String>().cloned().or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
        keep.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(msg);
    }));
    let m = dense(10, 0.8);
    let mut missed = Vec::new();
    shadow::ON.store(true, Relaxed);
    for (k, field) in shadow::FIELDS.iter().enumerate().take(11) {
        seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        shadow::FAULT.store(k + 1, Relaxed);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&m, &opts(w1))));
        let msgs = seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let want = format!("the lanes differ from the serial refinement in {field} (");
        if r.is_ok() || !msgs.iter().any(|s| s.contains(&want)) {
            missed.push(format!("{field}: {}", if r.is_ok() { "no panic".to_string() } else { format!("{msgs:?}") }));
        }
    }
    shadow::FAULT.store(0, Relaxed);
    shadow::ON.store(false, Relaxed);
    std::panic::set_hook(old_hook);
    assert!(missed.is_empty(), "the shadow missed a changed item: {missed:?}");
}


