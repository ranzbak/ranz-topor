//! NPTH pads keep `hole_clearance` in every geometric stage (task HC, DX fix #3).
//!
//! An NPTH pad is a net-0 pad (copper clearance 0.2 mm) *and* a drilled hole
//! (`hole_clearance` 0.35 mm): every stage that consumes the pad copper (graph capacity,
//! search, sketch / taut anchors, relax) must charge the larger of the two, as the
//! `Checker` does. The fixture pinches a corridor between the hole and the board edge to
//! 0.75 mm: legal for copper clearance (0.2 + 0.25 + 0.2 = 0.65), illegal for the hole
//! (0.35 + 0.25 + 0.2 = 0.80).

use topor_engine::check::Checker;
use topor_engine::geom::{dequantise, dist_prim, Prim, Seg, Vec2};
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::{route, ConnReport, RouteResult};

const MM: f64 = 1_000_000.0;
const HOLE_D: f64 = 3.2;
const HOLE_CLEARANCE: f64 = 0.35;

/// `y` is KiCad-frame up-positive; board y = -y.
fn circle_pad(r: &str, n: &str, net: i32, x: f64, y: f64, d: f64) -> String {
    format!(
        r#"{{ "uuid": "{r}{n}", "ref": "{r}", "number": "{n}", "net": {net},
          "center": [{}, {}], "layers": [0],
          "shapes": {{ "0": {{ "kind": "circle", "d": {} }} }} }}"#,
        (x * MM) as i64,
        (-y * MM) as i64,
        (d * MM) as i64
    )
}

fn npth_pad(x: f64, y: f64) -> String {
    format!(
        r#"{{ "uuid": "H1", "ref": "H1", "number": "", "net": 0, "attr": "npth",
          "center": [{}, {}], "layers": [0],
          "drill": {{ "w": {d}, "h": {d} }},
          "shapes": {{ "0": {{ "kind": "circle", "d": {d} }} }} }}"#,
        (x * MM) as i64,
        (-y * MM) as i64,
        d = (HOLE_D * MM) as i64
    )
}

/// Board outline: x 0..50, y up 0..`top`.
fn outline(top: f64) -> String {
    let t = (top * MM) as i64;
    let w = (50.0 * MM) as i64;
    [
        ((0, 0), (w, 0)),
        ((w, 0), (w, -t)),
        ((w, -t), (0, -t)),
        ((0, -t), (0, 0)),
    ]
    .iter()
    .map(|&(a, b): &((i64, i64), (i64, i64))| {
        format!(
            r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
            a.0, a.1, b.0, b.1
        )
    })
    .collect::<Vec<_>>()
    .join(",")
}

/// Hole at (25, 20); net 1 from (10, 20.8) to (40, 20.8) (the straight line crosses the
/// hole; the shortest detour is over its top). The board edge is `gap` mm above the hole's
/// top (y = 21.6), `edge_clearance` 0.2 mm; the lower side of the hole is open.
fn model(gap: f64) -> BoardModel {
    let top = 20.0 + 0.5 * HOLE_D + gap;
    let pads = [
        circle_pad("A", "1", 1, 10.0, 20.8, 0.5),
        circle_pad("B", "1", 1, 40.0, 20.8, 0.5),
        npth_pad(25.0, 20.0),
    ];
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }} ],
          "pads": [ {} ],
          "holes": [ {{ "at": [{}, {}], "d": {d}, "kind": "npth" }} ],
          "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": {h}, "edge_clearance": 200000 }} }}"#,
        pads.join(","),
        (25.0 * MM) as i64,
        (-20.0 * MM) as i64,
        outline(top),
        d = (HOLE_D * MM) as i64,
        h = (HOLE_CLEARANCE * MM) as i64,
    );
    let mut m = BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"));
    for l in &mut m.layers {
        l.routable = l.id == 0;
    }
    m
}

fn opts() -> Options {
    Options {
        wall_clock_s: 600.0,
        threads: 1,
        ..Options::default()
    }
}

fn n1(r: &RouteResult) -> &ConnReport {
    &r.connections[0]
}

fn check(m: &BoardModel, r: &RouteResult) {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// Smallest copper-edge-to-hole-edge gap of the routed tracks, mm.
fn min_hole_gap_mm(r: &RouteResult) -> f64 {
    let c = Vec2 {
        x: 25.0 * MM,
        y: -20.0 * MM,
    };
    let pt = Prim::Seg(Seg { a: c, b: c });
    let mut best = f64::INFINITY;
    for it in &r.items {
        if let Item::Track(t) = it {
            best = best.min(dist_prim(&dequantise(&t.prim), &pt) - 0.5 * t.width as f64);
        }
    }
    (best - 0.5 * HOLE_D * MM) / MM
}

#[test]
fn edge_pinch_legal_for_copper_but_not_for_hole_is_not_taken() {
    let m = model(0.75);
    let r = route(&m, &opts());
    assert_eq!(n1(&r).status, "routed", "{} {}", n1(&r).status, n1(&r).reason);
    check(&m, &r);
    let gap = min_hole_gap_mm(&r);
    assert!(gap >= HOLE_CLEARANCE - 1e-6, "gap to hole {gap} mm");
    // Search and metrization agree: no pinch is found and then rejected by the self-check.
    assert!(
        !n1(&r).reason.contains("hole"),
        "metrization rejected a path at the hole: {}",
        n1(&r).reason
    );
    assert!(r.warnings.iter().all(|w| !w.contains("hole")), "{:?}", r.warnings);
}

#[test]
fn edge_pinch_wide_enough_for_the_hole_is_routed_clean() {
    // 0.9 mm fits the hole clearance: routes either way, hole clearance holds.
    let m = model(0.9);
    let r = route(&m, &opts());
    assert_eq!(n1(&r).status, "routed", "{} {}", n1(&r).status, n1(&r).reason);
    check(&m, &r);
    assert!(min_hole_gap_mm(&r) >= HOLE_CLEARANCE - 1e-6);
}

#[test]
fn checker_models_a_slot_hole_as_its_capsule_like_the_router() {
    // NPTH slot 1.8 x 2.4 mm (long axis y). A vertical track whose copper edge is 0.36 mm from
    // the slot's flat side is legal for hole_clearance 0.35; a circle of d 2.4 mm would call it
    // a violation (the D17 / D18 slot cases of dcsgonebadge).
    let s = r#"{ "layers": [ { "id": 0, "routable": true } ],
        "nets": [ { "code": 1, "name": "N1" } ],
        "pads": [ { "uuid": "d", "ref": "D", "number": "", "net": 0, "attr": "npth",
                    "center": [0, 0], "layers": [0], "drill": { "w": 1800000, "h": 2400000 },
                    "shapes": { "0": { "kind": "oval", "w": 1800000, "h": 2400000 } } } ],
        "holes": [ { "at": [0, 0], "d": 2400000, "kind": "npth", "slot": [1800000, 2400000] } ],
        "rules": { "min_clearance": 0, "hole_clearance": 350000, "default": { "clearance": 200000 } } }"#;
    let m = BoardModel::from_json(s).unwrap();
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let c = Checker::new(&m, &o);
    let x = 900_000 + 350_000 + 125_000 + 10_000;
    let item = |x: i64| -> Item {
        serde_json::from_str(&format!(
            r#"{{ "net": 1, "layer": 0, "width": 250000,
                 "prim": {{ "kind": "seg", "a": [{x}, -1000000], "b": [{x}, 1000000] }}, "group": null }}"#
        ))
        .unwrap()
    };
    let ok = c.check_items(&[item(x)]);
    assert!(ok.violations.is_empty(), "{:?}", ok.violations);
    let bad = c.check_items(&[item(x - 30_000)]);
    assert!(!bad.violations.is_empty());
}
