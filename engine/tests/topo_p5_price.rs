//! Shopping list 5.2 completion (user 2026-11-10, Q15 (a) + (b)): the TopoR loop's reroute.
//!
//! (a) Price = criterion: a reroute is priced with the archive's own predicted-violation measure
//! ([`Predicted`], from [`CapModel::violations`]) at the `violation` weight, so a reroute that
//! adds an over-full passage pays exactly that measure times the weight, and one that removes it
//! gains as much.
//!
//! (b) Layer-aware crossings (Polubasov 2010 «Оптимизация разводки», figs. 4, 6, 9–10; Polubasov
//! 2001 p. 4): with the stretches' present layers ([`State::layers_now`]) a crossing of a stretch
//! on the other layer than the wire can keep costs no via, a crossing of a stretch on the wire's
//! only layer costs the via pair (down before it, up again to the pad's layer).

use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::archive::Objective;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::factors::{events, Event, Move, Registry, WireCtx};
use topor_engine::topo::init::{Conn, InitWeights};
use topor_engine::topo::layers::{self, LayerParams, LayerReport};
use topor_engine::topo::optimise::{criteria_of, evaluation, layers_now, reroute_set, Predicted, Rerouter};
use topor_engine::topo::pack::Packer;
use topor_engine::topo::refine::CapModel;
use topor_engine::topo::state::{State, Wire, WireId};

const MM: f64 = 1e6;
const WS0: f64 = 450_000.0;

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
            "nets":[{{"code":1,"name":"A"}},{{"code":2,"name":"B"}},{{"code":3,"name":"C"}},{{"code":7,"name":"W1"}},{{"code":8,"name":"W2"}}],
            "pads":[{}],"outline":[{}],"rules":{{"per_net":{{}}}}}}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn conn_of(conns: &[Conn], net: i32) -> Conn {
    *conns.iter().find(|c| c.net == net).expect("a connection of the net")
}

fn alive_of(st: &State, net: i32) -> WireId {
    st.wires.iter().position(|w| w.alive && w.net == net).expect("an alive wire of the net")
}

fn wire_of(c: &Conn) -> Wire {
    Wire { net: c.net, w: c.w, s: c.s, a: c.a, b: c.b, pa: c.pa, pb: c.pb, turns: Vec::new(), passes: Vec::new(), tris: Vec::new(), alive: true }
}

/// The events of `c` laid along `path` into `st`.
fn insert_events(st: &State, c: &Conn, path: &[(usize, usize)], tris: &[usize]) -> Vec<Event> {
    events(st, &Move::Insert { wire: &wire_of(c), path, tris, scale: (c.w + c.s) / WS0 })
}

fn forced(ev: &[Event]) -> usize {
    ev.iter().filter(|e| matches!(e, Event::LayerForced { .. })).count()
}

/// Net 1 runs on F.Cu (SMD pads at (2, 5) and (18, 5)); net 2's stretch spans the board from edge
/// to edge at x = 10, so net 1 has to cross it. Crossing net 2 while it lies on B.Cu costs net 1
/// no via; while it lies on F.Cu, net 1 must go down to B.Cu before the crossing and up again to
/// its pad: two vias, priced as two vias (Polubasov 2001 p. 4: a stretch between two crossings, or
/// a crossing and a pad, needs a via when its ends lie on different layers). Without the present
/// layers (the layer-blind search before 5.2) net 2 is a through-hole wire free on both layers,
/// so the crossing costs nothing either way.
#[test]
fn a_crossing_is_priced_by_the_vias_it_implies_against_the_present_layer() {
    let m = board((0.0, 0.0, 20.0, 10.0), &[(1, 2.0, 5.0, false, 1.0), (1, 18.0, 5.0, false, 1.0), (2, 10.0, 0.6, true, 1.0), (2, 10.0, 9.4, true, 1.0)]);
    let reg = Registry::standard();
    let via_mm = reg.price(&Event::Via { wire: 0, count: 1, at: None }, &WireCtx::default());
    assert_eq!(via_mm, 25.0);
    let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, ..BuildOpts::default() }).unwrap();
    let conns = b.connections(&m);
    let (c1, c2) = (conn_of(&conns, 1), conn_of(&conns, 2));
    let mut st = State::new(&b.proj);
    let wt = |c: &Conn| InitWeights { max_expansions: 2_000_000, ..InitWeights::from_registry(&reg, &WireCtx { net: c.net, tier: 3 }) };
    let w2 = st.route_conn(&c2, &wt(&c2), (c2.w + c2.s) / WS0).expect("net 2 routes").0;
    st.cross_vias = true;
    let bottom = 1;
    // Net 2 on B.Cu (layer 1): one event labelled 1.
    let on_b = layers_now(&st, &[vec![(0, 0.5, 1)]], bottom);
    let on_f = layers_now(&st, &[vec![(0, 0.5, 0)]], bottom);
    assert!(on_b[w2].iter().all(|&m| m == 0b10) && on_f[w2].iter().all(|&m| m == 0b01));
    st.layers_now = on_b;
    let (path, tris) = st.search(&c1, &wt(&c1), (c1.w + c1.s) / WS0).expect("net 1 routes");
    let ev_b = insert_events(&st, &c1, &path, &tris);
    assert_eq!(ev_b.iter().filter(|e| matches!(e, Event::Crossing { same_net: false, .. })).count(), 1, "net 1 crosses net 2 once");
    assert_eq!(forced(&ev_b), 0, "crossing a stretch on B.Cu, net 1 stays on its F.Cu pads' layer: no via");
    // The same path with net 2 on F.Cu: a via pair.
    st.layers_now = on_f;
    let ev_f = insert_events(&st, &c1, &path, &tris);
    assert_eq!(forced(&ev_f), 2, "crossing a stretch on F.Cu forces a via pair: {ev_f:?}");
    let ctx = WireCtx { net: 1, tier: 3 };
    let price = |ev: &[Event]| ev.iter().map(|e| reg.price(e, &ctx)).sum::<f64>();
    assert!((price(&ev_f) - price(&ev_b) - 2.0 * via_mm).abs() < 1e-9, "the via pair costs exactly two vias: {} vs {}", price(&ev_f), price(&ev_b));
    // The search sees it too: with net 2 on F.Cu its cheapest path still crosses (there is no way
    // round) and pays the two vias.
    let (p2, t2) = st.search(&c1, &wt(&c1), (c1.w + c1.s) / WS0).expect("net 1 routes");
    assert_eq!(forced(&insert_events(&st, &c1, &p2, &t2)), 2);
    // Layer-blind (no present layers): net 2 is free on both layers, the crossing implies no via.
    st.layers_now.clear();
    assert_eq!(forced(&insert_events(&st, &c1, &path, &tris)), 0);
    // A stretch inside a via interval may be on either layer: no via either.
    st.layers_now = layers_now(&st, &[vec![(0, 0.5, 0), (st.wires[w2].passes.len(), 0.5, 1)]], bottom);
    assert!(st.layers_now[w2].iter().all(|&m| m == 0b11));
    assert_eq!(forced(&insert_events(&st, &c1, &path, &tris)), 0);
}

/// The door board: two wall pads (nets 7, 8) leave a door 1.3 mm wide (4.5 < x < 5.8) that holds
/// two tracks per layer; nets 1 and 2 run through it, both on the bottom layer (they cross
/// nothing). Net 3 ((7.3, 1) to (7.3, 9)) either goes through the door too (short, but three
/// tracks on one layer: the door is over-full, a predicted violation) or round the right wall
/// (longer, no violation).
fn door() -> BoardModel {
    board(
        (0.0, 0.0, 20.0, 10.0),
        &[(7, 2.0, 5.0, true, 5.0), (8, 8.3, 5.0, true, 5.0), (1, 4.85, 1.0, true, 0.4), (1, 4.85, 9.0, true, 0.4), (2, 5.45, 1.0, true, 0.4), (2, 5.45, 9.0, true, 0.4), (3, 7.3, 1.0, true, 0.4), (3, 7.3, 9.0, true, 0.4)],
    )
}

/// A rerouter as the loop's: the price of a wire is its own and social factors without the
/// search's over-room count (`overflow` at 0); the predicted violations of the board are the
/// archive's measure of the state as the loop layers it, at the `violation` weight `viol_mm`.
struct Measured<'a, 'p> {
    conns: &'a [Conn],
    reg: Registry,
    /// Keep the search out of the door (a pass hook), to lay net 3 round the wall.
    avoid_door: bool,
    packer: &'a Packer<'p>,
    smd: &'a dyn Fn(usize) -> Option<usize>,
    viol_mm: f64,
    viol_old: f64,
    pending: Option<(LayerReport, Predicted)>,
    /// The last measure taken (kept after an undo, for the test).
    last: Option<Predicted>,
}

fn measure(st: &State, packer: &Packer, lr: &LayerReport) -> Predicted {
    let dirs = packer.taut_dirs(st);
    let cm = CapModel::new(st, packer, &dirs, &lr.events, 1, 50.0);
    Predicted::of(cm.violations(), lr.unlayerable.len(), 0)
}

fn local_price(st: &mut State, reg: &Registry, w: WireId) -> f64 {
    let path = st.detach(w);
    let wire = st.wires[w].clone();
    let scale = (wire.w + wire.s) / WS0;
    let p: f64 = events(st, &Move::Insert { wire: &wire, path: &path, tris: &wire.tris, scale }).iter().filter(|e| !matches!(e, Event::Overflow { .. })).map(|e| reg.price(e, &WireCtx { net: wire.net, tier: 3 })).sum();
    st.reattach(w, &path);
    p
}

impl<'p> Rerouter<'p> for Measured<'_, 'p> {
    fn price(&mut self, st: &mut State<'p>, w: WireId) -> f64 {
        local_price(st, &self.reg, w)
    }
    fn place(&mut self, st: &mut State<'p>, gone: &[WireId]) -> Option<Vec<WireId>> {
        let mut out = Vec::new();
        for &g in gone {
            let c = conn_of(self.conns, st.wires[g].net);
            out.push(lay(st, &c, &self.reg, self.avoid_door)?);
        }
        Some(out)
    }
    fn violations(&mut self, st: &mut State<'p>, after: bool) -> f64 {
        if !after {
            return self.viol_old;
        }
        let lr = layers::assign_with(st, self.smd, &LayerParams { cap_mm: 50.0, ..LayerParams::default() }, Some(self.packer));
        let p = measure(st, self.packer, &lr);
        self.pending = Some((lr, p));
        self.last = Some(p);
        p.price(self.viol_mm)
    }
    fn undo(&mut self) {
        self.pending = None;
    }
}

/// Whether edge `e` lies in the door (between the walls, 4.5 < x < 5.8, 2.5 < y < 7.5 mm).
fn in_door(st: &State, e: usize) -> bool {
    let [p, q] = st.proj.edges[e];
    let m = (st.proj.pos[p] + st.proj.pos[q]) * 0.5;
    m.x > 4.4 * MM && m.x < 5.9 * MM && m.y < -2.4 * MM && m.y > -7.6 * MM
}

/// Lay connection `c` with the full search; `avoid_door`: every pass through the door costs 10 m.
fn lay(st: &mut State, c: &Conn, reg: &Registry, avoid_door: bool) -> Option<WireId> {
    let wt = InitWeights { max_expansions: 2_000_000, ..InitWeights::from_registry(reg, &WireCtx { net: c.net, tier: 3 }) };
    let hook = |_net: i32, e: usize| if avoid_door && in_door(st, e) { 10_000.0 } else { 0.0 };
    let (path, tris) = st.search_hooked(c, &wt, (c.w + c.s) / WS0, Some(&hook))?;
    Some(st.insert(wire_of(c), &path, tris))
}

/// Is the wire of net 3 through the door (left of the right wall's left side at x = 5.8)?
fn through_door(st: &State, w: WireId) -> bool {
    st.polyline(w).iter().any(|p| p.x < 5.9 * MM && (p.y < -2.6 * MM && p.y > -7.4 * MM))
}

#[test]
fn a_reroute_pays_exactly_the_archives_violation_measure_and_gains_it_back() {
    let m = door();
    let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, ..BuildOpts::default() }).unwrap();
    let conns = b.connections(&m);
    let reg = Registry::standard();
    let viol_mm = reg.price(&Event::Violation { kind: String::new(), at: Default::default(), layer: String::new(), depth_nm: 0.0, len_nm: 0.0, against: String::new(), cause: String::new() }, &WireCtx::default());
    assert_eq!(viol_mm, 1000.0, "the violation factor: 1000 mm per violation");
    let packer = Packer::for_model(&m, &Options::default(), &b.proj);
    let smd = |o: usize| b.smd_layer(o);
    let mut st = State::new(&b.proj);
    st.taut_len = true;
    for net in [1, 2] {
        lay(&mut st, &conn_of(&conns, net), &reg, false).expect("routes");
    }
    // Net 3 round the right wall. (The search alone would take the door: its room summed over
    // both layers seems to hold three tracks, the per-layer measure of the archive does not; the
    // mismatch the price of 5.2 removes.)
    let w3 = lay(&mut st, &conn_of(&conns, 3), &reg, true).expect("routes");
    assert!(!through_door(&st, w3), "net 3 goes round: {:?}", st.polyline(w3));
    let lr0 = layers::assign_with(&st, &smd, &LayerParams { cap_mm: 50.0, ..LayerParams::default() }, Some(&packer));
    let p0 = measure(&st, &packer, &lr0);
    assert_eq!(p0, Predicted { count: 0, graded: 0.0, ..Predicted::default() }, "two tracks on the bottom layer fit the door: no violation");

    // The reroute through the door: 8 mm shorter, but the door is then over-full on one layer.
    let mut r = Measured { conns: &conns, reg: reg.clone(), avoid_door: false, packer: &packer, smd: &smd, viol_mm, viol_old: p0.price(viol_mm), pending: None, last: None };
    let snap = format!("{:?}", st.topology());
    let out = reroute_set(&mut st, &[w3], &mut r, 0.0);
    let p1 = r.last.expect("measured");
    assert!(p1.count >= 1 && p1.graded >= p1.count as f64, "the door over-full: {p1:?}");
    // The move paid exactly the archive's measure x the violation weight.
    assert!((out.viol_new - viol_mm * p1.graded).abs() < 1e-9 && out.viol_old == 0.0, "{out:?} {p1:?}");
    let c1 = criteria_of(p1, viol_mm, 0, 0.0, 0, 0.0);
    assert_eq!(c1.errors, p1.count, "the archive counts the same violations");
    let obj = Objective::LengthVias { mm_per_via: 25.0 };
    assert!((evaluation(&obj, &c1, viol_mm) - out.viol_new).abs() < 1e-9, "the archive's evaluation of them is the move's price");
    assert!(out.new - out.viol_new < out.old - out.viol_old, "net 3 is shorter through the door: {out:?}");
    assert!(!out.kept, "but a violation costs more than the length it saves: {out:?}");
    assert_eq!(format!("{:?}", st.topology()), snap, "refused: the state as it was");

    // With no violation weight the same move pays on length alone: a price, not a gate.
    r.viol_mm = 0.0;
    let out0 = reroute_set(&mut st, &[w3], &mut r, 0.0);
    assert!(out0.kept && out0.viol_new == 0.0, "{out0:?}");
    let w3b = alive_of(&st, 3);
    assert!(through_door(&st, w3b), "net 3 through the door: {:?}", st.polyline(w3b));
    let (_, p_in) = r.pending.take().expect("measured");
    assert_eq!(p_in, p1, "the same over-full door");

    // From the over-full door, the reroute round the wall gains the violation back, exactly.
    let mut r2 = Measured { conns: &conns, reg: reg.clone(), avoid_door: true, packer: &packer, smd: &smd, viol_mm, viol_old: p_in.price(viol_mm), pending: None, last: None };
    let out2 = reroute_set(&mut st, &[w3b], &mut r2, 0.0);
    assert!(out2.kept, "{out2:?}");
    let (_, p2) = r2.pending.take().expect("measured");
    assert_eq!(p2.count, 0);
    assert!((out2.viol_old - out2.viol_new - viol_mm * p1.graded).abs() < 1e-9, "the gain is the same measure x weight: {out2:?}");
    assert!(!through_door(&st, alive_of(&st, 3)));
}

/// The loop's criteria and evaluation of a hand-made measure: two over-full items 0.3 and 0.2
/// over, one crossing no layer separates, one forced connection.
#[test]
fn the_archive_criteria_and_the_move_price_are_one_measure() {
    let p = Predicted::of((2, 2.5), 1, 1);
    // The crossing no layer separates is a predicted short (a short when drawn), one of the four.
    assert_eq!(p, Predicted { count: 4, graded: 4.5, shorts: 1, ..Predicted::default() });
    assert_eq!(p.shorts_ranked(false, 1.0).shorts, 0, "[optimize] pick_shorts off: not ranked");
    assert_eq!(p.with_foreign((2, 3.0, 2)).shorts, 3, "a traversal through foreign copper is a predicted short too");
    let k3 = p.shorts_ranked(true, 3.0);
    assert_eq!((k3.shorts, k3.short_extra), (1, 2.0), "short_weight 3: a predicted short counts 3 in the ranked count");
    assert_eq!(criteria_of(k3, 1000.0, 3, 100.0, 1, 0.0).ranked(), 6.0, "4 errors + (3 - 1) x 1 short");
    assert_eq!(p.shorts_ranked(true, 0.5).short_extra, 0.0, "below 1 counts as 1");
    let rule_area = p.with_foreign((1, 1.5, 0));
    assert_eq!((rule_area.count, rule_area.shorts), (5, 1), "a traversal of a no-tracks rule area: a violation, no short");
    assert_eq!(p.price(1000.0), 4500.0);
    assert_eq!(p.penalty(1000.0), 500.0);
    let c = criteria_of(p, 1000.0, 3, 100.0, 1, 0.0);
    assert_eq!((c.errors, c.shorts, c.violations, c.vias, c.shrinkages), (4, 1, 4, 3, 1));
    let obj = Objective::LengthVias { mm_per_via: 25.0 };
    // length + 25 mm/via + 1000 mm x graded.
    assert!((evaluation(&obj, &c, 1000.0) - (100.0 + 75.0 + 4500.0)).abs() < 1e-9);
    // One more violation 0.3 over adds 1300 mm, whatever else changes.
    let q = Predicted::of((3, 3.8), 1, 1);
    assert!((q.price(1000.0) - p.price(1000.0) - 1300.0).abs() < 1e-9);
}
