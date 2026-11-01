//! M4-D1 (A4): own-net / same-net capacity, wall-aware (design note
//! `docs/progress/notes/m4-capacity-steiner-design.md` §1).
use std::collections::HashSet;
use topor_engine::geom::{dequantise, Prim, Vec2};
use topor_engine::graph::{CapacityMode, GraphRules, RouteGraph};
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::sketch::{build_layer_cdt, ConstraintSet};

const MM: f64 = 1_000_000.0;
const EPS: f64 = 5_000.0;

fn outline(x1: f64, y1: f64) -> String {
    polygon(&[(0.0, 0.0), (x1, 0.0), (x1, y1), (0.0, y1)])
}

/// Closed outline through `pts` (mm, KiCad frame).
fn polygon(pts: &[(f64, f64)]) -> String {
    let q = |p: (f64, f64)| ((p.0 * MM).round() as i64, (p.1 * MM).round() as i64);
    (0..pts.len())
        .map(|i| {
            let (a, b) = (q(pts[i]), q(pts[(i + 1) % pts.len()]));
            format!(
                r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
                a.0, a.1, b.0, b.1
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn pad(name: &str, net: i32, x: f64, y: f64, shape: &str) -> String {
    format!(
        r#"{{ "uuid": "{name}", "ref": "{name}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [0], "shapes": {{ "0": {shape} }} }}"#,
        (x * MM) as i64,
        (y * MM) as i64
    )
}

fn roundrect(w: f64, h: f64, rc: f64) -> String {
    format!(
        r#"{{ "kind": "roundrect", "w": {}, "h": {}, "rc": {} }}"#,
        (w * MM) as i64,
        (h * MM) as i64,
        (rc * MM) as i64
    )
}

fn circle(d: f64) -> String {
    format!(r#"{{ "kind": "circle", "d": {} }}"#, (d * MM) as i64)
}

/// Graph of layer 0 of a board with `pads` inside a `w x h` mm outline (clearance 0.2, edge
/// clearance 0.5, hole clearance 0.25).
fn graph(pads: &[String], w: f64, h: f64) -> RouteGraph {
    graph_in(pads, &outline(w, h))
}

fn graph_in(pads: &[String], outline: &str) -> RouteGraph {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ], "pads": [ {} ], "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pads.join(","),
        outline
    );
    let m = BoardModel::from_json(&s).unwrap();
    let obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let ls = build_layer_cdt(obs.layer(0), &[], ConstraintSet::Routing).unwrap();
    let outline: Vec<Prim> = m.outline.iter().map(dequantise).collect();
    let rules = GraphRules {
        eps: EPS,
        min_clearance: 0.0,
        edge_clearance: 500_000.0,
        hole_clearance: 250_000.0,
    };
    RouteGraph::build(&ls, obs.layer(0), &outline, &[], &rules)
}

fn vert_at(g: &RouteGraph, x: f64, y: f64) -> usize {
    // the model is in the KiCad frame, the graph in the math frame (Y flipped)
    let p = Vec2 {
        x: x * MM,
        y: -y * MM,
    };
    let (v, d) = g
        .verts
        .iter()
        .enumerate()
        .map(|(i, vi)| (i, (vi.at - p).len()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!(d < 10.0, "no vertex at ({x}, {y}): nearest {d} nm");
    v
}

fn edge_at(g: &RouteGraph, a: (f64, f64), b: (f64, f64)) -> usize {
    let (va, vb) = (vert_at(g, a.0, a.1), vert_at(g, b.0, b.1));
    g.edge(va, vb)
        .unwrap_or_else(|| panic!("no edge {a:?} - {b:?}"))
}

/// The two faces of `e` and the vertex opposite `e` in each: a one-crossing path through `e`.
fn through(g: &RouteGraph, e: usize) -> (usize, usize, [usize; 2]) {
    let ge = &g.edges[e];
    let f = [ge.faces[0].unwrap(), ge.faces[1].unwrap()];
    let third = |fi: usize| {
        *g.faces[fi]
            .v
            .iter()
            .find(|&&v| v != ge.a && v != ge.b)
            .unwrap()
    };
    (third(f[0]), third(f[1]), f)
}

/// Design note §1 row 1: VCC U3.1 of dcsgone (SOT-23-5, 1.325 x 0.6 mm roundrect, corner
/// r 0.15): its 0.3 mm tip edge between the two corner anchors has M3 cap 0, so a 0.5 mm track
/// of the pad's own net never fitted (overuse crossing, escape vias). Own ends: no gap, the
/// radius given back, usable 0.6 = the pad width, need 0.505 -> fits; a foreign net does not.
#[test]
fn own_tip_edge_fits_own_net() {
    let mut g = graph(
        &[
            pad("u", 1, 10.0, 10.0, &roundrect(1.325, 0.6, 0.15)),
            pad("x", 2, 20.0, 10.0, &circle(1.0)),
        ],
        30.0,
        20.0,
    );
    let (w, c) = (500_000.0, 200_000.0);
    // inset corners: x 10 +- 0.5125, y 10 +- 0.15
    let tip = edge_at(&g, (10.5125, 9.85), (10.5125, 10.15));
    let side = edge_at(&g, (9.4875, 10.15), (10.5125, 10.15));
    assert!((g.edges[tip].len - 0.3 * MM).abs() < 1.0);
    assert!(g.edges[tip].cap.abs() < 1.0, "M3 cap of the tip is 0");
    assert!(!g.fits_at(tip, w, c, EPS), "M3: unfit for every net");
    let p = g.fit_parts(tip, 1, w, c, EPS);
    assert!((p.hi - p.lo - 0.6 * MM).abs() < 1.0, "{p:?}");
    assert!((p.need - 0.505 * MM).abs() < 1.0, "{p:?}");
    assert!(g.fits_for(tip, 1, w, c), "own tip fits its own net");
    assert!(!g.fits_for(tip, 2, w, c), "foreign net: the M3 rule");
    // long side: L 1.025, usable 1.325
    let ps = g.fit_parts(side, 1, w, c, EPS);
    assert!((ps.hi - ps.lo - 1.325 * MM).abs() < 1.0, "{ps:?}");
    assert!(!g.fits_at(side, w, c, EPS) && g.fits_for(side, 1, w, c));
    // the M3 switch restores the net-blind rule
    g.cap_mode = CapacityMode {
        own_net: false,
        ..CapacityMode::default()
    };
    assert!(!g.fits_for(tip, 1, w, c) && !g.fits_for(side, 1, w, c));
}

/// Design note §1 rows 3/4 (#207 U7.6, 0.625 x 0.25, r 0.0625; wiccon #52): a strand of the
/// pad's own net already crosses the own side edge; a second strand of the same net charges no
/// gap against it (0.205 + 0.205 <= 0.625), a foreign one still does; the overuse of two
/// same-net strands is 0 where the M3 accounting charged a clearance between them.
#[test]
fn two_same_net_strands_share_one_tip_edge() {
    let mut g = graph(
        &[
            pad("u", 1, 10.0, 10.0, &roundrect(0.625, 0.25, 0.0625)),
            pad("x", 2, 20.0, 10.0, &circle(1.0)),
        ],
        30.0,
        20.0,
    );
    let (w, c) = (200_000.0, 200_000.0);
    // inset corners: x 10 +- 0.25, y 10 +- 0.0625: long side L 0.5
    let side = edge_at(&g, (9.75, 10.0625), (10.25, 10.0625));
    assert!((g.edges[side].cap - 0.375 * MM).abs() < 1.0);
    assert!(!g.fits_at(side, w, c, EPS), "M3: 0.375 < 0.605");
    assert!(g.fits_for(side, 1, w, c));
    let (s, t, f) = through(&g, side);
    g.insert_path_net(0, 1, s, t, &[(side, 0)], &f, w, c);
    assert_eq!(g.strand_net(0), 1);
    let p = g.fit_parts(side, 1, w, c, EPS);
    assert!((p.need - 0.41 * MM).abs() < 1.0, "{p:?}");
    assert!((p.hi - p.lo - 0.625 * MM).abs() < 1.0, "{p:?}");
    assert!(g.fits_for(side, 1, w, c), "second same-net strand fits");
    assert!(!g.fits_for(side, 2, w, c), "a foreign strand does not");
    g.insert_path_net(1, 1, s, t, &[(side, 1)], &f, w, c);
    assert_eq!(
        g.overuse(side),
        0.0,
        "two same-net strands on their own pad's side"
    );
    let mut m3 = g.clone();
    m3.cap_mode.own_net = false;
    assert!(m3.overuse(side) > 0.0, "M3 charged them as overuse");
    // a third, foreign strand: the own end gives nothing back once a foreign strand is there
    g.insert_path(2, s, t, &[(side, 2)], &f, w, c);
    assert!(g.overuse(side) > 0.0);
    assert!(!g.fits_for(side, 1, w, c));
    // remove_conn forgets the net
    g.remove_conn(0);
    assert_eq!(g.strand_net(0), topor_engine::graph::UNKNOWN_NET);
}

/// Design note §1 row 7 (F6's U2.28 failure: a corridor the vertex-only capacity cannot see):
/// two pads of one net 1.2 mm apart with a notch of the board outline pointing at the gap
/// between them, its apex 0.1 mm below their centre line. The edge between the pads is M3-fit
/// (cap 1.0 >= 0.655: "fit by accident", the apex is no end of it) and fits the own-net rule
/// without walls (usable 1.4), but copper keeping the 0.5 mm edge clearance from the apex can
/// cross the centre line only within 0.21 mm of either pad's copper extent: the wall term
/// leaves 0.21 < 0.255 -> unfit; the F6 behaviour is the switch off.
#[test]
fn own_end_edge_against_outline_wall_stays_unfit() {
    let notch = polygon(&[
        (0.0, 0.0),
        (10.5, 0.0),
        (10.6, 0.9),
        (10.7, 0.0),
        (30.0, 0.0),
        (30.0, 20.0),
        (0.0, 20.0),
    ]);
    let pads = |y: f64| {
        vec![
            pad("a", 1, 10.0, y, &circle(0.2)),
            pad("b", 1, 11.2, y, &circle(0.2)),
            pad("x", 2, 20.0, 10.0, &circle(1.0)),
        ]
    };
    let mut g = graph_in(&pads(1.0), &notch);
    let (w, c) = (250_000.0, 200_000.0);
    let e = edge_at(&g, (10.0, 1.0), (11.2, 1.0));
    assert!(g.fits_at(e, w, c, EPS), "M3: cap 1.0 >= 0.655");
    let p = g.fit_parts(e, 1, w, c, EPS);
    assert!(p.wall && (p.hi - p.lo - 1.4 * MM).abs() < 1.0, "{p:?}");
    let free = g.wall_free_len(e, 1, p.c_wall, p.lo, p.hi);
    assert!((free - 0.2101 * MM).abs() < 0.001 * MM, "{free}");
    assert!(
        !g.fits_for(e, 1, w, c),
        "the apex blocks the middle of the span"
    );
    g.cap_mode.wall_aware = false;
    assert!(
        g.fits_for(e, 1, w, c),
        "F6 (no wall term) accepts the corridor"
    );
    // the same pair 2 mm higher clears the apex
    let g2 = graph_in(&pads(3.0), &notch);
    let e2 = edge_at(&g2, (10.0, 3.0), (11.2, 3.0));
    assert!(g2.fits_for(e2, 1, w, c));
}

/// With no own end and no same-net strand the rule is exactly M3 (byte-identical boards):
/// every edge of a board of foreign pads gives the same fit and the same overuse.
#[test]
fn foreign_edges_keep_the_m3_rule() {
    let mut g = graph(
        &[
            pad("a", 1, 10.0, 10.0, &circle(1.0)),
            pad("b", 2, 11.9, 10.0, &circle(1.0)),
            pad("c", 3, 11.0, 11.6, &circle(1.0)),
        ],
        30.0,
        20.0,
    );
    let (w, c) = (250_000.0, 200_000.0);
    for e in 0..g.edges.len() {
        for net in [4, 7] {
            assert_eq!(g.fits_for(e, net, w, c), g.fits_at(e, w, c, EPS), "e{e}");
        }
    }
    let e = edge_at(&g, (10.0, 10.0), (11.9, 10.0));
    let (s, t, f) = through(&g, e);
    for k in 0..3 {
        g.insert_path_net(k, 4 + k as i32, s, t, &[(e, k)], &f, w, c);
    }
    let mut m3 = g.clone();
    m3.cap_mode.own_net = false;
    assert!(g.overuse(e) > 0.0);
    assert_eq!(g.overuse(e).to_bits(), m3.overuse(e).to_bits());
    assert_eq!(g.fits_for(e, 9, w, c), m3.fits_for(e, 9, w, c));
}

/// Transit pinch (A4, `transit_pinch`): a pad 0.25 mm from the board edge (eprom's via next to
/// the right edge). Both graph edges from the pad to the edge's corners fit (the vertex-only
/// capacity sees the corners, 10 mm away), but a strand going around the pad between it and
/// the edge has no room: the face transit is refused; away from the edge, for the pad's own
/// net, or with the switch off it is not.
#[test]
fn transit_between_pad_and_outline_is_pinched() {
    let (w, c) = (250_000.0, 200_000.0);
    let near = |x: f64| {
        graph(
            &[
                pad("x", 2, x, 10.0, &circle(0.8)),
                pad("y", 3, 15.0, 10.0, &circle(1.0)),
            ],
            30.0,
            20.0,
        )
    };
    // the face (pad, (30, 0), (30, 20)) and its two spokes from the pad
    let transit = |g: &RouteGraph, x: f64, net: i32| -> bool {
        let v = vert_at(g, x, 10.0);
        let (c1, c2) = (vert_at(g, 30.0, 0.0), vert_at(g, 30.0, 20.0));
        let f = *g
            .faces_of_vertex(v)
            .iter()
            .find(|&&f| g.faces[f].v.contains(&c1) && g.faces[f].v.contains(&c2))
            .expect("face pad / right edge");
        let (e1, e2) = (g.edge(v, c1).unwrap(), g.edge(v, c2).unwrap());
        assert!(g.fits_for(e1, net, w, c) && g.fits_for(e2, net, w, c));
        g.transit_fits(f, e1, e2, net, w, c, EPS)
    };
    let mut g = near(29.35);
    assert!(!transit(&g, 29.35, 1), "0.25 mm to the edge: no room");
    assert!(transit(&g, 29.35, 2), "own net runs over its own pad");
    g.cap_mode.transit = false;
    assert!(transit(&g, 29.35, 1));
    let g2 = near(27.0);
    assert!(transit(&g2, 27.0, 1), "2.6 mm to the edge: room");
}

fn eprom_front_only() -> BoardModel {
    let mut m =
        BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).unwrap();
    for l in &mut m.layers {
        l.routable = l.id == 0;
    }
    m
}

/// The F6 regression of the M2 guard `eprom_ripped_front_only_is_the_m2_result`: with own-net
/// capacity the F.Cu-only negotiation never routes +5V U2.28 -> J1.2 at ε (its corridor along
/// the top edge is pinched below ε), so the zero-slack retry must also try failed connections
/// (`zero_slack_failed`); the M3 rule got there only because +5V happened to be routed as a
/// detour at its best iteration.
#[test]
fn front_only_plus5v_needs_zero_slack_for_failed() {
    let m = eprom_front_only();
    let o = Options {
        wall_clock_s: 600.0,
        threads: 1,
        ..Options::default()
    };
    let r = topor_engine::route::route(&m, &o);
    assert_eq!(r.routed, 27, "{}", r.zero_slack);
    let zs: Vec<&str> = r.zero_slack_wires.iter().map(|z| z.name.as_str()).collect();
    assert_eq!(zs, vec!["+5V"]);
    let off = Options {
        zero_slack_failed: false,
        ..o.clone()
    };
    let r_off = topor_engine::route::route(&m, &off);
    assert_eq!(
        r_off.routed, 26,
        "without failed candidates +5V stays unrouted"
    );
    let mut m3 = o.clone();
    m3.expert.own_net_capacity = false;
    m3.expert.transit_pinch = false;
    m3.zero_slack_failed = false;
    assert_eq!(
        topor_engine::route::route(&m, &m3).routed,
        27,
        "M3, by luck"
    );
}

/// The 0.5 mm-pitch row of dcsgone U4 (0.825 x 0.25 roundrect, r 0.0625): a 0.2 mm track of
/// the middle pad's net leaves its 0.25 mm tip with 0.275 mm to each neighbour (>= 0.2): the tip
/// edge (L 0.125, M3 cap 0) fits, the wall term included; a 0.5 mm track cannot (w > pad).
#[test]
fn fine_pitch_tip_fits_between_neighbours() {
    let rr = roundrect(0.825, 0.25, 0.0625);
    let g = graph(
        &[
            pad("p1", 1, 10.0, 9.5, &rr),
            pad("p2", 2, 10.0, 10.0, &rr),
            pad("p3", 3, 10.0, 10.5, &rr),
        ],
        30.0,
        20.0,
    );
    // inset corners of p2: x 10 +- 0.35, y 10 +- 0.0625
    let tip = edge_at(&g, (10.35, 9.9375), (10.35, 10.0625));
    assert!(!g.fits_at(tip, 200_000.0, 200_000.0, EPS));
    assert!(
        g.fits_for(tip, 2, 200_000.0, 200_000.0),
        "{:?}",
        g.fit_parts(tip, 2, 200_000.0, 200_000.0, EPS)
    );
    assert!(!g.fits_for(tip, 2, 500_000.0, 200_000.0));
    assert!(!g.fits_for(tip, 1, 200_000.0, 200_000.0));
}
