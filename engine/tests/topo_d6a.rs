//! D6a tests (ladder bug fixes): copper graphics and pre-placed plane stubs are projection
//! obstacles, a no-via keepout closes the via sites over it, the P2 layer pin prices no via for a
//! wire that stays on its SMD pad's layer under another net's pad, and a plane stub may not graze
//! a neighbouring pad of its own net.

use topor_engine::check::Checker;
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::board::{self, BuildOpts, Extra};
use topor_engine::topo::draw::{track_item, Drawer};
use topor_engine::topo::init::{Conn, InitWeights, TreeEnd};
use topor_engine::topo::pack::Packer;
use topor_engine::topo::state::{point_in_poly, State};

const MM: f64 = 1e6;

/// Two-layer board 40 x 25 mm (KiCad frame) with SMD rect pads `(ref, net, x, y, w, h, layer)`,
/// and extra JSON members (`copper_shapes`, `keepouts`) in `extra`.
fn board(pads: &[(&str, i32, f64, f64, f64, f64, u32)], extra: &str) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (40.0, 0.0), (40.0, 25.0), (0.0, 25.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .map(|&(r, net, x, y, w, h, l)| {
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}}}}"#,
                p(x),
                p(y),
                p(w),
                p(h)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"K"}}],
            "pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}{extra}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let p = |v: f64| (v * MM) as i64;
    format!("[[{},{}],[{},{}],[{},{}],[{},{}]]", p(x0), p(y0), p(x1), p(y0), p(x1), p(y1), p(x0), p(y1))
}

fn opts() -> BuildOpts {
    BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }
}

#[test]
fn copper_graphic_is_an_obstacle_on_its_layer_only() {
    let m = board(
        &[("A1", 1, 6.0, 10.0, 1.0, 1.0, 0), ("B1", 1, 24.0, 10.0, 1.0, 1.0, 0)],
        &format!(r#","copper_shapes":[{{"uuid":"s","net":0,"layer":0,"poly":{}}}]"#, rect(14.0, 7.0, 16.0, 13.0)),
    );
    let b = board::build(&m, &opts()).unwrap();
    let shape: Vec<_> = b.proj.obstacles.iter().filter(|o| o.net.is_none() && !o.drilled).collect();
    assert_eq!(shape.len(), 1);
    assert_eq!(shape[0].layers.0, 0b01, "F.Cu only");
    // No centre vertex: a copper graphic is no terminal.
    let o = b.proj.obstacles.iter().position(|o| o.net.is_none() && !o.drilled).unwrap();
    assert!(b.centre[o].is_none());
    // Off: no such obstacle (the gate and witness inputs).
    let b0 = board::build(&m, &BuildOpts { copper_shapes: false, ..opts() }).unwrap();
    assert!(b0.proj.obstacles.iter().all(|o| o.net.is_some()));
}

#[test]
fn extra_copper_is_an_obstacle_after_the_pads() {
    let m = board(&[("A1", 1, 6.0, 10.0, 1.0, 1.0, 0), ("B1", 1, 24.0, 10.0, 1.0, 1.0, 0)], "");
    let at = Vec2 { x: 15.0 * MM, y: -10.0 * MM };
    let ex = [
        Extra::Via { net: 2, at, d: 0.6 * MM, ring: 0.0 },
        Extra::Track { net: 2, layer: 0, a: Vec2 { x: 15.0 * MM, y: -11.0 * MM }, b: Vec2 { x: 15.0 * MM, y: -10.3 * MM }, width: 0.25 * MM },
    ];
    let b0 = board::build(&m, &opts()).unwrap();
    let b = board::build_full(&m, &opts(), &[], &ex).unwrap();
    let n0 = b0.proj.obstacles.len();
    assert_eq!(b.proj.obstacles.len(), n0 + 2);
    // The pads keep their obstacle ids; the via is drilled, the track on F.Cu; no centres.
    assert_eq!(b.pad_of[..n0], b0.pad_of[..]);
    assert!(b.proj.obstacles[n0].drilled && point_in_poly(at, &b.proj.obstacles[n0].poly));
    assert_eq!(b.proj.obstacles[n0 + 1].layers.0, 0b01);
    assert!(b.centre[n0].is_none() && b.centre[n0 + 1].is_none());
}

#[test]
fn no_via_keepout_closes_the_via_sites_over_it() {
    let ko = format!(r#","keepouts":[{{"name":"k","layers":[2],"poly":{},"no_tracks":false,"no_vias":true}}]"#, rect(10.0, 5.0, 30.0, 20.0));
    let m = board(&[("A1", 1, 3.0, 3.0, 1.0, 1.0, 0), ("B1", 1, 37.0, 22.0, 1.0, 1.0, 0)], &ko);
    let b = board::build(&m, &opts()).unwrap();
    assert_eq!(b.proj.no_via.len(), 1, "a B.Cu-only no-via keepout still closes through vias");
    let pk = Packer::for_model(&m, &Options::default(), &b.proj);
    let tri_at = |x: f64, y: f64| {
        let p = Vec2 { x: x * MM, y: -y * MM };
        (0..b.proj.tris.len()).find(|&t| point_in_poly(p, &b.proj.tris[t].map(|v| b.proj.pos[v]))).unwrap()
    };
    let (inside, outside) = (tri_at(20.0, 12.5), tri_at(35.0, 3.0));
    assert!(!pk.via_site(inside, 1, 0.3 * MM, 0.2 * MM));
    assert!(pk.via_site(outside, 1, 0.3 * MM, 0.2 * MM));
}

#[test]
fn smd_wire_under_a_foreign_pad_on_the_other_layer_needs_no_via() {
    // R (net A, B.Cu) under M (net K, F.Cu); the wire R -> U stays on B.Cu: the layer pin prices no
    // via (D5 charged one per pass over M and per narrow edge).
    let m = board(
        &[("R1", 1, 15.37, 17.14, 2.6, 1.8, 2), ("M1", 2, 14.44, 17.24, 1.4, 1.4, 0), ("U1", 1, 35.25, 3.55, 1.0, 1.0, 2), ("N1", 2, 9.5, 12.5, 1.0, 1.0, 0)],
        "",
    );
    let b = board::build(&m, &opts()).unwrap();
    let mut st = State::new(&b.proj);
    let ob = |r: &str| m.pads.iter().position(|p| p.ref_ == r).map(|i| b.pad_of.iter().position(|&x| x == i).unwrap()).unwrap();
    let (pa, pb) = (ob("R1"), ob("U1"));
    let c = Conn { net: 1, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM };
    let (w, _) = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed");
    let wi = st.wires[w].clone();
    let edges: Vec<usize> = wi.passes.iter().map(|&p| st.passes[p].edge).collect();
    let end = |v, obs| TreeEnd { v, obs, on: None, adj: 0.0, arms: [None; 4] };
    assert!(st.forced_edges(1, wi.w, wi.s, &end(wi.a, wi.pa), &end(wi.b, wi.pb), &edges, &wi.tris).is_empty());
    // To an F.Cu pad instead: the layer changes once, one via.
    let m2 = board(
        &[("R1", 1, 15.37, 17.14, 2.6, 1.8, 2), ("M1", 2, 14.44, 17.24, 1.4, 1.4, 0), ("U1", 1, 35.25, 3.55, 1.0, 1.0, 0), ("N1", 2, 9.5, 12.5, 1.0, 1.0, 0)],
        "",
    );
    let b2 = board::build(&m2, &opts()).unwrap();
    let mut st2 = State::new(&b2.proj);
    let ob2 = |r: &str| m2.pads.iter().position(|p| p.ref_ == r).map(|i| b2.pad_of.iter().position(|&x| x == i).unwrap()).unwrap();
    let (pa, pb) = (ob2("R1"), ob2("U1"));
    let c2 = Conn { net: 1, pa, pb, a: b2.centre[pa].unwrap(), b: b2.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM };
    let (w2, _) = st2.route_conn(&c2, &InitWeights::default(), 1.0).expect("routed");
    let wi = st2.wires[w2].clone();
    let edges: Vec<usize> = wi.passes.iter().map(|&p| st2.passes[p].edge).collect();
    assert_eq!(st2.forced_edges(1, wi.w, wi.s, &end(wi.a, wi.pa), &end(wi.b, wi.pb), &edges, &wi.tris).len(), 1);
}

#[test]
fn plane_stub_may_not_graze_an_own_net_pad() {
    // FPC-connector-like column (a 2-layer badge, D6a): GND pads 1.1 x 0.3 mm at 0.5 mm pitch on B.Cu.
    let m = board(&[("J16", 1, 20.0, 10.0, 1.1, 0.3, 2), ("J15", 1, 20.0, 10.5, 1.1, 0.3, 2)], "");
    let b = board::build(&m, &opts()).unwrap();
    let st = State::new(&b.proj);
    let o = Options::default();
    let ck = Checker::new(&m, &o);
    let dr = Drawer::new(&m, &o, &b, &st, &ck);
    let own = m.pads.iter().position(|p| p.ref_ == "J16").unwrap();
    let c = Vec2 { x: 20.0 * MM, y: -10.0 * MM };
    // Down-left past J15's corner (KiCad frame y down = model -y): grazes it.
    let graze = track_item(1, 2, 0.2 * MM, c, Vec2 { x: 18.98 * MM, y: -10.68 * MM }, None);
    // Clipping J15's corner with the centre line is no better (KiCad: 0.1025 mm).
    let clip = track_item(1, 2, 0.2 * MM, c, Vec2 { x: 19.38 * MM, y: -10.9 * MM }, None);
    assert!(!dr.stub_own_ok(&clip, own, &[]));
    // Straight out along the pad's axis: clear of J15.
    let clear = track_item(1, 2, 0.2 * MM, c, Vec2 { x: 21.6 * MM, y: -10.0 * MM }, None);
    assert!(!dr.stub_own_ok(&graze, own, &[]));
    assert!(dr.stub_own_ok(&clear, own, &[]));
    // Off without a minimum connection width.
    let mut m0 = m.clone();
    m0.rules.min_connection = 0;
    let ck0 = Checker::new(&m0, &o);
    let dr0 = Drawer::new(&m0, &o, &b, &st, &ck0);
    assert!(dr0.stub_own_ok(&graze, own, &[]));
}

#[test]
fn a_via_clears_its_own_pad_by_the_arc_tolerance() {
    // a capacitor pad (D6a): a +5V via 6 um into KiCad's pad polygon, which reaches max_error
    // (5 um here) past the exact copper. Clear by less than that: refused; by more: fine.
    use topor_engine::vias::ViaSize;
    let m = board(&[("C1", 1, 20.0, 10.0, 1.0, 1.0, 2), ("C2", 1, 30.0, 10.0, 1.0, 1.0, 2)], "");
    let b = board::build(&m, &opts()).unwrap();
    let st = State::new(&b.proj);
    let o = Options::default();
    let ck = Checker::new(&m, &o);
    let dr = Drawer::new(&m, &o, &b, &st, &ck);
    let sz = ViaSize { d: 600_000, drill: 300_000 };
    // Pad edge at x = 20.5 mm; via radius 0.3 mm.
    let at = |gap_um: f64| Vec2 { x: 20.5 * MM + 0.3 * MM + gap_um * 1e3, y: -10.0 * MM };
    assert!(!dr.via_own_pad_ok(at(2.0), sz, 1));
    assert!(dr.via_own_pad_ok(at(6.0), sz, 1));
    assert!(dr.via_own_pad_ok(Vec2 { x: 20.0 * MM, y: -10.0 * MM }, sz, 1), "centred in the pad");
}
