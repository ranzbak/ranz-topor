use topor_engine::decompose::{decompose, order, terminal_of, Connection, TermKind, TermRef};
use topor_engine::geom::{IVec2, TrackPrim, Vec2};
use topor_engine::model::{
    BoardModel, DanglingEnd, KeptItem, Net, NetRule, Options, Pad, PadRef, PadShape, Pour, Pt,
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
    let d = decompose(&m, &Options::default(), 0);
    assert_eq!(d.connections.len(), 1);
    let c = &d.connections[0];
    assert_eq!(
        (c.from.label.as_str(), c.to.label.as_str()),
        ("U1.1", "U1.2")
    );
    assert!((c.est_len - 10.0 * MM as f64).abs() < 1.0);
    assert_eq!(c.net, 1);
    assert_eq!(c.layer, 0);
    assert_eq!(c.tier, 3);
    assert_eq!(c.width, m.rules.net_rule(1).width);
    assert_eq!(d.nets.len(), 1);
    assert!(d.nets[0].in_scope && !d.nets[0].pour_served);
    assert_eq!(d.nets[0].connections, vec![0]);
}

#[test]
fn four_pad_net_is_mst_of_three() {
    let m = model(&[(0, 0), (10, 0), (20, 0), (10, 30)]);
    let d = decompose(&m, &Options::default(), 0);
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
    let d = decompose(&m, &Options::default(), 0);
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
    let d = decompose(&m, &Options::default(), 0);
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
    });
    let d = decompose(&m, &Options::default(), 0);
    assert!(d.connections.is_empty());
    assert!(d.nets[0].pour_served);
    assert_eq!(d.nets[0].clusters, 1);

    m.pours[0].pads = vec!["U1.1".into()];
    let d = decompose(&m, &Options::default(), 0);
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
    });
    let d = decompose(&m, &Options::default(), 0);
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
    let d = decompose(&m, &scoped(s), 0);
    assert_eq!(labels(&d), vec![("U1.1".into(), "U1.2".into())]);
}

#[test]
fn scope_single_pad_connects_to_nearest_cluster() {
    let m = model(&[(0, 0), (10, 0), (25, 0)]);
    let s = Scope {
        pads: vec![PadRef::try_from("U1.3".to_string()).unwrap()],
        ..Scope::default()
    };
    let d = decompose(&m, &scoped(s), 0);
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
    let d = decompose(&m, &Options::default(), 0);
    assert_eq!(d.connections.len(), 2);
    assert_eq!(d.connections[1].width, 400_000);
    assert_eq!(d.connections[1].tier, 2);

    let d = decompose(
        &m,
        &scoped(Scope {
            nets: vec!["A".into()],
            ..Scope::default()
        }),
        0,
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
        0,
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
        0,
    );
    assert_eq!(d.connections.len(), 1);
    assert_eq!(d.connections[0].net, 2);
}

fn conn(id: usize, net: i32, width: i64, tier: u8, len_mm: f64) -> Connection {
    let t = TermRef {
        kind: TermKind::Pad(0),
        label: "U1.1".into(),
        at: Vec2 { x: 0.0, y: 0.0 },
    };
    Connection {
        id,
        net,
        layer: 0,
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
    let d = decompose(&m, &Options::default(), 0);
    assert_eq!(d.connections.len(), 1);
    let c = &d.connections[0];
    assert_eq!(c.from.kind, TermKind::Dangling("k1".into()));
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
    let d = decompose(&m, &Options::default(), 0);
    assert_eq!(d.nets[0].clusters, 2);
    assert_eq!(d.connections.len(), 1);
    assert_eq!(d.connections[0].from.kind, TermKind::Via(0));
    assert_eq!(d.connections[0].from.label, "v1");
}
