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
use topor_engine::model::{BoardModel, Options};
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
    }
}

#[test]
fn pad_exits_are_opt_in() {
    assert!(!Options::default().pad_exits);
    assert!(Options::from_json(r#"{"pad_exits": true}"#).expect("options parse").pad_exits);
}
