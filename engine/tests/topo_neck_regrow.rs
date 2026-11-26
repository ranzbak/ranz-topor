//! neck-regrow (docs/dev/design/neck-regrow.md): a neck only where room is missing, decided in the
//! plan, and never narrow-wide-narrow (user 2026-11-07, 2026-11-26).

mod topo_d6b;

use topo_d6b::{board, conn, ob, opts};
use topor_engine::model::PadShape;
use topor_engine::topo::board as tb;
use topor_engine::topo::init::InitWeights;
use topor_engine::topo::state::State;

const MM: f64 = 1e6;

/// A column of seven 1.6 x 0.3 mm rounded pads at 0.5 mm pitch (x = 10, y 8.5..11.5, gaps 0.2 mm),
/// as qmtech's ADV7511 pins 15-21; R3 (net 3) is the wire's pad, its target T west of the column
/// and a little south (the plan leaves R3 round its lower-left corner).
fn column(rc: f64) -> topor_engine::model::BoardModel {
    let mut pads: Vec<(&str, i32, f64, f64, f64, f64, u32)> = Vec::new();
    let names = ["R0", "R1", "R2", "R3", "R4", "R5", "R6"];
    for (i, n) in names.iter().enumerate() {
        let net = if (1..=5).contains(&i) { i as i32 } else { 6 };
        pads.push((n, net, 10.0, 8.5 + 0.5 * i as f64, 1.6, 0.3, 0));
    }
    pads.push(("T", 3, 4.0, 12.0, 1.0, 1.0, 0));
    let mut m = board(20.0, 16.0, 6, &pads);
    if rc > 0.0 {
        for p in m.pads.iter_mut().filter(|p| p.ref_.starts_with('R')) {
            for s in p.shapes.values_mut() {
                *s = PadShape::RoundRect { w: (1.6 * MM) as i64, h: (0.3 * MM) as i64, rc: (rc * MM) as i64 };
            }
        }
    }
    m
}

/// (1) qmtech ADV_DVDD (pad ADV7511.19): the plan necked the first passes because `exit_tight`
/// judged the even pass points of edges inside the pad's own rounded-corner fan, 0.24 mm from the
/// next pad. Along the pad's axis the exit has 0.5 - 0.15 = 0.35 mm to each neighbour, more than
/// the 0.125 + 0.2 a 0.25 mm wire needs: no neck.
#[test]
fn a_row_pad_with_room_on_its_axis_is_no_tight_exit() {
    for rc in [0.075, 0.0] {
        let m = column(rc);
        let b = tb::build(&m, &opts()).unwrap();
        let mut st = State::new(&b.proj);
        st.floors.insert(3, 0.1 * MM);
        let c = conn(&m, &b, 3, "R3", "T");
        let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed").0;
        let at_pad = if st.wires[w].pa == ob(&m, &b, "R3") { 0 } else { 1 };
        assert!(!st.exit_tight(w, at_pad), "rc {rc}: the axis exit has room");
        st.refit_wire(w);
        let ws: Vec<f64> = st.wires[w].passes.iter().map(|&p| st.pass_w[p] / MM).collect();
        assert!(ws.iter().all(|&x| (x - 0.25).abs() < 1e-9), "rc {rc}: no neck planned, widths {ws:?}");
    }
}

/// (1) The same column with a 0.15 mm gap between pads (pitch 0.45): along the axis a 0.25 mm wire
/// with 0.2 mm clearance needs 0.325 mm to each neighbour and has 0.3: the exit is tight (the
/// guard against a fix that never necks). With rounded corners too: the passes of the pad's own
/// corner fan are skipped, the first passes outside it are judged.
#[test]
fn a_row_pad_without_room_on_its_axis_is_a_tight_exit() {
    for rc in [0.0, 0.075] {
        let mut pads: Vec<(&str, i32, f64, f64, f64, f64, u32)> = Vec::new();
        let names = ["R0", "R1", "R2", "R3", "R4", "R5", "R6"];
        for (i, n) in names.iter().enumerate() {
            let net = if (1..=5).contains(&i) { i as i32 } else { 6 };
            pads.push((n, net, 10.0, 8.5 + 0.45 * i as f64, 1.6, 0.3, 0));
        }
        pads.push(("T", 3, 4.0, 12.0, 1.0, 1.0, 0));
        let mut m = board(20.0, 16.0, 6, &pads);
        if rc > 0.0 {
            for p in m.pads.iter_mut().filter(|p| p.ref_.starts_with('R')) {
                for s in p.shapes.values_mut() {
                    *s = PadShape::RoundRect { w: (1.6 * MM) as i64, h: (0.3 * MM) as i64, rc: (rc * MM) as i64 };
                }
            }
        }
        let b = tb::build(&m, &opts()).unwrap();
        let mut st = State::new(&b.proj);
        st.floors.insert(3, 0.1 * MM);
        let c = conn(&m, &b, 3, "R3", "T");
        let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed").0;
        let at_pad = if st.wires[w].pa == ob(&m, &b, "R3") { 0 } else { 1 };
        assert!(st.exit_tight(w, at_pad), "rc {rc}: 0.3 mm to the neighbours on the axis, 0.325 needed");
    }
}

/// (1) neck_reentry's P2 (a 0.4 mm square pad 0.452 mm above the upper wall, the wire runs right
/// along the wall to its end): the even pass points come 0.33 mm near the wall and the funnel's
/// leg cuts the wall's corner at 0.40 mm, but a 0.5 mm wire with 0.2 mm clearance has room (the
/// drawing wraps the corner at 0.45): no tight exit at P2. P1 (in its 0.3 mm channel) is tight.
#[test]
fn neck_reentry_p2_exit_has_room_round_the_wall_corner() {
    let path = format!("{}/tests/fixtures/neckrg/model-neck_reentry.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let b = tb::build(&m, &opts()).unwrap();
    let mut st = State::new(&b.proj);
    st.floors.insert(1, 0.2 * MM);
    let mut c = conn(&m, &b, 1, "P1", "P2");
    c.w = 0.5 * MM;
    let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed").0;
    let at_p1 = if st.wires[w].pa == ob(&m, &b, "P1") { 0 } else { 1 };
    assert!(st.exit_tight(w, at_p1), "P1's channel fits 0.3 mm only");
    assert!(!st.exit_tight(w, 1 - at_p1), "P2 has room round the wall's corner");
}

/// (2) R-a: a 0.4 mm wire from A to B whose both ends leave through 0.7 mm gaps between walls
/// (0.8 mm needed at the nominal width, 0.6 at the 0.2 mm floor), with ample room in between: the
/// plan necks both ends and keeps the middle's full-width room (user 2026-11-26, option a: the
/// neighbours are laid out as without the join); the drawing necks the whole wire (end to end:
/// `neck_both_ends_is_one_neck_over_the_whole_wire`).
#[test]
fn necks_at_both_ends_keep_the_full_width_room_in_the_middle_of_the_plan() {
    let pads = vec![
        ("A", 1, 3.0, 10.0, 0.4, 0.4, 0),
        ("B", 1, 27.0, 10.0, 0.4, 0.4, 0),
        // channels round A (x 2..6) and B (x 24..28): walls on both layers, net 2
        ("WA1", 2, 4.0, 9.175, 4.0, 0.95, 99),
        ("WA2", 2, 4.0, 10.825, 4.0, 0.95, 99),
        ("WB1", 2, 26.0, 9.175, 4.0, 0.95, 99),
        ("WB2", 2, 26.0, 10.825, 4.0, 0.95, 99),
        // small pads 3 mm off the line in the middle: several triangulation edges with room
        ("M1", 2, 9.0, 7.0, 0.3, 0.3, 99),
        ("M2", 2, 12.0, 13.0, 0.3, 0.3, 99),
        ("M3", 2, 15.0, 7.0, 0.3, 0.3, 99),
        ("M4", 2, 18.0, 13.0, 0.3, 0.3, 99),
        ("M5", 2, 21.0, 7.0, 0.3, 0.3, 99),
    ];
    let m = board(30.0, 20.0, 2, &pads);
    let b = tb::build(&m, &opts()).unwrap();
    let mut st = State::new(&b.proj);
    st.floors.insert(1, 0.2 * MM);
    let mut c = conn(&m, &b, 1, "A", "B");
    c.w = 0.4 * MM;
    let w = st.route_conn(&c, &InitWeights::default(), 1.0).expect("routed").0;
    st.refit_wire(w);
    let ws: Vec<f64> = st.wires[w].passes.iter().map(|&p| st.pass_w[p] / MM).collect();
    let n = ws.len();
    let (na, nb, mid) = st.neck_runs(w);
    assert!(n > 6, "the middle is crossed by several edges ({n} passes)");
    assert!(na > 0 && nb > 0 && na + nb < n && mid.is_empty(), "a neck at each end, widths {ws:?}");
    assert!(ws[na..n - nb].iter().all(|&x| (x - 0.4).abs() < 1e-9), "full-width room in the middle, widths {ws:?}");
}

// ------------------------------------------------------------------------------ end to end
// The patterns of harness/neckrg_patterns.py (APPROVED, user 2026-11-26) (models: harness.radii_patterns.model_fixture,
// engine/tests/fixtures/neckrg); PWR is 0.5 mm with a 0.2 mm floor.

use topor_engine::control::RunControl;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options};

/// A track piece (mm): `(width, a, b)`.
type Piece = (f64, (f64, f64), (f64, f64));

/// PWR's track pieces in path order from `start` (mm): `(width, a, b)`.
fn pwr_chain(p: &str, start: (f64, f64)) -> (Vec<Piece>, usize) {
    let path = format!("{}/tests/fixtures/neckrg/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    let o: Options = serde_json::from_value(val).unwrap();
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    let mut rest: Vec<Piece> = r
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Track(t) => match t.prim {
                TrackPrim::Seg { a, b } => Some((t.width as f64 / MM, (a.x as f64 / MM, a.y as f64 / MM), (b.x as f64 / MM, b.y as f64 / MM))),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let d = |p: (f64, f64), q: (f64, f64)| ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt();
    let mut out = Vec::new();
    let mut cur = start;
    while !rest.is_empty() {
        let k = (0..rest.len()).min_by(|&i, &j| d(cur, rest[i].1).min(d(cur, rest[i].2)).total_cmp(&d(cur, rest[j].1).min(d(cur, rest[j].2)))).unwrap();
        let (w, mut a, mut b) = rest.remove(k);
        if d(cur, b) < d(cur, a) {
            std::mem::swap(&mut a, &mut b);
        }
        out.push((w, a, b));
        cur = b;
    }
    (out, r.connections_total - r.routed)
}

/// The invariant: along the path, one necked stretch at most, and it touches an end.
fn necked_stretches(ch: &[Piece]) -> (usize, bool) {
    let thin: Vec<bool> = ch.iter().map(|x| x.0 < 0.5 - 1e-4).collect();
    let n = (0..thin.len()).filter(|&i| thin[i] && (i == 0 || !thin[i - 1])).count();
    let touches = thin.first().copied().unwrap_or(false) || thin.last().copied().unwrap_or(false);
    (n, n == 0 || touches)
}

/// (2) R-a on neck_both_ends: both channel ends need the neck; the necks join, the whole wire is
/// one 0.3 mm stretch (main: 0.304 | 0.5 | 0.304).
#[test]
fn neck_both_ends_is_one_neck_over_the_whole_wire() {
    let (ch, unrouted) = pwr_chain("neck_both_ends", (1.5, 3.0));
    assert_eq!(unrouted, 0);
    let (n, touches) = necked_stretches(&ch);
    assert!(n == 1 && touches && ch.iter().all(|x| x.0 < 0.5 - 1e-4), "{ch:?}");
}

/// (3) R-b on neck_reentry: PWR necks out of P1's channel, then comes back round the wall into
/// P2 just above P1: that leg stays full width (main narrowed it again inside P1's circle).
#[test]
fn neck_reentry_has_one_neck_at_p1() {
    let (ch, unrouted) = pwr_chain("neck_reentry", (1.6, 4.5));
    assert_eq!(unrouted, 0);
    let (n, touches) = necked_stretches(&ch);
    assert!(n == 1 && touches, "one neck from P1: {ch:?}");
    assert!(ch.last().unwrap().0 > 0.5 - 1e-4, "full width into P2: {ch:?}");
}
