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

fn opts(grace: f64, reduce: bool, realise: &str) -> Options {
    Options::from_json(&format!(
        r#"{{"timeout_scope": "all", "wall_clock_s": 86400.0, "timeout_overrun_s": "none", "opt_loop": "topor", "initial": "once", "runs": 1, "topo_rounds": 1, "drawing_grace_s": {grace}, "via_reduce": {reduce}, "realise": "{realise}"}}"#
    ))
    .expect("options parse")
}

/// A run with the limit reached at the hook point `at` (`None`: no limit reached).
fn run(o: &Options, at: Option<&'static str>) -> RouteResult {
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (l2, once) = (ctl.limit.clone(), Arc::new(AtomicBool::new(false)));
    ctl.set_test_hook(move |p| {
        if at == Some(p) && !once.swap(true, Ordering::SeqCst) {
            l2.reach();
        }
        false
    });
    route_with(&board(), o, &ctl)
}

/// The copper as a sorted multiset of debug strings (order-free).
fn copper(r: &RouteResult) -> Vec<String> {
    let mut v: Vec<String> = r.items.iter().map(|i| format!("{i:?}")).collect();
    v.sort();
    v
}

/// The pour stitching's line without its timing.
fn stitch_line(r: &RouteResult) -> String {
    r.warnings.iter().find(|w| w.starts_with("pour stitching:")).map(|w| w.split(" (").next().unwrap_or("").to_string()).unwrap_or_default()
}

fn statuses(r: &RouteResult) -> Vec<(i32, String)> {
    let mut v: Vec<(i32, String)> = r.connections.iter().map(|c| (c.net, c.status.clone())).collect();
    v.sort();
    v
}

#[test]
fn the_fixture_has_vias_to_reduce_and_pour_pieces_to_stitch() {
    let r = run(&opts(60.0, false, "sequential"), None);
    assert_eq!(r.routed, r.connections_total, "{:?}", r.connections);
    assert!(r.vias > 0, "the crossing nets need a via: {}", r.vias);
    assert!(r.warnings.iter().any(|w| w.starts_with("pour stitching:")), "the top copper cuts the pour: {:?}", r.warnings);
}

/// grace 0, limit reached just before the via reduction: the reduction stops at its first check, so
/// the copper is exactly the copper as drawn (the run without the reduction): every wire, the same
/// pour stitching (no island lost), nothing else changed.
#[test]
fn grace_zero_stops_only_the_via_reduction_and_never_a_wire_or_an_island() {
    let as_drawn = run(&opts(-1.0, false, "sequential"), Some("via reduction begin"));
    let bounded = run(&opts(0.0, true, "sequential"), Some("via reduction begin"));
    let l = bounded.limit.as_ref().expect("limit report");
    assert!(l.reached && l.draw_grace_hit && l.draw_grace_s == Some(0.0) && l.cut == "none", "{l:?}");
    assert!(bounded.warnings.iter().any(|w| w.starts_with("via reduction after drawing") && w.contains("stopped by [run] drawing_grace_s")), "{:?}", bounded.warnings);
    assert_eq!(statuses(&bounded), statuses(&as_drawn), "every wire is drawn");
    assert_eq!(bounded.routed, bounded.connections_total);
    assert_eq!(stitch_line(&bounded), stitch_line(&as_drawn), "the stitching is not cut");
    assert!(!stitch_line(&bounded).is_empty());
    assert_eq!(copper(&bounded), copper(&as_drawn), "only the via reduction was left out");
    assert!(!bounded.timed_out, "a bounded reduction is no cut-short result");
    let w = bounded.warnings.iter().find(|w| w.starts_with("time limit wait:")).unwrap_or_else(|| panic!("{:?}", bounded.warnings));
    assert!(w.contains("(part of the drawing)") && w.contains("drawing_grace_s = 0 s ran out"), "{w}");
}

/// Any negative value = no bound: the reduction runs to its end, the same copper as with no limit.
#[test]
fn a_negative_grace_lets_the_via_reduction_finish() {
    let free = run(&opts(60.0, true, "sequential"), None);
    for g in [-1.0, -5.0] {
        let r = run(&opts(g, true, "sequential"), Some("via reduction begin"));
        let l = r.limit.as_ref().expect("limit report");
        assert!(l.reached && !l.draw_grace_hit && l.draw_grace_s.is_none(), "{g}: {l:?}");
        assert_eq!(copper(&r), copper(&free), "{g}: unbounded = the run without a limit");
        assert!(r.warnings.iter().any(|w| w.starts_with("time limit wait:") && w.contains("drawing_grace_s is unbounded")), "{g}: {:?}", r.warnings);
    }
}

/// Without a limit the bound changes nothing (read only once the limit passed).
#[test]
fn without_a_limit_the_drawing_bound_changes_nothing() {
    let (a, b, c) = (run(&opts(60.0, true, "sequential"), None), run(&opts(0.0, true, "sequential"), None), run(&opts(-5.0, true, "sequential"), None));
    assert_eq!(copper(&a), copper(&b));
    assert_eq!(copper(&a), copper(&c));
    assert!(a.limit.as_ref().is_some_and(|l| !l.reached && !l.draw_grace_hit));
}

/// ERBS (opt-in) uses the drawing bound as its deadline in soft mode: grace 0 with the limit reached
/// before the drawing sends every stretch the sequential way (counted), every wire still drawn.
#[test]
fn erbs_stretches_past_the_bound_are_drawn_the_sequential_way() {
    let free = run(&opts(60.0, false, "erbs"), Some("loop ended"));
    let bounded = run(&opts(0.0, false, "erbs"), Some("loop ended"));
    let l = bounded.limit.as_ref().expect("limit report");
    assert!(l.reached && l.draw_grace_hit && l.draw_grace_erbs > 0, "{l:?} {:?}", bounded.warnings.iter().filter(|w| w.to_lowercase().contains("erbs")).collect::<Vec<_>>());
    assert_eq!(statuses(&bounded), statuses(&free), "every wire is drawn");
    assert_eq!(stitch_line(&bounded).is_empty(), stitch_line(&free).is_empty());
    assert!(free.limit.as_ref().is_some_and(|l| l.draw_grace_erbs == 0), "60 s is room enough: {:?}", free.limit);
    let w = bounded.warnings.iter().find(|w| w.starts_with("time limit wait:")).unwrap_or_else(|| panic!("{:?}", bounded.warnings));
    assert!(w.contains("drawn the sequential way instead of by their ERBS"), "{w}");
    // the warning behind ERBS_TIMEOUT names the bound that cut it, not the time limit alone
    let et = bounded.warnings.iter().find(|w| w.starts_with("erbs_timeout:")).unwrap_or_else(|| panic!("{:?}", bounded.warnings));
    assert!(et.contains("[run] drawing_grace_s") && !et.starts_with("erbs_timeout: the time limit passed"), "{et}");
}

/// An overrun cap that fires while ERBS runs (soft at its start) stops it, also with an unbounded
/// drawing grace: before, the deadline was read once and ERBS ran on to its end.
#[test]
fn an_overrun_cap_that_fires_during_erbs_stops_it() {
    let o = Options::from_json(
        r#"{"timeout_scope": "all", "wall_clock_s": 86400.0, "timeout_overrun_s": 1.5, "opt_loop": "topor", "initial": "once", "runs": 1, "topo_rounds": 1, "drawing_grace_s": -1.0, "via_reduce": false, "realise": "erbs"}"#,
    )
    .expect("options parse");
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (l2, once, nap) = (ctl.limit.clone(), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    ctl.set_test_hook(move |p| {
        if p == "loop ended" && !once.swap(true, Ordering::SeqCst) {
            l2.reach();
        }
        if p == "fail erbs sleeve" && !nap.swap(true, Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1800)); // the cap (1.5 s past the limit) passes before ERBS starts its work
        }
        false
    });
    let r = route_with(&board(), &o, &ctl);
    let et = r.warnings.iter().find(|w| w.starts_with("erbs_timeout:")).unwrap_or_else(|| panic!("{:?}", r.warnings));
    assert!(et.contains("overrun cap"), "{et}");
}
