use std::collections::HashSet;
use topor_engine::geom::{Orient, Vec2};
use topor_engine::model::{BoardModel, Options};
use topor_engine::obstacle::ObstacleSet;
use topor_engine::sketch::{
    build_layer_cdt, funnel, sketch_wire, stack_order, AnchorRef, LayerSketch, Sketch,
};
use topor_engine::wires::{Terminal, Wire};

const MM: f64 = 1_000_000.0;

fn mm(x: f64, y: f64) -> Vec2 {
    Vec2 {
        x: x * MM,
        y: y * MM,
    }
}

type P = (f64, f64);

fn portals(p: &[(P, P)]) -> Vec<(Vec2, Vec2)> {
    p.iter()
        .map(|&((lx, ly), (rx, ry))| (mm(lx, ly), mm(rx, ry)))
        .collect()
}

#[test]
fn funnel_straight_when_portal_is_wide() {
    let p = portals(&[((5., 3.), (5., 0.5))]);
    assert!(funnel(mm(0., 0.), mm(10., 2.), &p).is_empty());
}

#[test]
fn funnel_bends_right_around_right_vertex() {
    let p = portals(&[((5., 3.), (5., 1.5))]);
    assert_eq!(
        funnel(mm(0., 0.), mm(10., 2.), &p),
        vec![(0, false, Orient::Cw)]
    );
}

#[test]
fn funnel_bends_left_around_left_vertex() {
    let p = portals(&[((5., -1.5), (5., -3.))]);
    assert_eq!(
        funnel(mm(0., 0.), mm(10., -2.), &p),
        vec![(0, true, Orient::Ccw)]
    );
}

#[test]
fn funnel_two_bends_zigzag() {
    // coordinator ruling D1
    let p = portals(&[((4., -1.), (4., -5.)), ((8., 5.), (8., 1.))]);
    assert_eq!(
        funnel(mm(0., 0.), mm(12., 0.), &p),
        vec![(0, true, Orient::Ccw), (1, false, Orient::Cw)]
    );
}

#[test]
fn funnel_no_portals_is_empty() {
    assert!(funnel(mm(0., 0.), mm(1., 1.), &[]).is_empty());
}

fn model(pad_kicad_y_mm: f64) -> BoardModel {
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0 }}, {{ "id": 2 }} ],
          "pads": [ {{ "uuid": "p", "ref": "U1", "number": "1", "net": 2,
            "center": [5000000, {}], "layers": [0],
            "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        (pad_kicad_y_mm * MM) as i64
    );
    BoardModel::from_json(&s).unwrap()
}

fn wire(id: usize, hand: &[(f64, f64)]) -> Wire {
    let h: Vec<Vec2> = hand.iter().map(|&(x, y)| mm(x, y)).collect();
    Wire {
        id,
        net: 3,
        layer: 0,
        width: 250_000,
        start: Terminal::Dangling { at: h[0] },
        goal: Terminal::Dangling {
            at: *h.last().unwrap(),
        },
        hand: h,
        kept_uuids: vec![],
        hand_len: 0.0,
    }
}

fn layer_sketch(pad_math_y: f64, wires: &[Wire]) -> LayerSketch {
    // math y = -kicad y
    let m = model(-pad_math_y);
    let obs = ObstacleSet::build(&m, &Options::default(), &HashSet::new(), &[]);
    let terms: Vec<Vec2> = wires
        .iter()
        .flat_map(|w| [w.hand[0], *w.hand.last().unwrap()])
        .collect();
    build_layer_cdt(obs.layer(0), &terms).unwrap()
}

fn pad_anchor(ls: &LayerSketch) -> usize {
    ls.point_to_anchor.iter().flatten().copied().next().unwrap()
}

#[test]
fn sketch_wire_around_one_pad() {
    // coordinator ruling D2: pad at (5, -0.5), hand below it
    let w = wire(0, &[(0., 0.), (5., -1.), (10., 0.)]);
    let ls = layer_sketch(-0.5, std::slice::from_ref(&w));
    let a = pad_anchor(&ls);
    let s = sketch_wire(&w, &ls).unwrap();
    assert_eq!(
        s.seq,
        vec![AnchorRef {
            anchor: a,
            sigma: Orient::Ccw
        }]
    );
}

#[test]
fn sketch_wire_hand_on_other_side_gives_cw() {
    let w = wire(0, &[(0., 0.), (5., 2.), (10., 0.)]);
    let ls = layer_sketch(0.5, std::slice::from_ref(&w));
    let a = pad_anchor(&ls);
    let s = sketch_wire(&w, &ls).unwrap();
    assert_eq!(
        s.seq,
        vec![AnchorRef {
            anchor: a,
            sigma: Orient::Cw
        }]
    );
}

#[test]
fn sketch_wire_straight_hand_has_empty_seq() {
    let w = wire(0, &[(0., 0.), (10., 0.)]);
    let ls = layer_sketch(0.5, std::slice::from_ref(&w));
    let s = sketch_wire(&w, &ls).unwrap();
    assert!(s.seq.is_empty());
}

#[test]
fn sketch_wire_vertex_on_cdt_vertex_is_error() {
    let w = wire(0, &[(0., 0.), (5., 0.5), (10., 0.)]);
    let ls = layer_sketch(0.5, std::slice::from_ref(&w));
    assert!(sketch_wire(&w, &ls).is_err());
}

fn two_wires(y0: f64, y1: f64) -> (LayerSketch, Vec<Sketch>) {
    let ws = vec![
        wire(0, &[(0., 0.), (5., y0), (10., 0.)]),
        wire(1, &[(0., 0.), (5., y1), (10., 0.)]),
    ];
    let ls = layer_sketch(-0.5, &ws);
    let sk: Vec<Sketch> = ws.iter().map(|w| sketch_wire(w, &ls).unwrap()).collect();
    (ls, sk)
}

#[test]
fn stack_order_inner_first() {
    let (ls, sk) = two_wires(-1., -2.);
    let a = pad_anchor(&ls);
    let key = AnchorRef {
        anchor: a,
        sigma: Orient::Ccw,
    };
    let so = stack_order(&sk, &ls);
    assert_eq!(so[&key], vec![0, 1]);
    let (ls, sk) = two_wires(-2., -1.);
    let so = stack_order(&sk, &ls);
    assert_eq!(so[&key], vec![1, 0]);
}

#[test]
fn sketch_is_deterministic() {
    let (ls, a) = two_wires(-1., -2.);
    let b: Vec<Sketch> = [
        wire(0, &[(0., 0.), (5., -1.), (10., 0.)]),
        wire(1, &[(0., 0.), (5., -2.), (10., 0.)]),
    ]
    .iter()
    .map(|w| sketch_wire(w, &ls).unwrap())
    .collect();
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.seq, y.seq);
        assert_eq!(x.crossings, y.crossings);
    }
}
