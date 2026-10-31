//! A1 slot-search speed: the face clear radius skip, the per-search memo and the early stop of
//! `clearance_bend` are exact-preserving (each mode returns what the full scan returns).
use proptest::prelude::*;
use std::collections::HashSet;
use topor_engine::decompose::{Connection, TermKind, TermRef};
use topor_engine::geom::{dequantise, point_arc, point_seg, Prim, Seg, Vec2};
use topor_engine::graph::{GraphRules, RouteGraph};
use topor_engine::model::BoardModel;
use topor_engine::model::Options;
use topor_engine::obstacle::{Core, ObstacleSet};
use topor_engine::search::{probe_bend_ok_mode, probe_clearance_bend_mode, BendMode};
use topor_engine::sketch::{build_layer_cdt, ConstraintSet, LayerSketch};

const MM: f64 = 1_000_000.0;

struct Built {
    g: RouteGraph,
    ls: LayerSketch,
    obs: ObstacleSet,
}

/// `pads`: `(net, x mm, y mm, diameter mm)`, named `p0, p1, ...`; board 60 x 40 mm.
fn build(pads: &[(i32, f64, f64, f64)]) -> Built {
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
    let (x0, x1, y0, y1) = (0.0, 60.0, 0.0, 40.0);
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

fn conn(b: &Built, net: i32, from: usize, to: usize) -> Connection {
    let term = |v: usize, l: &str| TermRef {
        kind: TermKind::Pad(0),
        label: l.to_string(),
        at: b.g.verts[v].at,
        layers: vec![0],
    };
    Connection {
        id: 0,
        net,
        width: 250_000,
        clearance: 200_000,
        tier: 3,
        from: term(vert(b, from), "a"),
        to: term(vert(b, to), "b"),
        est_len: 0.0,
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

const SKIP: BendMode = BendMode {
    skip: true,
    memo: false,
    early: false,
};
const MEMO: BendMode = BendMode {
    skip: false,
    memo: true,
    early: false,
};
const EARLY: BendMode = BendMode {
    skip: false,
    memo: false,
    early: true,
};

/// Distinct pads at least `gap` mm apart from the raw points, plus two net-1 end pads.
fn board(raw: Vec<(f64, f64)>, gap: f64) -> Option<(Built, Connection)> {
    let mut pads: Vec<(i32, f64, f64, f64)> = Vec::new();
    for (x, y) in raw {
        if pads.iter().all(|p| (p.1 - x).hypot(p.2 - y) > gap) {
            pads.push((2 + pads.len() as i32, x, y, 1.0));
        }
    }
    if pads.len() < 8 {
        return None;
    }
    pads.push((1, 1.0, 1.0, 1.0));
    pads.push((1, 59.0, 39.0, 1.0));
    let b = build(&pads);
    let c = conn(&b, 1, pads.len() - 2, pads.len() - 1);
    Some((b, c))
}

type Bend = (Option<(Vec2, Option<usize>)>, usize, usize);

fn probe(
    b: &Built,
    c: &Connection,
    a: Vec2,
    t: Vec2,
    portals: &[usize],
    m: BendMode,
    n: usize,
) -> Bend {
    probe_clearance_bend_mode(&b.g, b.obs.layer(0), c, a, t, portals, m, n)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// The disk `D(incentre(f), clear - reach)` of every face is free of every non-face vertex
    /// (resp. non-own wall) by at least the extent the search charges, for any reach; and the
    /// skipping scan returns the full scan's bend on random boards and chords.
    #[test]
    fn clear_radius_skip_is_conservative(
        raw in proptest::collection::vec((2.0f64..58.0, 2.0f64..38.0), 8..70),
        gap in 1.3f64..4.0,
        ax in 1.0f64..59.0, ay in 1.0f64..39.0, tx in 1.0f64..59.0, ty in 1.0f64..39.0,
        pts in proptest::collection::vec((0.0f64..1.0, 0.0f64..std::f64::consts::TAU), 6),
    ) {
        let Some((b, c)) = board(raw, gap) else { return Ok(()); };
        let g = &b.g;
        let lobs = b.obs.layer(0);
        let reach = 1.0e6; // an arbitrary connection reach (c + w/2 + eps)
        for (f, face) in g.faces.iter().enumerate() {
            let cv = g.clear_r[f] - g.rho_max() - reach;
            let cw = g.clear_w[f] - reach;
            for &(frac, ang) in &pts {
                let dir = Vec2 { x: ang.cos(), y: ang.sin() };
                if cv > 0.0 {
                    let p = g.incentre[f] + dir * (cv * frac);
                    for (u, vi) in g.verts.iter().enumerate() {
                        if !face.v.contains(&u) {
                            prop_assert!((vi.at - p).len() - vi.r - vi.clr >= reach - 1.0);
                        }
                    }
                }
                if cw > 0.0 {
                    let p = g.incentre[f] + dir * (cw * frac);
                    for (wi, w) in lobs.walls.iter().enumerate() {
                        if g.face_walls[f].contains(&wi) {
                            continue;
                        }
                        let d = match &w.core {
                            Core::Point(q) => (*q - p).len(),
                            Core::Seg(s) => point_seg(p, &Seg { a: s.a, b: s.b }),
                            Core::Arc(ar) => point_arc(p, ar),
                        };
                        // r_core + static clearance (own / edge 0.5 mm / hole 0.25 mm) <= d - reach
                        prop_assert!(d - w.r_core >= reach - 1.0, "wall {wi} too close");
                    }
                }
            }
        }
        let (a, t) = (v2(ax, ay), v2(tx, ty));
        prop_assume!((a - t).len() > 1.0);
        let portals = crossed_edges(g, a, t, 64);
        let exact = probe(&b, &c, a, t, &portals, BendMode::EXACT, 1);
        let skip = probe(&b, &c, a, t, &portals, SKIP, 1);
        prop_assert_eq!(exact.0, skip.0);
    }

    #[test]
    fn memoised_bend_equals_recomputed(
        raw in proptest::collection::vec((2.0f64..58.0, 2.0f64..38.0), 8..40),
        ax in 1.0f64..59.0, ay in 1.0f64..39.0, tx in 1.0f64..59.0, ty in 1.0f64..39.0,
        cap in 0usize..9,
    ) {
        let Some((b, c)) = board(raw, 2.0) else { return Ok(()); };
        let (a, t) = (v2(ax, ay), v2(tx, ty));
        prop_assume!((a - t).len() > 1.0);
        let portals = crossed_edges(&b.g, a, t, cap);
        let exact = probe(&b, &c, a, t, &portals, BendMode::EXACT, 1);
        let memo = probe(&b, &c, a, t, &portals, MEMO, 3);
        prop_assert_eq!(exact.0, memo.0);
        prop_assert_eq!(memo.2, 2, "two of three identical calls are memo hits");
        // the same chord with another portal list is the same key and the same bend
        let other = probe(&b, &c, a, t, &[], MEMO, 1);
        prop_assert_eq!(exact.0, other.0);
    }

    #[test]
    fn early_stop_bend_equals_full_scan(
        raw in proptest::collection::vec((2.0f64..58.0, 2.0f64..38.0), 8..60),
        ax in 1.0f64..59.0, ay in 1.0f64..39.0, tx in 1.0f64..59.0, ty in 1.0f64..39.0,
    ) {
        let Some((b, c)) = board(raw, 1.4) else { return Ok(()); };
        let (a, t) = (v2(ax, ay), v2(tx, ty));
        prop_assume!((a - t).len() > 1.0);
        let portals = crossed_edges(&b.g, a, t, 64);
        let exact = probe(&b, &c, a, t, &portals, BendMode::EXACT, 1);
        let early = probe(&b, &c, a, t, &portals, EARLY, 1);
        let fast = probe(&b, &c, a, t, &portals, BendMode::FAST, 1);
        prop_assert_eq!(exact.0, early.0);
        prop_assert_eq!(exact.0, fast.0);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// `bend_ok` memoised + cached candidate lists == the uncached, unmemoised answer.
    #[test]
    fn memoised_bend_ok_equals_recomputed(
        raw in proptest::collection::vec((2.0f64..58.0, 2.0f64..38.0), 8..50),
        ax in 1.0f64..59.0, ay in 1.0f64..39.0, tx in 1.0f64..59.0, ty in 1.0f64..39.0,
        ui in 0usize..50, ang in 0.0f64..std::f64::consts::TAU,
    ) {
        let Some((b, c)) = board(raw, 1.6) else { return Ok(()); };
        let (a, t) = (v2(ax, ay), v2(tx, ty));
        let u = ui % b.g.verts.len();
        let side = Vec2 { x: ang.cos(), y: ang.sin() };
        let lobs = b.obs.layer(0);
        let exact = probe_bend_ok_mode(&b.g, lobs, &c, a, u, t, side, BendMode::EXACT, 1);
        let memo = probe_bend_ok_mode(&b.g, lobs, &c, a, u, t, side, MEMO, 3);
        prop_assert_eq!(exact, memo);
    }
}

/// The skip is not vacuous: on a sparse board a long chord through big faces skips scans, and
/// still returns the exact result.
#[test]
fn skip_fires_on_sparse_board() {
    let pads = [
        (2, 10.0, 5.0, 1.0),
        (3, 50.0, 5.0, 1.0),
        (4, 10.0, 35.0, 1.0),
        (5, 50.0, 35.0, 1.0),
        (1, 4.0, 20.0, 1.0),
        (1, 56.0, 20.0, 1.0),
    ];
    let b = build(&pads);
    let c = conn(&b, 1, 4, 5);
    let (a, t) = (v2(4.0, 20.0), v2(56.0, 20.0));
    let portals = crossed_edges(&b.g, a, t, 64);
    let exact = probe(&b, &c, a, t, &portals, BendMode::EXACT, 1);
    let skip = probe(&b, &c, a, t, &portals, SKIP, 1);
    assert_eq!(exact.0, skip.0);
    assert!(
        skip.1 > 0,
        "expected skipped sub-chord scans, got {}",
        skip.1
    );
}
