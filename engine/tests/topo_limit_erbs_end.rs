//! limit-wait r6 (from the r5 verifier's grid, vf-limit5 264cc88): every ERBS end path (drawing bound / wall cap /
//! CPU harden / Cancel / hard / none) x drawing_grace_s (-1, 0, >0) x (soft, hard): result.json draw_grace_hit /
//! draw_grace_erbs, the erbs_timeout text (it names what stopped ERBS first, `erbs::ErbsStop`) and the engine's wait
//! line. The tiny 2-layer board routes in well under 1 s, so the round's 1 s floor (core.rs `floor`) is still ahead
//! when ERBS runs in the cases without a nap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

const MM: f64 = 1e6;

fn board() -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
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

#[derive(Clone, Copy, PartialEq, Debug)]
enum Act {
    Reach,
    Harden,
    Cancel,
}

/// What should have cut ERBS: `Bound` = drawing_grace_s, `Cap` = the overrun cap (wall or CPU),
/// `Hard` = the hard limit, `Cancel` = the user's Cancel, `Free` = nothing.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Want {
    Bound,
    Cap,
    Hard,
    Cancel,
    Free,
    /// The hard limit on the wall clock inside the round's 1 s floor: no ERBS cut yet.
    FloorKept,
}

struct Case {
    name: &'static str,
    hard: bool,
    wall: f64,
    grace: f64,
    cap: f64,
    lp: &'static str,
    /// at the TopoR loop's first stage check (the limit falls in the loop step)
    in_loop: &'static [Act],
    at_end: &'static [Act],
    in_nap: &'static [Act],
    nap_ms: u64,
    want: Want,
}

fn apply(l: &RunControl, acts: &[Act]) {
    for a in acts {
        match a {
            Act::Reach => l.limit.reach(),
            Act::Harden => {
                l.limit.reach();
                l.limit.harden()
            }
            Act::Cancel => l.request_cancel(),
        }
    }
}

fn run(c: &Case) -> RouteResult {
    let o = Options::from_json(&format!(
        r#"{{"wall_clock_s": {}, "timeout_mode": "{}", "timeout_overrun_s": {}, "opt_loop": "{}", "initial": "once", "runs": 1, "topo_rounds": 1, "drawing_grace_s": {}, "via_reduce": false, "realise": "erbs"}}"#,
        c.wall,
        if c.hard { "hard" } else { "soft" },
        c.cap,
        c.lp,
        c.grace
    ))
    .expect("options parse");
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    let (in_loop, at_end, in_nap, nap_ms) = (c.in_loop.to_vec(), c.at_end.to_vec(), c.in_nap.to_vec(), c.nap_ms);
    let (once, nap, lp) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    ctl.set_test_hook(move |p| {
        if p == "loop stage 1 check" && !lp.swap(true, Ordering::SeqCst) {
            apply(&c2, &in_loop);
        }
        if p == "loop ended" && !once.swap(true, Ordering::SeqCst) {
            apply(&c2, &at_end);
        }
        if p == "fail erbs sleeve" && !nap.swap(true, Ordering::SeqCst) {
            apply(&c2, &in_nap);
            std::thread::sleep(std::time::Duration::from_millis(nap_ms));
        }
        false
    });
    route_with(&board(), &o, &ctl)
}

fn check(c: &Case) {
    let r = run(c);
    let et = r.warnings.iter().find(|w| w.starts_with("erbs_timeout:")).cloned();
    let wl = r.warnings.iter().find(|w| w.starts_with("time limit wait:")).cloned();
    let l = r.limit.clone();
    eprintln!("{}: erbs_timeout {et:?}\n  wait {wl:?}\n  limit {l:?}\n  routed {}/{} runtime {:.3} s", c.name, r.routed, r.connections_total, r.runtime_s);
    let hit = l.as_ref().is_some_and(|l| l.draw_grace_hit);
    let n = l.as_ref().map_or(0, |l| l.draw_grace_erbs);
    let et_s = et.clone().unwrap_or_default();
    // the round's 1 s floor still ahead at ERBS (the cases without a nap): a cut then is the live check's
    let young = r.runtime_s < 0.9;
    match c.want {
        Want::Bound => {
            assert!(hit && n > 0, "{}: bound cut wanted, limit {l:?}", c.name);
            assert!(et_s.starts_with("erbs_timeout: [run] drawing_grace_s passed past the time limit before"), "{}: {et_s}", c.name);
        }
        Want::Cap => {
            assert!(!hit && n == 0, "{}: cap cut is no drawing_grace hit: {l:?}", c.name);
            assert!(et_s.starts_with("erbs_timeout: the overrun cap passed"), "{}: {et_s}", c.name);
        }
        Want::Hard => {
            assert!(!hit && n == 0, "{}: hard: {l:?}", c.name);
            assert!(et_s.starts_with("erbs_timeout: the time limit passed ([run] timeout_mode = hard)"), "{}: {et_s}", c.name);
        }
        Want::Cancel => {
            assert!(!hit && n == 0, "{}: a Cancel is no drawing_grace hit: {l:?}", c.name);
            assert!(et_s.starts_with("erbs_timeout: the run was cancelled before"), "{}: {et_s}", c.name);
        }
        Want::Free => {
            assert!(!hit && n == 0, "{}: {l:?}", c.name);
            assert!(et.is_none(), "{}: {et_s}", c.name);
        }
        Want::FloorKept => {
            // r5 finding 4, lead 2026-11-17: on the wall clock the round's 1 s floor holds for ERBS too (a wall cap
            // below it cuts nothing while the round is younger than 1 s)
            assert!(!hit && n == 0, "{}: {l:?}", c.name);
            assert!(et.is_none() || !young, "{}: cut inside the floor: {et_s}", c.name);
        }
    }
    assert!(!et_s.contains("or the run was cancelled") && !et_s.contains("overrun cap [run]"), "{}: {et_s}", c.name);
    // never: a drawing_grace_s hit without a bound
    if c.grace < 0.0 || c.hard {
        assert!(!hit, "{}: no bound (grace {} hard {}), yet hit: {l:?}", c.name, c.grace, c.hard);
    }
    if let Some(w) = &wl {
        // the drawing bound named as having run out only with a hit; "finished within drawing_grace_s" never after a bound cut
        assert!(!w.contains("s ran out, so the drawing's optional passes stopped") || hit, "{}: {w}", c.name);
        assert!(!(c.want == Want::Bound && w.contains("finished within [run] drawing_grace_s")), "{}: {w}", c.name);
        // r5 finding 1: "every wire was drawn" only when every wire was drawn
        assert!(!w.contains("every wire was drawn") || r.routed == r.connections_total, "{}: routed {}/{}: {w}", c.name, r.routed, r.connections_total);
    }
}

macro_rules! cases {
    ($($f:ident: $c:expr;)*) => { $( #[test] fn $f() { check(&$c); } )* };
}

use Act::*;
const W: f64 = 86400.0;
const NONE: &[Act] = &[];
cases! {
    // soft, wall clock
    lw_s_none_gm1: Case { name: "s_none_gm1", hard: false, wall: W, grace: -1.0, cap: 1.5, lp: "topor", in_loop: NONE, at_end: NONE, in_nap: NONE, nap_ms: 0, want: Want::Free };
    lw_s_none_g0: Case { name: "s_none_g0", hard: false, wall: W, grace: 0.0, cap: 1.5, lp: "topor", in_loop: NONE, at_end: NONE, in_nap: NONE, nap_ms: 0, want: Want::Free };
    lw_s_none_g60: Case { name: "s_none_g60", hard: false, wall: W, grace: 60.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: NONE, in_nap: NONE, nap_ms: 0, want: Want::Free };
    lw_s_bound_g0: Case { name: "s_bound_g0", hard: false, wall: W, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 0, want: Want::Bound };
    lw_s_bound_g03: Case { name: "s_bound_g03", hard: false, wall: W, grace: 0.3, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 800, want: Want::Bound };
    lw_s_nobound_gm1: Case { name: "s_nobound_gm1", hard: false, wall: W, grace: -1.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 800, want: Want::Free };
    lw_s_nobound_g60: Case { name: "s_nobound_g60", hard: false, wall: W, grace: 60.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 800, want: Want::Free };
    lw_s_cap_gm1: Case { name: "s_cap_gm1", hard: false, wall: W, grace: -1.0, cap: 1.5, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1800, want: Want::Cap };
    lw_s_cap_g60: Case { name: "s_cap_g60", hard: false, wall: W, grace: 60.0, cap: 1.5, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1800, want: Want::Cap };
    lw_s_cap_g3_first: Case { name: "s_cap_g3_first", hard: false, wall: W, grace: 3.0, cap: 1.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1500, want: Want::Cap };
    // r5 finding 1: the bound first, then the cap cuts the drawing (routed 0/3): no "every wire was drawn"
    lw_s_both_g0_bound_first: Case { name: "s_both_g0_bound_first", hard: false, wall: W, grace: 0.0, cap: 1.5, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1800, want: Want::Bound };
    lw_s_both_g03_bound_first: Case { name: "s_both_g03_bound_first", hard: false, wall: W, grace: 0.3, cap: 1.5, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1800, want: Want::Bound };
    // r5 finding 1, wait line: the limit falls in the TopoR loop (which ends within limit_grace_s), the drawing bound
    // then cuts ERBS and the cap cuts the drawing of the traces: the "; then the drawing:" note must not say every wire was drawn
    lw_s_loop_bound_then_cap: Case { name: "s_loop_bound_then_cap", hard: false, wall: W, grace: 0.0, cap: 1.5, lp: "topor", in_loop: &[Reach], at_end: NONE, in_nap: NONE, nap_ms: 1800, want: Want::Bound };
    // r5 finding 5 (m_cut_nocap / m_cut_ge): both passed at the first check, the cap first (the wall cap counts from
    // the round's 1 s floor on, ~1 s; the bound from the drawing's start + 1.5 s)
    lw_s_both_cap_first: Case { name: "s_both_cap_first", hard: false, wall: W, grace: 1.5, cap: 0.1, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1800, want: Want::Cap };
    // ... and inside the floor the earlier wall cap does not count: the bound (0.3 s) came first
    lw_s_both_cap_in_floor: Case { name: "s_both_cap_in_floor", hard: false, wall: W, grace: 0.3, cap: 0.1, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1000, want: Want::Bound };
    // soft, CPU clock (engine wall limit off and engine cap 0, like run.py): reach at the limit, harden at the cap
    lw_c_harden_gm1: Case { name: "c_harden_gm1", hard: false, wall: 0.0, grace: -1.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: &[Harden], nap_ms: 0, want: Want::Cap };
    // r5 finding 5 (m_cut_nonow): harden with the bound 60 s ahead
    lw_c_harden_g60: Case { name: "c_harden_g60", hard: false, wall: 0.0, grace: 60.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: &[Harden], nap_ms: 0, want: Want::Cap };
    // the bound passed at the drawing start, harden (no instant) after it: the bound came first
    lw_c_harden_g0: Case { name: "c_harden_g0", hard: false, wall: 0.0, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: &[Harden], nap_ms: 0, want: Want::Bound };
    lw_c_bound_g0: Case { name: "c_bound_g0", hard: false, wall: 0.0, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 0, want: Want::Bound };
    // r5 finding 3: the CPU limit is reached once ERBS has its options (the bound read live, not a snapshot)
    lw_c_late_reach_g0: Case { name: "c_late_reach_g0", hard: false, wall: 0.0, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: NONE, in_nap: &[Reach], nap_ms: 200, want: Want::Bound };
    lw_c_late_reach_gm1: Case { name: "c_late_reach_gm1", hard: false, wall: 0.0, grace: -1.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: NONE, in_nap: &[Reach], nap_ms: 200, want: Want::Free };
    lw_c_late_harden_gm1: Case { name: "c_late_harden_gm1", hard: false, wall: 0.0, grace: -1.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: NONE, in_nap: &[Harden], nap_ms: 0, want: Want::Cap };
    // Cancel (r5 finding 2): the TopoR loop's one drawing runs through a Cancel; with loop = cycles a Cancel stops ERBS
    lw_k_topor_g0: Case { name: "k_topor_g0", hard: false, wall: W, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: NONE, in_nap: &[Cancel], nap_ms: 0, want: Want::Free };
    lw_k_cycles_g0: Case { name: "k_cycles_g0", hard: false, wall: W, grace: 0.0, cap: 0.0, lp: "cycles", in_loop: NONE, at_end: NONE, in_nap: &[Cancel], nap_ms: 0, want: Want::Cancel };
    lw_k_cycles_gm1: Case { name: "k_cycles_gm1", hard: false, wall: W, grace: -1.0, cap: 0.0, lp: "cycles", in_loop: NONE, at_end: NONE, in_nap: &[Cancel], nap_ms: 0, want: Want::Cancel };
    lw_k_bound_then_cancel_g0: Case { name: "k_bound_then_cancel_g0", hard: false, wall: W, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: &[Cancel], nap_ms: 0, want: Want::Bound };
    lw_k_cycles_none_g60: Case { name: "k_cycles_none_g60", hard: false, wall: W, grace: 60.0, cap: 0.0, lp: "cycles", in_loop: NONE, at_end: NONE, in_nap: NONE, nap_ms: 0, want: Want::Free };
    lw_h_cancel_cycles_g0: Case { name: "h_cancel_cycles_g0", hard: true, wall: W, grace: 0.0, cap: 0.0, lp: "cycles", in_loop: NONE, at_end: NONE, in_nap: &[Cancel], nap_ms: 0, want: Want::Cancel };
    // hard
    lw_h_none_g0: Case { name: "h_none_g0", hard: true, wall: W, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: NONE, in_nap: NONE, nap_ms: 0, want: Want::Free };
    lw_h_reach_gm1: Case { name: "h_reach_gm1", hard: true, wall: W, grace: -1.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1800, want: Want::Hard };
    lw_h_reach_g0: Case { name: "h_reach_g0", hard: true, wall: W, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1800, want: Want::Hard };
    lw_h_reach_g60: Case { name: "h_reach_g60", hard: true, wall: W, grace: 60.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 1800, want: Want::Hard };
    // r5 finding 4, lead 2026-11-17: the CPU clock's hard limit (harden) cuts ERBS at once, no 1 s floor
    lw_h_cpu_g0: Case { name: "h_cpu_g0", hard: true, wall: 0.0, grace: 0.0, cap: 0.0, lp: "topor", in_loop: NONE, at_end: &[Harden], in_nap: NONE, nap_ms: 0, want: Want::Hard };
    // ... and the wall clock keeps its floor: a wall cap of 0.05 s below it cuts nothing
    lw_h_cap_floor_g0: Case { name: "h_cap_floor_g0", hard: true, wall: W, grace: 0.0, cap: 0.05, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 100, want: Want::FloorKept };
    lw_s_capped_floor_gm1: Case { name: "s_capped_floor_gm1", hard: false, wall: W, grace: -1.0, cap: 0.05, lp: "topor", in_loop: NONE, at_end: &[Reach], in_nap: NONE, nap_ms: 100, want: Want::FloorKept };
}
