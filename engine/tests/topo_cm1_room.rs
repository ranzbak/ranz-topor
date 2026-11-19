//! CM1 (cm1-room, docs/dev/design/cm1-room.md): a planned via is a feature of the capacity
//! test. The wiccon GUI case in miniature (reports/wiccon-gui27-20261117-2215.md, cluster A):
//! two 2.5 x 1.0 mm SMD pads on F.Cu 1.0 mm apart (U22 TXD above, U21 another net below), TXD's
//! other end a small B.Cu pad in the middle of that gap (so TXD's via has to sit in the gap), and
//! RST from left to right at the gap's height on F.Cu. The gap holds the via (0.6) or RST
//! (0.25 + 2 x 0.2), never both (1.25 > 1.0): the plan must route RST round a pad, and must
//! predict the conflict when it does not.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

/// The gap board's pads `(ref, net, x, y, w, h, layer)` (KiCad frame, mm): net 1 TXD, 2 RST,
/// 3 RXD (U21, one pad: no connection).
const GAP_PADS: [(&str, i32, f64, f64, f64, f64, i32); 5] = [
    ("U22", 1, 6.0, 4.0, 2.5, 1.0, 0),
    ("T1", 1, 6.0, 5.0, 0.3, 0.3, 2),
    ("U21", 3, 6.0, 6.0, 2.5, 1.0, 0),
    ("R1", 2, 1.0, 5.0, 1.0, 1.0, 0),
    ("R2", 2, 11.0, 5.0, 1.0, 1.0, 0),
];

fn gap() -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (12.0, 0.0), (12.0, 10.0), (0.0, 10.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pj: Vec<String> = GAP_PADS
        .iter()
        .enumerate()
        .map(|(i, &(r, net, x, y, w, h, l))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                p(x),
                p(y),
                p(w),
                p(h)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"cm1gap",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"TXD"}},{{"code":2,"name":"RST"}},{{"code":3,"name":"RXD"}}],
            "pads":[{}],"keepouts":[],"outline":[{}],
            "rules":{{"min_clearance":200000,"min_track_width":250000,"min_via_diameter":600000,"min_via_drill":300000,
                     "min_annular":50000,"hole_clearance":250000,"hole_to_hole":250000,"edge_clearance":500000,
                     "default":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000}}}}}}"#,
        pj.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn opts(via_cuts: bool) -> Options {
    Options { via_cuts, ..Options::default() }
}

/// tw4 merge round (2026-11-19): CM1 is the default after the equal-time A/B, and a JSON options
/// object without the key (a direct engine call, the harness) gets it too.
#[test]
fn via_cuts_is_the_default() {
    assert!(Options::default().via_cuts);
    assert!(Options::from_json("{}").expect("options parse").via_cuts);
}

/// The picked plan predicts what the drawing hits: with via cuts the drawing of the pick is
/// clean (RST goes round a pad or the via leaves the gap), and the plan's prediction equals 0.
#[test]
fn a_via_in_a_pad_gap_takes_its_room_from_the_wire_through_the_gap() {
    let m = gap();
    let r = route_with(&m, &opts(true), &RunControl::new());
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    let lp = r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).expect("loop line");
    assert!(rep.violations.is_empty(), "{:?}\n{lp}", rep.violations);
    assert!(lp.contains("predicted 0 violations"), "{lp}");
}

/// The same board before CM1 (`[capacity] via_cuts = false`): the plan predicts 0 and the drawing
/// hits the gap, or the board does not show the case (then the test above proves nothing).
#[test]
fn without_via_cuts_the_plan_misses_the_gap() {
    let m = gap();
    let r = route_with(&m, &opts(false), &RunControl::new());
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    let lp = r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).expect("loop line");
    assert!(lp.contains("predicted 0 violations"), "{lp}");
    assert!(!rep.violations.is_empty(), "the old model must draw into the gap: {lp}");
}

/// tw4 T2: with via cuts every refinement ends with stored via prices equal to a fresh pricing
/// (`CapModel::via_check`). The qmtech USB corner (4 layers, planned vias, many rejected moves
/// in the loop-local judge's serial refinement) is the case: a rejected move must restore its
/// wires' via points with their prices (tw4t2-ae57e35: 383+ stale entries per run before).
#[test]
fn via_prices_stay_fresh_through_rejected_moves() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::refine::viacheck;
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses");
    let o = Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "via_cuts": true}"#).expect("options parse");
    viacheck::ON.store(true, Ordering::Relaxed);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let _ = route_with(&m, &o, &ctl);
    let (bad, n) = (viacheck::BAD.load(Ordering::Relaxed), viacheck::CHECKED.load(Ordering::Relaxed));
    assert!(n > 0, "no priced via entry was checked: the case does not show");
    assert_eq!(bad, 0, "{bad} stale via prices of {n}");
}

/// tw4 fix round 2 (verifier F1): the local judge prices the via terms only for its focus (the free
/// wires and the traversers of their paths' vertex star, union of both states). That cut must be exact
/// for the judged change: every judgement priced again with every model wire in focus gives the same
/// change of the count and of the graded price (qmtech USB corner, the via moves of the loop).
#[test]
fn the_via_focus_of_the_local_judge_equals_every_wire_in_focus() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::refine::viacheck;
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses");
    let o = Options::from_json(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "via_cuts": true}"#).expect("options parse");
    viacheck::FOCUS_ON.store(true, Ordering::Relaxed);
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let _ = route_with(&m, &o, &ctl);
    let (bad, n) = (viacheck::FOCUS_BAD.load(Ordering::Relaxed), viacheck::FOCUS_CHECKED.load(Ordering::Relaxed));
    eprintln!("via focus: {n} local judgements checked, {bad} differ");
    assert!(n > 0, "no local judgement was checked: the case does not show");
    assert_eq!(bad, 0, "{bad} of {n} local judgements differ from the judge with every wire in focus");
}

/// tw4-r2 (2): `[capacity] via_best_point` and `[capacity] via_over_ramp` are opt-in (default =
/// the model before them, bit-identical), and a JSON options object sets them.
#[test]
fn via_best_point_and_ramp_are_opt_in() {
    let d = Options::default();
    assert!(!d.via_best_point);
    assert_eq!(d.via_over_ramp_mm, 0.0);
    let o = Options::from_json(r#"{"via_best_point": true, "via_over_ramp_mm": 0.05}"#).expect("options parse");
    assert!(o.via_best_point);
    assert!((o.via_over_ramp_mm - 0.05).abs() < 1e-12);
}

/// tw4-r2 (2): with the via measured at its best point and the graded ramp, every refinement still
/// ends with stored via prices equal to a fresh pricing, and the local judge's via focus stays exact
/// (the best point lies in the via's own triangle, so its neighbourhood, crossers and partners are
/// the ones the repricing already covers). The qmtech USB corner, as the two tests above.
#[test]
fn best_point_and_ramp_prices_stay_fresh_and_focus_exact() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::refine::viacheck;
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses");
    let o = Options::from_json(
        r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "via_cuts": true, "via_best_point": true, "via_over_ramp_mm": 0.05}"#,
    )
    .expect("options parse");
    viacheck::ON.store(true, Ordering::Relaxed);
    viacheck::FOCUS_ON.store(true, Ordering::Relaxed);
    let (b0, f0) = (viacheck::BAD.load(Ordering::Relaxed), viacheck::FOCUS_BAD.load(Ordering::Relaxed));
    let (n0, g0) = (viacheck::CHECKED.load(Ordering::Relaxed), viacheck::FOCUS_CHECKED.load(Ordering::Relaxed));
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let _ = route_with(&m, &o, &ctl);
    let (bad, n) = (viacheck::BAD.load(Ordering::Relaxed) - b0, viacheck::CHECKED.load(Ordering::Relaxed) - n0);
    let (fbad, fn_) = (viacheck::FOCUS_BAD.load(Ordering::Relaxed) - f0, viacheck::FOCUS_CHECKED.load(Ordering::Relaxed) - g0);
    assert!(n > 0 && fn_ > 0, "the case does not show: {n} via entries, {fn_} judgements checked");
    assert_eq!(bad, 0, "{bad} stale via prices of {n}");
    assert_eq!(fbad, 0, "{fbad} of {fn_} local judgements differ from the judge with every wire in focus");
}

/// tw4-r2 (2b): with the via discs packed at their heights (`[capacity] via_disc_height`), every
/// refinement still ends with the stored discs (heights), leans and edge prices equal to fresh ones
/// (`viacheck` covers them with the option on), and the local judge's via focus stays exact. The
/// qmtech USB corner, as above; with the best point too (the combination the A/B measures).
#[test]
fn disc_heights_stay_fresh_and_focus_exact() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::refine::viacheck;
    let path = format!("{}/tests/fixtures/qmtech-usb-crop.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses");
    let o = Options::from_json(
        r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "via_cuts": true, "via_best_point": true, "via_disc_height": true}"#,
    )
    .expect("options parse");
    viacheck::ON.store(true, Ordering::Relaxed);
    viacheck::FOCUS_ON.store(true, Ordering::Relaxed);
    let (b0, f0) = (viacheck::BAD.load(Ordering::Relaxed), viacheck::FOCUS_BAD.load(Ordering::Relaxed));
    let (n0, g0) = (viacheck::CHECKED.load(Ordering::Relaxed), viacheck::FOCUS_CHECKED.load(Ordering::Relaxed));
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let _ = route_with(&m, &o, &ctl);
    let (bad, n) = (viacheck::BAD.load(Ordering::Relaxed) - b0, viacheck::CHECKED.load(Ordering::Relaxed) - n0);
    let (fbad, fn_) = (viacheck::FOCUS_BAD.load(Ordering::Relaxed) - f0, viacheck::FOCUS_CHECKED.load(Ordering::Relaxed) - g0);
    assert!(n > 0 && fn_ > 0, "the case does not show: {n} entries, {fn_} judgements checked");
    assert_eq!(bad, 0, "{bad} stale discs, leans or prices of {n}");
    assert_eq!(fbad, 0, "{fbad} of {fn_} local judgements differ from the judge with every wire in focus");
}

/// The gap board planned by hand (each connection its shortest path, the layer assignment as the
/// loop's), measured by the capacity model under `o`: `(violations count, graded, via_cut_count,
/// located via cut points, via cut list length)`.
fn gap_measure(o: &Options) -> (usize, f64, usize, usize, usize) {
    use topor_engine::topo::board::{self as tb, BuildOpts};
    use topor_engine::topo::init::InitWeights;
    use topor_engine::topo::layers::{self, LayerParams};
    use topor_engine::topo::pack::Packer;
    use topor_engine::topo::refine::CapModel;
    use topor_engine::topo::state::State;
    let m = gap();
    let b = tb::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    for c in &conns {
        st.route_conn(c, &InitWeights::default(), (c.w + c.s) / 400_000.0).expect("routes");
    }
    let packer = Packer::for_model(&m, o, &b.proj);
    let smd = |ob: usize| b.smd_layer(ob);
    let lr = layers::assign_with(&st, &smd, &LayerParams::default(), Some(&packer));
    let dirs = packer.taut_dirs(&st);
    let cm = CapModel::new(&st, &packer, &dirs, &lr.events, 1, 1.0);
    let (n, g) = cm.violations();
    (n, g, cm.via_cut_count(), cm.via_cut_points().len(), cm.via_cut_list().len())
}

fn gap_opts(json: &str) -> Options {
    Options::from_json(json).expect("options parse")
}

/// tw4-r2 (2) behaviour: TXD's via in the gap with RST through it is over-full at its planned
/// point. Measured at the least-over site point of its triangle (`via_best_point`) the via is
/// never priced higher than at the planned point (the planned point is one of the candidates), and
/// on this board strictly lower (a site point nearer an end of the gap leaves RST more of the cut).
#[test]
fn best_point_prices_the_gap_via_no_worse_and_here_lower() {
    let off = gap_measure(&gap_opts(r#"{"via_cuts": true}"#));
    let on = gap_measure(&gap_opts(r#"{"via_cuts": true, "via_best_point": true}"#));
    let none = gap_measure(&gap_opts(r#"{"via_cuts": false}"#));
    eprintln!("gap off {off:?} best {on:?} no-cuts {none:?}");
    assert!(off.2 >= 1 && off.1 > none.1, "the case shows: an over-full via cut at the planned point {off:?} vs {none:?}");
    assert!(on.1 <= off.1 + 1e-9, "the best point is never priced above the planned one: {on:?} vs {off:?}");
    assert!(on.1 < off.1 - 1e-6, "a better site point exists in the via's triangle: {on:?} vs {off:?}");
}

/// tw4-r2 (2) behaviour: with a ramp far past the gap's overage (`via_over_ramp_mm` 100) the via
/// entry stays priced (graded above the board without via cuts, below the step) but is no
/// predicted violation: the compass locates no via cut and the P4 list holds none.
#[test]
fn the_ramp_keeps_a_small_overage_priced_but_out_of_the_compass() {
    let full = gap_measure(&gap_opts(r#"{"via_cuts": true}"#));
    let soft = gap_measure(&gap_opts(r#"{"via_cuts": true, "via_over_ramp_mm": 100.0}"#));
    let none = gap_measure(&gap_opts(r#"{"via_cuts": false}"#));
    eprintln!("gap full {full:?} ramp {soft:?} no-cuts {none:?}");
    assert!(full.3 >= 1 && full.4 >= 1, "the step: located and listed {full:?}");
    assert_eq!((soft.2, soft.3, soft.4), (0, 0, 0), "under the ramp: not counted, not located, not listed {soft:?}");
    assert!(soft.1 > none.1 + 1e-9 && soft.1 < full.1 - 1e-6, "graded between: {none:?} < {soft:?} < {full:?}");
}

/// tw4-r2 (2) behaviour: the best point's candidates are site points of the triangle where a via
/// keeps its clearance to every other net's pad (static room); on the gap board some triangles
/// lose some of their 9 site points that way.
#[test]
fn best_point_candidates_keep_the_via_clear_of_other_pads() {
    use topor_engine::topo::board::{self as tb, BuildOpts};
    use topor_engine::topo::pack::{tri_site_points, Packer};
    let m = gap();
    let b = tb::build(&m, &BuildOpts::default()).unwrap();
    let packer = Packer::for_model(&m, &gap_opts(r#"{"via_cuts": true, "via_best_point": true}"#), &b.proj);
    let (r, s) = (0.3 * MM, 0.2 * MM);
    // Distance from a point (math frame, y up) to a pad rectangle of another net.
    let clear = |q: topor_engine::geom::Vec2| {
        GAP_PADS
            .iter()
            .filter(|p| p.1 != 1)
            .map(|&(_, _, x, y, w, h, _)| {
                let dx = ((q.x - x * MM).abs() - 0.5 * w * MM).max(0.0);
                let dy = ((q.y + y * MM).abs() - 0.5 * h * MM).max(0.0);
                (dx * dx + dy * dy).sqrt()
            })
            .fold(f64::INFINITY, f64::min)
    };
    let mut fewer = 0;
    for t in 0..b.proj.tris.len() {
        let [a, bb, c] = b.proj.tris[t].map(|v| b.proj.pos[v]);
        let all = tri_site_points(a, bb, c);
        let cands = packer.via_cands(t, 1, 2.0 * r, s);
        for q in cands.iter() {
            assert!(all.contains(q), "triangle {t}: a candidate is a site point");
            assert!(clear(*q) >= r + s - 1e3, "triangle {t}: candidate {q:?} only {:.3} mm from another net's pad", clear(*q) / MM);
        }
        if all.iter().any(|&q| clear(q) < r + s - 1e3) {
            assert!(cands.len() < 9, "triangle {t}: a site point too near another pad is no candidate");
            fewer += 1;
        }
    }
    assert!(fewer > 0, "the case shows: some site points lack room");
}
