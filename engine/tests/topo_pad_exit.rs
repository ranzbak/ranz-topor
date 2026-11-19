//! tw4-r2 (1), T1 part 2 (`[capacity] pad_exits`, docs/dev/design/tw4-r2.md (1)): the pad exit
//! as a plan object, and the two user-approved T1 patterns as regression tests
//! (`harness/fit_patterns.py`: exit_tip_from_plan, neck_or_detour; fixtures in
//! tests/fixtures/fit/).
//!
//! - exit_tip_from_plan: SIG leaves U1.3 of a 0.5 mm pitch row (0.2 mm gaps, a 0.25 mm wire needs
//!   0.65 mm) by the tip the plan's path takes (north, round the row's end), not by the tip the
//!   straight line to its target points at (south, into the package): the exit is read from the
//!   plan.
//! - neck_or_detour: VBUS (0.8 mm) passes the 1.3 mm gap between J1.2 and J3.1 instead of
//!   necking through the 0.84 mm one between J1.1 and J1.2.

use topor_engine::check::Checker;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::init::InitWeights;
use topor_engine::topo::pack::Packer;
use topor_engine::topo::padexit::Side;
use topor_engine::topo::state::State;

fn fixture(name: &str) -> (BoardModel, serde_json::Value) {
    let path = format!("{}/tests/fixtures/fit/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json");
    (BoardModel::from_json(&doc["model"].to_string()).expect("model"), doc)
}

/// The expected (optimal) witness length of the pattern's first wire (mm).
fn witness_mm(doc: &serde_json::Value) -> f64 {
    let pts: Vec<(f64, f64)> = doc["wires"][0]["witness"].as_array().unwrap().iter().map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())).collect();
    pts.windows(2).map(|x| ((x[1].0 - x[0].0).powi(2) + (x[1].1 - x[0].1).powi(2)).sqrt()).sum()
}

fn opts(pad_exits: bool) -> Options {
    Options::from_json(&format!(r#"{{"runs": 1, "wall_clock_s": 120.0, "pad_exits": {pad_exits}}}"#)).expect("options")
}

/// The plan object: on the planner's own path of SIG (the projection's route of the connection),
/// U1.3's exit leaves by the tip its leaving pass lies beyond, north (+axis in the math frame),
/// though the straight line to the target points south; the stub is the floor-or-wider width
/// that keeps its clearance and starts on the pad's tip.
#[test]
fn the_exit_is_read_from_the_plan_not_the_straight_line() {
    let (m, _) = fixture("exit_tip_from_plan");
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    assert_eq!(conns.len(), 1);
    let mut st = State::new(&b.proj);
    st.route_conn(&conns[0], &InitWeights::default(), (conns[0].w + conns[0].s) / 400_000.0).expect("routes");
    let packer = Packer::for_model(&m, &opts(true), &b.proj);
    assert!(packer.pad_exits);
    let wi = &st.wires[0];
    let ex = packer.exits_of(&st, 0, packer.exit_floor(wi.net, wi.w));
    assert!(!ex.is_empty(), "U1.3 is fine pitch: a planned exit");
    let pad = |o: usize| b.proj.obstacles[o].center;
    // U1.3 is at (4.0, 3.1) mm (KiCad frame; math y = -3.1).
    let e = ex.iter().find(|g| (pad(g.pad).x - 4.0e6).abs() < 1e3).expect("the exit at U1.3");
    let target = if e.end == 0 { pad(wi.pb) } else { pad(wi.pa) };
    let straight_south = target.y < pad(e.pad).y;
    assert!(straight_south, "the straight line points south (the case shows)");
    let plan_north = e.at.y > pad(e.pad).y;
    match (plan_north, e.side) {
        (true, Side::TipPlus) | (false, Side::TipMinus) => {}
        other => panic!("exit side {:?} does not follow the plan's leaving pass (north {plan_north}): {other:?}", e.side),
    }
    assert!(e.dir.y.signum() == if plan_north { 1.0 } else { -1.0 });
    // The stub starts on the tip and reaches s + w_e / 2 beyond it.
    assert!(((e.a.y - pad(e.pad).y).abs() - 0.4e6).abs() < 1e3, "{:?}", e.a);
    assert!(((e.b - e.a).len() - (wi.s + 0.5 * e.w_e)).abs() < 1.0);
    assert!(e.w_e <= wi.w + 1e-9 && e.w_e >= packer.exit_floor(wi.net, wi.w) - 1e-9);
    // Off: no planned exits.
    let off = Packer::for_model(&m, &opts(false), &b.proj);
    assert!(!off.pad_exits);
}

/// Regression (user-approved pattern): SIG is routed, the copper is clean, it leaves U1.3 by
/// the north tip and takes no detour beyond the expected path's length (+10 %), with and
/// without the planned exits.
#[test]
fn exit_tip_from_plan_routes_clean_without_detour() {
    let (m, doc) = fixture("exit_tip_from_plan");
    let want = witness_mm(&doc);
    for on in [false, true] {
        let r = route(&m, &opts(on));
        assert_eq!(r.failed, 0, "pad_exits {on}: {:?}", r.warnings);
        let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
        assert!(rep.violations.is_empty(), "pad_exits {on}: {:?}", rep.violations);
        let got = r.total_length_nm as f64 / 1e6;
        assert!(got <= 1.1 * want, "pad_exits {on}: {got:.3} mm vs expected {want:.3} mm");
    }
}

/// Regression (user-approved pattern): VBUS keeps its 0.8 mm through the wide gap, clean, no
/// detour beyond the expected path (+10 %).
#[test]
fn neck_or_detour_takes_the_wide_gap() {
    let (m, doc) = fixture("neck_or_detour");
    let want = witness_mm(&doc);
    for on in [false, true] {
        let r = route(&m, &opts(on));
        assert_eq!(r.failed, 0, "pad_exits {on}: {:?}", r.warnings);
        let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
        assert!(rep.violations.is_empty(), "pad_exits {on}: {:?}", rep.violations);
        let got = r.total_length_nm as f64 / 1e6;
        assert!(got <= 1.1 * want, "pad_exits {on}: {got:.3} mm vs expected {want:.3} mm");
        // The pattern's answer, not only a short clean route: no necking anywhere (every
        // stretch at the wire's 0.8 mm) and the crossing of the cut line y = 7.5 mm lies inside
        // the wide gap E1 (x 4.84..6.14 mm, the whole 0.8 mm track), not in the 0.84 mm gap N1
        // (x 2.30..3.14 mm), where a 0.4 mm neck would pass.
        let mut crossings = 0;
        for it in &r.items {
            let Item::Track(t) = it else { continue };
            assert!(t.width >= 800_000, "pad_exits {on}: necked to {} nm: {t:?}", t.width);
            let TrackPrim::Seg { a, b } = t.prim else { panic!("pad_exits {on}: unexpected arc in {t:?}") };
            let (y0, y1) = (a.y as f64, b.y as f64);
            if (y0 - 7.5e6) * (y1 - 7.5e6) < 0.0 {
                crossings += 1;
                let x = a.x as f64 + (b.x - a.x) as f64 * (7.5e6 - y0) / (y1 - y0);
                let half = 0.5 * t.width as f64;
                assert!(x - half >= 4.84e6 - 1.0 && x + half <= 6.14e6 + 1.0, "pad_exits {on}: crosses y = 7.5 mm at x = {:.3} mm, not through E1: {t:?}", x / 1e6);
            }
        }
        assert!(crossings >= 1, "pad_exits {on}: the route does not cross the cut line");
    }
}

#[test]
fn pad_exits_are_opt_in() {
    assert!(!Options::default().pad_exits);
    assert!(Options::from_json(r#"{"pad_exits": true}"#).expect("options parse").pad_exits);
}

/// The static exit and the via-room term on the 0.5 mm pitch row of exit_tip_from_plan: out of
/// U1.3's north tip the 0.25 mm wire's stub keeps its clearance at full width (0.35 mm from its
/// centre line to the neighbours' copper), out of its long side it stands on U1.4 (graded over);
/// a via of the wire in the exit zone beside U1.4 is over, one clear of the row is not. With the
/// option on, the capacity model's stored entries equal fresh ones.
#[test]
fn the_exit_terms_measure_the_row() {
    let (m, _) = fixture("exit_tip_from_plan");
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    st.route_conn(&conns[0], &InitWeights::default(), (conns[0].w + conns[0].s) / 400_000.0).expect("routes");
    let packer = Packer::for_model(&m, &opts(true), &b.proj);
    let wi = st.wires[0].clone();
    let floor = packer.exit_floor(wi.net, wi.w);
    let u13 = (0..b.proj.obstacles.len()).find(|&o| (b.proj.obstacles[o].center.x - 4.0e6).abs() < 1e3 && (b.proj.obstacles[o].center.y + 3.1e6).abs() < 1e3).expect("U1.3");
    let tip = packer.exit_static(u13, Side::TipPlus, wi.net, wi.w, wi.s, floor).expect("fine pitch");
    assert!((tip.w_e - wi.w).abs() < 1.0 && tip.over <= 0.0, "{tip:?}");
    assert!(tip.neigh.len() >= 2, "both row neighbours");
    // A 0.29 mm wire (floor 0.25): the tip stub still keeps 0.2 mm at full width (0.35 - 0.145 =
    // 0.205 mm), so the widest fitting width is the wire's own, not the floor.
    let wide = packer.exit_static(u13, Side::TipPlus, wi.net, 0.29e6, wi.s, floor).expect("fine pitch");
    assert!((wide.w_e - 0.29e6).abs() < 1.0 && floor < 0.29e6 - 1e3, "the widest width that fits: {wide:?} (floor {floor})");
    let side = packer.exit_static(u13, Side::LongPlus, wi.net, wi.w, wi.s, floor).expect("fine pitch");
    assert!(side.over > 0.1e6, "the side stub stands on U1.4: {side:?}");
    assert!((side.w_e - floor).abs() < 1.0, "at worst the floor: {side:?}");
    // The via room term at the exit.
    let ex = packer.exits_of(&st, 0, floor);
    let e = ex.iter().find(|g| g.pad == u13).expect("exit");
    let r = 0.3e6;
    let beside = topor_engine::geom::Vec2 { x: 4.5e6, y: e.b.y };
    let clear = topor_engine::geom::Vec2 { x: e.b.x, y: e.b.y + e.dir.y * 3.0e6 };
    let o = topor_engine::topo::padexit::exit_via_room(&st, e, &[beside], r, wi.s, 0.0);
    assert!(o.count == 1 && o.graded > 1.0, "{o:?}");
    let o = topor_engine::topo::padexit::exit_via_room(&st, e, &[clear], r, wi.s, 0.0);
    assert_eq!(o.count, 0, "{o:?}");
    // The deficit is the clearance the via disc lacks: 1 mm north of U1.3's tip a 0.3 mm via is
    // clear of U1.2 and U1.4 by more than its 0.2 mm (negative deficit); beside U1.4 it lacks
    // r + s - distance.
    let far = topor_engine::geom::Vec2 { x: 4.0e6, y: e.a.y + e.dir.y * 1.0e6 };
    assert!(topor_engine::topo::padexit::via_deficit(&st, &e.neigh, far, r, wi.s) < -0.3e6);
    let d = topor_engine::topo::padexit::via_deficit(&st, &e.neigh, beside, r, wi.s);
    assert!(d > 0.0 && d <= r + wi.s + 1.0, "{d}");
    // Stored = fresh with the option on (exit entries included).
    let dirs = packer.taut_dirs(&st);
    let evl = vec![vec![(0, 0.5, 0)]];
    let cm = topor_engine::topo::refine::CapModel::new(&st, &packer, &dirs, &evl, 1, 50.0);
    assert_eq!(cm.via_check().0, 0);
    assert!(!cm.exits_of(0).is_empty());
}

/// `[capacity] pour_cuts` (tw4-r2 (3)) is opt-in: off by default (default copper = main), the
/// option parses, and its cell budget has its documented default.
#[test]
fn pour_cuts_are_opt_in() {
    let d = Options::default();
    assert!(!d.pour_cuts);
    assert_eq!(d.pour_cut_cells, 1.0e5);
    let o = Options::from_json(r#"{"pour_cuts": true, "pour_cut_cells": 250000.0}"#).expect("options parse");
    assert!(o.pour_cuts && o.pour_cut_cells == 250000.0);
    assert!(!Options::from_json("{}").expect("options parse").pour_cuts);
}
