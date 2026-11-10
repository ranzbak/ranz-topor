//! Q14 tests (planner shorts): the cut from a pad to the board outline under a rule area along
//! the edge (Q14a), a pad-to-edge gap that holds the neck but not the nominal width, and the
//! planned traversals through foreign copper (Q14b).

mod topo_d6b;

use topo_d6b::{board, conn, ob, opts};
use topor_engine::model::{BoardModel, Keepout, Options, Pt};
use topor_engine::topo::board as tb;
use topor_engine::topo::demand::Item;
use topor_engine::topo::init::InitWeights;
use topor_engine::topo::layers::{self, LayerParams};
use topor_engine::topo::optimise::foreign_copper;
use topor_engine::topo::pack::Packer;
use topor_engine::topo::refine::CapModel;
use topor_engine::topo::state::{State, VertKind};

const MM: f64 = 1e6;

/// A no-tracks rule area on B.Cu (KiCad layer 2) over the rectangle `x0..x1` x `y0..y1` (mm,
/// KiCad frame; the loaded model is in the math frame, y up, as `BoardModel::from_json` flips it).
fn b_cu_rule_area(m: &mut BoardModel, x0: f64, y0: f64, x1: f64, y1: f64) {
    let p = |x: f64, y: f64| Pt { x: (x * MM) as i64, y: -(y * MM) as i64 };
    m.keepouts.push(Keepout {
        name: "closed".into(),
        layers: vec![2],
        poly: vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)],
        no_tracks: true,
        no_vias: true,
        no_pour: false,
        no_pads: false,
        no_footprints: false,
    });
}

/// The cuts from pad `o` to the left board edge (x = 0) on F.Cu: `(edge, room nm, length nm)`.
fn edge_cuts(b: &tb::Board, o: usize) -> Vec<(usize, f64, f64)> {
    let pr = &b.proj;
    (0..pr.edges.len())
        .filter_map(|e| {
            let [p, q] = pr.edges[e];
            let on_pad = |v: usize| matches!(pr.vkind[v], VertKind::Outline(x) if x == o);
            let on_edge = |v: usize| pr.pos[v].x.abs() < 1.0;
            ((on_pad(p) && on_edge(q)) || (on_pad(q) && on_edge(p))).then(|| (e, pr.room[e][0], (pr.pos[q] - pr.pos[p]).len()))
        })
        .collect()
}

/// planned_crossing_window in small: a 1 x 1 mm F.Cu pad of VCC 0.5 mm from the left edge (its
/// copper 0.5 mm from the outline = the copper-to-edge clearance), B.Cu closed by a rule area
/// along the outline. A 0.25 mm SDA track needs 0.2 + 0.25 + 0.5 mm between the pad and the edge
/// (pad clearance, width, edge clearance): no room at all. Before Q14a the rule area's corners on
/// the outline took the place of the board edge and the foot of the pad on the edge was dropped
/// (the rule area's side along the outline "blocked" it), so every cut from the pad to the edge
/// had infinite room on F.Cu and P2 planned SDA through there (DRC: shorting_items with the pad,
/// copper_edge_clearance).
#[test]
fn a_pad_at_the_edge_clearance_leaves_no_room_to_the_edge_under_a_rule_area() {
    let mut m = board(20.0, 14.0, 2, &[("J1", 1, 1.0, 7.0, 1.0, 1.0, 0), ("J2", 1, 19.0, 7.0, 1.0, 1.0, 0)]);
    b_cu_rule_area(&mut m, 0.0, 0.0, 11.8, 14.0);
    let b = tb::build(&m, &opts()).unwrap();
    let o = ob(&m, &b, "J1");
    let cuts = edge_cuts(&b, o);
    assert!(!cuts.is_empty(), "the pad has cuts to the left edge");
    for &(e, room, len) in &cuts {
        assert!(room.is_finite() && room <= len + 1.0, "cut e{e} from the pad to the edge has room {room} on F.Cu (length {len})");
    }
    let (e, room, _) = cuts.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
    assert!((room - 0.5 * MM).abs() < 1e3, "the narrowest pad-to-edge cut is the 0.5 mm gap: {room}");
    let sda = Item { net: 2, w: 0.25 * MM, s: 0.2 * MM };
    assert!(!b.proj.fits_alone(e, 0, sda), "no track fits between the pad and the edge (need {} > room {room})", b.proj.edge_demand(e, &[sda]));
    assert!((b.proj.edge_demand(e, &[sda]) - 0.95 * MM).abs() < 1.0, "need = pad clearance + width + edge clearance");
}

/// The same pad 1.5 mm from the edge: the gap holds the track and the cut keeps its full room.
#[test]
fn a_pad_far_enough_from_the_edge_keeps_the_room_of_the_gap() {
    let mut m = board(20.0, 14.0, 2, &[("J1", 1, 2.0, 7.0, 1.0, 1.0, 0), ("J2", 1, 19.0, 7.0, 1.0, 1.0, 0)]);
    b_cu_rule_area(&mut m, 0.0, 0.0, 11.8, 14.0);
    let b = tb::build(&m, &opts()).unwrap();
    let cuts = edge_cuts(&b, ob(&m, &b, "J1"));
    let (e, room, _) = cuts.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1)).expect("cuts to the edge");
    assert!((room - 1.5 * MM).abs() < 1e3, "room = the 1.5 mm gap: {room}");
    assert!(b.proj.fits_alone(e, 0, Item { net: 2, w: 0.25 * MM, s: 0.2 * MM }), "a 0.25 mm track fits a 1.5 mm gap");
    // Without the rule area the cut is the same (it was right there already).
    let m0 = board(20.0, 14.0, 2, &[("J1", 1, 2.0, 7.0, 1.0, 1.0, 0), ("J2", 1, 19.0, 7.0, 1.0, 1.0, 0)]);
    let b0 = tb::build(&m0, &opts()).unwrap();
    let r0 = edge_cuts(&b0, ob(&m0, &b0, "J1")).iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
    assert!((r0 - room).abs() < 1.0, "same room with and without the rule area: {r0} vs {room}");
}

/// A wall of net 2 (F.Cu; B.Cu closed by a rule area over the whole board, as in
/// planned_crossing_window) across the board leaves a 1.05 mm gap to the board edge at each end:
/// a 0.5 mm wire (0.2 mm clearance) needs 0.2 + 0.5 + 0.5 = 1.2 mm there, at its
/// 0.25 mm neck floor 0.95 mm. The plan necks it through the gap (one shrinkage, M-C, TopoR 6.3
/// manual p. 56: the minimum width "in bottlenecks"), and the capacity model finds nothing
/// over-full; with a 0.5 mm gap not even the neck fits and the gap is over-full (a predicted
/// violation the loop pays for, costly not failed).
#[test]
fn a_pad_to_edge_gap_for_the_neck_but_not_the_width_is_planned_as_a_neck() {
    for (gap, fits) in [(1.05, true), (0.5, false)] {
        let h = 20.0 - 2.0 * gap;
        let pads = vec![("A", 1, 3.0, 10.0, 1.0, 1.0, 0), ("B", 1, 27.0, 10.0, 1.0, 1.0, 0), ("W", 2, 15.0, 10.0, 1.0, h, 0)];
        let mut m = board(30.0, 20.0, 2, &pads);
        b_cu_rule_area(&mut m, 0.0, 0.0, 30.0, 20.0);
        let b = tb::build(&m, &opts()).unwrap();
        let mut st = State::new(&b.proj);
        st.floors.insert(1, 0.25 * MM);
        let mut c = conn(&m, &b, 1, "A", "B");
        c.w = 0.5 * MM;
        let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("placed").0;
        let pr = &b.proj;
        let wall = ob(&m, &b, "W");
        // The narrowest pass between the wall and the edge.
        let gap_pass = st.wires[w].passes.iter().copied().filter(|&p| {
            let [x, y] = pr.edges[st.passes[p].edge];
            let k = |v: usize| pr.vkind[v];
            (matches!(k(x), VertKind::Outline(o) if o == wall) && k(y) == VertKind::Edge) || (matches!(k(y), VertKind::Outline(o) if o == wall) && k(x) == VertKind::Edge)
        })
        .min_by(|&a, &c| pr.room[st.passes[a].edge][0].total_cmp(&pr.room[st.passes[c].edge][0]));
        let p = gap_pass.unwrap_or_else(|| panic!("gap {gap}: the wire passes between the wall and the edge: {:?}", st.polyline(w)));
        let e = st.passes[p].edge;
        assert!((pr.room[e][0] - gap * MM).abs() < 1e3, "gap {gap}: room {} of the wall-to-edge cut", pr.room[e][0]);
        let packer = Packer::for_model(&m, &Options::default(), &b.proj);
        let smd = |o: usize| b.smd_layer(o);
        let lr = layers::assign_with(&st, &smd, &LayerParams::default(), Some(&packer));
        let dirs = packer.taut_dirs(&st);
        let cm = CapModel::new(&st, &packer, &dirs, &lr.events, 1, 1.0);
        let (na, nb, mid) = st.neck_runs(w);
        let shrinkages = usize::from(na > 0) + usize::from(nb > 0) + usize::from(!mid.is_empty());
        if fits {
            assert!((st.pass_w[p] - 0.25 * MM).abs() < 1.0, "gap {gap}: the pass is necked to the floor: {}", st.pass_w[p]);
            assert_eq!(shrinkages, 1, "gap {gap}: one necked stretch ({na} {nb} {mid:?})");
            assert_eq!(cm.violations().0, 0, "gap {gap}: the neck fits, nothing over-full");
        } else {
            assert!(cm.violations().0 >= 1, "gap {gap}: not even the neck fits: over-full ({:?})", cm.violations());
        }
    }
}

/// Q14b: a wire across a foreign F.Cu pad (net 2) counts once as a violation when its layering
/// puts it on F.Cu there, graded by the length inside the pad (a wider pad costs more); on B.Cu,
/// or inside a via interval (the via can go on either side), not at all.
#[test]
fn a_planned_traversal_through_a_foreign_pad_is_a_graded_violation() {
    let mut graded = Vec::new();
    for size in [1.0, 3.0] {
        // The foreign F.Cu pad P (y 8..12) between two walls of net 3 on both layers, 0.1 mm
        // from it and 0.5 mm from the board edge: the only free way is under P on B.Cu.
        let pads = vec![
            ("A", 1, 3.0, 10.0, 1.0, 1.0, 0),
            ("B", 1, 27.0, 10.0, 1.0, 1.0, 0),
            ("P", 2, 15.0, 10.0, size, 4.0, 0),
            ("Q", 2, 25.0, 3.0, 1.0, 1.0, 0),
            ("WN", 3, 15.0, 4.2, size, 7.4, 99),
            ("WS", 3, 15.0, 15.8, size, 7.4, 99),
        ];
        let m = board(30.0, 20.0, 3, &pads);
        let b = tb::build(&m, &opts()).unwrap();
        let mut st = State::new(&b.proj);
        let w = st.route_conn(&conn(&m, &b, 1, "A", "B"), &InitWeights::default(), 1.0).expect("placed").0;
        let pad = ob(&m, &b, "P");
        let n = st.wires[w].tris.len();
        assert!(st.wires[w].tris.iter().any(|&t| b.proj.tri_obs[t].contains(&pad)), "pad {size}: the wire crosses the pad (the B.Cu way under it is free): {:?}", st.polyline(w));
        let on = |bits: u64| {
            let mut l = vec![Vec::new(); st.wires.len()];
            l[w] = vec![bits; n];
            foreign_copper(&st, &l)
        };
        let f = on(1);
        assert_eq!(f.0, 1, "pad {size}: one violation on F.Cu: {f:?}");
        assert!(f.1 > 1.0, "pad {size}: graded above the count: {f:?}");
        assert_eq!(on(2), (0, 0.0), "pad {size}: on B.Cu under the F.Cu pad: no violation");
        assert_eq!(on(3), (0, 0.0), "pad {size}: inside a via interval: no violation");
        graded.push(f.1);
    }
    assert!(graded[1] > graded[0], "a longer run through the pad costs more: {graded:?}");
}

