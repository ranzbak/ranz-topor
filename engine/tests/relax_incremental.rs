//! Fix-B2: incremental metrization (`Relaxer`, design note `m2-metrize-design.md` §2).
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use topor_engine::geom::{Orient, Vec2};
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::relax::{relax_slots, Relaxer, RunOpts, SlotNesting};
use topor_engine::sketch::{AnchorRef, Sketch};
use topor_engine::taut::{Chain, TautError};
use topor_engine::wires::{Terminal, Wire};

const MM: f64 = 1_000_000.0;

fn v(x: f64, y: f64) -> Vec2 {
    Vec2 {
        x: x * MM,
        y: y * MM,
    }
}

fn pad(net: i32, x: f64, y: f64, d: i64) -> String {
    format!(
        r#"{{ "uuid": "p", "ref": "U1", "number": "1", "net": {net},
            "center": [{}, {}], "layers": [0],
            "shapes": {{ "0": {{ "kind": "circle", "d": {d} }} }} }}"#,
        (x * MM) as i64,
        (-y * MM) as i64
    )
}

fn obstacles(pads: &[String]) -> ObstacleSet {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }} ], "pads": [ {} ], "outline": [ ],
             "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }},
             "per_net": {{ }},
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

fn far() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn flat(c: &Result<Chain, TautError>) -> String {
    match c {
        Ok(c) => format!(
            "{:?}",
            c.prims
                .iter()
                .map(|p| {
                    let b = p.bbox();
                    (
                        b.min.x.to_bits(),
                        b.min.y.to_bits(),
                        b.max.x.to_bits(),
                        b.max.y.to_bits(),
                    )
                })
                .collect::<Vec<_>>()
        ),
        Err(e) => format!("{e:?}"),
    }
}

fn flatten(chains: &[Result<Chain, TautError>]) -> Vec<String> {
    chains.iter().map(flat).collect()
}

type Inner = ((usize, AnchorRef), Vec<Vec<usize>>);

fn nesting(inner: &[Inner]) -> SlotNesting {
    SlotNesting {
        inner: inner.iter().cloned().collect(),
        sides: HashMap::new(),
    }
}

fn run_opts(opts: &Options, deadline: Instant, anytime: bool) -> RunOpts<'_> {
    RunOpts {
        opts,
        deadline,
        zs: None,
        anytime,
    }
}

/// Two independent groups 10 mm apart, each three pads in a row (anchors 0..3 below, 3..6
/// above) with three wires below the pads wrapping them Ccw: wire 0 nested inside 1 inside 2,
/// wire 3 inside 4 inside 5.
fn two_groups() -> (ObstacleSet, Vec<Wire>, Vec<Sketch>, SlotNesting) {
    let mut pads: Vec<String> = Vec::new();
    for g in 0..2 {
        for x in [3.0, 6.0, 9.0] {
            pads.push(pad(2, x, -0.5 + 10.0 * g as f64, 800_000));
        }
    }
    let obs = obstacles(&pads);
    let mut wires = Vec::new();
    let mut sk = Vec::new();
    let mut inner: Vec<Inner> = Vec::new();
    for i in 0..6 {
        let (g, j) = (i / 3, i % 3);
        let y = -0.5 + 10.0 * g as f64 - [0.5, 1.1, 1.6][j];
        wires.push(wire(i, 20 + i as i32, 250_000, v(0.0, y), v(12.0, y)));
        let seq: Vec<AnchorRef> = (0..3).map(|p| aref(3 * g + p, Orient::Ccw)).collect();
        for &a in &seq {
            inner.push(((i, a), vec![(3 * g..i).collect()]));
        }
        sk.push(sketch(i, seq));
    }
    (obs, wires, sk, nesting(&inner))
}

fn fresh(wires: &[Wire], sk: &[Sketch], obs: &ObstacleSet, n: &SlotNesting) -> Relaxer {
    let mut rx = Relaxer::new(wires.to_vec(), obs);
    for (k, s) in sk.iter().enumerate() {
        rx.set(k, &s.seq);
    }
    rx.set_nesting(n);
    rx
}

#[test]
fn run_all_from_scratch_equals_full_relax_and_rerun_is_a_no_op() {
    let (obs, wires, sk, n) = two_groups();
    let full = relax_slots(&wires, &sk, &obs, &n, &Options::default(), far(), None);
    assert!(full.converged && full.chains.iter().all(Result::is_ok));
    let mut rx = fresh(&wires, &sk, &obs, &n);
    let o = Options::default();
    let st = rx.run(&[], &obs, &run_opts(&o, far(), true));
    assert!(st.converged && !st.timed_out);
    assert_eq!(st.sweeps, full.iterations);
    assert_eq!(flatten(rx.chains()), flatten(&full.chains));
    // Empty change set: nothing is re-tautened, every chain is byte-identical.
    let before = flatten(rx.chains());
    let st = rx.run(&[], &obs, &run_opts(&o, far(), true));
    assert_eq!(st.tautened, Vec::<usize>::new());
    assert_eq!(st.sweeps, 0);
    assert!(st.converged);
    assert_eq!(flatten(rx.chains()), before);
    // Same nesting again: no wire is dirty.
    rx.set_nesting(&n);
    let st = rx.run(&[], &obs, &run_opts(&o, far(), true));
    assert_eq!(st.tautened, Vec::<usize>::new());
    assert_eq!(flatten(rx.chains()), before);
}

#[test]
fn first_call_completes_a_pass_before_the_deadline_counts() {
    let (obs, wires, sk, n) = two_groups();
    let mut rx = fresh(&wires, &sk, &obs, &n);
    let o = Options::default();
    let st = rx.run(&[], &obs, &run_opts(&o, Instant::now(), true));
    assert!(st.timed_out, "{st:?}");
    assert_eq!(st.sweeps, 1);
    assert!(
        rx.chains().iter().all(Result::is_ok),
        "{:?}",
        flatten(rx.chains())
    );
    // Not anytime (hand-mode wrappers): the deadline is honoured before the first tauten.
    let mut rx = fresh(&wires, &sk, &obs, &n);
    let st = rx.run(&[], &obs, &run_opts(&o, Instant::now(), false));
    assert!(st.timed_out);
    assert!(rx.chains().iter().all(Result::is_err));
}

#[test]
fn moving_one_wire_retautens_its_neighbours_only() {
    let (obs, wires, sk, n) = two_groups();
    let mut rx = fresh(&wires, &sk, &obs, &n);
    let o = Options::default();
    rx.run(&[], &obs, &run_opts(&o, far(), true));
    let before = flatten(rx.chains());
    // Wire 0 (innermost of the lower group) leaves: wires 1 and 2 lose a stacked wire and are
    // re-tautened; the upper group is not touched.
    rx.remove(0);
    let st = rx.run(&[], &obs, &run_opts(&o, far(), true));
    assert!(st.converged);
    assert!(st.tautened.contains(&1), "{:?}", st.tautened);
    assert!(
        st.tautened.iter().all(|&k| k == 1 || k == 2),
        "{:?}",
        st.tautened
    );
    let after = flatten(rx.chains());
    assert!(rx.chains()[0].is_err());
    assert_ne!(after[1], before[1], "wire 1 keeps the old stack radius");
    assert_eq!(&after[3..], &before[3..]);
    // Back in: only the lower group is tautened and the chains return to the old fixed point.
    rx.set(0, &sk[0].seq);
    let st = rx.run(&[], &obs, &run_opts(&o, far(), true));
    assert!(
        st.tautened.contains(&0) && st.tautened.contains(&1),
        "{:?}",
        st.tautened
    );
    assert!(st.tautened.iter().all(|&k| k < 3), "{:?}", st.tautened);
    assert_eq!(flatten(rx.chains()), before);
}

#[test]
fn nesting_change_marks_only_the_affected_wire_dirty() {
    let (obs, wires, sk, n) = two_groups();
    let mut rx = fresh(&wires, &sk, &obs, &n);
    let o = Options::default();
    rx.run(&[], &obs, &run_opts(&o, far(), true));
    // Wire 2 no longer has wire 1 inside it at pad 0: only wire 2 is dirty.
    let mut n2 = n.clone();
    n2.inner.insert((2, aref(0, Orient::Ccw)), vec![vec![0]]);
    let dirty = rx.set_nesting(&n2);
    assert_eq!(dirty, vec![2]);
    let st = rx.run(&[], &obs, &run_opts(&o, far(), true));
    assert!(st.tautened.contains(&2), "{:?}", st.tautened);
    assert!(st.tautened.iter().all(|&k| k < 3), "{:?}", st.tautened);
}
