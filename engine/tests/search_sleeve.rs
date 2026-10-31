//! RC3 sleeve-restricted clearance queries and the M2 carry-overs D2, D3, D7, D10.
use proptest::prelude::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use topor_engine::decompose::{Connection, TermKind, TermRef};
use topor_engine::geom::{dequantise, Prim, Vec2};
use topor_engine::graph::{GraphRules, RouteGraph};
use topor_engine::model::{BoardModel, DrcSpot, Options, Pt};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::search::{
    ghost, probe_bend_ok, probe_clearance_bend, search, SearchOutcome, SearchParams, SlotPath,
    LOS_PORTAL_CAP,
};
use topor_engine::sketch::{build_layer_cdt, ConstraintSet, LayerSketch};

const MM: f64 = 1_000_000.0;
const W: f64 = 250_000.0;
const C: f64 = 200_000.0;

struct Built {
    g: RouteGraph,
    ls: LayerSketch,
    obs: ObstacleSet,
}

/// `pads`: `(net, x mm, y mm, diameter mm)`, named `p0, p1, ...`; board `x0..x1` by `0..h` mm.
fn build(x0: f64, x1: f64, h: f64, pads: &[(i32, f64, f64, f64)], spots: &[DrcSpot]) -> Built {
    build_y(x0, x1, 0.0, h, pads, spots)
}

fn build_y(
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
    pads: &[(i32, f64, f64, f64)],
    spots: &[DrcSpot],
) -> Built {
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y, d))| {
            format!(
                r#"{{ "uuid": "p{i}", "ref": "p{i}", "number": "1", "net": {net},
                  "center": [{}, {}], "layers": [0],
                  "shapes": {{ "0": {{ "kind": "circle", "d": {} }} }} }}"#,
                (x * MM) as i64,
                (-y * MM) as i64,
                (d * MM) as i64
            )
        })
        .collect();
    let seg = |a: (f64, f64), b: (f64, f64)| {
        format!(
            r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
            (a.0 * MM) as i64,
            (-a.1 * MM) as i64,
            (b.0 * MM) as i64,
            (-b.1 * MM) as i64
        )
    };
    let outline = [
        seg((x0, y0), (x1, y0)),
        seg((x1, y0), (x1, y1)),
        seg((x1, y1), (x0, y1)),
        seg((x0, y1), (x0, y0)),
    ]
    .join(",");
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ], "pads": [ {} ], "outline": [ {outline} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pj.join(",")
    );
    let m = BoardModel::from_json(&s).unwrap();
    let opts = Options {
        drc_spots: spots.to_vec(),
        ..Options::default()
    };
    let obs = ObstacleSet::build(&m, &opts, &HashSet::new(), &[]);
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

fn vert(b: &Built, i: usize) -> usize {
    let label = format!("pad:p{i}:1");
    let ai = b
        .obs
        .layer(0)
        .anchors
        .iter()
        .position(|a| a.label == label)
        .unwrap();
    b.ls.cdt.vertex_index(ai)
}

fn conn(b: &Built, net: i32, from: usize, to: usize) -> (Connection, usize, usize) {
    let (s, t) = (vert(b, from), vert(b, to));
    let term = |v: usize, l: &str| TermRef {
        kind: TermKind::Pad(0),
        label: l.to_string(),
        at: b.g.verts[v].at,
    };
    (
        Connection {
            id: 0,
            net,
            layer: 0,
            width: 250_000,
            clearance: 200_000,
            tier: 3,
            from: term(s, "a"),
            to: term(t, "b"),
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
        deadline: Instant::now() + Duration::from_secs(60),
        eps: None,
    }
}

fn found(o: SearchOutcome) -> SlotPath {
    match o {
        SearchOutcome::Found(p) => p,
        other => panic!("expected Found, got {other:?}"),
    }
}

fn v2(x: f64, y: f64) -> Vec2 {
    Vec2 {
        x: x * MM,
        y: y * MM,
    }
}

/// Edges of `g` whose segment properly crosses the chord `a -> t` (at most `cap`).
fn crossed_edges(g: &RouteGraph, a: Vec2, t: Vec2, cap: usize) -> Vec<usize> {
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    (0..g.edges.len())
        .filter(|&e| {
            let (pa, pb) = (g.verts[g.edges[e].a].at, g.verts[g.edges[e].b].at);
            side(a, t, pa) * side(a, t, pb) < 0.0 && side(pa, pb, a) * side(pa, pb, t) < 0.0
        })
        .take(cap)
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn sleeve_query_matches_full_query_first_bend(
        raw in proptest::collection::vec((2.0f64..58.0, 2.0f64..38.0), 8..30),
        ax in 1.0f64..59.0, ay in 1.0f64..39.0, tx in 1.0f64..59.0, ty in 1.0f64..39.0,
        cap in 0usize..7,
    ) {
        let mut pads: Vec<(i32, f64, f64, f64)> = Vec::new();
        for (x, y) in raw {
            if pads.iter().all(|p| (p.1 - x).hypot(p.2 - y) > 2.0) {
                pads.push((2 + pads.len() as i32, x, y, 1.0));
            }
        }
        prop_assume!(pads.len() >= 8);
        let mut all = pads.clone();
        all.push((1, 1.0, 1.0, 1.0));
        all.push((1, 59.0, 39.0, 1.0));
        let b = build(0.0, 60.0, 40.0, &all, &[]);
        let (c, _, _) = conn(&b, 1, all.len() - 2, all.len() - 1);
        let (a, t) = (v2(ax, ay), v2(tx, ty));
        prop_assume!((a - t).len() > 1.0);
        let portals = crossed_edges(&b.g, a, t, cap);
        let full = probe_clearance_bend(&b.g, b.obs.layer(0), &c, a, t, &portals, true);
        let sleeve = probe_clearance_bend(&b.g, b.obs.layer(0), &c, a, t, &portals, false);
        prop_assert_eq!(full, sleeve);
    }
}

#[test]
fn ghost_candidates_bounded_on_long_board() {
    let mut pads = Vec::new();
    for j in 0..20 {
        for i in 0..10 {
            let net = if i + j == 0 || (i == 9 && j == 19) {
                1
            } else {
                2 + (i + 10 * j)
            };
            pads.push((net, 5.0 + 3.0 * i as f64, 5.0 + 3.0 * j as f64, 1.0));
        }
    }
    let b = build(0.0, 40.0, 70.0, &pads, &[]);
    let (c, s, t) = conn(&b, 1, 0, 199);
    let hist = vec![0.0; b.g.edges.len()];
    let (path, _, _) = ghost(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)).expect("ghost path");
    assert!(path.expansions > 0);
    let bound = path.expansions * (LOS_PORTAL_CAP + 1) * 64;
    assert!(
        path.candidates <= bound,
        "candidates {} > bound {bound} (expansions {})",
        path.candidates,
        path.expansions
    );
}

#[test]
fn statically_unfit_edge_is_hard() {
    // Board 3.2 mm wide: pads 2 (10, 10) and 3 (11.2, 10) leave 0.2 mm between them and no room
    // outside; the terminals sit on either side of that pinch.
    let pads = [
        (1, 10.6, 5.0, 1.0),
        (1, 10.6, 15.0, 1.0),
        (2, 10.0, 10.0, 1.0),
        (3, 11.2, 10.0, 1.0),
    ];
    let b = build(9.0, 12.2, 20.0, &pads, &[]);
    let (c, s, t) = conn(&b, 1, 0, 1);
    let e = b.g.edge(vert(&b, 2), vert(&b, 3)).expect("pinch edge");
    assert!(!b.g.fits_at(e, W, C, 5_000.0), "empty edge must be unfit");
    let hist = vec![0.0; b.g.edges.len()];
    // the slot-free ghost never crosses it (it used to, at GHOST_BLOCK_NM, as the only way
    // across); the planar search keeps unfit edges as costly overuse crossings (route_engine's
    // eprom board needs that for its zero-slack corridor)
    match ghost(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)) {
        None => {}
        Some((gp, _, _)) => assert!(
            gp.crossings.iter().all(|&(x, _)| x != e),
            "ghost crosses the unfit edge"
        ),
    }
}

#[test]
fn direct_path_rejects_third_vertex_inside_band() {
    let pads = [
        (1, 10.0, 10.0, 1.0),
        (1, 20.0, 10.0, 1.0),
        (2, 15.0, 10.6, 1.0),
    ];
    // the board's lower edge hugs the pads so no outline vertex breaks the a-b Delaunay edge
    let b = build_y(5.0, 25.0, 8.5, 14.0, &pads, &[]);
    let (c, s, t) = conn(&b, 1, 0, 1);
    assert!(b.g.edge(s, t).is_some(), "terminals adjacent in the CDT");
    let hist = vec![0.0; b.g.edges.len()];
    let p = found(search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)));
    assert!(
        !p.crossings.is_empty(),
        "direct path ignored the foreign pad"
    );
    assert!(p.est_len > 10.0 * MM + 1.0, "{}", p.est_len);
}

#[test]
fn spot_vertex_blocks_only_listed_net() {
    let spot = DrcSpot {
        layer: 0,
        at: Pt {
            x: 15_000_000,
            y: -10_300_000,
        },
        r: 400_000,
        net: 1,
    };
    let len = |net: i32| {
        let pads = [(net, 10.0, 10.0, 1.0), (net, 20.0, 10.0, 1.0)];
        let b = build(0.0, 50.0, 40.0, &pads, std::slice::from_ref(&spot));
        let (c, s, t) = conn(&b, net, 0, 1);
        let hist = vec![0.0; b.g.edges.len()];
        let p = found(search(&b.g, b.obs.layer(0), &c, s, t, &params(&hist)));
        p.est_len
    };
    let (l2, l1) = (len(2), len(1));
    assert!(
        (l2 - 10.0 * MM).abs() < 1.0,
        "net 2 passes the spot straight: {l2}"
    );
    assert!(
        l1 > 10.0 * MM + 10_000.0,
        "net 1 bends around the spot: {l1}"
    );
}

#[test]
fn bend_pinched_against_foreign_vertex_is_rejected() {
    // pad U (r 0.8) with a foreign pad V centred 1.3 mm above it: the arc on V's side is
    // pinched. Pads W1, W2 flanking the gap between them break the U-V CDT edge, so V is a non-adjacent vertex.
    let pads = [
        (1, 5.0, 10.0, 1.0),
        (1, 25.0, 10.0, 1.0),
        (2, 15.0, 10.0, 1.6),
        (3, 15.0, 11.3, 1.0),
        (4, 14.6, 10.65, 0.2),
        (5, 15.4, 10.65, 0.2),
    ];
    let b = build(0.0, 50.0, 40.0, &pads, &[]);
    let (c, _, _) = conn(&b, 1, 0, 1);
    let (a, t) = (v2(5.0, 10.0), v2(25.0, 10.0));
    let u = vert(&b, 2);
    let (obs, g) = (b.obs.layer(0), &b.g);
    assert!(
        !probe_bend_ok(g, obs, &c, a, u, t, v2(0.0, 1.0)),
        "pinched side accepted"
    );
    assert!(
        probe_bend_ok(g, obs, &c, a, u, t, v2(0.0, -1.0)),
        "free side rejected"
    );
}
