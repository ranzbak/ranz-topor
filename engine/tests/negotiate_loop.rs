use std::collections::HashSet;
use std::time::{Duration, Instant};
use topor_engine::decompose::{order, Connection, TermKind, TermRef};
use topor_engine::geom::{dequantise, Prim};
use topor_engine::graph::{CrossEntry, GraphRules, RouteGraph};
use topor_engine::model::{BoardModel, Options};
use topor_engine::negotiate::*;
use topor_engine::obstacle::ObstacleSet;
use topor_engine::search::planar::PlanarPath as SlotPath;
use topor_engine::sketch::{build_layer_cdt, ConstraintSet, LayerSketch};

const MM: f64 = 1_000_000.0;

fn seg(a: (i64, i64), b: (i64, i64)) -> String {
    format!(
        r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
        a.0 * 1_000_000,
        a.1 * 1_000_000,
        b.0 * 1_000_000,
        b.1 * 1_000_000
    )
}

fn outline50x40() -> String {
    [
        ((0, 0), (50, 0)),
        ((50, 0), (50, 40)),
        ((50, 40), (0, 40)),
        ((0, 40), (0, 0)),
    ]
    .iter()
    .map(|&(a, b)| seg(a, b))
    .collect::<Vec<_>>()
    .join(",")
}

fn round_pad(name: &str, net: i32, x_mm: f64, y_mm: f64) -> String {
    format!(
        r#"{{ "uuid": "{name}", "ref": "{name}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [0],
          "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        (x_mm * MM) as i64,
        (y_mm * MM) as i64
    )
}

struct Built {
    g: RouteGraph,
    ls: LayerSketch,
    obs: ObstacleSet,
}

fn small_pad(name: &str, net: i32, x_mm: f64, y_mm: f64) -> String {
    round_pad(name, net, x_mm, y_mm).replace("1000000 }", "300000 }")
}

fn build(pads: &[(&str, i32, f64, f64)], extra: &str) -> Built {
    let pads_json: Vec<String> = pads
        .iter()
        .map(|&(n, net, x, y)| {
            if n.starts_with('s') {
                small_pad(n, net, x, y)
            } else {
                round_pad(n, net, x, y)
            }
        })
        .collect();
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ], "pads": [ {} ], {extra} "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pads_json.join(","),
        outline50x40()
    );
    let m = BoardModel::from_json(&s).unwrap();
    let obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let ls = build_layer_cdt(obs.layer(0), &[], ConstraintSet::Routing).unwrap();
    let outline: Vec<Prim> = m.outline.iter().map(dequantise).collect();
    let rules = GraphRules {
        eps: 5_000.0,
        min_clearance: 0.0,
        edge_clearance: 500_000.0,
        hole_clearance: 250_000.0,
    };
    let g = RouteGraph::build(&ls, obs.layer(0), &outline, &[], &rules);
    Built { g, ls, obs }
}

fn vert(b: &Built, name: &str) -> usize {
    let label = format!("pad:{name}:1");
    let ai = b
        .obs
        .layer(0)
        .anchors
        .iter()
        .position(|a| a.label == label)
        .unwrap();
    b.ls.cdt.vertex_index(ai)
}

fn mkconn(b: &Built, id: usize, net: i32, from: &str, to: &str) -> (Connection, (usize, usize)) {
    let (s, t) = (vert(b, from), vert(b, to));
    let term = |v: usize, l: &str| TermRef {
        kind: TermKind::Pad(0),
        label: l.to_string(),
        at: b.g.verts[v].at,
        layers: vec![0],
    };
    let est = (b.g.verts[s].at - b.g.verts[t].at).len();
    (
        Connection {
            id,
            net,
            width: 250_000,
            clearance: 200_000,
            tier: 3,
            from: term(s, from),
            to: term(t, to),
            est_len: est,
        },
        (s, t),
    )
}

fn setup(b: &Built, specs: &[(i32, &str, &str)]) -> (Vec<Connection>, Vec<(usize, usize)>) {
    specs
        .iter()
        .enumerate()
        .map(|(i, &(n, f, t))| mkconn(b, i, n, f, t))
        .unzip()
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

fn run(
    b: &mut Built,
    conns: &[Connection],
    verts: &[(usize, usize)],
) -> (Negotiator, u32, Vec<Option<SlotPath>>, usize) {
    let (neg, over, last, fail, _) = run_first(b, conns, verts);
    (neg, over, last, fail)
}

type FirstRun = (
    Negotiator,
    u32,
    Vec<Option<SlotPath>>,
    usize,
    Vec<Option<SlotPath>>,
);

/// `run` plus the paths of iteration 1 (`IterOutcome.paths`).
fn run_first(b: &mut Built, conns: &[Connection], verts: &[(usize, usize)]) -> FirstRun {
    let mut first: Vec<Option<SlotPath>> = Vec::new();
    let mut neg = Negotiator::new(b.g.edges.len(), conns.len());
    let opts = Options::default();
    let mut active: Vec<usize> = (0..conns.len()).collect();
    let mut last = Vec::new();
    for _ in 0..opts.pathfinder_iter_cap {
        let out = neg.iteration(
            &mut b.g,
            b.obs.layer(0),
            conns,
            &active,
            verts,
            &opts,
            far(),
        );
        let over = neg.penalise_overuse(&b.g);
        for (&c, bi) in &out.blocked {
            neg.penalise_blocked(c, bi);
        }
        last = out.paths.clone();
        if first.is_empty() {
            first = out.paths.clone();
        }
        if over == 0 && out.blocked.is_empty() && out.ripped.is_empty() {
            return (neg, out.overuse_edges as u32, last, 0, first);
        }
        let mut a: std::collections::BTreeSet<usize> = out.blocked.keys().copied().collect();
        a.extend(out.ripped.iter().copied());
        a.extend(overused_conns(&b.g));
        active = a.into_iter().collect();
    }
    (neg, 99, last, 1, first)
}

/// A wall of pads at x = 25 with a single 2 mm gap (capacity for one wire) at y = 19..21;
/// the wall spans y = 13.4..26.6, so the detours above / below cost ~40 % more.
fn channel_board() -> Built {
    build(
        &[
            ("sa", 1, 10.0, 19.6),
            ("sb", 2, 10.0, 20.4),
            ("ta", 1, 40.0, 19.6),
            ("tb", 2, 40.0, 20.4),
            ("u1", 3, 25.0, 19.0),
            ("u2", 3, 25.0, 16.46),
            ("u3", 3, 25.0, 13.92),
            ("w1", 3, 25.0, 21.0),
            ("w2", 3, 25.0, 23.54),
            ("w3", 3, 25.0, 26.08),
        ],
        "",
    )
}

/// A sealed wall of pads (pitch 1.4 mm, gaps too narrow for a wire) at x = 12, 2 mm from the starts (the overuse penalty scales with the leg length) from y = 0.8 up
/// to y = 36.4 with the single 2 mm channel at y = 19..21 and an open top (the detour costs
/// ~2x): in iteration 1 both wires take the channel and only the history switches one away.
fn tall_channel_board() -> Built {
    let mut pads: Vec<(String, i32, f64, f64)> = vec![
        ("sa".into(), 1, 10.0, 19.6),
        ("sb".into(), 2, 10.0, 20.4),
        ("ta".into(), 1, 40.0, 19.6),
        ("tb".into(), 2, 40.0, 20.4),
    ];
    for k in 0..14 {
        pads.push((format!("u{}", k + 1), 3, 12.0, 19.0 - 1.4 * k as f64));
    }
    for k in 0..12 {
        pads.push((format!("w{}", k + 1), 3, 12.0, 21.0 + 1.4 * k as f64));
    }
    let refs: Vec<(&str, i32, f64, f64)> = pads
        .iter()
        .map(|(n, net, x, y)| (n.as_str(), *net, *x, *y))
        .collect();
    build(&refs, "")
}

fn run_channel() -> (Vec<Option<SlotPath>>, usize, Vec<usize>) {
    let mut b = channel_board();
    let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let (_, over, paths, fail) = run(&mut b, &conns, &verts);
    assert_eq!(fail, 0, "did not converge in the iteration cap");
    assert_eq!(over, 0);
    let ch = [b.g.edge(vert(&b, "u1"), vert(&b, "w1")).unwrap()];
    let users: Vec<usize> = (0..2)
        .filter(|&c| {
            let (x, _, _) = b.g.path_of(c).unwrap();
            x.iter().any(|(e, _)| ch.contains(e))
        })
        .collect();
    assert_eq!(
        (0..b.g.edges.len())
            .filter(|&e| b.g.overuse(e) > 0.0)
            .count(),
        0
    );
    (paths, users.len(), users)
}

#[test]
fn negotiate_contended_channel_routes_both() {
    let (p1, n1, _) = run_channel();
    assert!(p1.iter().all(|p| p.is_some()), "both Found");
    assert_eq!(n1, 1, "exactly one wire uses the channel");
    let (p2, _, _) = run_channel();
    let key = |p: &Vec<Option<SlotPath>>| -> Vec<Vec<(usize, usize)>> {
        p.iter()
            .map(|x| x.as_ref().unwrap().crossings.clone())
            .collect()
    };
    assert_eq!(key(&p1), key(&p2));
}

/// D4: the history switch is exercised: in iteration 1 both wires take the channel (overuse),
/// the overuse raises `history[channel]`, and the wire that ends up detouring is one that used
/// the channel in iteration 1.
#[test]
fn negotiate_contended_channel_history_switch() {
    let mut b = tall_channel_board();
    let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let (neg, over, last, fail, first) = run_first(&mut b, &conns, &verts);
    assert_eq!((fail, over), (0, 0));
    let ch = b.g.edge(vert(&b, "u1"), vert(&b, "w1")).unwrap();
    let uses = |p: &Option<SlotPath>| p.as_ref().unwrap().crossings.iter().any(|&(e, _)| e == ch);
    assert!(
        first.iter().all(uses),
        "both wires take the channel in iteration 1"
    );
    assert!(neg.history[ch] > 0.0, "history {}", neg.history[ch]);
    let detour: Vec<usize> = (0..2).filter(|&c| !uses(&last[c])).collect();
    assert_eq!(detour.len(), 1, "exactly one wire switched to the detour");
    assert!(uses(&first[detour[0]]));
}

#[test]
fn blocked_connection_is_bumped_first_next_iteration() {
    let b = channel_board();
    let (conns, _) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let mut neg = Negotiator::new(b.g.edges.len(), 2);
    assert_eq!(order(&conns, &neg.bumps)[0], 0);
    let info = BlockInfo {
        edge: None,
        ghost: None,
        last_reached: None,
    };
    neg.penalise_blocked(1, &info);
    assert_eq!(neg.bumps[1], 1);
    assert!(neg.history.iter().all(|&h| h == 0.0));
    assert_eq!(order(&conns, &neg.bumps)[0], 1);
}

#[test]
fn overuse_raises_history_on_overused_edges_only() {
    let mut b = channel_board();
    let (u1, w1) = (vert(&b, "u1"), vert(&b, "w1"));
    let e = b.g.edge(u1, w1).unwrap();
    let cap = b.g.edges[e].cap;
    let mut neg = Negotiator::new(b.g.edges.len(), 8);
    assert_eq!(neg.penalise_overuse(&b.g), 0);
    let mut c = 0;
    while b.g.overuse(e) == 0.0 {
        b.g.insert(
            c,
            e,
            c,
            CrossEntry {
                conn: c,
                width: 250_000.0,
                clr: 200_000.0,
                k: None,
            },
        );
        c += 1;
        assert!(c < 8, "cap {cap}");
    }
    assert_eq!(neg.penalise_overuse(&b.g), 1);
    for (i, &h) in neg.history.iter().enumerate() {
        assert_eq!(h, if i == e { HIST_OVERUSE } else { 0.0 });
    }
}

#[test]
fn u_clinch_detected_and_regular_absent() {
    let mut b = build(
        &[
            ("n1", 1, 20.0, 20.0),
            ("n2", 1, 30.0, 20.0),
            ("f1", 2, 25.0, 28.0),
            ("f2", 2, 25.0, 12.0),
        ],
        "",
    );
    // F first so that it crosses the N edge, then N detours.
    let (conns, verts) = setup(&b, &[(2, "f1", "f2"), (1, "n1", "n2")]);
    let e = b.g.edge(verts[1].0, verts[1].1).expect("N edge exists");
    let opts = Options::default();
    let mut neg = Negotiator::new(b.g.edges.len(), 2);
    let out = neg.iteration(&mut b.g, b.obs.layer(0), &conns, &[0], &verts, &opts, far());
    assert!(out.paths[0].is_some());
    assert!(
        b.g.edges[e].cross.iter().any(|x| x.conn == 0),
        "F crosses the N edge"
    );
    let out = neg.iteration(&mut b.g, b.obs.layer(0), &conns, &[1], &verts, &opts, far());
    assert!(
        !out.paths[1].as_ref().unwrap().crossings.is_empty(),
        "N detours"
    );
    let rep = detect_clinches(&b.g, &conns, &out.paths, &verts);
    assert_eq!(rep.u, vec![(0, e)]);
    assert!(rep.regular.is_empty());
}

#[test]
fn iteration_respects_deadline() {
    let mut b = channel_board();
    let (conns, verts) = setup(&b, &[(1, "sa", "ta"), (2, "sb", "tb")]);
    let mut neg = Negotiator::new(b.g.edges.len(), 2);
    let out = neg.iteration(
        &mut b.g,
        b.obs.layer(0),
        &conns,
        &[0, 1],
        &verts,
        &Options::default(),
        Instant::now(),
    );
    assert!(out.timed_out);
    assert!(out.paths.iter().all(|p| p.is_none()));
}

/// Fix B: A (a1-a2, 10 mm) is routed first and cuts B's only short class (the b1-b2 CDT edge,
/// 5 mm); B's alternative around a1 is more than twice as long, so B's ghost path rips A. A then
/// takes the longer way round b1 and both converge, B on its short class.
#[test]
fn blocked_by_strand_rips_up_blocker() {
    let mut b = build(
        &[
            ("a1", 1, 20.0, 20.0),
            ("a2", 1, 30.0, 20.0),
            ("b1", 2, 25.0, 17.5),
            ("b2", 2, 25.0, 22.5),
        ],
        "",
    );
    let (mut conns, verts) = setup(&b, &[(1, "a1", "a2"), (2, "b1", "b2")]);
    conns[0].tier = 2; // A is routed first
    let (_, _, paths, fail) = run(&mut b, &conns, &verts);
    assert_eq!(fail, 0, "did not converge in the iteration cap");
    let pb = paths[1].as_ref().expect("B routed");
    assert!(
        pb.est_len < 6.0 * MM,
        "B keeps its short class: {}",
        pb.est_len
    );
    let pa = paths[0].as_ref().expect("A routed");
    assert!(pa.est_len > 10.0 * MM, "A detours: {}", pa.est_len);
    assert_eq!(
        (0..b.g.edges.len())
            .filter(|&e| b.g.overuse(e) > 0.0)
            .count(),
        0
    );
}
