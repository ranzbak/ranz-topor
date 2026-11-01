//! G1 continuation of hand-started traces (M4a / A14): a connection that starts at the free end of
//! a hand track leaves it along the hand direction (`g1_angle_deg <= 1`), on the hand's side of
//! an obstacle, with the same width and layer; `dangling_ends = "fixed"` is the M3 behaviour.
use topor_engine::geom::{dequantise, Orient, Prim, Vec2};
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1_000_000.0;

fn mm(v: f64) -> i64 {
    (v * MM).round() as i64
}

fn pad(ref_: &str, net: i32, x: f64, y: f64, d: f64) -> String {
    format!(
        r#"{{ "uuid": "{ref_}1", "ref": "{ref_}", "number": "1", "net": {net},
          "center": [{}, {}], "layers": [0], "shapes": {{ "0": {{ "kind": "circle", "d": {} }} }} }}"#,
        mm(x),
        mm(y),
        mm(d)
    )
}

/// Net 1: pad A (10, 20) with a hand stub to `end` (free end), pad B at `b`; optionally a net-2
/// blocker pad. Single copper layer, 50 x 40 mm.
fn model(end: (f64, f64), b: (f64, f64), blocker: Option<(f64, f64, f64)>) -> BoardModel {
    let (dx, dy) = (end.0 - 10.0, end.1 - 20.0);
    let l = dx.hypot(dy);
    let mut pads = vec![pad("A", 1, 10.0, 20.0, 1.0), pad("B", 1, b.0, b.1, 1.0)];
    if let Some((x, y, d)) = blocker {
        pads.push(pad("C", 2, x, y, d));
    }
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }}, {{ "code": 2, "name": "N2" }} ],
          "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [50000000, 0] }},
            {{ "kind": "seg", "a": [50000000, 0], "b": [50000000, 40000000] }},
            {{ "kind": "seg", "a": [50000000, 40000000], "b": [0, 40000000] }},
            {{ "kind": "seg", "a": [0, 40000000], "b": [0, 0] }} ],
          "pads": [ {pads} ],
          "kept": [ {{ "uuid": "stub", "net": 1, "layer": 0, "width": 250000,
            "prim": {{ "kind": "seg", "a": [{ax}, {ay}], "b": [{ex}, {ey}] }} }} ],
          "dangling_ends": [ {{ "item": "stub", "net": 1, "layer": 0, "width": 250000,
            "end": [{ex}, {ey}], "direction": [{ux}, {uy}] }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000 }} }}"#,
        pads = pads.join(","),
        ax = mm(10.0),
        ay = mm(20.0),
        ex = mm(end.0),
        ey = mm(end.1),
        ux = dx / l,
        uy = dy / l,
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn opts(mode: &str) -> Options {
    Options {
        wall_clock_s: 20.0,
        dangling_ends: mode.into(),
        ..Options::default()
    }
}

fn unit(v: Vec2) -> Vec2 {
    v.unit().unwrap()
}

/// Travel tangent of `p` at its start (`at_end` false) or end, KiCad frame.
fn tangent(p: &Prim, at_end: bool) -> Vec2 {
    match p {
        Prim::Seg(s) => unit(s.b - s.a),
        Prim::Arc(a) => {
            let pt = if at_end { a.end() } else { a.start() };
            let t = unit(pt - a.c).perp();
            if a.sigma == Orient::Ccw {
                t
            } else {
                -t
            }
        }
    }
}

fn prims(r: &RouteResult) -> Vec<Prim> {
    r.items
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) => Some(dequantise(&t.prim)),
            _ => None,
        })
        .collect()
}

fn angle_deg(a: Vec2, b: Vec2) -> f64 {
    unit(a).dot(unit(b)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Angle between the hand direction and the tangent of the routed copper at the stub end.
fn leave_angle(r: &RouteResult, end: (f64, f64), hand: Vec2) -> f64 {
    let e = Vec2 {
        x: mm(end.0) as f64,
        y: mm(end.1) as f64,
    };
    let ps = prims(r);
    let first = ps
        .iter()
        .find(|p| match p {
            Prim::Seg(s) => (s.a - e).len() < 2.0 || (s.b - e).len() < 2.0,
            Prim::Arc(a) => (a.start() - e).len() < 2.0 || (a.end() - e).len() < 2.0,
        })
        .expect("a track touches the stub end");
    let (start, _) = match first {
        Prim::Seg(s) => ((s.a - e).len() < 2.0, ()),
        Prim::Arc(a) => ((a.start() - e).len() < 2.0, ()),
    };
    // travel away from the end: forward tangent at the start, reversed tangent at the end
    let t = if start {
        tangent(first, false)
    } else {
        -tangent(first, true)
    };
    angle_deg(t, hand)
}

#[test]
fn dangling_end_continues_tangent() {
    // stub from A (10, 20) up-right to (13, 18); B is down-right, so the straight way would
    // leave the stub end at ~60 degrees off the hand direction.
    let end = (13.0, 18.0);
    let m = model(end, (40.0, 28.0), None);
    let r = route(&m, &opts("continue"));
    assert_eq!((r.routed, r.connections_total), (1, 1), "{:?}", r.connections);
    let c = &r.connections[0];
    assert_eq!(c.from.kind, "dangling");
    let g1 = c.g1_angle_deg.expect("g1 reported for a continued end");
    assert!(g1 <= 1.0, "g1 {g1}");
    let hand = Vec2 { x: 3.0, y: -2.0 };
    let a = leave_angle(&r, end, hand);
    assert!(a <= 1.0, "leave angle {a}");
    // same width and layer, hand copper untouched (only new items are reported)
    for it in &r.items {
        let Item::Track(t) = it else { panic!("no vias expected") };
        assert_eq!((t.layer, t.width, t.net), (0, 250_000, 1));
    }
    assert!(r.items.len() >= 2, "a chain, not one straight segment");
    // the same board in "fixed" mode kinks at the stub end
    let rf = route(&m, &opts("fixed"));
    assert_eq!(rf.routed, 1);
    assert!(leave_angle(&rf, end, hand) > 10.0);
    assert!(rf.connections[0].g1_angle_deg.is_none());
}

#[test]
fn dangling_end_straight_ahead_needs_no_arc() {
    // B straight ahead of the stub: one straight segment, g1 0
    let end = (13.0, 20.0);
    let m = model(end, (40.0, 20.0), None);
    let r = route(&m, &opts("continue"));
    assert_eq!(r.routed, 1);
    assert!(r.connections[0].g1_angle_deg.unwrap() < 0.1);
    assert_eq!(r.items.len(), 1);
}

/// Mean y (KiCad frame) of the routed copper between x = 22 and 30 mm.
fn mid_y(r: &RouteResult) -> f64 {
    let mut ys = Vec::new();
    for p in prims(r) {
        for t in 0..=20 {
            let f = f64::from(t) / 20.0;
            let q = match &p {
                Prim::Seg(s) => s.a + (s.b - s.a) * f,
                Prim::Arc(a) => {
                    let ang = a.a_start + a.sigma.sign() * a.sweep * f;
                    a.c + Vec2 {
                        x: ang.cos(),
                        y: ang.sin(),
                    } * a.r
                }
            };
            if (22.0 * MM..30.0 * MM).contains(&q.x) {
                ys.push(q.y);
            }
        }
    }
    ys.iter().sum::<f64>() / ys.len() as f64
}

#[test]
fn dangling_hint_keeps_side() {
    // a 6 mm blocker of another net sits slightly above the straight line stub end -> B, so the
    // shortest way passes below it; the stub points up, so the continued copper passes above.
    let end = (13.0, 20.0 - 0.5);
    let m = model(end, (40.0, 20.0), Some((26.0, 19.5, 6.0)));
    let rc = route(&m, &opts("continue"));
    assert_eq!(rc.routed, 1, "{:?}", rc.connections);
    assert!(rc.connections[0].g1_angle_deg.unwrap() <= 1.0);
    let yc = mid_y(&rc);
    assert!(yc < 19.5 * MM, "continued copper passes above the blocker: y {yc}");
    let rf = route(&m, &opts("fixed"));
    assert_eq!(rf.routed, 1);
    assert!(mid_y(&rf) > 19.5 * MM, "fixed mode takes the shorter way below");
}

#[test]
fn dangling_fixed_mode_is_m3() {
    // "fixed": the end is a fixed point (a plain junction); same copper as with no dangling
    // record at all
    let end = (13.0, 18.0);
    let m = model(end, (40.0, 28.0), None);
    let rf = route(&m, &opts("fixed"));
    let mut m0 = m.clone();
    m0.dangling_ends.clear();
    let r0 = route(&m0, &opts("continue"));
    assert_eq!(rf.routed, 1);
    assert_eq!(rf.items, r0.items);
    assert!(rf.connections[0].g1_angle_deg.is_none());
}

#[test]
fn dangling_continue_is_deterministic_over_threads() {
    let m = model((13.0, 18.0), (40.0, 28.0), None);
    let mut outs = Vec::new();
    for th in [1usize, 2, 4, 1] {
        let o = Options {
            threads: th,
            ..opts("continue")
        };
        let r = route(&m, &o);
        outs.push(serde_json::to_string(&r.items).unwrap());
    }
    assert!(outs.windows(2).all(|w| w[0] == w[1]));
}
