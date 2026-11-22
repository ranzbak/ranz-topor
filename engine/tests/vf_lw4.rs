//! `[run] drawing_grace_s` (user 2026-11-17, B3): with the soft time limit, the drawing's optional
//! passes (the via reduction; ERBS with `realise = "erbs"`) stop once the drawing ran this long past
//! the later of the limit and its own start. Every wire is still drawn and the pour stitching is
//! never cut by it (never drop). Synthetic board: three crossing two-pin nets on two layers (vias for
//! the reduction) and a GND pour on F.Cu the top-layer copper cuts (pieces for the stitching).
//! time-limit-fix r2: these tests check the time limit inside the final drawing, which only
//! `[run] timeout_scope = "all"` counts (the default "optimizing" keeps it off the clock), so
//! their options set it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

const MM: f64 = 1e6;

/// 12 x 12 mm, two layers (F.Cu 0, B.Cu 2), THT pads `(net, x, y)` 1.4 mm at the board edge, nets 1-3
/// signals, net 4 GND with two pads and a pour on F.Cu over the whole board.
fn board() -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    // three chords from edge to edge that cross pairwise (an odd cycle: two layers need a via), the
    // GND pads in two corners on either side of the A chord
    let pads = [(1, 0.8, 6.0), (1, 11.2, 6.0), (2, 3.5, 0.8), (2, 8.5, 11.2), (3, 8.5, 0.8), (3, 3.5, 11.2), (4, 1.2, 1.2), (4, 10.8, 10.8)];
    let c = [(0.0, 0.0), (12.0, 0.0), (12.0, 12.0), (0.0, 12.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth","center":[{},{}],"rotation_deg":0.0,
                    "layers":[0,2],"shapes":{{"0":{{"kind":"circle","d":{d}}},"2":{{"kind":"circle","d":{d}}}}},"drill":{{"w":{dr},"h":{dr}}}}}"#,
                p(x),
                p(y),
                d = p(1.4),
                dr = p(0.7)
            )
        })
        .collect();
    let ring = format!("[[{},{}],[{},{}],[{},{}],[{},{}]]", p(0.5), p(0.5), p(11.5), p(0.5), p(11.5), p(11.5), p(0.5), p(11.5));
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}},{{"code":4,"name":"GND"}}],
            "pads":[{}],
            "pours":[{{"uuid":"z0","net":4,"layers":[0],"pads":["U6.1","U7.1"],"vias":[],"clearance":200000,"min_thickness":250000,
                "thermal_gap":500000,"spoke_width":500000,"pad_connection":2,"island_removal":0,"fills":[{{"layer":0,"outer":{ring}}}]}}],
            "outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        pj.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).expect("board parses")
}

/// ERBS run with the limit reached at "loop ended" and a nap of `nap_ms` before ERBS works.
fn erbs_run(grace: f64, cap: f64, nap_ms: u64) -> RouteResult {
    let o = Options::from_json(&format!(
        r#"{{"timeout_scope": "all", "wall_clock_s": 86400.0, "timeout_overrun_s": {cap}, "opt_loop": "topor", "initial": "once", "runs": 1, "topo_rounds": 1, "drawing_grace_s": {grace}, "via_reduce": false, "realise": "erbs"}}"#
    ))
    .expect("options parse");
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (l2, once, nap) = (ctl.limit.clone(), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    ctl.set_test_hook(move |p| {
        if p == "loop ended" && !once.swap(true, Ordering::SeqCst) {
            l2.reach();
        }
        if p == "fail erbs sleeve" && !nap.swap(true, Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(nap_ms));
        }
        false
    });
    route_with(&board(), &o, &ctl)
}

fn dump(tag: &str, r: &RouteResult) {
    eprintln!("VF4 {tag}: limit {:?}", r.limit);
    for w in r.warnings.iter().filter(|w| w.starts_with("time limit") || w.starts_with("erbs_timeout") || w.starts_with("WARNING") || w.contains("TIME")) {
        eprintln!("VF4 {tag}:   {w}");
    }
    if let Some(l) = &r.limit {
        eprintln!("VF4JSON {tag} {}", serde_json::to_string(l).unwrap_or_default());
    }
}

/// The cap cuts ERBS, drawing_grace_s unbounded: the report must not say drawing_grace_s ran out.
#[test]
fn vf_cap_cut_with_unbounded_grace_is_not_a_drawing_grace_hit() {
    let r = erbs_run(-1.0, 1.5, 1800);
    dump("A grace=-1 cap=1.5", &r);
    let l = r.limit.as_ref().expect("limit");
    assert!(r.warnings.iter().any(|w| w.starts_with("erbs_timeout:") && w.contains("overrun cap")), "{:?}", r.warnings);
    assert!(!l.draw_grace_hit && l.draw_grace_erbs == 0, "drawing_grace_s is unbounded, yet: {l:?}");
}

/// The cap (1.5 s) cuts ERBS before a 60 s drawing bound: no "drawing_grace_s = 60 s ran out" text.
#[test]
fn vf_cap_cut_before_a_long_grace_does_not_name_drawing_grace() {
    let r = erbs_run(60.0, 1.5, 1800);
    dump("B grace=60 cap=1.5", &r);
    let l = r.limit.as_ref().expect("limit");
    assert!(r.warnings.iter().any(|w| w.starts_with("erbs_timeout:") && w.contains("overrun cap")), "{:?}", r.warnings);
    let w = r.warnings.iter().find(|w| w.starts_with("time limit wait:")).cloned().unwrap_or_default();
    assert!(!w.contains("drawing_grace_s = 60 s ran out"), "{w}");
    assert!(!l.draw_grace_hit, "{l:?}");
}

/// drawing_grace_s = 0.3 cuts, no cap: names drawing_grace_s (sanity).
#[test]
fn vf_grace_only() {
    let r = erbs_run(0.3, 0.0, 800);
    dump("D grace=0.3 nocap", &r);
    assert!(r.warnings.iter().any(|w| w.starts_with("erbs_timeout:") && w.contains("drawing_grace_s")), "{:?}", r.warnings);
}
