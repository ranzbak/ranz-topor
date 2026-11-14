//! D6b tests: layer-aware crossings in P2 (a crossing of two stretches held to the same single
//! layer is priced where it happens: a via when one of them can leave that layer there, an
//! overflow-priced violation when neither can).

use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, Board, BuildOpts};
use topor_engine::topo::init::{Conn, InitWeights};
use topor_engine::topo::layers::{self, LayerParams};
use topor_engine::topo::state::State;

const MM: f64 = 1e6;

/// Two-layer board `w` x `h` mm (KiCad frame) with SMD rect pads `(ref, net, x, y, w, h, layer)`
/// (layer 0 = F.Cu, 2 = B.Cu, 99 = both) and nets 1..=`nets`.
pub fn board(w: f64, h: f64, nets: usize, pads: &[(&str, i32, f64, f64, f64, f64, u32)]) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .map(|&(r, net, x, y, w, h, l)| {
            let ls: Vec<u32> = if l == 99 { vec![0, 2] } else { vec![l] };
            let shape = format!(r#"{{"kind":"rect","w":{},"h":{}}}"#, p(w), p(h));
            let shapes: Vec<String> = ls.iter().map(|l| format!(r#""{l}":{shape}"#)).collect();
            let ls: Vec<String> = ls.iter().map(|l| l.to_string()).collect();
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[{}],"shapes":{{{}}}}}"#,
                p(x),
                p(y),
                ls.join(","),
                shapes.join(",")
            )
        })
        .collect();
    let nets: Vec<String> = (1..=nets).map(|i| format!(r#"{{"code":{i},"name":"N{i}"}}"#)).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{}],
            "pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        nets.join(","),
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

pub fn opts() -> BuildOpts {
    BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }
}

/// The obstacle of pad `r`.
pub fn ob(m: &BoardModel, b: &Board, r: &str) -> usize {
    let i = m.pads.iter().position(|p| p.ref_ == r).unwrap();
    b.pad_of.iter().position(|&x| x == i).unwrap()
}

pub fn conn(m: &BoardModel, b: &Board, net: i32, ra: &str, rb: &str) -> Conn {
    let (pa, pb) = (ob(m, b, ra), ob(m, b, rb));
    Conn { net, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM }
}

/// N1 runs west-east and N2 north-south through a cross-shaped corridor (walls on both layers,
/// net N4); their pads are on F.Cu, so both wires are held to F.Cu. A foreign B.Cu pad (N3) covers
/// the middle of the crossing square: under it neither can change layer, so they must not cross
/// there (one takes vias and crosses where it can change layer, or passes the other's end pad on
/// B.Cu) (D6a, a 2-layer badge: 8 "crossing ... cannot be layered: both
/// stretches are fixed to the same layer", all inside the fan of a foreign pad on the other side).
#[test]
fn crossing_of_two_wires_held_to_one_layer_moves_off_the_foreign_pad() {
    let pads = vec![
        ("A1", 1, 2.0, 10.0, 1.0, 1.0, 0),
        ("A2", 1, 28.0, 10.0, 1.0, 1.0, 0),
        ("B1", 2, 15.3, 2.0, 1.0, 1.0, 0),
        ("B2", 2, 15.3, 18.0, 1.0, 1.0, 0),
        // Corner blocks (corridor y 7..13, x 12..18.5) and end caps behind the pads.
        ("W0", 4, 6.25, 3.75, 11.5, 6.5, 99),
        ("W1", 4, 24.0, 3.75, 11.0, 6.5, 99),
        ("W2", 4, 6.25, 16.25, 11.5, 6.5, 99),
        ("W3", 4, 24.0, 16.25, 11.0, 6.5, 99),
        ("W4", 4, 15.25, 0.85, 6.5, 0.7, 99),
        ("W5", 4, 15.25, 19.15, 6.5, 0.7, 99),
        ("W6", 4, 0.85, 10.0, 0.7, 6.0, 99),
        ("W7", 4, 29.15, 10.0, 0.7, 6.0, 99),
        // The foreign B.Cu pad over the middle of the crossing square.
        ("K0", 3, 15.6, 10.2, 4.0, 4.0, 2),
        ("K1", 3, 25.0, 10.0, 1.0, 1.0, 2),
    ];
    let m = board(30.0, 20.0, 4, &pads);
    let b = board::build(&m, &opts()).unwrap();
    for vias in [false, true] {
        let mut st = State::new(&b.proj);
        st.cross_vias = vias;
        let wt = InitWeights::default();
        st.route_conn(&conn(&m, &b, 2, "B1", "B2"), &wt, 1.0).expect("N2 routed");
        st.route_conn(&conn(&m, &b, 1, "A1", "A2"), &wt, 1.0).expect("N1 routed");
        let lr = layers::assign(&st, &|o| b.smd_layer(o), &LayerParams::default());
        assert!(lr.unlayerable.is_empty(), "crossing_vias {vias}: crossings that cannot be layered at {:?}", lr.unlayerable);
        assert!(lr.vias >= 1);
    }
}

#[test]
fn cross_pin_cases() {
    use topor_engine::topo::init::{cross_pin, CrossLayer};
    let (f, b, both) = (0b01u64, 0b10u64, 0b11u64);
    // The other stretch is free: no constraint.
    assert_eq!(cross_pin(f, both, both, both, true), (CrossLayer::Free, f));
    // Held to F.Cu: this wire leaves F.Cu for free when it may be on B.Cu.
    assert_eq!(cross_pin(both, both, f, both, true), (CrossLayer::Free, b));
    // Both held to F.Cu: a via on this wire, now on B.Cu.
    assert_eq!(cross_pin(f, both, f, both, true), (CrossLayer::Via, b));
    // This wire's triangle allows F.Cu only: the via goes on the other wire.
    assert_eq!(cross_pin(f, f, f, both, true), (CrossLayer::Via, f));
    // Neither triangle allows another layer: unlayerable.
    assert_eq!(cross_pin(f, f, f, f, true), (CrossLayer::Unlayerable, f));
    // crossing_vias = "crossing": the via case is the crossing price's, the pin stays.
    assert_eq!(cross_pin(f, both, f, both, false), (CrossLayer::Free, f));
    assert_eq!(cross_pin(both, both, f, both, false), (CrossLayer::Free, both));
    assert_eq!(cross_pin(f, f, f, f, false), (CrossLayer::Unlayerable, f));
}

#[test]
fn a_plane_via_site_must_stand_inside_the_pour_fill() {
    use topor_engine::geom::Vec2;
    use topor_engine::model::{Pour, PourFill, Pt};
    use topor_engine::topo::planes::in_fill;
    let sq = |x0: i64, y0: i64, x1: i64, y1: i64| vec![Pt { x: x0, y: y0 }, Pt { x: x1, y: y0 }, Pt { x: x1, y: y1 }, Pt { x: x0, y: y1 }];
    let mm = 1_000_000;
    let fill = PourFill { layer: 0, outer: sq(0, 0, 6 * mm, 6 * mm), holes: vec![sq(2 * mm, 2 * mm, 4 * mm, 4 * mm)] };
    let mut pour: Pour = serde_json::from_str(r#"{"uuid":"z","net":1,"layers":[0],"pads":[],"vias":[],"fills":[]}"#).unwrap();
    pour.fills.push(fill);
    let p = |x: f64, y: f64| Vec2 { x: x * 1e6, y: y * 1e6 };
    let r = 0.3e6;
    assert!(in_fill(&pour, p(1.0, 1.0), r), "inside, clear of the edges");
    assert!(!in_fill(&pour, p(8.0, 1.0), r), "outside the pour (qmtech 1V8 L pads)");
    assert!(!in_fill(&pour, p(0.2, 1.0), r), "the via disc pokes out of the fill");
    assert!(!in_fill(&pour, p(3.0, 3.0), r), "in a hole of the fill");
    assert!(!in_fill(&pour, p(1.8, 3.0), r), "too near a hole");
}

/// Route `c` as the core does with `net_tree = "free"` (component to component, any pad or branch
/// point; topo_d5.rs); returns the new wire.
fn tree_route(st: &mut State, b: &Board, c: &Conn) -> usize {
    use std::collections::BTreeMap;
    use topor_engine::topo::init::TreeEnd;
    use topor_engine::topo::state::{VertId, Wire};
    use topor_engine::topo::tree::{self, EndsCtx};
    let rad: Vec<f64> = (0..b.proj.pos.len()).map(|v| b.proj.free_radius(b.proj.pos[v], i32::MIN)).collect();
    let mut pads: BTreeMap<i32, Vec<(VertId, usize)>> = BTreeMap::new();
    for (o, ob) in b.proj.obstacles.iter().enumerate() {
        if let (Some(net), Some(v)) = (ob.net, b.centre[o]) {
            pads.entry(net).or_default().push((v, o));
        }
    }
    let smd: Vec<bool> = (0..b.proj.obstacles.len()).map(|o| b.smd_layer(o).is_some()).collect();
    let cx = EndsCtx { pads: &pads, rad: &rad, junctions: true, reward: 1.0, smd_cost: 0.0, smd: &smd, pad_nm: 1.0 * MM };
    let comp = tree::components(st, c.net);
    let (ca, cb) = (tree::comp_of(&comp, c.a), tree::comp_of(&comp, c.b));
    let src = tree::ends_of(st, &cx, c.net, &comp, ca, c.w, c.s, [c.a, c.b]);
    let dst = tree::ends_of(st, &cx, c.net, &comp, cb, c.w, c.s, [c.a, c.b]);
    let ((path, tris), i, j) = st.search_tree(c, &src, &dst, &InitWeights::default(), 1.0, None).expect("a path");
    let (sa, sb): (TreeEnd, TreeEnd) = (src[i], dst[j]);
    for e in [sa, sb] {
        if let Some((w, k)) = e.on {
            st.add_turn(w, k, e.v);
        }
    }
    let wire = Wire { net: c.net, w: c.w, s: c.s, a: sa.v, b: sb.v, pa: sa.obs, pb: sb.obs, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
    st.insert(wire, &path, tris)
}

/// The layer label of wire `w` at traversal `k`, fraction `s` (its last event at or before that
/// point, else its first event, else `bottom`): what the drawing does with P3's events.
fn label_at(ev: &[(usize, f64, i32)], k: usize, s: f64, bottom: i32) -> i32 {
    let mut l = ev.first().map_or(bottom, |e| e.2);
    for e in ev {
        if (e.0, e.1) <= (k, s) {
            l = e.2;
        }
    }
    l
}

/// D6b item 2a (Polubasov 2001 §5 p. 9 tee rule inside P3): a trunk between two pads free on both
/// layers and a branch from an F.Cu SMD pad that ends on it at a branch point. The tee is one
/// layer domain, so P3 puts the trunk on F.Cu at the junction and the whole tree needs no via
/// (D5: P3 labelled the trunk without the junction, the post-P3 fix-up then forced the branch
/// onto the trunk's B.Cu and a via next to the junction).
#[test]
fn the_tee_is_one_layer_domain_in_p3() {
    let m = board(60.0, 40.0, 1, &[("T1", 1, 5.0, 10.0, 1.5, 1.5, 99), ("T2", 1, 55.0, 10.0, 1.5, 1.5, 99), ("C", 1, 30.0, 32.0, 1.0, 1.0, 0)]);
    let b = board::build(&m, &opts()).unwrap();
    let mut st = State::new(&b.proj);
    let trunk = tree_route(&mut st, &b, &conn(&m, &b, 1, "T1", "T2"));
    let br = tree_route(&mut st, &b, &conn(&m, &b, 1, "C", "T1"));
    let js = topor_engine::topo::tree::junctions(&st);
    assert_eq!(js.len(), 1, "the branch ends on the trunk at a branch point");
    let (bw, end, tw, k, _) = js[0];
    assert_eq!((bw, tw), (br, trunk));
    let rep = layers::assign(&st, &|o| b.smd_layer(o), &LayerParams::default());
    let n = st.wires[br].passes.len();
    let (bk, bs) = if end == 0 { (0, 0.0) } else { (n, 1.0) };
    let lt = label_at(&rep.events[trunk], k, 0.5, 1);
    let lb = label_at(&rep.events[br], bk, bs, 1);
    assert_eq!(lt, lb, "trunk and branch meet on one layer at the junction: {:?} / {:?}", rep.events[trunk], rep.events[br]);
    assert_eq!(lb, 0, "on the branch pad's F.Cu");
    assert_eq!(rep.vias, 0);
}
