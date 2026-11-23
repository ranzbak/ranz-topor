use topor_engine::decompose::{decompose, order, terminal_of, Connection, TermKind, TermRef};
use topor_engine::geom::{IVec2, TrackPrim, Vec2};
use topor_engine::model::{
    BoardModel, PourFill, DanglingEnd, KeptItem, Net, NetRule, Options, Pad, PadRef, PadShape, Pour, Pt,
    Scope,
};
use topor_engine::wires::Terminal;

const MM: i64 = 1_000_000;

fn pad(num: usize, net: i32, x: i64, y: i64) -> Pad {
    Pad {
        uuid: format!("p{num}"),
        ref_: "U1".into(),
        number: format!("{num}"),
        net,
        attr: Default::default(),
        center: Pt {
            x: x * MM,
            y: y * MM,
        },
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

/// Model in the math frame (built directly, no JSON flip). Pads of net 1 at the given mm
/// positions are U1.1, U1.2, ...
fn model(pts: &[(i64, i64)]) -> BoardModel {
    let mut m = BoardModel {
        nets: vec![
            Net {
                code: 1,
                name: "A".into(),
            },
            Net {
                code: 2,
                name: "B".into(),
            },
        ],
        ..BoardModel::default()
    };
    for (i, &(x, y)) in pts.iter().enumerate() {
        m.pads.push(pad(i + 1, 1, x, y));
    }
    m
}

fn labels(d: &topor_engine::decompose::Decomposition) -> Vec<(String, String)> {
    d.connections
        .iter()
        .map(|c| (c.from.label.clone(), c.to.label.clone()))
        .collect()
}

fn scoped(s: Scope) -> Options {
    Options {
        scope: s,
        ..Options::default()
    }
}

#[test]
fn two_pad_net_is_one_connection() {
    let m = model(&[(0, 0), (10, 0)]);
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.connections.len(), 1);
    let c = &d.connections[0];
    assert_eq!(
        (c.from.label.as_str(), c.to.label.as_str()),
        ("U1.1", "U1.2")
    );
    assert!((c.est_len - 10.0 * MM as f64).abs() < 1.0);
    assert_eq!(c.net, 1);
    assert_eq!(c.tier, 3);
    assert_eq!(c.width, m.rules.net_rule(1).width);
    assert_eq!(d.nets.len(), 1);
    assert!(d.nets[0].in_scope && !d.nets[0].pour_served);
    assert_eq!(d.nets[0].connections, vec![0]);
}

#[test]
fn four_pad_net_is_mst_of_three() {
    let m = model(&[(0, 0), (10, 0), (20, 0), (10, 30)]);
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.connections.len(), 3);
    assert_eq!(
        labels(&d),
        vec![
            ("U1.1".into(), "U1.2".into()),
            ("U1.2".into(), "U1.3".into()),
            ("U1.2".into(), "U1.4".into()),
        ]
    );
    assert!(d.connections.iter().enumerate().all(|(i, c)| c.id == i));
}

#[test]
fn partial_trace_cluster_attaches_at_track_end() {
    let mut m = model(&[(0, 0), (30, 0)]);
    m.kept.push(seg("k1", 1, (0.0, 0.0), (12.0, 0.0)));
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.nets[0].clusters, 2);
    assert_eq!(d.connections.len(), 1);
    let c = &d.connections[0];
    assert_eq!(c.from.kind, TermKind::Copper("k1".into()));
    assert!((c.from.at.x - 12.0 * MM as f64).abs() < 1.0 && c.from.at.y.abs() < 1.0);
    assert_eq!(c.to.kind, TermKind::Pad(1));
    assert!((c.to.at.x - 30.0 * MM as f64).abs() < 1.0);
    assert!((c.est_len - 18.0 * MM as f64).abs() < 1.0);
    assert!(matches!(
        terminal_of(&c.from),
        Terminal::Junction { at } if (at.x - 12.0 * MM as f64).abs() < 1.0
    ));
    assert!(matches!(terminal_of(&c.to), Terminal::Pad { pad: 1, .. }));
}

#[test]
fn complete_net_yields_zero_connections() {
    let mut m = model(&[(0, 0), (30, 0)]);
    m.kept.push(seg("k1", 1, (0.0, 0.0), (30.0, 0.0)));
    let d = decompose(&m, &Options::default(), &[0]);
    assert!(d.connections.is_empty());
    assert_eq!(d.nets[0].clusters, 1);
    assert!(!d.nets[0].pour_served);
}

#[test]
fn pour_served_net() {
    let mut m = model(&[(0, 0), (30, 0)]);
    m.pours.push(Pour {
        uuid: "z".into(),
        net: 1,
        layers: vec![0],
        pads: vec!["U1.1".into(), "U1.2".into()],
        vias: vec![],
        fills: vec![],
        ..Pour::default()
    });
    let d = decompose(&m, &Options::default(), &[0]);
    assert!(d.connections.is_empty());
    assert!(d.nets[0].pour_served);
    assert_eq!(d.nets[0].clusters, 1);

    m.pours[0].pads = vec!["U1.1".into()];
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.connections.len(), 1);
    assert!(!d.nets[0].pour_served);
}

#[test]
fn pour_on_other_layer_does_not_join_smd_pad() {
    // Pads are F.Cu-only SMD (layer 0); the pour is on B.Cu (layer 2): listed or not, no union.
    let mut m = model(&[(0, 0), (30, 0)]);
    m.pours.push(Pour {
        uuid: "z".into(),
        net: 1,
        layers: vec![2],
        pads: vec!["U1.1".into(), "U1.2".into()],
        vias: vec![],
        fills: vec![],
        ..Pour::default()
    });
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.connections.len(), 1);
    assert!(!d.nets[0].pour_served);
    assert_eq!(d.nets[0].clusters, 2);
}

#[test]
fn scope_pads_restricts_to_those_pads() {
    let m = model(&[(0, 0), (10, 0), (20, 0)]);
    let s = Scope {
        pads: vec![
            PadRef::try_from("U1.1".to_string()).unwrap(),
            PadRef::try_from("U1.2".to_string()).unwrap(),
        ],
        ..Scope::default()
    };
    let d = decompose(&m, &scoped(s), &[0]);
    assert_eq!(labels(&d), vec![("U1.1".into(), "U1.2".into())]);
}

#[test]
fn scope_single_pad_connects_to_nearest_cluster() {
    let m = model(&[(0, 0), (10, 0), (25, 0)]);
    let s = Scope {
        pads: vec![PadRef::try_from("U1.3".to_string()).unwrap()],
        ..Scope::default()
    };
    let d = decompose(&m, &scoped(s), &[0]);
    assert_eq!(d.connections.len(), 1);
    let c = &d.connections[0];
    let mut l = [c.from.label.as_str(), c.to.label.as_str()];
    l.sort();
    assert_eq!(l, ["U1.2", "U1.3"]);
}

#[test]
fn scope_nets_and_netclasses() {
    let mut m = model(&[(0, 0), (10, 0)]);
    m.pads.push(pad(3, 2, 0, 20));
    m.pads.push(pad(4, 2, 10, 20));
    m.rules.per_net.insert(
        2,
        NetRule {
            netclass: "power".into(),
            width: 400_000,
            ..NetRule::default()
        },
    );
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.connections.len(), 2);
    assert_eq!(d.connections[1].width, 400_000);
    assert_eq!(d.connections[1].tier, 2);

    let d = decompose(
        &m,
        &scoped(Scope {
            nets: vec!["A".into()],
            ..Scope::default()
        }),
        &[0],
    );
    assert_eq!(d.connections.len(), 1);
    assert_eq!(d.connections[0].net, 1);
    assert!(d.nets.iter().find(|n| n.net == 1).unwrap().in_scope);
    let b = d.nets.iter().find(|n| n.net == 2).unwrap();
    assert!(!b.in_scope && b.connections.is_empty());

    let d = decompose(
        &m,
        &scoped(Scope {
            netclasses: vec!["power".into()],
            ..Scope::default()
        }),
        &[0],
    );
    assert_eq!(d.connections.len(), 1);
    assert_eq!(d.connections[0].net, 2);
}

#[test]
fn scope_area_selects_pads_inside() {
    // The area is in the KiCad frame (y down); the model is in the math frame (y up), so a
    // pad at math (0, -20) mm sits at KiCad y = +20 mm.
    let mut m = model(&[(0, 0), (10, 0)]);
    m.pads.push(pad(3, 2, 0, -20));
    m.pads.push(pad(4, 2, 10, -20));
    let sq = |x0: i64, y0: i64, x1: i64, y1: i64| {
        vec![
            Pt {
                x: x0 * MM,
                y: y0 * MM,
            },
            Pt {
                x: x1 * MM,
                y: y0 * MM,
            },
            Pt {
                x: x1 * MM,
                y: y1 * MM,
            },
            Pt {
                x: x0 * MM,
                y: y1 * MM,
            },
        ]
    };
    let d = decompose(
        &m,
        &scoped(Scope {
            area: Some(sq(-5, 15, 15, 25)),
            ..Scope::default()
        }),
        &[0],
    );
    assert_eq!(d.connections.len(), 1);
    assert_eq!(d.connections[0].net, 2);
}

fn conn(id: usize, net: i32, width: i64, tier: u8, len_mm: f64) -> Connection {
    let t = TermRef {
        kind: TermKind::Pad(0),
        label: "U1.1".into(),
        at: Vec2 { x: 0.0, y: 0.0 },
        layers: vec![0],
    };
    Connection {
        id,
        net,
        width,
        clearance: 200_000,
        tier,
        from: t.clone(),
        to: t,
        est_len: len_mm * MM as f64,
    }
}

#[test]
fn order_is_tier_width_length_then_net() {
    // ids: 0 power (tier 2, 0.4 mm, 20 mm), 1 Signal 2 mm, 2 Signal 5 mm
    let cs = vec![
        conn(0, 1, 400_000, 2, 20.0),
        conn(1, 2, 250_000, 3, 2.0),
        conn(2, 3, 250_000, 3, 5.0),
    ];
    assert_eq!(order(&cs, &[0, 0, 0]), vec![0, 1, 2]);
    assert_eq!(order(&cs, &[0, 0, 1]), vec![2, 0, 1]);
    // listed in another order the key still decides
    let rev = vec![cs[2].clone(), cs[0].clone(), cs[1].clone()];
    assert_eq!(order(&rev, &[]), vec![1, 2, 0]);
    // ties: net, then id
    let tie = vec![conn(0, 5, 250_000, 3, 5.0), conn(1, 4, 250_000, 3, 5.0)];
    assert_eq!(order(&tie, &[0, 0]), vec![1, 0]);
}

#[test]
fn dangling_end_is_a_terminal() {
    let mut m = model(&[(0, 0), (30, 0)]);
    m.kept.push(seg("k1", 1, (0.0, 0.0), (5.0, 0.0)));
    m.dangling_ends.push(DanglingEnd {
        item: "k1".into(),
        net: 1,
        layer: 0,
        width: 250_000,
        end: Pt { x: 5 * MM, y: 0 },
        direction: [1.0, 0.0],
    });
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.connections.len(), 1);
    let c = &d.connections[0];
    assert_eq!(c.from.kind, TermKind::Dangling("k1".into(), Some([1.0, 0.0])));
    assert!((c.from.at.x - 5.0 * MM as f64).abs() < 1.0);
    assert!(matches!(terminal_of(&c.from), Terminal::Dangling { .. }));
}

#[test]
fn via_is_a_terminal_and_joins_clusters() {
    use topor_engine::model::Via;
    let mut m = model(&[(0, 0), (30, 0)]);
    m.vias.push(Via {
        uuid: "v1".into(),
        net: 1,
        at: Pt { x: 15 * MM, y: 0 },
        d: 600_000,
        drill: 300_000,
        layers: vec![0, 2],
        free: false,
        locked: false,
    });
    // track from pad 1 to the via centre: one cluster {pad1, via}; pad 2 alone
    m.kept.push(seg("k1", 1, (0.0, 0.0), (15.0, 0.0)));
    let d = decompose(&m, &Options::default(), &[0]);
    assert_eq!(d.nets[0].clusters, 2);
    assert_eq!(d.connections.len(), 1);
    assert_eq!(d.connections[0].from.kind, TermKind::Via(0));
    assert_eq!(d.connections[0].from.label, "v1");
}

// ---- M3 Task 6: terminal layers and pour completion ----

fn pt(x: i64, y: i64) -> Pt {
    Pt { x: x * MM, y: y * MM }
}

fn rect_fill(layer: u32, x0: i64, y0: i64, x1: i64, y1: i64) -> PourFill {
    PourFill {
        layer,
        outer: vec![pt(x0, y0), pt(x1, y0), pt(x1, y1), pt(x0, y1)],
        holes: vec![],
    }
}

fn tht(num: usize, net: i32, x: i64, y: i64) -> Pad {
    let mut p = pad(num, net, x, y);
    p.layers = vec![0, 31];
    p.shapes = [
        (0u32, PadShape::Circle { d: 1_600_000 }),
        (31u32, PadShape::Circle { d: 1_600_000 }),
    ]
    .into();
    p
}

fn smd_b(num: usize, net: i32, x: i64, y: i64) -> Pad {
    let mut p = pad(num, net, x, y);
    p.layers = vec![31];
    p.shapes = [(31u32, PadShape::Circle { d: 1_600_000 })].into();
    p
}

fn gnd_pour(pads: &[&str], fills: Vec<PourFill>) -> Pour {
    Pour {
        uuid: "zz".into(),
        net: 1,
        layers: vec![31],
        pads: pads.iter().map(|s| s.to_string()).collect(),
        vias: vec![],
        fills,
        ..Pour::default()
    }
}

/// Net 1: THT pad U1.1 at (10,15) inside the fill, SMD F.Cu pads at the given spots.
fn pour_model(extra: &[(i64, i64)], listed: &[&str], fills: Vec<PourFill>) -> BoardModel {
    let mut m = model(&[]);
    m.pads.push(tht(1, 1, 10, 15));
    for (i, &(x, y)) in extra.iter().enumerate() {
        m.pads.push(pad(i + 2, 1, x, y));
    }
    m.pours.push(gnd_pour(listed, fills));
    m
}

const L2: [u32; 2] = [0, 31];

#[test]
fn terminals_carry_their_copper_layers() {
    let mut m = model(&[]);
    m.pads.push(tht(1, 1, 0, 0));
    m.pads.push(smd_b(2, 1, 10, 0));
    m.pads.push(pad(3, 1, 20, 0));
    m.vias.push(topor_engine::model::Via {
        uuid: "v1".into(),
        net: 1,
        at: pt(30, 0),
        d: 600_000,
        drill: 300_000,
        layers: vec![0, 31],
        free: false,
        locked: false,
    });
    let d = decompose(&m, &Options::default(), &L2);
    let mut by: std::collections::BTreeMap<String, Vec<u32>> = Default::default();
    for c in &d.connections {
        for t in [&c.from, &c.to] {
            by.insert(t.label.clone(), t.layers.clone());
        }
    }
    assert_eq!(by["U1.1"], vec![0, 31]);
    assert_eq!(by["U1.2"], vec![31]);
    assert_eq!(by["U1.3"], vec![0]);
    assert_eq!(by["v1"], vec![0, 31]);
}

#[test]
fn decompose_layers_argument_filters_terminal_layers() {
    let mut m = model(&[]);
    m.pads.push(smd_b(1, 1, 0, 0));
    m.pads.push(tht(2, 1, 10, 0));
    let d = decompose(&m, &Options::default(), &[0]);
    let c = &d.connections[0];
    assert_eq!(c.from.layers, Vec::<u32>::new());
    assert_eq!(c.to.layers, vec![0]);
}

#[test]
fn pour_completion_connects_unreached_cluster_to_pour() {
    let m = pour_model(&[(40, 15)], &["U1.1"], vec![rect_fill(31, 0, 0, 30, 30)]);
    let d = decompose(&m, &Options::default(), &L2);
    assert_eq!(d.connections.len(), 1);
    let c = &d.connections[0];
    assert_eq!(c.from.label, "U1.2");
    assert_eq!(c.to.kind, TermKind::Pour(0));
    assert_eq!(c.to.label, "pour:zz");
    assert_eq!(c.to.layers, vec![31]);
    assert!((c.est_len - 10.0 * MM as f64).abs() < 1.0);
    assert!((c.to.at.x - 30.0 * MM as f64).abs() < 1.0 && (c.to.at.y - 15.0 * MM as f64).abs() < 1.0);
    assert!(!d.nets[0].pour_served);
    assert_eq!(d.nets[0].connections, vec![0]);
    assert!(matches!(terminal_of(&c.to), Terminal::Site { .. }));
}

#[test]
fn pour_served_net_without_gaps_has_no_connections() {
    let m = pour_model(&[(20, 20)], &["U1.1", "U1.2"], vec![rect_fill(31, 0, 0, 30, 30)]);
    // U1.2 is F.Cu SMD: the pour (B.Cu) does not join it, so it needs a connection...
    let d = decompose(&m, &Options::default(), &L2);
    assert_eq!(d.connections.len(), 1);
    // ...whereas a THT pad listed by the pour is served.
    let mut m = pour_model(&[], &["U1.1", "U1.2"], vec![rect_fill(31, 0, 0, 30, 30)]);
    m.pads.push(tht(2, 1, 20, 20));
    let d = decompose(&m, &Options::default(), &L2);
    assert!(d.connections.is_empty());
    assert!(d.nets[0].pour_served);
}

#[test]
fn inside_cluster_has_zero_length_pour_connection() {
    let m = pour_model(&[(20, 20)], &["U1.1"], vec![rect_fill(31, 0, 0, 30, 30)]);
    let d = decompose(&m, &Options::default(), &L2);
    assert_eq!(d.connections.len(), 1);
    assert_eq!(d.connections[0].est_len, 0.0);
}

#[test]
fn two_unreached_clusters_each_connect_to_the_pour_not_each_other() {
    let m = pour_model(&[(40, 15), (40, 20)], &["U1.1"], vec![rect_fill(31, 0, 0, 30, 30)]);
    let d = decompose(&m, &Options::default(), &L2);
    assert_eq!(d.connections.len(), 2);
    assert!(d.connections.iter().all(|c| c.to.kind == TermKind::Pour(0)));
    let mut f: Vec<_> = d.connections.iter().map(|c| c.from.label.clone()).collect();
    f.sort();
    assert_eq!(f, ["U1.2", "U1.3"]);
}

#[test]
fn pour_without_fills_falls_back_to_m2() {
    let m = pour_model(&[(40, 15)], &["U1.1"], vec![]);
    let d = decompose(&m, &Options::default(), &L2);
    assert!(d.warnings.iter().any(|w| w.contains("pour zz has no fill outline")));
    assert_eq!(d.connections.len(), 1);
    assert!(d.connections.iter().all(|c| !matches!(c.to.kind, TermKind::Pour(_))));
    assert_eq!(
        (d.connections[0].from.label.as_str(), d.connections[0].to.label.as_str()),
        ("U1.1", "U1.2")
    );
}

/// pour-pad-join: footprint J1 with four THT shield pads all numbered "SH", net 1, and the B.Cu pour of
/// net 1 filling (0,0)-(30,30) mm with thermal spokes (KiCad's usual zone settings). `at` = the pads'
/// centres in pad-list order; `listed` = the pour's pad list (one entry per pad it reaches, as the IPC and
/// pcbnew exporters write it).
fn shield_model(at: &[(i64, i64)], listed: &[&str]) -> BoardModel {
    let mut m = model(&[]);
    for (i, &(x, y)) in at.iter().enumerate() {
        let mut p = tht(i + 1, 1, x, y);
        p.ref_ = "J1".into();
        p.number = "SH".into();
        m.pads.push(p);
    }
    let mut z = gnd_pour(listed, vec![rect_fill(31, 0, 0, 30, 30)]);
    (z.clearance, z.min_thickness, z.thermal_gap, z.spoke_width, z.pad_connection) = (200_000, 254_000, 500_000, 500_000, 1);
    m.pours.push(z);
    m
}

#[test]
fn same_numbered_pads_the_fill_reaches_are_all_joined() {
    // All four shield pads stand in the fill, listed four times: KiCad calls the net complete, so there is
    // nothing to route. The first-match join left three of them out of the pour (phantom pad-to-pour
    // connections on a complete board: the private board P's fast empty runs).
    let m = shield_model(&[(5, 5), (25, 5), (5, 25), (25, 25)], &["J1.SH"; 4]);
    let d = decompose(&m, &Options::default(), &L2);
    assert!(d.connections.is_empty(), "{:?}", labels(&d));
    assert!(d.nets[0].pour_served);
}

#[test]
fn same_numbered_pad_outside_the_fill_keeps_its_connection() {
    // Four shield pads, the fill reaches three (listed three times); the fourth at (40, 15) stands 10 mm
    // right of the fill. Whatever its place in the pad list, exactly that pad gets the one connection, to
    // the pour; the three in the fill are served.
    let inside = [(5, 5), (25, 5), (5, 25)];
    for k in 0..4 {
        let mut at: Vec<(i64, i64)> = inside.to_vec();
        at.insert(k, (40, 15));
        let m = shield_model(&at, &["J1.SH"; 3]);
        let d = decompose(&m, &Options::default(), &L2);
        assert_eq!(d.connections.len(), 1, "outside pad at {k}: {:?}", labels(&d));
        let c = &d.connections[0];
        assert_eq!(c.to.kind, TermKind::Pour(0));
        assert!((c.from.at.x - 40.0 * MM as f64).abs() < 1.0, "outside pad at {k}: from {:?}", c.from.at);
        assert!((c.est_len - 10.0 * MM as f64).abs() < 1.0, "{}", c.est_len);
        for a in (0..4).filter(|&a| a != k) {
            assert_ne!(d.pad_cluster[&a], d.conn_clusters[0].0, "pad {a} in the fill is pour-served");
        }
    }
}

/// pour-pad-join r2: net 1 in two B.Cu pours with nothing joining them yet: `z0` (0..30 x 0..30 mm) holds THT pads
/// U1.1 .. U1.`n0` at (5 + 10 k, 15), each listed once; `z1` (50..70 x 0..30 mm) holds connector J1's two THT shield
/// pads "SH" at (55, 15) and (65, 15), listed twice. `u2`: z1 also holds THT pad U2.1 at (60, 25), listed once (a
/// singly-listed pad beside the shield pads). Spoke settings as `shield_model`.
fn two_pour_model(n0: usize, u2: bool) -> BoardModel {
    let mut m = model(&[]);
    for k in 0..n0 {
        m.pads.push(tht(k + 1, 1, 5 + 10 * k as i64, 15));
    }
    for (i, x) in [55, 65].into_iter().enumerate() {
        let mut p = tht(10 + i, 1, x, 15);
        p.ref_ = "J1".into();
        p.number = "SH".into();
        m.pads.push(p);
    }
    let mut z1_pads = vec!["J1.SH", "J1.SH"];
    if u2 {
        let mut p = tht(1, 1, 60, 25);
        p.ref_ = "U2".into();
        p.uuid = "u2".into();
        m.pads.push(p);
        z1_pads.push("U2.1");
    }
    let z0_pads: Vec<String> = (1..=n0).map(|k| format!("U1.{k}")).collect();
    let z0_refs: Vec<&str> = z0_pads.iter().map(String::as_str).collect();
    for (uuid, listed, fill) in [("z0", z0_refs, rect_fill(31, 0, 0, 30, 30)), ("z1", z1_pads, rect_fill(31, 50, 0, 70, 30))] {
        let mut z = gnd_pour(&listed, vec![fill]);
        z.uuid = uuid.into();
        (z.clearance, z.min_thickness, z.thermal_gap, z.spoke_width, z.pad_connection) = (200_000, 254_000, 500_000, 500_000, 1);
        m.pours.push(z);
    }
    m
}

/// The side of the net a terminal is on: 0 = pour z0 and its U1 pads, 1 = pour z1 and the pads in it.
fn side(t: &TermRef) -> u8 {
    match t.kind {
        TermKind::Pour(z) => z as u8,
        _ if t.label.starts_with("U1.") => 0,
        _ => 1,
    }
}

#[test]
fn shield_pads_in_a_second_pour_join_that_pour() {
    // The shield pads stand in pour z1, which nothing joins to z0 yet. The fill joins them to z1 whether or not z1
    // is the net's main component (z0 with 3 listed pads is; with 1 it is not): one real connection from the z1
    // group to the z0 group, no pad-to-pour connection inside z1. (The main component's pads alone left z1 without
    // a port when z0 was the main one: two phantom J1.SH -> z1 connections, and the z1 group never joined to z0.)
    for n0 in [1, 3] {
        for u2 in [false, true] {
            let d = decompose(&two_pour_model(n0, u2), &Options::default(), &L2);
            assert_eq!(d.connections.len(), 1, "n0 {n0} u2 {u2}: {:?}", labels(&d));
            let c = &d.connections[0];
            let mut sides = [side(&c.from), side(&c.to)];
            sides.sort_unstable();
            assert_eq!(sides, [0, 1], "n0 {n0} u2 {u2}: {:?}", labels(&d));
        }
    }
}

#[test]
fn same_numbered_pads_split_over_two_pours_each_join_their_own() {
    // J1 has one shield pad in each pour, and each pour lists "J1.SH" once: the label names two pads, the fill
    // decides; each pad is joined to the pour it stands in, so the only connection is the one between the pours.
    let mut m = two_pour_model(3, false);
    m.pads.iter_mut().find(|p| p.number == "SH").unwrap().center = pt(25, 25);
    m.pours[0].pads.push("J1.SH".into());
    m.pours[1].pads.truncate(1);
    let d = decompose(&m, &Options::default(), &L2);
    assert_eq!(d.connections.len(), 1, "{:?}", labels(&d));
    let c = &d.connections[0];
    let mut sides = [side(&c.from), side(&c.to)];
    sides.sort_unstable();
    assert_eq!(sides, [0, 1], "{:?}", labels(&d));
    let sh: Vec<usize> = (0..m.pads.len()).filter(|&i| m.pads[i].number == "SH").collect();
    assert_ne!(d.pad_cluster[&sh[0]], d.pad_cluster[&sh[1]], "each shield pad in its own pour's cluster");
}
