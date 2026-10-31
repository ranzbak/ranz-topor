use std::collections::HashSet;
use std::time::{Duration, Instant};
use topor_engine::decompose::{Connection, TermKind, TermRef};
use topor_engine::geom::{dequantise, Prim, Vec2};
use topor_engine::graph::{CrossEntry, GraphRules, Port, RouteGraph};
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::search::{ghost, search, SearchOutcome, SearchParams, SlotPath};
use topor_engine::sketch::{build_layer_cdt, ConstraintSet, LayerSketch};

const MM: f64 = 1_000_000.0;
const W: f64 = 250_000.0;
const C: f64 = 200_000.0;

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

fn build(pads: &[(&str, i32, f64, f64)], extra: &str) -> Built {
    let pads_json: Vec<String> = pads
        .iter()
        .map(|&(n, net, x, y)| round_pad(n, net, x, y))
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

fn conn(b: &Built, id: usize, net: i32, from: &str, to: &str) -> (Connection, usize, usize) {
    let (s, t) = (vert(b, from), vert(b, to));
    let term = |v: usize, l: &str| TermRef {
        kind: TermKind::Pad(0),
        label: l.to_string(),
        at: b.g.verts[v].at,
    };
    (
        Connection {
            id,
            net,
            layer: 0,
            width: 250_000,
            clearance: 200_000,
            tier: 3,
            from: term(s, from),
            to: term(t, to),
            est_len: 0.0,
        },
        s,
        t,
    )
}

fn params(hist: &[f64]) -> SearchParams<'_> {
    SearchParams {
        history: hist,
        weight: 1.0,
        present_overuse: 4.0,
        expansion_cap: usize::MAX,
        deadline: Instant::now() + Duration::from_secs(30),
    }
}

fn found(o: SearchOutcome) -> SlotPath {
    match o {
        SearchOutcome::Found(p) => p,
        other => panic!("expected Found, got {other:?}"),
    }
}

fn route(b: &mut Built, id: usize, net: i32, from: &str, to: &str) -> SearchOutcome {
    let (c, s, t) = conn(b, id, net, from, to);
    let hist = vec![0.0; b.g.edges.len()];
    let out = search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist));
    if let SearchOutcome::Found(p) = &out {
        b.g.insert_path(id, s, t, &p.crossings, &p.faces, W, C);
    }
    out
}

/// No two strands cross inside any face (independent re-derivation of the cyclic port order).
fn assert_planar(g: &RouteGraph) {
    for (f, face) in g.faces.iter().enumerate() {
        let mut seq: Vec<Port> = Vec::new();
        for j in 0..3 {
            let v = face.v[j];
            seq.push(Port::Vertex(v));
            let e = face.e[(j + 2) % 3];
            let n = g.slot_count(e);
            let order: Vec<usize> = if g.edges[e].a == v {
                (0..n).collect()
            } else {
                (0..n).rev().collect()
            };
            seq.extend(order.into_iter().map(|i| Port::Cross(e, i)));
        }
        let pos = |p: Port| seq.iter().position(|&x| x == p);
        let mut strands = Vec::new();
        for &e in &face.e {
            for i in 0..g.slot_count(e) {
                let q = g.strand(e, i, f);
                if q == Port::Detached {
                    continue;
                }
                let (a, b) = (pos(Port::Cross(e, i)).unwrap(), pos(q).unwrap());
                strands.push((a.min(b), a.max(b)));
            }
        }
        for &(a0, a1) in &strands {
            for &(b0, b1) in &strands {
                assert!(
                    !(a0 < b0 && b0 < a1 && a1 < b1),
                    "face {f}: strands ({a0},{a1}) and ({b0},{b1}) cross"
                );
            }
        }
    }
}

#[test]
fn search_adjacent_terminals_zero_crossings() {
    let mut b = build(&[("a", 1, 10.0, 10.0), ("b", 1, 20.0, 10.0)], "");
    let (c, s, t) = conn(&b, 0, 1, "a", "b");
    let e = b.g.edge(s, t).expect("adjacent CDT vertices");
    let hist = vec![0.0; b.g.edges.len()];
    let p = found(search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)));
    assert!(p.crossings.is_empty());
    assert!((p.est_len - 10.0 * MM).abs() < 1.0, "{}", p.est_len);
    b.g.insert_path(0, s, t, &p.crossings, &p.faces, W, C);
    assert_eq!(b.g.along(e), Some(0));
    // the edge is now taken: a second connection cannot run along it
    let (c2, s2, t2) = conn(&b, 1, 1, "a", "b");
    let o = search(&b.g, b.obs.layer(0), &c2, s2, t2, &params(&hist));
    if let SearchOutcome::Found(p2) = &o {
        assert!(!p2.crossings.is_empty());
    }
}

#[test]
fn search_goes_around_a_foreign_pad() {
    let mut b = build(
        &[
            ("a", 1, 5.0, 10.0),
            ("b", 1, 35.0, 10.0),
            ("x", 2, 20.0, 10.0),
        ],
        "",
    );
    let p = found(route(&mut b, 0, 1, "a", "b"));
    assert!(!p.crossings.is_empty());
    assert_eq!(p.faces.len(), p.crossings.len() + 1);
    assert!(
        p.est_len >= 30.0 * MM && p.est_len <= 30.6 * MM,
        "{}",
        p.est_len
    );
    // all crossings on one side of the pad centre
    let x = vert(&b, "x");
    let side = |e: usize| {
        let ge = &b.g.edges[e];
        let m = (b.g.verts[ge.a].at + b.g.verts[ge.b].at) * 0.5;
        (m.y - b.g.verts[x].at.y).signum()
    };
    let s0 = side(p.crossings[0].0);
    assert!(p.crossings.iter().all(|&(e, _)| side(e) == s0));
    assert_planar(&b.g);
}

#[test]
fn search_respects_capacity() {
    let mut b = build(
        &[
            ("a", 1, 10.0, 11.0),
            ("b", 1, 30.0, 11.0),
            ("u", 2, 20.0, 10.0),
            ("w", 2, 20.0, 12.0),
        ],
        "",
    );
    let (u, w) = (vert(&b, "u"), vert(&b, "w"));
    let ch = b.g.edge(u, w).expect("channel edge");
    assert!(b.g.fits(ch, W, C));
    // a wire already runs through the channel (pre-placed with insert_path)
    let f = [
        b.g.edges[ch].faces[0].unwrap(),
        b.g.edges[ch].faces[1].unwrap(),
    ];
    let third = |f: usize| *b.g.faces[f].v.iter().find(|&&v| v != u && v != w).unwrap();
    b.g.insert_path(7, third(f[0]), third(f[1]), &[(ch, 0)], &f, W, C);
    assert!(!b.g.fits(ch, W, C));
    let p = match route(&mut b, 0, 1, "a", "b") {
        SearchOutcome::Found(p) => p,
        SearchOutcome::Blocked { .. } => return,
        o => panic!("{o:?}"),
    };
    for &(e, _) in &p.crossings {
        assert_ne!(e, ch, "crossed the full channel edge");
    }
    assert_planar(&b.g);
}

#[test]
fn slot_transition_nests_corner_groups() {
    let mut b = build(
        &[
            ("a", 1, 5.0, 10.0),
            ("b", 1, 35.0, 10.0),
            ("x", 2, 20.0, 10.0),
        ],
        "",
    );
    let first = found(route(&mut b, 0, 1, "a", "b"));
    let second = found(route(&mut b, 1, 1, "a", "b"));
    assert_planar(&b.g);
    // the second wire nests outside the first on every shared sleeve edge
    let shared: Vec<usize> = first
        .crossings
        .iter()
        .map(|c| c.0)
        .filter(|e| second.crossings.iter().any(|c| c.0 == *e))
        .collect();
    assert!(!shared.is_empty());
    for e in shared {
        let slot_of = |conn: usize| {
            b.g.path_of(conn)
                .unwrap()
                .0
                .iter()
                .find(|c| c.0 == e)
                .unwrap()
                .1
        };
        assert_ne!(slot_of(0), slot_of(1), "edge {e}");
    }
    // a third connection of another net still keeps planarity
    let mut b2 = build(
        &[
            ("a", 1, 5.0, 10.0),
            ("b", 1, 35.0, 10.0),
            ("x", 2, 20.0, 10.0),
            ("c", 3, 12.0, 14.0),
            ("d", 3, 28.0, 14.0),
            ("e", 4, 12.0, 6.0),
            ("f", 4, 28.0, 6.0),
        ],
        "",
    );
    for (i, (net, from, to)) in [(1, "a", "b"), (3, "c", "d"), (4, "e", "f")]
        .into_iter()
        .enumerate()
    {
        let _ = route(&mut b2, i, net, from, to);
        assert_planar(&b2.g);
    }
}

#[test]
fn planar_invariant_holds_for_many_connections() {
    let names = ["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9"];
    let mut pads = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let x = 6.0 + 4.0 * (i % 5) as f64 + if i / 5 == 1 { 1.5 } else { 0.0 };
        let y = 10.0 + 6.0 * (i / 5) as f64 + 0.7 * (i % 3) as f64;
        pads.push((*n, 1 + (i as i32 % 5), x, y));
    }
    let mut b = build(&pads, "");
    // nets: i % 5 + 1, so (i, i+5) share a net
    let mut routed = 0;
    for i in 0..5 {
        let (from, to) = (names[i], names[i + 5]);
        if let SearchOutcome::Found(_) = route(&mut b, i, 1 + i as i32, from, to) {
            routed += 1;
        }
        assert_planar(&b.g);
    }
    assert!(routed >= 3, "routed {routed}");
}

/// Fix A: a bend around a pad pinched against the outline is inadmissible: the pad (r 0.5,
/// wrap radius 0.83) sits 1.0 mm from the right edge, the wire band needs 0.63 mm beside it.
#[test]
fn search_rejects_bend_pinched_against_wall() {
    let mut b = build(
        &[
            ("a", 1, 49.0, 10.0),
            ("p", 9, 49.0, 20.0),
            ("b", 1, 49.0, 30.0),
        ],
        "",
    );
    let (c, s, t) = conn(&b, 0, 1, "a", "b");
    let hist = vec![0.0; b.g.edges.len()];
    let o = search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist));
    match o {
        SearchOutcome::Found(p) => {
            // the inner class: no crossed edge reaches the right-hand outline corners
            for &(e, _) in &p.crossings {
                let ge = &b.g.edges[e];
                for v in [ge.a, ge.b] {
                    assert!(
                        b.g.verts[v].at.x < 49.9 * MM,
                        "path squeezes between the pad and the right edge"
                    );
                }
            }
        }
        SearchOutcome::Blocked { .. } => {}
        SearchOutcome::Timeout => panic!("timeout"),
    }
    b.g.remove_conn(0);
}

#[test]
fn search_blocked_by_wall_reports_ghost() {
    let kept = r#""kept": [ { "uuid": "k9", "net": 9, "layer": 0, "width": 250000, "locked": true,
        "prim": { "kind": "seg", "a": [20000000, 0], "b": [20000000, 40000000] } } ],"#;
    let mut b = build(&[("a", 1, 10.0, 20.0), ("b", 1, 30.0, 20.0)], kept);
    let (c, s, t) = conn(&b, 0, 1, "a", "b");
    let hist = vec![0.0; b.g.edges.len()];
    let o = search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist));
    let SearchOutcome::Blocked {
        ghost: Some(gp),
        blocked_edge: Some(e),
        last_reached,
    } = o
    else {
        panic!("expected Blocked with ghost, got {o:?}");
    };
    assert!(b.g.edges[e].constraint && !b.g.passable(e, 1));
    let (u, v) = (b.g.edges[e].a, b.g.edges[e].b);
    assert_eq!(
        b.ls.constraint_owner[&(u, v)].3,
        "k9".replace("k9", "kept:k9")
    );
    let k = gp.crossings.iter().position(|c| c.0 == e).unwrap();
    assert_eq!(last_reached, k.checked_sub(1).map(|i| gp.crossings[i]));
    // ghost() agrees
    let (g2, be, lr) = ghost(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)).unwrap();
    assert_eq!((be, lr), (Some(e), last_reached));
    assert_eq!(g2.crossings, gp.crossings);
    b.g.remove_conn(0);
}

#[test]
fn search_expansion_cap_and_deadline() {
    let b = build(
        &[
            ("a", 1, 5.0, 10.0),
            ("b", 1, 35.0, 10.0),
            ("x", 2, 20.0, 10.0),
        ],
        "",
    );
    let (c, s, t) = conn(&b, 0, 1, "a", "b");
    let hist = vec![0.0; b.g.edges.len()];
    let mut p = params(&hist);
    p.expansion_cap = 1;
    let o = search(&b.g, b.obs.layer(0), &c, s, t, &p);
    assert!(
        matches!(o, SearchOutcome::Blocked { .. } | SearchOutcome::Timeout),
        "{o:?}"
    );
    let mut p = params(&hist);
    p.deadline = Instant::now() - Duration::from_secs(1);
    assert!(matches!(
        search(&b.g, b.obs.layer(0), &c, s, t, &p),
        SearchOutcome::Timeout
    ));
}

#[test]
fn search_is_deterministic() {
    let b = build(
        &[
            ("a", 1, 5.0, 10.0),
            ("b", 1, 35.0, 10.0),
            ("x", 2, 20.0, 10.0),
            ("y", 3, 20.0, 14.0),
        ],
        "",
    );
    let (c, s, t) = conn(&b, 0, 1, "a", "b");
    let hist = vec![0.0; b.g.edges.len()];
    let p1 = found(search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)));
    let p2 = found(search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)));
    assert_eq!(format!("{p1:?}"), format!("{p2:?}"));
    assert!(p1.expansions > 0);
}

#[test]
fn history_steers_the_search_away() {
    let b = build(
        &[
            ("a", 1, 5.0, 10.0),
            ("b", 1, 35.0, 10.0),
            ("x", 2, 20.0, 10.0),
        ],
        "",
    );
    let (c, s, t) = conn(&b, 0, 1, "a", "b");
    let mut hist = vec![0.0; b.g.edges.len()];
    let p1 = found(search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)));
    for &(e, _) in &p1.crossings {
        hist[e] = 50.0;
    }
    let p2 = found(search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)));
    assert_ne!(p1.crossings, p2.crossings);
}

#[allow(dead_code)]
fn unused(_: CrossEntry, _: Vec2) {}
