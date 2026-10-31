#![allow(clippy::needless_range_loop)]
use std::collections::HashSet;
use topor_engine::geom::{dequantise, Aabb, Prim, Vec2};
use topor_engine::graph::{CrossEntry, GraphRules, Port, RouteGraph, TerminalInfo};
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::sketch::{build_layer_cdt, ConstraintSet};

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

fn round_pad(uuid: &str, net: i32, x_mm: f64, y_mm: f64) -> String {
    format!(
        r#"{{ "uuid": "{uuid}", "ref": "{uuid}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [0],
          "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        (x_mm * MM) as i64,
        (y_mm * MM) as i64
    )
}

struct Built {
    g: RouteGraph,
    ls: topor_engine::sketch::LayerSketch,
    obs: ObstacleSet,
}

fn build_with(body: &str, edge_clr: i64, terminals: &[TerminalInfo]) -> Built {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ], {body},
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": {edge_clr} }} }}"#
    );
    let m = BoardModel::from_json(&s).unwrap();
    let obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let tpos: Vec<Vec2> = terminals.iter().map(|t| t.at).collect();
    let ls = build_layer_cdt(obs.layer(0), &tpos, ConstraintSet::Routing).unwrap();
    let outline: Vec<Prim> = m.outline.iter().map(dequantise).collect();
    let rules = GraphRules {
        eps: 5_000.0,
        min_clearance: 0.0,
        edge_clearance: edge_clr as f64,
        hole_clearance: 250_000.0,
    };
    let g = RouteGraph::build(&ls, obs.layer(0), &outline, terminals, &rules);
    Built { g, ls, obs }
}

fn vert_of_label(b: &Built, label: &str) -> usize {
    let l = b.obs.layer(0);
    let ai = l.anchors.iter().position(|a| a.label == label).unwrap();
    b.ls.cdt.vertex_index(ai)
}

fn two_pads_b(d_mm: f64) -> (Built, usize) {
    let body = format!(
        r#""pads": [ {}, {} ], "outline": [ {} ]"#,
        round_pad("a", 1, 10.0, 10.0),
        round_pad("b", 2, 10.0 + d_mm, 10.0),
        outline50x40()
    );
    let b = build_with(&body, 500_000, &[]);
    let (va, vb) = (vert_of_label(&b, "pad:a:1"), vert_of_label(&b, "pad:b:1"));
    let e = b.g.edge(va, vb).expect("edge between pads");
    (b, e)
}

fn two_pads(d_mm: f64) -> (RouteGraph, usize) {
    let (b, e) = two_pads_b(d_mm);
    (b.g, e)
}

fn ent(conn: usize) -> CrossEntry {
    CrossEntry {
        conn,
        width: W,
        clr: C,
        k: None,
    }
}

#[test]
fn capacity_two_round_pads() {
    let (g, e) = two_pads(2.6);
    assert!((g.edges[e].cap - 1_600_000.0).abs() < 1.0);
    let mut g = g;
    let mut n = 0;
    while g.fits(e, W, C) {
        g.insert(n, e, n, ent(n));
        n += 1;
    }
    assert_eq!(n, 3);
    assert!(g.overuse(e) < 1.0);
    g.insert(3, e, 3, ent(3));
    assert!((g.overuse(e) - 420_000.0).abs() < 1.0);
}

#[test]
fn capacity_boundary_single_wire() {
    for (d, want) in [(2.0, 1), (1.65, 0), (1.66, 1)] {
        let (mut g, e) = two_pads(d);
        let mut n = 0;
        while g.fits(e, W, C) {
            g.insert(n, e, n, ent(n));
            n += 1;
        }
        assert_eq!(n, want, "d = {d}");
    }
}

#[test]
fn mixed_clearance_uses_max_gap() {
    // cap 1.0: existing conn c 0.25 plus new c 0.2: 0.255 + 0.25 + 0.255 + 2 * 0.2 = 1.16
    let (mut g, e) = two_pads(2.0);
    assert!((g.edges[e].cap - 1_000_000.0).abs() < 1.0);
    g.insert(
        0,
        e,
        0,
        CrossEntry {
            conn: 0,
            width: W,
            clr: 250_000.0,
            k: None,
        },
    );
    assert!(!g.fits(e, W, C));
}

#[test]
fn edge_vertex_uses_edge_clearance() {
    // pad at (10, 10), outline corner (0, 0) and edge clearance 0.25
    let body = format!(
        r#""pads": [ {} ], "outline": [ {} ]"#,
        round_pad("a", 1, 10.0, 10.0),
        outline50x40()
    );
    let b = build_with(&body, 250_000, &[]);
    let va = vert_of_label(&b, "pad:a:1");
    let l = b.obs.layer(0);
    let ci = l
        .anchors
        .iter()
        .position(|a| a.label == "edge" && a.c.x.abs() < 1.0 && a.c.y.abs() < 1.0)
        .unwrap();
    let vc = b.ls.cdt.vertex_index(ci);
    let e = b.g.edge(va, vc).expect("pad - corner edge");
    assert!((b.g.edges[e].c_e - 250_000.0).abs() < 1e-9);
    assert!((b.g.verts[vc].clr - 250_000.0).abs() < 1e-9);
    assert_eq!(b.g.verts[va].nets, vec![1]);
    assert!((b.g.verts[va].r - 500_000.0).abs() < 1e-9);
}

#[test]
fn pad_hull_constraint_passable_only_for_owner_net() {
    let body = format!(
        r#""pads": [ {{ "uuid": "p", "ref": "U1", "number": "1", "net": 1,
              "center": [10000000, 10000000], "layers": [0],
              "shapes": {{ "0": {{ "kind": "rect", "w": 1000000, "h": 2000000 }} }} }} ],
           "outline": [ {} ]"#,
        outline50x40()
    );
    let b = build_with(&body, 500_000, &[]);
    let hull: Vec<usize> = (0..b.g.edges.len())
        .filter(|&e| b.g.edges[e].constraint && b.g.edges[e].owner_nets == vec![1])
        .collect();
    assert!(!hull.is_empty());
    for &e in &hull {
        assert!(b.g.passable(e, 1));
        assert!(!b.g.passable(e, 2));
    }
    // outline edges: owner-less, never passable
    for e in 0..b.g.edges.len() {
        if b.g.edges[e].constraint && b.g.edges[e].owner_nets.is_empty() {
            assert!(!b.g.passable(e, 1));
        }
    }
}

#[test]
fn faces_outside_outline_are_impassable() {
    let (b, _) = two_pads_b(2.6);
    let l = b.obs.layer(0);
    let ci = l
        .anchors
        .iter()
        .position(|a| a.label == "edge" && a.c.x.abs() < 1.0 && a.c.y.abs() < 1.0)
        .unwrap();
    let vc = b.ls.cdt.vertex_index(ci);
    // some face touches the corner from outside; its non-constraint edges are impassable
    let mut outside_edges = 0;
    for (e, ge) in b.g.edges.iter().enumerate() {
        let touches_outside = ge.faces.iter().flatten().any(|&f| !b.g.faces[f].inside)
            || ge.faces.iter().any(|f| f.is_none());
        if touches_outside {
            outside_edges += 1;
            for net in [0, 1, 2, 3] {
                assert!(!b.g.passable(e, net), "edge {e}");
            }
        }
    }
    assert!(outside_edges > 0);
    // frame corner - outline corner edge
    let frame = (0..b.g.verts.len())
        .find(|&v| b.g.verts[v].anchor.is_none() && b.g.edge(v, vc).is_some())
        .expect("a frame corner is joined to the outline corner");
    let e = b.g.edge(frame, vc).unwrap();
    assert!(!b.g.passable(e, 1));
    assert!(b.g.faces.iter().any(|f| !f.inside) && b.g.faces.iter().any(|f| f.inside));
}

#[test]
fn remove_conn_restores_slots() {
    let (mut g, e) = two_pads(2.6);
    for n in 0..3 {
        g.insert(n, e, n, ent(n));
    }
    g.remove_conn(1);
    assert_eq!(g.slot_count(e), 2);
    let order: Vec<usize> = g.edges[e].cross.iter().map(|x| x.conn).collect();
    assert_eq!(order, vec![0, 2]);
    assert!(g.occupancy(1).is_empty());
    // the survivors' occupancy slots follow
    assert_eq!(g.occupancy(0), &[(e, 0)]);
    assert_eq!(g.occupancy(2), &[(e, 1)]);
    // insertion in the middle shifts later occupancy
    g.insert(5, e, 1, ent(5));
    assert_eq!(g.occupancy(2), &[(e, 2)]);
    assert_eq!(g.occupancy(5), &[(e, 1)]);
}

#[test]
fn topology_is_sorted_and_complete() {
    use topor_engine::cdt::Cdt;
    let p = |x: f64, y: f64| Vec2 { x, y };
    let c = Cdt::build(&[p(0., 0.), p(10., 0.), p(10., 10.), p(0., 10.)], &[]).unwrap();
    let t = c.topology();
    assert_eq!(t.edges.len(), 5);
    assert_eq!(t.faces.len(), 2);
    assert_eq!(4 + t.faces.len() - t.edges.len(), 1);
    assert!(t.edges.windows(2).all(|w| w[0] < w[1]));
    assert!(t.edges.iter().all(|e| e.0 < e.1));
    assert!(t.faces.windows(2).all(|w| w[0] < w[1]));
    for f in &t.faces {
        assert!(f[0] < f[1] && f[0] < f[2]);
        let (a, b, cc) = (c.vertex_pos(f[0]), c.vertex_pos(f[1]), c.vertex_pos(f[2]));
        assert!((b - a).cross(cc - a) > 0.0, "ccw");
    }
    // exactly the 4 boundary edges are shared by one inner face
    let mut uses = std::collections::HashMap::new();
    for f in &t.faces {
        for j in 0..3 {
            let (x, y) = (f[j], f[(j + 1) % 3]);
            *uses.entry((x.min(y), x.max(y))).or_insert(0) += 1;
        }
    }
    assert_eq!(uses.values().filter(|&&n| n == 1).count(), 4);
}

// --- A1: orientation conventions -------------------------------------------------------------

#[test]
fn orientation_conventions() {
    let (g, _) = two_pads(2.6);
    for (fi, f) in g.faces.iter().enumerate() {
        let (a, b, c) = (g.verts[f.v[0]].at, g.verts[f.v[1]].at, g.verts[f.v[2]].at);
        assert!((b - a).cross(c - a) > 0.0, "face {fi} is ccw");
        for i in 0..3 {
            let e = &g.edges[f.e[i]];
            let (x, y) = (f.v[(i + 1) % 3], f.v[(i + 2) % 3]);
            assert_eq!((e.a, e.b), (x.min(y), x.max(y)), "e[i] opposite v[i]");
        }
    }
    for e in &g.edges {
        let (pa, pb) = (g.verts[e.a].at, g.verts[e.b].at);
        for (side, f) in e.faces.iter().enumerate() {
            let Some(f) = f else { continue };
            let face = &g.faces[*f];
            let opp = face
                .v
                .iter()
                .copied()
                .find(|&v| v != e.a && v != e.b)
                .unwrap();
            let s = (pb - pa).cross(g.verts[opp].at - pa);
            assert!(
                if side == 0 { s > 0.0 } else { s < 0.0 },
                "faces[0] is left of a -> b"
            );
        }
    }
}

// --- A2: insert_path / strand / path_of ------------------------------------------------------

/// Two parallel paths through a strip of three faces' worth of edges is awkward to hand-build,
/// so walk a real fan: start at vertex `v`, cross the edges opposite `v` in consecutive faces.
fn fan_edges(g: &RouteGraph, v: usize) -> (Vec<usize>, Vec<usize>) {
    // faces around v that are inside; take the first two sharing an edge incident to v
    let fs = g.faces_of_vertex(v);
    for &f in fs {
        for &h in fs {
            if f == h || !g.faces[f].inside || !g.faces[h].inside {
                continue;
            }
            let shared = g.faces[f]
                .e
                .iter()
                .find(|e| g.edges[**e].faces.contains(&Some(h)));
            if let Some(&s) = shared {
                if g.edges[s].a == v || g.edges[s].b == v {
                    // path from v: crosses the edge opposite v in f (face f), then ... keep it simple
                    let opp_f = g.faces[f].e[g.faces[f].v.iter().position(|&x| x == v).unwrap()];
                    let opp_h = g.faces[h].e[g.faces[h].v.iter().position(|&x| x == v).unwrap()];
                    return (vec![opp_f, opp_h], vec![f, h]);
                }
            }
        }
    }
    panic!("no fan");
}

#[test]
fn insert_path_strand_path_of_round_trip() {
    let (b, _) = two_pads_b(2.6);
    let mut g = b.g;
    let v = vert_of_label_g(&g, &b.obs, &b.ls, "pad:a:1");
    let (es, fs) = fan_edges(&g, v);
    // a made-up path v -> cross es[0] (leaving fs[0], entering fs[1]) -> cross es[1]?
    // Use faces fs[0], fs[1] and a single crossing: the shared edge between them.
    let shared = g.faces[fs[0]]
        .e
        .iter()
        .copied()
        .find(|e| g.edges[*e].faces.contains(&Some(fs[1])))
        .unwrap();
    let goal = g.faces[fs[1]]
        .v
        .iter()
        .copied()
        .find(|&x| x != g.edges[shared].a && x != g.edges[shared].b)
        .unwrap();
    let _ = es;
    g.insert_path(7, v, goal, &[(shared, 0)], &[fs[0], fs[1]], W, C);
    let (cr, fcs, (s, t)) = g.path_of(7).unwrap();
    assert_eq!(cr, &[(shared, 0)]);
    assert_eq!(fcs, &[fs[0], fs[1]]);
    assert_eq!((s, t), (v, goal));
    assert_eq!(g.strand(shared, 0, fs[0]), Port::Vertex(v));
    assert_eq!(g.strand(shared, 0, fs[1]), Port::Vertex(goal));
    assert_eq!(g.edges[shared].cross[0].k, Some(0));
    assert_eq!(g.occupancy(7), &[(shared, 0)]);

    // a second path that crosses the same edge and a neighbouring one: occupancy follows shifts
    g.insert_path(8, v, goal, &[(shared, 0)], &[fs[0], fs[1]], W, C);
    assert_eq!(g.occupancy(7), &[(shared, 1)]);
    assert_eq!(g.occupancy(8), &[(shared, 0)]);
    assert_eq!(g.path_of(7).unwrap().0, &[(shared, 1)]);
    g.remove_conn(8);
    assert_eq!(g.occupancy(7), &[(shared, 0)]);
    assert!(g.path_of(8).is_none());
    g.remove_conn(7);
    assert!(g.path_of(7).is_none());
    assert_eq!(g.slot_count(shared), 0);
}

fn vert_of_label_g(
    _g: &RouteGraph,
    obs: &ObstacleSet,
    ls: &topor_engine::sketch::LayerSketch,
    label: &str,
) -> usize {
    let ai = obs
        .layer(0)
        .anchors
        .iter()
        .position(|a| a.label == label)
        .unwrap();
    ls.cdt.vertex_index(ai)
}

#[test]
fn strand_two_crossings_links_cross_ports() {
    // walk three consecutive faces via two shared edges of a real strip
    let (b, _) = two_pads_b(2.6);
    let mut g = b.g;
    let start = (0..g.faces.len())
        .find_map(|f0| {
            if !g.faces[f0].inside {
                return None;
            }
            for &e1 in &g.faces[f0].e {
                let Some(f1) = g.edges[e1]
                    .faces
                    .iter()
                    .flatten()
                    .copied()
                    .find(|&f| f != f0)
                else {
                    continue;
                };
                if !g.faces[f1].inside {
                    continue;
                }
                for &e2 in &g.faces[f1].e {
                    if e2 == e1 {
                        continue;
                    }
                    let Some(f2) = g.edges[e2]
                        .faces
                        .iter()
                        .flatten()
                        .copied()
                        .find(|&f| f != f1)
                    else {
                        continue;
                    };
                    if f2 != f0 && g.faces[f2].inside {
                        return Some((f0, e1, f1, e2, f2));
                    }
                }
            }
            None
        })
        .unwrap();
    let (f0, e1, f1, e2, f2) = start;
    let sv = g.faces[f0].v[0];
    let gv = g.faces[f2].v[0];
    g.insert_path(3, sv, gv, &[(e1, 0), (e2, 0)], &[f0, f1, f2], W, C);
    assert_eq!(g.strand(e1, 0, f0), Port::Vertex(sv));
    assert_eq!(g.strand(e1, 0, f1), Port::Cross(e2, 0));
    assert_eq!(g.strand(e2, 0, f1), Port::Cross(e1, 0));
    assert_eq!(g.strand(e2, 0, f2), Port::Vertex(gv));
}

#[test]
fn plain_insert_is_detached() {
    let (mut g, e) = two_pads(2.6);
    g.insert(0, e, 0, ent(0));
    let f = g.edges[e].faces[0].unwrap();
    assert_eq!(g.strand(e, 0, f), Port::Detached);
    assert!(g.path_of(0).is_none());
    assert_eq!(g.edges[e].cross[0].k, None);
}

// --- A3 / A5 ---------------------------------------------------------------------------------

#[test]
fn verts_near_and_rho_max() {
    let (b, _) = two_pads_b(2.6);
    let g = &b.g;
    let c = Vec2 {
        x: 10.0 * MM,
        y: -10.0 * MM,
    };
    let near = g.verts_near(Aabb {
        min: c - Vec2 {
            x: 0.1 * MM,
            y: 0.1 * MM,
        },
        max: c + Vec2 {
            x: 3.0 * MM,
            y: 0.1 * MM,
        },
    });
    assert_eq!(near.len(), 2);
    assert!(near.windows(2).all(|w| w[0] < w[1]));
    let all = g.verts_near(Aabb {
        min: Vec2 { x: -1e12, y: -1e12 },
        max: Vec2 { x: 1e12, y: 1e12 },
    });
    assert_eq!(all, (0..g.verts.len()).collect::<Vec<_>>());
    let want = g.verts.iter().map(|v| v.r + v.clr).fold(0.0, f64::max);
    assert_eq!(g.rho_max(), want);
    assert!((g.rho_max() - 700_000.0).abs() < 1.0); // pad r 0.5 + clr 0.2
    assert_eq!(g.rules.eps, 5_000.0);
    assert_eq!(g.rules.edge_clearance, 500_000.0);
}

// --- A4 --------------------------------------------------------------------------------------

#[test]
fn along_blocks_passable_until_removed() {
    let (b, e) = two_pads_b(2.6);
    let mut g = b.g;
    let (a, bb) = (g.edges[e].a, g.edges[e].b);
    assert!(g.passable(e, 1));
    g.insert_path(4, a, bb, &[], &[], W, C);
    assert_eq!(g.edges[e].along, Some(4));
    assert!(!g.passable(e, 1));
    assert_eq!(g.path_of(4).unwrap().0.len(), 0);
    g.remove_conn(4);
    assert_eq!(g.edges[e].along, None);
    assert!(g.passable(e, 1));
}

#[test]
fn terminal_vertices_carry_owner_and_nets() {
    let at = Vec2 {
        x: 20.0 * MM,
        y: -20.0 * MM,
    };
    let body = format!(
        r#""pads": [ {} ], "outline": [ {} ]"#,
        round_pad("a", 1, 10.0, 10.0),
        outline50x40()
    );
    let b = build_with(
        &body,
        500_000,
        &[TerminalInfo {
            at,
            net: 3,
            owner: "junction:x".into(),
            clr: 200_000.0,
            r: 0.0,
        }],
    );
    let v = (0..b.g.verts.len())
        .find(|&v| b.g.verts[v].owner.as_deref() == Some("junction:x"))
        .unwrap();
    assert_eq!(b.g.verts[v].nets, vec![3]);
    assert_eq!(b.g.verts[v].anchor, None);
    assert_eq!(b.g.verts[v].r, 0.0);
}

#[test]
fn terminal_vertex_uses_net_clearance_and_junction_radius() {
    let t_at = Vec2 {
        x: 20.0 * MM,
        y: -20.0 * MM,
    };
    let body = format!(
        r#""pads": [ {} ], "outline": [ {} ]"#,
        round_pad("a", 1, 10.0, 10.0),
        outline50x40()
    );
    let b = build_with(
        &body,
        500_000,
        &[TerminalInfo {
            at: t_at,
            net: 3,
            owner: "junction:x".into(),
            clr: 300_000.0,
            r: 200_000.0,
        }],
    );
    let t = (0..b.g.verts.len())
        .find(|&v| b.g.verts[v].owner.as_deref() == Some("junction:x"))
        .unwrap();
    assert_eq!(b.g.verts[t].clr, 300_000.0);
    assert_eq!(b.g.verts[t].r, 200_000.0);
    let e =
        b.g.edges
            .iter()
            .enumerate()
            .find(|(_, e)| e.a == t || e.b == t);
    let (_, e) = e.unwrap();
    let (o, other) = if e.a == t { (e.b, e.a) } else { (e.a, e.b) };
    let _ = other;
    let d = (b.g.verts[t].at - b.g.verts[o].at).len();
    assert!((e.cap - (d - 200_000.0 - b.g.verts[o].r)).abs() < 1.0);
    assert!(e.c_e >= 300_000.0);
}

// --- Fix-B1 (a): slot nesting from the graph ---------------------------------------------------

#[test]
fn slot_nesting_follows_slot_order() {
    use topor_engine::relax::slot_nesting;
    use topor_engine::sketch::AnchorRef;
    let (b, _) = two_pads_b(2.6);
    let mut g = b.g;
    let v = vert_of_label_g(&g, &b.obs, &b.ls, "pad:a:1");
    let (_, fs) = fan_edges(&g, v);
    let shared = g.faces[fs[0]]
        .e
        .iter()
        .copied()
        .find(|e| g.edges[*e].faces.contains(&Some(fs[1])))
        .unwrap();
    let goal = g.faces[fs[1]]
        .v
        .iter()
        .copied()
        .find(|&x| x != g.edges[shared].a && x != g.edges[shared].b)
        .unwrap();
    // conn 7 first, conn 8 inserted at slot 0 shifts 7 to slot 1: 8 is nearer edge end `a`
    g.insert_path(7, v, goal, &[(shared, 0)], &[fs[0], fs[1]], W, C);
    g.insert_path(8, v, goal, &[(shared, 0)], &[fs[0], fs[1]], W, C);
    // wires: k = 0 is conn 7, k = 1 is conn 8
    let n = slot_nesting(&g, &[7, 8]);
    let ge = &g.edges[shared];
    assert!(ge.a == v || ge.b == v, "shared edge is incident to v");
    let av = g.verts[v].anchor.expect("pad vertex is an anchor");
    // the wire nearer v on the edge is inside the other one at v, on the side v is passed on
    let (inner, outer) = if ge.a == v { (1usize, 0usize) } else { (0, 1) };
    let side = n.sides[&(outer, av)];
    assert_eq!(n.sides[&(inner, av)], side);
    let key = |k: usize| {
        (
            k,
            AnchorRef {
                anchor: av,
                sigma: side,
            },
        )
    };
    assert_eq!(n.inner[&key(outer)], vec![vec![inner]]);
    assert_eq!(n.inner[&key(inner)], vec![Vec::<usize>::new()]);
    // a conn unknown to the wire list (capacity-only entry) is skipped
    let n = slot_nesting(&g, &[7]);
    assert_eq!(n.inner[&key(0)].concat(), Vec::<usize>::new());
}

#[test]
fn graph_set_site_recomputes_only_incident_edges() {
    use topor_engine::vias::{SiteOrigin, ViaSite};
    let body = format!(
        r#""pads": [ {}, {} ], "outline": [ {} ]"#,
        round_pad("a", 1, 10.0, 10.0),
        round_pad("b", 2, 14.0, 10.0),
        outline50x40()
    );
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ], {body},
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#
    );
    let m = BoardModel::from_json(&s).unwrap();
    let mut obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let sites = vec![ViaSite {
        at: Vec2 {
            x: 12.0 * MM,
            y: 3.0 * MM,
        },
        max_d: 600_000,
        max_drill: 300_000,
        origin: SiteOrigin::Face,
    }];
    let sa = obs.add_site_anchors(&sites, &[0]);
    let ls = build_layer_cdt(obs.layer(0), &[], ConstraintSet::Routing).unwrap();
    let outline: Vec<Prim> = m.outline.iter().map(dequantise).collect();
    let rules = GraphRules {
        eps: 5_000.0,
        min_clearance: 0.0,
        edge_clearance: 500_000.0,
        hole_clearance: 250_000.0,
    };
    let mut g = RouteGraph::build(&ls, obs.layer(0), &outline, &[], &rules);
    let v = ls.vertex_at(sites[0].at).unwrap();
    assert_eq!(g.verts[v].site, Some(0));
    assert_eq!((g.verts[v].r, g.verts[v].clr), (0.0, 0.0));
    assert!(obs.layer(0).anchors[sa[0][0]].site == Some(0));
    assert!(g.edge_version.iter().all(|&x| x == 0));
    let caps: Vec<f64> = g.edges.iter().map(|e| e.cap).collect();
    let around: HashSet<usize> = g
        .faces_of_vertex(v)
        .iter()
        .flat_map(|&f| g.faces[f].e)
        .collect();
    let inc = g.set_site(v, Some((1, 300_000.0, 200_000.0, "via:0:0".into())));
    assert!(!inc.is_empty());
    let want: Vec<usize> = (0..g.edges.len())
        .filter(|&e| g.edges[e].a == v || g.edges[e].b == v)
        .collect();
    assert_eq!(inc, want);
    let mut untouched = 0;
    for e in 0..g.edges.len() {
        let incident = g.edges[e].a == v || g.edges[e].b == v;
        assert_eq!(g.edge_version[e] > 0, around.contains(&e), "edge {e}");
        if incident {
            assert!((g.edges[e].cap - (caps[e] - 300_000.0)).abs() < 1e-6);
        } else {
            assert_eq!(g.edges[e].cap, caps[e], "non-incident cap untouched");
        }
        untouched += usize::from(g.edge_version[e] == 0);
    }
    assert!(untouched > 0, "edges away from the site keep their version");
    assert!(g.rho_max() >= 500_000.0);
    g.set_site(v, None);
    for e in 0..g.edges.len() {
        assert_eq!(g.edges[e].cap, caps[e]);
    }
}

#[test]
fn edge_version_bumps_on_insert_and_remove() {
    let (mut g, e) = two_pads(2.6);
    let v0 = g.edge_version[e];
    g.insert(0, e, 0, ent(0));
    let v1 = g.edge_version[e];
    assert!(v1 > v0);
    g.remove_conn(0);
    assert!(g.edge_version[e] > v1);
}
