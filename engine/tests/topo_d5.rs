//! D5 tests (net trees that follow the routing): the multi-source search joins two components
//! of a net, a pad already on a trace is a through-pad junction, and a branch point on a trace
//! makes the trunk turn there.

use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::init::{Conn, InitWeights, TreeEnd};
use topor_engine::topo::state::{State, VertId, Wire, NO_OBS};
use topor_engine::topo::tree::{self, EndsCtx};
use std::collections::BTreeMap;

const MM: f64 = 1e6;

/// Two-layer board 60 x 40 mm with through-hole pads (net, x, y) mm of 1.5 mm, KiCad frame.
fn board(pads: &[(i32, f64, f64)]) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (60.0, 0.0), (60.0, 40.0), (0.0, 40.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y))| {
            let sh = format!(r#"{{"kind":"circle","d":{}}}"#, p(1.5));
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"pth",
                    "center":[{cx},{cy}],"rotation_deg":0.0,"layers":[0, 2],
                    "shapes":{{"0":{sh},"2":{sh}}},"drill":{{"w":{dr},"h":{dr}}}}}"#,
                cx = p(x),
                cy = p(y),
                dr = p(0.8)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// Route `c` the way the core does with `net_tree = "free"`: from the component of its first pad
/// to the component of its second, any pad or branch point; returns the new wire.
fn tree_route(st: &mut State, c: &Conn, pads: &BTreeMap<i32, Vec<(VertId, usize)>>, rad: &[f64], reward: f64) -> usize {
    let smd = vec![false; st.proj.obstacles.len()];
    let cx = EndsCtx { pads, rad, junctions: true, reward, smd_cost: 0.0, smd: &smd, pad_nm: 1.0 * MM };
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

fn setup(m: &BoardModel) -> (board::Board, Vec<f64>) {
    let b = board::build(m, &BuildOpts::default()).unwrap();
    let rad: Vec<f64> = (0..b.proj.pos.len()).map(|v| b.proj.free_radius(b.proj.pos[v], i32::MIN)).collect();
    (b, rad)
}

fn conn(b: &board::Board, oa: usize, ob: usize) -> Conn {
    Conn { net: 1, pa: oa, pb: ob, a: b.centre[oa].unwrap(), b: b.centre[ob].unwrap(), w: 0.25 * MM, s: 0.2 * MM }
}

/// GPIO04 on wiccon (D5, the user's sketch): the decomposition planned top -> middle and
/// top -> far; the far connection starts from the middle pad (a through-pad junction) instead
/// of a second trace from the top pad, because the middle pad already belongs to the top pad's
/// component and is nearer: the tree follows the routing.
#[test]
fn a_pad_already_on_the_tree_is_a_junction() {
    let m = board(&[(1, 10.0, 20.0), (1, 20.0, 20.0), (1, 40.0, 20.0)]);
    let (b, rad) = setup(&m);
    let mut st = State::new(&b.proj);
    let mut pads: BTreeMap<i32, Vec<(VertId, usize)>> = BTreeMap::new();
    pads.insert(1, (0..3).map(|o| (b.centre[o].unwrap(), o)).collect());
    let w0 = tree_route(&mut st, &conn(&b, 0, 1), &pads, &rad, 1.0);
    assert_eq!((st.wires[w0].pa, st.wires[w0].pb), (0, 1));
    let w1 = tree_route(&mut st, &conn(&b, 0, 2), &pads, &rad, 1.0);
    let ends = [st.wires[w1].pa, st.wires[w1].pb];
    assert!(ends.contains(&1) && ends.contains(&2), "the far pad hangs off the middle pad: {ends:?}");
}

/// A pad off to the side of a long trunk joins it at a branch point (a free vertex of a
/// triangle the trunk runs through): the trunk turns there, the branch ends there, and the
/// branch is shorter than a separate trace to either end pad.
#[test]
fn a_side_pad_joins_the_trunk_at_a_branch_point() {
    let m = board(&[(1, 5.0, 10.0), (1, 55.0, 10.0), (1, 30.0, 32.0)]);
    let (b, rad) = setup(&m);
    let mut st = State::new(&b.proj);
    let mut pads: BTreeMap<i32, Vec<(VertId, usize)>> = BTreeMap::new();
    pads.insert(1, (0..3).map(|o| (b.centre[o].unwrap(), o)).collect());
    let trunk = tree_route(&mut st, &conn(&b, 0, 1), &pads, &rad, 1.0);
    let br = tree_route(&mut st, &conn(&b, 2, 0), &pads, &rad, 1.0);
    let wi = &st.wires[br];
    assert!(wi.pa == NO_OBS || wi.pb == NO_OBS, "the branch ends on the trunk");
    let v = if wi.pa == NO_OBS { wi.a } else { wi.b };
    assert_eq!(st.wires[trunk].turns.iter().filter(|t| t.1 == v).count(), 1, "the trunk turns at the branch point");
    let len: f64 = st.polyline(br).windows(2).map(|x| (x[1] - x[0]).len()).sum();
    assert!(len < 25.0 * MM, "branch {:.1} mm, a separate trace to an end pad would be > 33 mm", len / MM);
    // Ripping the trunk takes the branch with it.
    let gone = tree::dependents(&st, &[trunk].into_iter().collect());
    assert!(gone.contains(&br));
    tree::remove_wire(&mut st, br);
    assert!(st.wires[trunk].turns.is_empty(), "the trunk runs straight again");
}
