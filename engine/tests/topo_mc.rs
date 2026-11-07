//! M-C tests (planner fidelity): pad exits between fine-pitch neighbours and width in P2.

mod topo_d6b;

use topo_d6b::{board, conn, ob, opts};
use topor_engine::topo::board as tb;
use topor_engine::topo::init::InitWeights;
use topor_engine::topo::state::{State, VertKind};

const MM: f64 = 1e6;

/// A row of seven 1.6 x 0.3 mm pads at 0.5 mm pitch (x = 10, y 8.5..11.5, F.Cu; the gaps between
/// neighbours are 0.2 mm), nets 1..=5 on R1..R5 and net 6 on the end pads; the target T of net 3
/// straight north of the row, beyond R0.
fn row() -> topor_engine::model::BoardModel {
    let mut pads: Vec<(&str, i32, f64, f64, f64, f64, u32)> = Vec::new();
    let names = ["R0", "R1", "R2", "R3", "R4", "R5", "R6"];
    for (i, n) in names.iter().enumerate() {
        let net = if (1..=5).contains(&i) { i as i32 } else { 6 };
        pads.push((n, net, 10.0, 8.5 + 0.5 * i as f64, 1.6, 0.3, 0));
    }
    pads.push(("T", 3, 10.0, 3.0, 1.0, 1.0, 0));
    board(20.0, 16.0, 6, &pads)
}

/// Pad exits (M-C item 2): the wire of R3 to T must not leave R3 through the 0.2 mm gap to a
/// row neighbour, where a 0.25 mm track with 0.2 mm clearance needs 0.125 + 0.2 mm (the
/// drawing's packer: margin 0 at the own pad, clearance + w/2 at the other end). Before M-C an
/// edge with one end on the wire's own pad was no cut for it (room infinite), so P2 planned the
/// exit through the side gap towards T and the drawing clipped the neighbour.
#[test]
fn a_wire_leaves_a_fine_pitch_pad_through_its_tip() {
    let m = row();
    let b = tb::build(&m, &opts()).unwrap();
    let mut st = State::new(&b.proj);
    let c = conn(&m, &b, 3, "R3", "T");
    let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed").0;
    let pr = &b.proj;
    let row: Vec<usize> = ["R0", "R1", "R2", "R3", "R4", "R5", "R6"].iter().map(|r| ob(&m, &b, r)).collect();
    let of = |v: usize| match pr.vkind[v] {
        VertKind::Outline(o) if row.contains(&o) => Some(o),
        _ => None,
    };
    for &p in &st.wires[w].passes {
        let e = st.passes[p].edge;
        let [a, z] = pr.edges[e];
        let len = (pr.pos[z] - pr.pos[a]).len();
        // A cut between two row pads too short for the track beside its own pad.
        let pt = |v: usize| (pr.pos[v].x / MM, pr.pos[v].y / MM, pr.vkind[v]);
        assert!(!(of(a).is_some() && of(z).is_some() && of(a) != of(z) && len < 0.125 * MM + 0.2 * MM - 1.0), "the wire crosses a {:.3} mm gap between row pads: {:?} {:?}; path {:?}", len / MM, pt(a), pt(z), st.polyline(w).iter().map(|q| (q.x / MM, q.y / MM)).collect::<Vec<_>>());
    }
}

/// Width in P2 (M-C item 1): a 0.4 mm wire whose only way is a 0.7 mm gap between walls on both
/// layers (0.8 mm needed at the nominal width with 0.2 mm clearance, 0.6 mm at the 0.2 mm floor):
/// the plan necks the pass through the gap and, from the nearest pad end, the stretch up to it.
#[test]
fn the_plan_necks_a_wire_through_a_bottleneck() {
    let pads = vec![
        ("A", 1, 3.0, 10.0, 1.0, 1.0, 0),
        ("B", 1, 27.0, 10.0, 1.0, 1.0, 0),
        ("WN", 2, 15.0, 4.825, 1.0, 9.65, 99),
        ("WS", 2, 15.0, 15.175, 1.0, 9.65, 99),
    ];
    let m = board(30.0, 20.0, 2, &pads);
    let b = tb::build(&m, &opts()).unwrap();
    let mut st = State::new(&b.proj);
    st.floors.insert(1, 0.2 * MM);
    let mut c = conn(&m, &b, 1, "A", "B");
    c.w = 0.4 * MM;
    let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed").0;
    let ws: Vec<f64> = st.wires[w].passes.iter().map(|&p| st.pass_w[p] / MM).collect();
    let (na, nb, mid) = st.neck_runs(w);
    assert!(mid.is_empty() && (na > 0) != (nb > 0), "one necked stretch from a pad end: {na} {nb} {mid:?}, widths {ws:?}");
    assert!(ws.iter().any(|&x| (x - 0.2).abs() < 1e-9) && ws.iter().any(|&x| (x - 0.4).abs() < 1e-9), "{ws:?}");
}
