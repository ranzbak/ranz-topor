//! tw4 fix round 2 (verifier F2): `[factors] neck_depth` (T1 part 1, opt-in) - the depth of a planned neck
//! as d² per bottleneck cluster ([`State::pass_d`]) and its price in the loop's whole-board measure. The
//! loop's search does not price the neck (a measure-only part), so on a small board no move proposes the
//! way round a bottleneck; the measure is tested through its part, not through a changed route.

mod topo_d6b;

use topo_d6b::{board, conn, opts};
use topor_engine::model::{BoardModel, Options};
use topor_engine::control::RunControl;
use topor_engine::route::route_with;
use topor_engine::topo::board as tb;
use topor_engine::topo::init::InitWeights;
use topor_engine::topo::state::State;

const MM: f64 = 1e6;

/// A 0.4 mm wire A -> B whose straight way is a 0.7 mm gap between two walls on both layers (0.8 mm
/// needed at the nominal width with 0.2 mm clearance, 0.6 mm at the 0.2 mm floor: d = 0.1 / 0.2 = 0.5).
/// The walls reach both board edges: the neck is the only way.
fn gap_board() -> BoardModel {
    let pads = vec![("A", 1, 3.0, 10.0, 1.0, 1.0, 0), ("B", 1, 27.0, 10.0, 1.0, 1.0, 0), ("WN", 2, 15.0, 4.825, 1.0, 9.65, 99), ("WS", 2, 15.0, 15.175, 1.0, 9.65, 99)];
    let mut m = board(30.0, 20.0, 2, &pads);
    let mut r = m.rules.default.clone();
    r.width = 400_000;
    m.rules.per_net.insert(1, r);
    m.rules.min_track_width = 200_000;
    m
}

/// The neck depth of the bottleneck is d² (0.25), not d, at the cluster's first pass only.
#[test]
fn the_neck_depth_of_a_bottleneck_is_d_squared() {
    let m = gap_board();
    let b = tb::build(&m, &opts()).unwrap();
    let mut st = State::new(&b.proj);
    st.floors.insert(1, 0.2 * MM);
    let mut c = conn(&m, &b, 1, "A", "B");
    c.w = 0.4 * MM;
    let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed").0;
    let ds: Vec<f64> = st.wires[w].passes.iter().map(|&p| st.pass_d[p]).collect();
    let nd = st.neck_depth_of(w);
    eprintln!("pass_d {ds:?}, neck depth {nd}");
    assert_eq!(ds.iter().filter(|&&d| d > 0.0).count(), 1, "one cluster, priced once: {ds:?}");
    assert!((nd - 0.25).abs() < 0.02, "d = 0.5 -> d² = 0.25, got {nd} ({ds:?})");
    assert!((st.neck_depth(None) - nd).abs() < 1e-12);
}

/// The whole-board measure carries the neck depth: neck_k x Σ d² with neck_k = weight / violation weight
/// (1000 mm), so on the closed gap board (the neck is the only way) the part is proportional to the weight
/// and 0 at weight 0 (`optimise::neckcheck`, the last measure of the loop).
#[test]
fn the_measure_prices_the_neck_depth_by_its_weight() {
    use std::sync::atomic::Ordering;
    use topor_engine::topo::optimise::neckcheck;
    let m = gap_board();
    let part = |weight: f64| -> f64 {
        let _ = route(&m, "global", weight);
        f64::from_bits(neckcheck::LAST.load(Ordering::Relaxed))
    };
    let (p0, p1, p2) = (part(0.0), part(500.0), part(1000.0));
    eprintln!("neck part at weight 0 / 500 / 1000: {p0} / {p1} / {p2}");
    assert_eq!(p0, 0.0);
    assert!(p2 > 0.05 && p2 <= 1.0 + 1e-9, "one neck, d² in (0, 1]: {p2}");
    assert!((p2 - 2.0 * p1).abs() < 1e-9, "proportional to the weight: {p1} {p2}");
}

fn route(m: &BoardModel, judge: &str, weight: f64) -> f64 {
    let s = format!(r#"{{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 3, "runs": 1, "loop_judge": "{judge}", "factors": {{"weight": {{"neck_depth": {weight}, "shrinkage": 0.0, "necking": 0.0}}}}}}"#);
    let o = Options::from_json(&s).expect("options parse");
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let r = route_with(m, &o, &ctl);
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    let c = r.connections.iter().find(|c| c.net == 1).expect("net 1");
    c.length_nm as f64 / MM
}
