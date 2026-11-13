//! Shopping list 5.10: pad exit stubs, the planned exit of a connection from a fine-pitch pad as a
//! fixed item of the projection (`board::Extra::Exit`).

mod topo_d6b;

use topo_d6b::{board, conn, ob, opts};
use topor_engine::geom::Vec2;
use topor_engine::topo::board as tb;
use topor_engine::topo::init::InitWeights;
use topor_engine::topo::state::{State, VertKind};

const MM: f64 = 1e6;
const W: f64 = 0.25 * MM;
const S: f64 = 0.2 * MM;

/// The row of `topo_mc`: seven 1.6 x 0.3 mm pads at 0.5 mm pitch (x = 10, y 8.5..11.5, F.Cu, gaps
/// 0.2 mm), nets 1..=5 on R1..R5 and net 6 on the end pads; `extra` more pads.
fn row(extra: &[(&'static str, i32, f64, f64, f64, f64, u32)]) -> topor_engine::model::BoardModel {
    let mut pads: Vec<(&str, i32, f64, f64, f64, f64, u32)> = Vec::new();
    let names = ["R0", "R1", "R2", "R3", "R4", "R5", "R6"];
    for (i, n) in names.iter().enumerate() {
        let net = if (1..=5).contains(&i) { i as i32 } else { 6 };
        pads.push((n, net, 10.0, 8.5 + 0.5 * i as f64, 1.6, 0.3, 0));
    }
    pads.extend_from_slice(extra);
    board(20.0, 16.0, 7, &pads)
}

fn exit(m: &topor_engine::model::BoardModel, r: &str, toward: (f64, f64)) -> tb::Extra {
    let pad = m.pads.iter().position(|p| p.ref_ == r).unwrap();
    // The model's math frame has y up (KiCad's y negated).
    tb::Extra::Exit { pad, toward: Vec2 { x: toward.0 * MM, y: -toward.1 * MM }, w: W, s: S }
}

fn bbox(poly: &[Vec2]) -> (f64, f64, f64, f64) {
    let f = |g: fn(&Vec2) -> f64, mx: bool| poly.iter().map(g).fold(if mx { f64::MIN } else { f64::MAX }, |a, b| if mx { a.max(b) } else { a.min(b) }) / MM;
    (f(|p| p.x, false), f(|p| p.y, false), f(|p| p.x, true), f(|p| p.y, true))
}

/// A pin of the row whose connection goes west gets one stub out of its west tip: as wide as the
/// wire, from the pad's tip (x = 9.2) to `clearance + width` past it (x = 8.75), on the pad's
/// layer and net, outside the pad's polygon.
#[test]
fn a_fine_pitch_pin_gets_a_stub_out_of_the_tip_facing_its_target() {
    let m = row(&[]);
    let b = tb::build_full(&m, &opts(), &[], &[exit(&m, "R3", (3.0, 10.0)), exit(&m, "R3", (2.0, 9.0))]).unwrap();
    assert_eq!(b.exit_stubs.len(), 1, "one stub per pad and tip: {:?}", b.exit_stubs);
    let (o, pad) = b.exit_stubs[0];
    assert_eq!(m.pads[pad].ref_, "R3");
    let st = &b.proj.obstacles[o];
    assert_eq!(st.net, Some(3));
    assert_eq!(st.layers, b.proj.obstacles[ob(&m, &b, "R3")].layers);
    let (x0, y0, x1, y1) = bbox(&st.poly);
    assert!((x0 - 8.75).abs() < 1e-3 && x1 <= 9.2 + 1e-9 && x1 > 9.18, "x {x0}..{x1}");
    assert!((y1 - y0 - 0.25).abs() < 1e-6 && ((y0 + y1) / 2.0 + 10.0).abs() < 1e-6, "y {y0}..{y1}");
}

/// No stub where the wire could leave through a side (no other net within `w + 2 s`), and none
/// that would break a clearance (another net's pad 0.3 mm beyond the tip).
#[test]
fn no_stub_without_fine_pitch_neighbours_or_against_a_clearance() {
    let m = board(20.0, 16.0, 3, &[("P", 1, 10.0, 8.0, 1.6, 0.3, 0), ("Q", 2, 10.0, 10.0, 1.6, 0.3, 0)]);
    let b = tb::build_full(&m, &opts(), &[], &[exit(&m, "P", (3.0, 8.0))]).unwrap();
    assert!(b.exit_stubs.is_empty(), "lone pad: {:?}", b.exit_stubs);
    let m = row(&[("X", 7, 8.6, 10.0, 0.3, 1.0, 0)]);
    let b = tb::build_full(&m, &opts(), &[], &[exit(&m, "R3", (3.0, 10.0))]).unwrap();
    assert!(b.exit_stubs.is_empty(), "a pad of another net 0.45 mm past the tip: {:?}", b.exit_stubs);
    // The east tip is free: a connection that way gets its stub there.
    let b = tb::build_full(&m, &opts(), &[], &[exit(&m, "R3", (18.0, 10.0))]).unwrap();
    assert_eq!(b.exit_stubs.len(), 1);
    assert!(bbox(&b.proj.obstacles[b.exit_stubs[0].0].poly).0 >= 10.8 - 1e-9);
}

/// The planner routes another net round the stub's end: R3's connection leaves west to the far
/// south-west (U), R2's (the pin north of it) to a target south-west of the row (T); the wire of
/// R2 never passes between R3's tip and its stub (no pass across an edge of the projection with
/// both ends on R3's pad or stub).
#[test]
fn another_net_is_planned_round_the_stub() {
    let m = row(&[("T", 2, 3.0, 13.0, 1.0, 1.0, 0), ("U", 3, 3.0, 15.0, 1.0, 1.0, 0)]);
    let b = tb::build_full(&m, &opts(), &[], &[exit(&m, "R3", (3.0, 15.0)), exit(&m, "R2", (3.0, 13.0))]).unwrap();
    assert_eq!(b.exit_stubs.len(), 2);
    let r3 = ob(&m, &b, "R3");
    let stub3 = b.exit_stubs.iter().find(|x| m.pads[x.1].ref_ == "R3").unwrap().0;
    let mut st = State::new(&b.proj);
    let c = conn(&m, &b, 2, "R2", "T");
    let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed").0;
    let pr = &b.proj;
    let of3 = |v: usize| matches!(pr.vkind[v], VertKind::Outline(o) | VertKind::Centre(o) if o == r3 || o == stub3);
    for &p in &st.wires[w].passes {
        let [a, z] = pr.edges[st.passes[p].edge];
        assert!(!(of3(a) && of3(z)), "the wire of R2 crosses R3's exit: {:?}", st.polyline(w).iter().map(|q| (q.x / MM, q.y / MM)).collect::<Vec<_>>());
    }
}

/// The end pin of a row whose target lies on its free side needs no stub (its wire may leave
/// through that side); towards the row it gets one.
#[test]
fn an_end_pin_towards_its_free_side_gets_no_stub() {
    let m = row(&[]);
    let b = tb::build_full(&m, &opts(), &[], &[exit(&m, "R0", (3.0, 5.0))]).unwrap();
    assert!(b.exit_stubs.is_empty(), "R0 towards the free north side: {:?}", b.exit_stubs);
    let b = tb::build_full(&m, &opts(), &[], &[exit(&m, "R0", (3.0, 12.0))]).unwrap();
    assert_eq!(b.exit_stubs.len(), 1, "R0 towards the row");
}

/// End to end through the core (`route::route` -> `Fixed::build` -> `exit_requests`): off (the
/// default) the build gets no `Extra::Exit`, so no stub note; on, the row's pins towards their
/// western targets get planned exit stubs. On a board without fine-pitch neighbours the option
/// changes nothing: the routed copper is the same item for item.
#[test]
fn the_option_adds_stubs_through_the_core_only_when_on() {
    use topor_engine::model::Options;
    let off = Options::from_json(r#"{"wall_clock_s": 60.0}"#).unwrap();
    assert!(!off.pad_exit_stubs, "default off");
    let on = Options { pad_exit_stubs: true, ..off.clone() };
    let stub_note = |r: &topor_engine::route::RouteResult| r.warnings.iter().find(|w| w.contains("planned exit stubs")).cloned();
    // The row, nets 1..=5 to test points on the west.
    let m = row(&[("T1", 1, 3.0, 3.0, 0.6, 0.6, 0), ("T2", 2, 3.0, 6.0, 0.6, 0.6, 0), ("T3", 3, 3.0, 9.0, 0.6, 0.6, 0), ("T4", 4, 3.0, 12.0, 0.6, 0.6, 0), ("T5", 5, 3.0, 15.0, 0.6, 0.6, 0)]);
    let r_off = topor_engine::route::route(&m, &off);
    assert_eq!(stub_note(&r_off), None, "off: no exit stub was built");
    let r_on = topor_engine::route::route(&m, &on);
    let note = stub_note(&r_on).expect("on: the row's pins get exit stubs");
    let n: usize = note.split("pad_exit_stubs): ").nth(1).and_then(|t| t.split(' ').next()).and_then(|t| t.parse().ok()).unwrap_or(0);
    assert!(n >= 1, "{note}");
    // Lone pads (no other net within reach): on and off route the same copper.
    let lone = tb_lone();
    let (a, b) = (topor_engine::route::route(&lone, &off), topor_engine::route::route(&lone, &on));
    assert_eq!(stub_note(&b), None);
    assert_eq!(a.items, b.items, "no fine-pitch pad: the option is inert");
}

fn tb_lone() -> topor_engine::model::BoardModel {
    topo_d6b::board(20.0, 16.0, 2, &[("A", 1, 10.0, 4.0, 1.6, 0.3, 0), ("B", 1, 3.0, 4.0, 0.6, 0.6, 0), ("C", 2, 10.0, 12.0, 1.6, 0.3, 0), ("D", 2, 3.0, 12.0, 0.6, 0.6, 0)])
}
