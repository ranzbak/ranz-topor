//! S3-complete tests: every P4 move fires on a small board and lowers its price, the descent
//! keeps only what pays, and `blocks_pad` changes a search choice (not only its price).

use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::factors::{standard_factors, Event, FactorConfig, Registry};
use topor_engine::topo::init::{Conn, InitWeights};
use topor_engine::topo::moves::{Kind, Labels, Tally, Topo};
use topor_engine::topo::pack::Packer;
use topor_engine::topo::refine::CapModel;
use topor_engine::topo::state::{State, WireId};

const MM: f64 = 1e6;
const WS0: f64 = 400_000.0;

/// Two-layer board, `outline` (x0, y0, x1, y1) mm, pads (net, x, y, th, size) mm, KiCad frame.
fn board(outline: (f64, f64, f64, f64), pads: &[(i32, f64, f64, bool, f64)]) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let (x0, y0, x1, y1) = outline;
    let c = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(net, x, y, th, d))| {
            let (attr, layers, drill) = if th { ("pth", "[0, 2]", format!(r#"{{"w":{0},"h":{0}}}"#, p(d / 2.0))) } else { ("smd", "[0]", "null".to_string()) };
            let sh = format!(r#"{{"kind":"rect","w":{0},"h":{0}}}"#, p(d));
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"{attr}",
                    "center":[{},{}],"rotation_deg":0.0,"layers":{layers},
                    "shapes":{{"0":{sh},"2":{sh}}},"drill":{drill}}}"#,
                p(x),
                p(y)
            )
        })
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn topo(conns: Vec<Conn>, wire_conn: Vec<usize>) -> Topo {
    let n = conns.len();
    Topo {
        reg: Registry::standard(),
        conns,
        tiers: vec![3; n],
        wire_conn,
        reroutes: vec![0; n],
        ws0: WS0,
        cap: 2_000_000,
        evals: 0,
        max_evals: 1000,
        ripped: 0,
        max_ripped: 1000,
        tight: Vec::new(),
        tally: Tally::default(),
    }
}

/// Route `conns` in order; the wire ids equal the connection indices.
fn route(st: &mut State, conns: &[Conn], w: &InitWeights) {
    for c in conns {
        st.route_conn(c, w, (c.w + c.s) / WS0).expect("routes");
    }
}

fn total_cost(t: &Topo, st: &mut State) -> f64 {
    let alive: Vec<WireId> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect();
    alive.into_iter().map(|w| t.wire_cost(st, w)).sum()
}

fn obs_at(b: &board::Board, x: f64, y: f64) -> usize {
    b.proj.obstacles.iter().position(|o| (o.center.x - x * MM).abs() < 1.0 && (o.center.y + y * MM).abs() < 1.0).expect("pad")
}

// ---------------------------------------------------------------- reroute

/// A wall with two doors; net 3 was routed through the far door under a congestion history that
/// is gone now: the reroute takes the near door and the wire's price drops.
#[test]
fn reroute_takes_the_cheaper_path_once_the_history_is_gone() {
    let m = board((0.0, 0.0, 13.0, 10.0), &[(7, 1.9, 5.0, true, 2.8), (8, 6.2, 5.0, true, 2.8), (9, 11.0, 5.0, true, 2.6), (3, 4.2, 0.4, true, 0.4), (3, 4.2, 9.6, true, 0.4)]);
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns: Vec<Conn> = b.connections(&m).into_iter().filter(|c| c.net == 3).collect();
    let mut st = State::new(&b.proj);
    let near = b.proj.edges_between(obs_at(&b, 1.9, 5.0), obs_at(&b, 6.2, 5.0));
    for &e in &near {
        st.history[e] = 5.0;
    }
    route(&mut st, &conns, &InitWeights::default());
    let through_near = |st: &State| st.passes_between(obs_at(&b, 1.9, 5.0), obs_at(&b, 6.2, 5.0)).iter().any(|&p| st.wires[st.passes[p].wire].alive);
    assert!(!through_near(&st), "the history sent it through the far door");
    for &e in &near {
        st.history[e] = 0.0;
    }
    let mut t = topo(conns, vec![0]);
    let before = total_cost(&t, &mut st);
    let nw = t.try_reroute(&mut st, 0).expect("the reroute fires");
    assert!(through_near(&st), "now through the near door");
    let after = total_cost(&t, &mut st);
    assert!(after < before - 1.0, "{before} -> {after}");
    assert_eq!(t.tally.applied_of(Kind::Reroute), 1);
    assert_eq!(t.reroutes[0], 1, "the reroute counter counts it");
    assert_eq!(t.wire_conn[nw], 0);
    // Moving it once more costs the reroute price: nothing to gain, nothing happens.
    assert!(t.try_reroute(&mut st, nw).is_none());
    assert!(st.check().is_ok());
}

// ---------------------------------------------------------------- attachment point

/// Net 3 has pads A (1, 5), C (7, 5) and B (9, 5); the tree is A-C plus A-B. A-B attached at C
/// instead (C-B) keeps the tree and is 6 mm shorter.
#[test]
fn attachment_moves_a_connection_to_a_nearer_pad_of_its_tree() {
    let m = board((0.0, 0.0, 10.0, 10.0), &[(3, 1.0, 5.0, true, 0.6), (3, 7.0, 7.0, true, 0.6), (3, 9.0, 5.0, true, 0.6)]);
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let base = b.connections(&m)[0];
    let (oa, oc, ob) = (obs_at(&b, 1.0, 5.0), obs_at(&b, 7.0, 7.0), obs_at(&b, 9.0, 5.0));
    let mk = |p: usize, q: usize| Conn { pa: p, pb: q, a: b.centre[p].unwrap(), b: b.centre[q].unwrap(), ..base };
    let conns = vec![mk(oa, oc), mk(oa, ob)];
    let mut st = State::new(&b.proj);
    route(&mut st, &conns, &InitWeights::default());
    let mut t = topo(conns, vec![0, 1]);
    let before = total_cost(&t, &mut st);
    let nw = t.try_attach(&mut st, 1, 3).expect("the attachment move fires");
    assert_eq!((t.conns[1].pa, t.conns[1].pb), (oc, ob), "now C-B");
    assert_eq!(t.tally.applied_of(Kind::Attach), 1);
    let after = total_cost(&t, &mut st);
    assert!(after < before - 3.0, "{before} -> {after}");
    assert_eq!(t.wire_conn[nw], 1);
    assert!(st.check().is_ok());
}

// ---------------------------------------------------------------- merge, channel swap, joint

/// Three parallel wires: u (net 3) at y = 4, v (net 3) at y = 5, x (net 4) at y = 6. `v` is put
/// on the far side of `x` along their shared edges: it crosses `x` twice.
fn parallel(v_net: i32) -> (BoardModel, Vec<Conn>) {
    let m = board(
        (0.0, 0.0, 12.0, 10.0),
        &[(3, 1.0, 4.0, true, 0.5), (3, 11.0, 4.0, true, 0.5), (v_net, 1.0, 5.0, true, 0.5), (v_net, 11.0, 5.0, true, 0.5), (4, 1.0, 6.0, true, 0.5), (4, 11.0, 6.0, true, 0.5), (9, 6.0, 8.5, true, 1.0), (9, 6.0, 1.5, true, 1.0)],
    );
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let base = b.connections(&m)[0];
    let mk = |net: i32, y: f64| {
        let (p, q) = (obs_at(&b, 1.0, y), obs_at(&b, 11.0, y));
        Conn { net, pa: p, pb: q, a: b.centre[p].unwrap(), b: b.centre[q].unwrap(), ..base }
    };
    let conns = vec![mk(3, 4.0), mk(v_net, 5.0), mk(4, 6.0)];
    (m, conns)
}

/// Route u, x, then v with its gaps beyond x wherever x passes the same edge. Returns whether
/// any gap moved.
fn misplace(st: &mut State, conns: &[Conn]) -> bool {
    let w = InitWeights::default();
    st.route_conn(&conns[0], &w, 1.0).unwrap();
    st.route_conn(&conns[2], &w, 1.0).unwrap(); // wire 1 = x
    let (mut path, tris) = st.search(&conns[1], &w, 1.0).unwrap();
    let mut moved = false;
    // Only interior edges: the first and last stay, so v leaves and joins its pads normally.
    let n = path.len();
    for (j, g) in path.iter_mut().enumerate().take(n.saturating_sub(1)).skip(1) {
        let (e, gap) = *g;
        if let Some(ix) = st.edge_order[e].iter().position(|&p| st.passes[p].wire == 1) {
            *g = (e, if gap <= ix { ix + 1 } else { ix });
            moved = true;
        }
        let _ = j;
    }
    let c = &conns[1];
    let wire = topor_engine::topo::state::Wire { net: c.net, w: c.w, s: c.s, a: c.a, b: c.b, pa: c.pa, pb: c.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
    st.insert(wire, &path, tris); // wire 2 = v
    moved
}

fn crossings_of(st: &State, w: WireId, other: WireId) -> usize {
    st.crossings(w).iter().filter(|x| x.other == other).count()
}

/// A door (0.65 mm between two wall pads, 1.3 mm on both layers together) holds three 0.2 mm
/// tracks only when two of them are of one net and side by side (no clearance between them). `v` (net 3) runs through it with `x`
/// (net 4) between it and `u` (net 3): over-full. The merge puts `v` next to `u`, which costs
/// two crossings with `x` but removes the overflow.
#[test]
fn merge_puts_a_stretch_next_to_its_own_net() {
    let m = board(
        (0.0, 0.0, 10.0, 10.0),
        &[(7, 2.5, 5.0, true, 3.0), (8, 6.15, 5.0, true, 3.0), (3, 4.15, 1.0, true, 0.3), (3, 4.15, 9.0, true, 0.3), (4, 4.33, 1.6, true, 0.3), (4, 4.33, 8.4, true, 0.3), (3, 4.5, 2.2, true, 0.3), (3, 4.5, 7.8, true, 0.3)],
    );
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let mk = |net: i32, x: f64, y0: f64, y1: f64| {
        let (p, q) = (obs_at(&b, x, y0), obs_at(&b, x, y1));
        Conn { net, pa: p, pb: q, a: b.centre[p].unwrap(), b: b.centre[q].unwrap(), w: 200_000.0, s: 200_000.0 }
    };
    let (u, v, x) = (mk(3, 4.5, 2.2, 7.8), mk(3, 4.15, 1.0, 9.0), mk(4, 4.33, 1.6, 8.4));
    let mut st = State::new(&b.proj);
    let w = InitWeights::default();
    st.route_conn(&u, &w, 1.0).unwrap(); // wire 0
    st.route_conn(&x, &w, 1.0).unwrap(); // wire 1
    let (mut path, tris) = st.search(&v, &w, 1.0).unwrap();
    // v on the far side of x wherever x and u share an edge with it.
    let mut forced = 0;
    for g in path.iter_mut() {
        let e = g.0;
        let pos = |wire: usize| st.edge_order[e].iter().position(|&p| st.passes[p].wire == wire);
        if let (Some(iu), Some(ix)) = (pos(0), pos(1)) {
            g.1 = if ix < iu { ix } else { ix + 1 };
            forced += 1;
        }
    }
    assert!(forced >= 1, "v shares the door with u and x");
    let wire = topor_engine::topo::state::Wire { net: 3, w: v.w, s: v.s, a: v.a, b: v.b, pa: v.pa, pb: v.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true };
    st.insert(wire, &path, tris); // wire 2
    let mut t = topo(vec![u, x, v], vec![0, 1, 2]);
    let before = t.wire_cost(&mut st, 2);
    assert!(t.try_merge(&mut st, 2), "the merge fires");
    let after = t.wire_cost(&mut st, 2);
    assert!(after < before, "{before} -> {after}");
    let door: Vec<usize> = st.passes_between(obs_at(&b, 2.5, 5.0), obs_at(&b, 6.15, 5.0));
    for e in b.proj.edges_between(obs_at(&b, 2.5, 5.0), obs_at(&b, 6.15, 5.0)) {
        let ws: Vec<usize> = st.edge_order[e].iter().map(|&p| st.passes[p].wire).collect();
        if let (Some(iu), Some(iv)) = (ws.iter().position(|&w| w == 0), ws.iter().position(|&w| w == 2)) {
            assert_eq!(iu.abs_diff(iv), 1, "u and v side by side on edge {e}: {ws:?}");
        }
    }
    assert!(!door.is_empty());
    assert_eq!(t.tally.applied_of(Kind::Merge), 1);
    assert!(st.check().is_ok());
}

#[test]
fn channel_swap_slides_a_crossing_along_the_shared_edges() {
    let (m, conns) = parallel(5);
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let mut st = State::new(&b.proj);
    assert!(misplace(&mut st, &conns));
    let x0 = crossings_of(&st, 2, 1);
    assert!(x0 >= 2, "{x0}");
    let mut t = topo(vec![conns[0], conns[2], conns[1]], vec![0, 1, 2]);
    let before = total_cost(&t, &mut st);
    assert!(t.try_channel_swap(&mut st, 2), "the swap fires");
    assert!(crossings_of(&st, 2, 1) < x0, "the crossing slid onto the other one and both left");
    assert!(total_cost(&t, &mut st) < before - 10.0);
    assert_eq!(t.tally.applied_of(Kind::ChannelSwap), 1);
    assert!(st.check().is_ok());
}

#[test]
fn joint_move_reroutes_two_crossing_wires_in_the_other_order() {
    let (m, conns) = parallel(5);
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let mut st = State::new(&b.proj);
    assert!(misplace(&mut st, &conns));
    let mut t = topo(vec![conns[0], conns[2], conns[1]], vec![0, 1, 2]);
    let before = total_cost(&t, &mut st);
    let (na, nb) = t.try_joint(&mut st, 2, 1).expect("the joint move fires");
    assert_eq!(crossings_of(&st, na, nb), 0);
    assert!(total_cost(&t, &mut st) < before - 10.0);
    assert_eq!(t.tally.applied_of(Kind::Joint), 1);
    assert_eq!((t.wire_conn[na], t.wire_conn[nb]), (2, 1));
    assert!(st.check().is_ok());
}

#[test]
fn double_crossing_swap_and_the_descent_keep_only_what_pays() {
    use topor_engine::topo::descent::descend_topo;
    let (m, conns) = parallel(5);
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let mut st = State::new(&b.proj);
    assert!(misplace(&mut st, &conns));
    let mut t = topo(vec![conns[0], conns[2], conns[1]], vec![0, 1, 2]);
    let before = total_cost(&t, &mut st);
    let reg = Registry::standard();
    let d = descend_topo(&mut st, &mut t, &reg, 1, 0.0);
    let after = total_cost(&t, &mut st);
    assert!(after < before - 10.0, "{before} -> {after} ({})", d.text());
    assert!(d.applied.iter().sum::<usize>() >= 1);
    assert!(st.total_crossings() <= 1);
    // On a clean state the double-crossing swap finds nothing and a second descent keeps nothing
    // but what lowers the price.
    let again = descend_topo(&mut st, &mut t, &reg, 2, 0.0);
    assert!(total_cost(&t, &mut st) <= after + 1e-6, "{}", again.text());
    assert!(st.check().is_ok());
    // The double-crossing swap alone (reused from P2): two crossings of one pair go.
    let mut st2 = State::new(&b.proj);
    misplace(&mut st2, &conns);
    let x0 = crossings_of(&st2, 2, 1);
    let swaps = st2.uncross_all(50);
    assert!(swaps >= 1 && crossings_of(&st2, 2, 1) + 2 <= x0, "{swaps} swaps, {x0} -> {}", crossings_of(&st2, 2, 1));
}

// ---------------------------------------------------------------- label moves

/// Two wires through a door that holds one track per layer: net 1 at x = 4.6, net 2 at x = 5.4,
/// both from y = 1 to y = 9 through the gap between the wall pads at (2.5, 5) and (7.5, 5).
fn door() -> BoardModel {
    board((0.0, 0.0, 10.0, 10.0), &[(7, 2.0, 5.0, true, 5.0), (8, 8.0, 5.0, true, 5.0), (1, 4.6, 1.0, true, 0.4), (1, 4.6, 9.0, true, 0.4), (2, 5.4, 1.0, true, 0.4), (2, 5.4, 9.0, true, 0.4)])
}

/// The passes of `w` through the door between the two wall pads.
fn door_pass(st: &State, b: &board::Board, w: WireId) -> usize {
    let door: Vec<usize> = st.passes_between(obs_at(b, 2.0, 5.0), obs_at(b, 8.0, 5.0));
    st.wires[w].passes.iter().position(|p| door.contains(p)).expect("through the door")
}

fn labels_case(f: impl FnOnce(&State, &board::Board, &Packer, &[Vec<topor_engine::geom::Vec2>])) {
    let m = door();
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    route(&mut st, &conns, &InitWeights::default());
    let packer = Packer::for_model(&m, &Options::default(), &b.proj);
    let polys: Vec<_> = (0..st.wires.len()).map(|w| st.polyline(w)).collect();
    f(&st, &b, &packer, &polys);
}

fn labels<'a>(st: &'a State<'a>, cm: CapModel<'a>, evl: Vec<Vec<(usize, f64, i32)>>, mask: &'a [Vec<u64>], partner: &'a [Vec<Option<(WireId, usize)>>]) -> Labels<'a> {
    Labels { st, cm, evl, mask, partner, bottom: 1, via_mm: 25.0, n_signal: 2, evals: 0, max_evals: 100, tally: Tally::default() }
}

#[test]
fn the_door_holds_one_track_per_layer() {
    labels_case(|st, _, packer, polys| {
        let both0 = vec![vec![(0, 0.5, 0)], vec![(0, 0.5, 0)]];
        assert!(CapModel::new(st, packer, polys, &both0, 1, 50.0).overfull() >= 1, "both on layer 0: over-full");
        let split = vec![vec![(0, 0.5, 0)], vec![(0, 0.5, 1)]];
        assert_eq!(CapModel::new(st, packer, polys, &split, 1, 50.0).overfull(), 0, "one per layer: fits");
    });
}

#[test]
fn via_shift_moves_a_layer_change_out_of_a_full_door() {
    labels_case(|st, b, packer, polys| {
        let (kd, n) = (door_pass(st, b, 0), st.wires[0].passes.len());
        assert!(kd >= 1 && kd + 2 <= n, "door pass {kd} of {n}");
        // Wire 0: layer 0 until after the door, then layer 1; wire 1 on layer 0: the door is full.
        let evl = vec![vec![(0, 0.5, 0), (kd + 1, 0.5, 0), (n, 0.5, 1)], vec![(0, 0.5, 0)]];
        let (mask, partner) = (vec![vec![0b11; 3], vec![0b11]], vec![vec![None; 3], vec![None]]);
        let cm = CapModel::new(st, packer, polys, &evl, 1, 50.0);
        let before = cm.total();
        assert!(before > 0.0);
        let mut lb = labels(st, cm, evl, &mask, &partner);
        assert_eq!(lb.via_shifts(0), 1, "the via shift fires");
        assert_eq!(lb.cm.total(), 0.0, "the door fits now");
        assert_eq!(lb.evl[0].windows(2).filter(|x| x[0].2 != x[1].2).count(), 1, "still one via");
        assert_eq!(lb.tally.applied_of(Kind::ViaShift), 1);
    });
}

#[test]
fn crossing_flip_removes_a_via() {
    labels_case(|st, _, packer, polys| {
        // A fabricated crossing between event 1 of wire 0 and event 0 of wire 1: wire 0 changes
        // layer for it, flipping the crossing makes wire 0 single-layer.
        let evl = vec![vec![(0, 0.5, 0), (2, 0.5, 1)], vec![(1, 0.5, 0)]];
        let mask = vec![vec![0b11; 2], vec![0b11]];
        let partner = vec![vec![None, Some((1, 0))], vec![Some((0, 1))]];
        let cm = CapModel::new(st, packer, polys, &evl, 1, 50.0);
        let mut lb = labels(st, cm, evl, &mask, &partner);
        assert_eq!(lb.flips(0), 1, "the flip fires");
        assert_eq!(lb.evl[0][1].2, 0);
        assert_eq!(lb.evl[1][0].2, 1);
        assert_eq!(lb.tally.applied_of(Kind::Flip), 1);
    });
}

#[test]
fn segment_lifting_puts_a_stretch_on_its_neighbours_layer() {
    labels_case(|st, _, packer, polys| {
        // Wire 0 on layer 0, a middle stretch on layer 1 (two vias), wire 1 on layer 1: lifting
        // the stretch onto layer 0 drops both vias and keeps the door fitting.
        let n = st.wires[0].passes.len();
        let evl = vec![vec![(0, 0.5, 0), (1, 0.5, 1), (n, 0.5, 0)], vec![(0, 0.5, 1)]];
        let (mask, partner) = (vec![vec![0b11; 3], vec![0b11]], vec![vec![None; 3], vec![None]]);
        let cm = CapModel::new(st, packer, polys, &evl, 1, 50.0);
        let mut lb = labels(st, cm, evl, &mask, &partner);
        assert!(lb.lifts(0) >= 1, "the lift fires");
        assert!(lb.evl[0].windows(2).filter(|x| x[0].2 != x[1].2).count() < 2, "{:?}", lb.evl[0]);
        assert_eq!(lb.cm.total(), 0.0);
        assert!(lb.tally.applied_of(Kind::Lift) >= 1);
    });
}

#[test]
fn a_label_move_that_overfills_the_door_is_refused() {
    labels_case(|st, _, packer, polys| {
        // Lifting wire 0 onto layer 1 would drop its two vias but fill the door on layer 1, where
        // wire 1 runs: refused by the capacity test.
        let n = st.wires[0].passes.len();
        let evl = vec![vec![(0, 0.5, 1), (1, 0.5, 0), (n, 0.5, 1)], vec![(0, 0.5, 1)]];
        let (mask, partner) = (vec![vec![0b11; 3], vec![0b11]], vec![vec![None; 3], vec![None]]);
        let cm = CapModel::new(st, packer, polys, &evl, 1, 50.0);
        let before = cm.total();
        let mut lb = labels(st, cm, evl.clone(), &mask, &partner);
        lb.lifts(0);
        let _ = before;
        assert!(lb.cm.total() <= before + 1e-9, "never more over-full");
    });
}

// ---------------------------------------------------------------- blocks_pad in the search

/// The search itself avoids the last free entry sector of an unrouted pad when `blocks_pad` is
/// dear, and takes it when it is free (S3 leftover: the S3 test only checked the price).
#[test]
fn blocks_pad_changes_the_search_choice() {
    let m = board((0.0, 0.0, 10.0, 10.0), &[(5, 5.0, 5.0, true, 0.8), (5, 9.0, 9.0, true, 0.8), (3, 1.0, 1.0, true, 0.4), (3, 1.0, 9.0, true, 0.4), (4, 2.0, 5.0, true, 0.4), (4, 8.0, 5.0, true, 0.4)]);
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let conns = b.connections(&m);
    let mut st = State::new(&b.proj);
    let w0 = InitWeights::default();
    st.route_conn(conns.iter().find(|c| c.net == 3).unwrap(), &w0, 1.0).unwrap();
    let x = b.proj.obstacles.iter().position(|o| o.net == Some(5) && (o.center.x - 5.0 * MM).abs() < 1.0).unwrap();
    let c4 = conns.iter().find(|c| c.net == 4).unwrap();
    let (_, tris) = st.search(c4, &w0, 1.0).unwrap();
    let t_star = *tris.iter().find(|t| st.ring[x].contains(t)).expect("the free path passes the pad's ring");
    // Stand-ins of another net's stretch in every other ring triangle: a degenerate traversal
    // (both ends at one corner) that occupies the triangle and crosses nothing.
    for &t in st.ring[x].clone().iter().filter(|&&t| t != t_star) {
        let v = b.proj.tris[t][0];
        let id = st.wires.len();
        st.wires.push(topor_engine::topo::state::Wire { net: 3, w: 1.0, s: 1.0, a: v, b: v, pa: usize::MAX, pb: usize::MAX, turns: Vec::new(), passes: Vec::new(), tris: vec![t], alive: true });
        st.tri_trav[t].push((id, 0));
    }
    let wt = |w: f64| {
        let reg = Registry::from_config(standard_factors(), &FactorConfig { weight: [("blocks_pad".to_string(), w)].into_iter().collect(), ..FactorConfig::default() }).0;
        InitWeights::from_registry(&reg, &topor_engine::topo::factors::WireCtx { net: 4, tier: 3 })
    };
    let free = st.search(c4, &wt(0.0), 1.0).unwrap().1;
    assert!(free.contains(&t_star), "blocks_pad 0: through the last free sector");
    let dear = st.search(c4, &wt(1000.0), 1.0).unwrap().1;
    assert!(!dear.contains(&t_star), "blocks_pad 1000: the search goes around it");
    // The reroute factor: one more move of a twice-moved connection costs twice the weight.
    let r = Registry::standard();
    assert_eq!(r.price(&Event::Rerouted { wire: 0, count: 2 }, &Default::default()), 10.0);
}
