use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use topor_engine::geom::{copper_gap, Orient, Prim, Track, Vec2};
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::relax::{relax, RelaxResult, Stacks};
use topor_engine::sketch::{AnchorRef, Sketch};
use topor_engine::taut::Chain;
use topor_engine::wires::{Terminal, Wire};

const MM: f64 = 1_000_000.0;

fn v(x: f64, y: f64) -> Vec2 {
    Vec2 {
        x: x * MM,
        y: y * MM,
    }
}

/// Pad (math-frame mm); the JSON is in the KiCad frame (y flipped).
fn pad(net: i32, x: f64, y: f64, d: i64) -> String {
    format!(
        r#"{{ "uuid": "p", "ref": "U1", "number": "1", "net": {net},
            "center": [{}, {}], "layers": [0],
            "shapes": {{ "0": {{ "kind": "circle", "d": {d} }} }} }}"#,
        (x * MM) as i64,
        (-y * MM) as i64
    )
}

fn obstacles(pads: &[String], per_net: &str) -> ObstacleSet {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }} ], "pads": [ {} ], "outline": [ ],
             "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }},
             "per_net": {{ {per_net} }},
             "min_clearance": 0, "hole_clearance": 250000, "edge_clearance": 200000 }} }}"#,
        pads.join(",")
    );
    let m = BoardModel::from_json(&s).unwrap();
    ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[])
}

fn wire(id: usize, net: i32, width: i64, a: Vec2, b: Vec2) -> Wire {
    Wire {
        id,
        net,
        layer: 0,
        width,
        start: Terminal::Dangling { at: a },
        goal: Terminal::Dangling { at: b },
        hand: vec![a, b],
        kept_uuids: vec![],
        hand_len: (b - a).len(),
    }
}

fn aref(anchor: usize, sigma: Orient) -> AnchorRef {
    AnchorRef { anchor, sigma }
}

fn sketch(wire: usize, seq: Vec<AnchorRef>) -> Sketch {
    Sketch {
        wire,
        seq,
        crossings: vec![],
    }
}

type Orders = HashMap<u32, HashMap<AnchorRef, Vec<usize>>>;

fn orders(entries: &[(AnchorRef, Vec<usize>)]) -> Orders {
    HashMap::from([(0, entries.iter().cloned().collect())])
}

fn run(wires: &[Wire], seqs: &[Vec<AnchorRef>], obs: &ObstacleSet, o: &Orders) -> RelaxResult {
    run_with(wires, seqs, obs, o, &Options::default(), far())
}

fn run_with(
    wires: &[Wire],
    seqs: &[Vec<AnchorRef>],
    obs: &ObstacleSet,
    o: &Orders,
    opts: &Options,
    deadline: Instant,
) -> RelaxResult {
    let sk: Vec<Sketch> = seqs
        .iter()
        .enumerate()
        .map(|(i, s)| sketch(i, s.clone()))
        .collect();
    relax(wires, &sk, obs, o, opts, deadline)
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn arcs(c: &Chain) -> Vec<(f64, usize)> {
    c.prims
        .iter()
        .enumerate()
        .filter_map(|(i, p)| match p {
            Prim::Arc(a) => Some((a.r, i)),
            _ => None,
        })
        .collect()
}

fn gap(a: &Prim, wa: f64, b: &Prim, wb: f64) -> f64 {
    copper_gap(&Track { prim: *a, w: wa }, &Track { prim: *b, w: wb })
}

fn min_gap(a: &Chain, wa: f64, b: &Chain, wb: f64) -> f64 {
    let mut m = f64::MAX;
    for p in &a.prims {
        for q in &b.prims {
            m = m.min(gap(p, wa, q, wb));
        }
    }
    m
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Pad A at (5, -0.8) r_core 0.4; wire 0 (w 0.25) passes at y = -1, wire 1 (w 0.4) at y = -2;
/// both wrap A with the pad on their left, wire 0 nested inside wire 1.
fn two_wire_setup(per_net: &str) -> (ObstacleSet, Vec<Wire>, Vec<Vec<AnchorRef>>, Orders) {
    let obs = obstacles(&[pad(2, 5.0, -0.8, 800_000)], per_net);
    let wires = vec![
        wire(0, 10, 250_000, v(0.0, -1.0), v(10.0, -1.0)),
        wire(1, 11, 400_000, v(0.0, -2.0), v(10.0, -2.0)),
    ];
    let a = aref(0, Orient::Ccw);
    let seqs = vec![vec![a], vec![a]];
    (obs, wires, seqs, orders(&[(a, vec![0, 1])]))
}

#[test]
fn two_wires_same_side_get_stack_radii() {
    let (obs, wires, seqs, o) = two_wire_setup("");
    let r = run(&wires, &seqs, &obs, &o);
    let c0 = r.chains[0].as_ref().unwrap();
    let c1 = r.chains[1].as_ref().unwrap();
    assert!(near(arcs(c0)[0].0, 730_000.0, 1.0), "{:?}", arcs(c0));
    assert!(near(arcs(c1)[0].0, 1_260_000.0, 1.0), "{:?}", arcs(c1));
    let g = min_gap(c0, 250_000.0, c1, 400_000.0);
    assert!(g >= 205_000.0 - 1.0, "gap {g}");
    assert!(r.converged);
}

#[test]
fn radius_formula_with_mixed_clearance() {
    // net 11 needs clearance 0.3: R_1 = max(0.4+0.3+0.2+0.005, 0.73+0.125+0.305+0.2) = 1.36
    let (obs, wires, seqs, o) = two_wire_setup(r#""11": { "clearance": 300000 }"#);
    let st = Stacks::new(o[&0].clone());
    let a = aref(0, Orient::Ccw);
    assert!(near(st.radius(a, 0, &wires, &obs), 730_000.0, 1e-6));
    assert!(near(st.radius(a, 1, &wires, &obs), 1_360_000.0, 1e-6));
    let r = run(&wires, &seqs, &obs, &o);
    assert!(near(
        arcs(r.chains[1].as_ref().unwrap())[0].0,
        1_360_000.0,
        1.0
    ));
}

#[test]
fn inner_wire_leaving_stack_shrinks_outer() {
    // wire 0 hand path moves far away from the pad (no anchors); wire 1 hugs the pad at y = -1.4
    let obs = obstacles(&[pad(2, 5.0, -0.8, 800_000)], "");
    let wires = vec![
        wire(0, 10, 250_000, v(0.0, 4.0), v(10.0, 4.0)),
        wire(1, 11, 400_000, v(0.0, -1.4), v(10.0, -1.4)),
    ];
    let a = aref(0, Orient::Ccw);
    let o = orders(&[(a, vec![0, 1])]);
    let r = run(&wires, &[vec![], vec![a]], &obs, &o);
    let c1 = r.chains[1].as_ref().unwrap();
    // 0.73 + (0.4 - 0.25) / 2 = rho of the 0.4 wire = 0.805
    assert!(near(arcs(c1)[0].0, 805_000.0, 1.0), "{:?}", arcs(c1));
    assert!(r.iterations >= 2);
}

#[test]
fn parallel_runs_keep_clearance() {
    // pads A (3, -0.5), B (8, -0.5); both wires wrap A then B with the pads on their left
    let obs = obstacles(
        &[pad(2, 3.0, -0.5, 800_000), pad(2, 8.0, -0.5, 800_000)],
        "",
    );
    let wires = vec![
        wire(0, 10, 250_000, v(0.0, -1.0), v(11.0, -1.0)),
        wire(1, 11, 250_000, v(0.0, -1.6), v(11.0, -1.6)),
    ];
    let (a, b) = (aref(0, Orient::Ccw), aref(1, Orient::Ccw));
    let o = orders(&[(a, vec![0, 1]), (b, vec![0, 1])]);
    let r = run(&wires, &[vec![a, b], vec![a, b]], &obs, &o);
    let c0 = r.chains[0].as_ref().unwrap();
    let c1 = r.chains[1].as_ref().unwrap();
    assert_eq!(c0.pattern(), "SASAS");
    assert_eq!(c1.pattern(), "SASAS");
    let g = gap(&c0.prims[2], 250_000.0, &c1.prims[2], 250_000.0);
    assert!(near(g, 205_000.0, 1_000.0), "straight-run gap {g}");
    assert!(min_gap(c0, 250_000.0, c1, 250_000.0) >= 205_000.0 - 1.0);
}

fn six_wires() -> (ObstacleSet, Vec<Wire>, Vec<Vec<AnchorRef>>, Orders) {
    let pads: Vec<String> = [3.0, 6.0, 9.0]
        .iter()
        .map(|&x| pad(2, x, -0.5, 800_000))
        .collect();
    let obs = obstacles(&pads, "");
    let below = [-1.0, -1.6, -2.1];
    let above = [0.0, 0.6, 1.1];
    let mut wires = Vec::new();
    let mut seqs = Vec::new();
    for (i, y) in below.iter().chain(above.iter()).enumerate() {
        wires.push(wire(i, 20 + i as i32, 250_000, v(0.0, *y), v(12.0, *y)));
        let sg = if i < 3 { Orient::Ccw } else { Orient::Cw };
        seqs.push((0..3).map(|p| aref(p, sg)).collect());
    }
    let mut ent = Vec::new();
    for p in 0..3 {
        ent.push((aref(p, Orient::Ccw), vec![0, 1, 2]));
        ent.push((aref(p, Orient::Cw), vec![3, 4, 5]));
    }
    (obs, wires, seqs, orders(&ent))
}

fn flatten(r: &RelaxResult) -> Vec<String> {
    r.chains
        .iter()
        .map(|c| match c {
            Ok(c) => format!(
                "{:?}",
                c.prims
                    .iter()
                    .map(|p| p.bbox().min.x.to_bits())
                    .collect::<Vec<_>>()
            ),
            Err(e) => format!("{e:?}"),
        })
        .collect()
}

#[test]
fn converges_and_is_deterministic() {
    let (obs, wires, seqs, o) = six_wires();
    let r1 = run(&wires, &seqs, &obs, &o);
    let r2 = run(&wires, &seqs, &obs, &o);
    assert!(r1.converged, "iterations {}", r1.iterations);
    assert!(r1.iterations <= 10, "iterations {}", r1.iterations);
    assert!(r1.chains.iter().all(|c| c.is_ok()), "{:?}", flatten(&r1));
    assert_eq!(flatten(&r1), flatten(&r2));
    assert_eq!(r1.iterations, r2.iterations);
}

#[test]
fn relax_iter_cap_respected() {
    let (obs, wires, seqs, o) = two_wire_setup("");
    let opts = Options {
        relax_iter_cap: 1,
        ..Options::default()
    };
    let r = run_with(&wires, &seqs, &obs, &o, &opts, far());
    assert_eq!(r.iterations, 1);
    assert!(!r.converged);
    assert!(!r.timed_out);
}

#[test]
fn deadline_sets_timed_out() {
    let (obs, wires, seqs, o) = six_wires();
    let t = Instant::now();
    let r = run_with(&wires, &seqs, &obs, &o, &Options::default(), Instant::now());
    assert!(r.timed_out);
    assert!(!r.converged);
    assert_eq!(r.chains.len(), 6);
    assert!(t.elapsed() < Duration::from_millis(50));
}

#[test]
fn failed_wire_does_not_block_others() {
    // wire 0 starts inside the foreign pad's inflated circle; wire 1 is clear
    let obs = obstacles(&[pad(2, 5.0, 0.0, 800_000)], "");
    let wires = vec![
        wire(0, 10, 250_000, v(5.2, 0.0), v(10.0, 3.0)),
        wire(1, 11, 250_000, v(0.0, 3.0), v(10.0, 3.0)),
    ];
    let r = run(&wires, &[vec![], vec![]], &obs, &orders(&[]));
    assert!(r.chains[0].is_err());
    assert!(r.chains[1].is_ok());
    assert!(r.converged);
}
