use topor_engine::geom::{IVec2, TrackPrim, Vec2};
use topor_engine::model::{
    BoardModel, ExistingCopper, KeptItem, Net, Options, Pad, PadShape, Pt, Via,
};
use topor_engine::wires::{extract_wires, Terminal, Wire};

const MM: i64 = 1_000_000;

/// Pad centres in mm: P1 (0,0), P2 (10,0), P3 (20,0), P4 (10,5); all net 5, circle d 1.6 mm.
const PADS: [(i64, i64); 4] = [(0, 0), (10, 0), (20, 0), (10, 5)];

fn pad(i: usize, net: i32, x: i64, y: i64) -> Pad {
    Pad {
        uuid: format!("p{i}"),
        ref_: "U1".into(),
        number: format!("{i}"),
        net,
        attr: Default::default(),
        center: Pt { x, y },
        rotation_deg: 0.0,
        layers: vec![0],
        shapes: [(0u32, PadShape::Circle { d: 1_600_000 })].into(),
        drill: None,
        local_clearance: None,
        net_tie_group: None,
        zone_connection: None,
        thermal_gap: None,
        spoke_angle_deg: None,
    }
}

fn iv(x: f64, y: f64) -> IVec2 {
    IVec2 {
        x: (x * MM as f64).round() as i32,
        y: (y * MM as f64).round() as i32,
    }
}

fn seg(u: &str, net: i32, a: (f64, f64), b: (f64, f64)) -> KeptItem {
    KeptItem {
        uuid: u.into(),
        net,
        layer: 0,
        width: 250_000,
        prim: TrackPrim::Seg {
            a: iv(a.0, a.1),
            b: iv(b.0, b.1),
        },
        locked: false,
    }
}

/// `pads` are 1-based indices into `PADS`.
fn model(pads: &[usize], kept: Vec<KeptItem>) -> BoardModel {
    let mut m = BoardModel {
        nets: vec![
            Net {
                code: 5,
                name: "A".into(),
            },
            Net {
                code: 6,
                name: "B".into(),
            },
        ],
        ..BoardModel::default()
    };
    for &i in pads {
        let (x, y) = PADS[i - 1];
        m.pads.push(pad(i, 5, x * MM, y * MM));
    }
    m.kept = kept;
    m
}

fn smooth() -> Options {
    Options {
        existing_copper: ExistingCopper::Smooth,
        ..Options::default()
    }
}

fn pad_of(t: &Terminal) -> Option<usize> {
    match t {
        Terminal::Pad { pad, .. } => Some(*pad),
        _ => None,
    }
}

fn mm(p: Vec2) -> (f64, f64) {
    (p.x / MM as f64, p.y / MM as f64)
}

fn close(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
}

fn junctions(ws: &[Wire]) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = Vec::new();
    for t in ws.iter().flat_map(|w| [&w.start, &w.goal]) {
        if let Terminal::Junction { at } = t {
            if !v.iter().any(|j| close(*j, mm(*at))) {
                v.push(mm(*at));
            }
        }
    }
    v
}

#[test]
fn two_segment_l_into_pad_is_one_wire() {
    let m = model(
        &[1, 2, 3],
        vec![
            seg("a", 5, (0.0, 0.0), (9.5, 0.3)),
            seg("b", 5, (9.5, 0.3), (9.7, 0.1)),
        ],
    );
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 1, "{:?}", e.skipped);
    let w = &e.wires[0];
    assert_eq!(pad_of(&w.start), Some(0));
    assert_eq!(pad_of(&w.goal), Some(1));
    let h: Vec<_> = w.hand.iter().map(|p| mm(*p)).collect();
    let exp = [(0.0, 0.0), (9.5, 0.3), (9.7, 0.1), (10.0, 0.0)];
    assert_eq!(h.len(), 4, "{h:?}");
    for (a, b) in h.iter().zip(exp) {
        assert!(close(*a, b), "{h:?}");
    }
    assert!((w.hand_len - 10_103_700.0).abs() < 300.0, "{}", w.hand_len);
    assert_eq!(w.kept_uuids, vec!["a", "b"]);
    assert_eq!((w.net, w.layer, w.width), (5, 0, 250_000));
}

#[test]
fn pass_through_pad_splits() {
    let m = model(
        &[1, 2, 3],
        vec![
            seg("a", 5, (0.0, 0.0), (10.0, 0.0)),
            seg("b", 5, (10.0, 0.0), (20.0, 0.0)),
        ],
    );
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 2, "{:?}", e.skipped);
    let on_p2 = e
        .wires
        .iter()
        .filter(|w| pad_of(&w.start) == Some(1) || pad_of(&w.goal) == Some(1))
        .count();
    assert_eq!(on_p2, 2);
    assert!(e.wires.iter().all(|w| w.start.at() != w.goal.at()));
}

#[test]
fn wires_t_junction_on_segment_interior() {
    let m = model(
        &[1, 3, 4],
        vec![
            seg("a", 5, (0.0, 0.0), (20.0, 0.0)),
            seg("b", 5, (10.0, 0.0), (10.0, 5.0)),
        ],
    );
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 3, "{:?}", e.skipped);
    let js: Vec<&Terminal> = e
        .wires
        .iter()
        .flat_map(|w| [&w.start, &w.goal])
        .filter(|t| matches!(t, Terminal::Junction { .. }))
        .collect();
    assert_eq!(
        js.len(),
        3,
        "each of the three wires touches the junction once"
    );
    assert!(js.iter().all(|t| close(mm(t.at()), (10.0, 0.0))));
    assert_eq!(junctions(&e.wires).len(), 1);
    assert!(js.iter().all(|t| t.is_fixed_point()));
    // both halves of the split through track carry its uuid
    let halves = e
        .wires
        .iter()
        .filter(|w| w.kept_uuids == vec!["a".to_string()])
        .count();
    assert_eq!(halves, 2);
}

#[test]
fn t_junction_on_diagonal_interior_within_rounding() {
    // (0,0)→(7,3) mm; the branch starts on the rounded interior point (3.5,1.5).
    let m = model(
        &[1],
        vec![
            seg("a", 5, (0.0, 0.0), (7.0, 3.0)),
            seg("b", 5, (3.5, 1.5), (3.5, 6.0)),
        ],
    );
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 3, "{:?}", e.skipped);
    assert_eq!(junctions(&e.wires).len(), 1);
}

#[test]
fn junction_vertex_degree_three() {
    let m = model(
        &[1, 2, 3],
        vec![
            seg("a", 5, (0.0, 0.0), (10.0, 2.0)),
            seg("b", 5, (10.0, 2.0), (10.0, 0.0)),
            seg("c", 5, (10.0, 2.0), (20.0, 0.0)),
        ],
    );
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 3, "{:?}", e.skipped);
    let j = junctions(&e.wires);
    assert_eq!(j.len(), 1);
    assert!(close(j[0], (10.0, 2.0)));
}

#[test]
fn dangling_end_is_terminal() {
    let m = model(&[1], vec![seg("a", 5, (0.0, 0.0), (5.0, 5.0))]);
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 1);
    let w = &e.wires[0];
    assert!(matches!(w.goal, Terminal::Dangling { .. }));
    assert!(w.goal.is_fixed_point());
    assert!(close(mm(*w.hand.last().unwrap()), (5.0, 5.0)));
    assert!(close(mm(w.goal.at()), (5.0, 5.0)));
}

#[test]
fn via_is_terminal() {
    let mut m = model(&[1], vec![seg("a", 5, (0.0, 0.0), (5.0, 5.0))]);
    m.vias.push(Via {
        uuid: "v".into(),
        net: 5,
        at: Pt {
            x: 5 * MM,
            y: 5 * MM,
        },
        d: 600_000,
        drill: 300_000,
        layers: vec![0, 1],
        free: false,
        locked: false,
    });
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 1);
    assert!(matches!(e.wires[0].goal, Terminal::Via { via: 0, .. }));
    assert!(!e.wires[0].goal.is_fixed_point());
}

#[test]
fn locked_net0_and_fixed_are_skipped() {
    let mut l = seg("l", 5, (0.0, 0.0), (5.0, 5.0));
    l.locked = true;
    let n0 = seg("n", 0, (0.0, 1.0), (5.0, 6.0));
    let ok = seg("o", 5, (0.0, 2.0), (5.0, 7.0));
    let m = model(&[], vec![l, n0, ok]);
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 1);
    assert!(e.skipped.contains(&("l".into(), "locked".into())));
    assert!(e.skipped.contains(&("n".into(), "net 0".into())));
    let f = extract_wires(&m, &Options::default());
    assert!(f.wires.is_empty());
    assert_eq!(f.skipped.len(), 3);
    assert!(f.skipped.iter().all(|s| s.1 == "existing_copper fixed"));
}

#[test]
fn scope_nets_filters_by_name() {
    let m = model(
        &[],
        vec![
            seg("a", 5, (0.0, 0.0), (5.0, 5.0)),
            seg("b", 6, (0.0, 1.0), (5.0, 6.0)),
        ],
    );
    let mut o = smooth();
    o.scope.nets = vec!["A".into()];
    let e = extract_wires(&m, &o);
    assert_eq!(e.wires.len(), 1);
    assert_eq!(e.wires[0].net, 5);
}

#[test]
fn loop_back_to_same_pad_is_skipped() {
    let m = model(
        &[1],
        vec![
            seg("a", 5, (0.0, 0.0), (3.0, 3.0)),
            seg("b", 5, (3.0, 3.0), (0.2, 0.1)),
        ],
    );
    let e = extract_wires(&m, &smooth());
    assert!(e.wires.is_empty());
    assert!(e.skipped.iter().any(|s| s.1 == "loop"), "{:?}", e.skipped);
}

#[test]
fn hand_arc_is_sampled() {
    let arc = KeptItem {
        uuid: "arc".into(),
        net: 5,
        layer: 0,
        width: 250_000,
        prim: TrackPrim::Arc {
            start: iv(5.0, 0.0),
            mid: IVec2 {
                x: 3_535_534,
                y: 3_535_534,
            },
            end: iv(0.0, 5.0),
        },
        locked: false,
    };
    let m = model(&[], vec![arc]);
    let e = extract_wires(&m, &smooth());
    assert_eq!(e.wires.len(), 1);
    let w = &e.wires[0];
    assert!(w.hand.len() >= 10, "{}", w.hand.len());
    let want = std::f64::consts::FRAC_PI_2 * 5.0 * MM as f64;
    assert!((w.hand_len - want).abs() / want < 1e-3, "{}", w.hand_len);
    for p in &w.hand {
        let r = p.len() / MM as f64;
        assert!((r - 5.0).abs() < 1e-3, "{r}");
    }
}

#[test]
fn ids_are_deterministic() {
    let m = model(
        &[1, 2, 3],
        vec![
            seg("a", 5, (0.0, 0.0), (10.0, 0.0)),
            seg("b", 5, (10.0, 0.0), (20.0, 0.0)),
            seg("c", 6, (0.0, 3.0), (4.0, 7.0)),
        ],
    );
    let a = extract_wires(&m, &smooth());
    let b = extract_wires(&m, &smooth());
    assert_eq!(a.wires.len(), 3);
    for (i, (x, y)) in a.wires.iter().zip(&b.wires).enumerate() {
        assert_eq!(x.id, i);
        assert_eq!(x.id, y.id);
        assert_eq!(x.kept_uuids, y.kept_uuids);
        assert_eq!(x.hand, y.hand);
    }
    let keys: Vec<_> = a.wires.iter().map(|w| (w.net, w.layer)).collect();
    let mut s = keys.clone();
    s.sort();
    assert_eq!(keys, s);
}
