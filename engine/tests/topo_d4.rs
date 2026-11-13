//! D4 tests (pour stitching, own-net junctions).

use topor_engine::geom::quant::{IVec2, TrackPrim};
use topor_engine::model::{BoardModel, Item, Options, TrackItem};
use topor_engine::pourcheck::check_pour_reach;
use topor_engine::topo::factors::{standard_factors, FactorConfig, Registry};
use topor_engine::topo::stitch::stitch;

const MM: i64 = 1_000_000;

/// 40 x 34 mm board, GND (net 1) poured on F.Cu (0) and B.Cu (31) over [2, 32] x [2, 32] mm;
/// GND SMD pad U1.1 on B.Cu at (4, 15), THT GND pad U3.1 at (20, 15); signal net 2.
fn model() -> BoardModel {
    let pad = |r: &str, net: i32, x: i64, y: i64, layers: &str, attr: &str| {
        let shapes: Vec<String> = layers.split(',').map(|l| format!(r#""{}": {{ "kind": "circle", "d": 1000000 }}"#, l.trim())).collect();
        format!(
            r#"{{ "uuid": "{r}", "ref": "{r}", "number": "1", "net": {net}, "attr": "{attr}", "center": [{x}, {y}], "layers": [{layers}], "shapes": {{ {} }} }}"#,
            shapes.join(",")
        )
    };
    let pads = [pad("U1", 1, 4 * MM, 15 * MM, "31", "smd"), pad("U3", 1, 20 * MM, 15 * MM, "0, 31", "pth")];
    let fill = |l: u32| format!(r#"{{ "layer": {l}, "outer": [[2000000, 2000000], [32000000, 2000000], [32000000, 32000000], [2000000, 32000000]] }}"#);
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
          "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [40000000, 0] }},
            {{ "kind": "seg", "a": [40000000, 0], "b": [40000000, 34000000] }},
            {{ "kind": "seg", "a": [40000000, 34000000], "b": [0, 34000000] }},
            {{ "kind": "seg", "a": [0, 34000000], "b": [0, 0] }} ],
          "pads": [ {} ],
          "pours": [ {{ "uuid": "zz", "net": 1, "layers": [0, 31], "pads": ["U1.1", "U3.1"],
            "vias": [], "clearance": 200000, "min_thickness": 254000, "thermal_gap": 500000,
            "spoke_width": 500000, "pad_connection": 2,
            "fills": [ {}, {} ] }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0 }} }}"#,
        pads.join(","),
        fill(0),
        fill(31)
    );
    BoardModel::from_json(&s).unwrap()
}

fn seg(net: i32, layer: u32, a: (i64, i64), b: (i64, i64)) -> Item {
    Item::Track(TrackItem {
        net,
        layer,
        width: 250_000,
        prim: TrackPrim::Seg { a: IVec2 { x: a.0 as i32, y: a.1 as i32 }, b: IVec2 { x: b.0 as i32, y: b.1 as i32 } },
        group: None,
        wire: None,
    })
}

fn reg() -> Registry {
    Registry::from_config(standard_factors(), &FactorConfig::default()).0
}

/// A foreign B.Cu track from edge to edge cuts the B.Cu pour; the piece left of it holds the
/// SMD pad U1.1 and nothing joins it to the rest (F.Cu has no copper of U1). One stitch via in
/// that island, near U1.1, where F.Cu is solid, joins it again.
#[test]
fn a_cut_off_island_with_a_pad_gets_a_stitch_via() {
    let m = model();
    let items = [seg(2, 31, (7 * MM, 1_000_000), (7 * MM, 33_000_000))];
    let before = check_pour_reach(&m, &items, &[]);
    assert_eq!(before.cuts.len(), 1, "{:?}", before.notes);
    assert_eq!(before.cuts[0].pads, vec![0]);
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    assert_eq!((s.before, s.after, s.island_vias, s.pad_stubs), (1, 0, 1, 0), "{:?}", s.notes);
    let Item::Via(v) = &s.items[0] else { panic!("{:?}", s.items) };
    assert_eq!(v.net, 1);
    // Left of the cut, near the pad.
    assert!(v.at.x < 7 * MM - 300_000 && (v.at.x - 4 * MM).abs() + (v.at.y - 15 * MM).abs() < 4 * MM, "{:?}", v.at);
    let mut all = items.to_vec();
    all.extend(s.items.iter().cloned());
    assert!(check_pour_reach(&m, &all, &[]).cuts.is_empty());
}

/// Nothing cut, nothing placed; the stage can be switched off.
#[test]
fn an_intact_pour_gets_no_stitch() {
    let m = model();
    let items = [seg(2, 31, (25 * MM, 20 * MM), (30 * MM, 20 * MM))];
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    assert_eq!((s.before, s.after, s.items.len()), (0, 0, 0), "{:?}", s.notes);
    assert!(Options::default().stitch_islands);
}

/// An island that holds no copper of the net is dropped by KiCad ("remove islands: always"),
/// so it is not cut off; with "never" it stays and is stitched.
#[test]
fn an_empty_island_counts_only_when_kicad_keeps_it() {
    let mut m = model();
    m.pads.retain(|p| p.ref_ != "U1");
    m.pours[0].pads = vec!["U3.1".into()];
    let items = [seg(2, 31, (7 * MM, 1_000_000), (7 * MM, 33_000_000))];
    assert!(check_pour_reach(&m, &items, &[]).cuts.is_empty());
    m.pours[0].island_removal = 1;
    let r = check_pour_reach(&m, &items, &[]);
    assert_eq!(r.cuts.len(), 1, "{:?}", r.notes);
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    assert_eq!((s.before, s.after, s.island_vias), (1, 0, 1), "{:?}", s.notes);
}

/// The planner's static via-site test and the drawing look at the same nine points of a
/// triangle: incentre, centroid and seven barycentric points, all inside it.
#[test]
fn via_site_points_are_the_drawings_and_inside_the_triangle() {
    use topor_engine::geom::Vec2;
    use topor_engine::topo::pack::tri_site_points;
    let (a, b, c) = (Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 4.0e6, y: 0.0 }, Vec2 { x: 0.0, y: 3.0e6 });
    let pts = tri_site_points(a, b, c);
    // Incentre of the 3-4-5 triangle: radius 1 mm at (1, 1).
    assert!((pts[0].x - 1.0e6).abs() < 1.0 && (pts[0].y - 1.0e6).abs() < 1.0, "{:?}", pts[0]);
    assert!((pts[1].x - 4.0e6 / 3.0).abs() < 1.0 && (pts[1].y - 1.0e6).abs() < 1.0);
    for p in pts {
        assert!(p.x > 0.0 && p.y > 0.0 && p.x / 4.0e6 + p.y / 3.0e6 < 1.0, "{p:?}");
    }
}

/// F.Cu only: a GND pour over [2, 20] x [2, 32] mm holding the SMD pad U1.1 at (19, 15), and a
/// second GND pour over [`x0`, `x0` + 4] x [13, 17] mm that touches nothing ("remove islands:
/// never", so KiCad keeps it). No other layer, so no via can join the island and it holds no pad.
fn island_model(x0: f64) -> BoardModel {
    let mm = |v: f64| (v * 1e6) as i64;
    let fill = |a: f64, b: f64, c: f64, d: f64| format!(r#"{{ "layer": 0, "outer": [[{}, {}], [{}, {}], [{}, {}], [{}, {}]] }}"#, mm(a), mm(b), mm(c), mm(b), mm(c), mm(d), mm(a), mm(d));
    let zone = |id: &str, pads: &str, f: String| {
        format!(
            r#"{{ "uuid": "{id}", "net": 1, "layers": [0], "pads": [{pads}], "vias": [], "clearance": 200000, "min_thickness": 254000,
              "thermal_gap": 500000, "spoke_width": 500000, "pad_connection": 2, "island_removal": 1, "fills": [ {f} ] }}"#
        )
    };
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
          "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [40000000, 0] }},
            {{ "kind": "seg", "a": [40000000, 0], "b": [40000000, 34000000] }},
            {{ "kind": "seg", "a": [40000000, 34000000], "b": [0, 34000000] }},
            {{ "kind": "seg", "a": [0, 34000000], "b": [0, 0] }} ],
          "pads": [ {{ "uuid": "U1", "ref": "U1", "number": "1", "net": 1, "attr": "smd", "center": [19000000, 15000000], "layers": [0],
                      "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }} ],
          "pours": [ {}, {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0 }} }}"#,
        zone("zm", r#""U1.1""#, fill(2.0, 2.0, 20.0, 32.0)),
        zone("zi", "", fill(x0, 13.0, x0 + 4.0, 17.0))
    );
    BoardModel::from_json(&s).unwrap()
}

/// [planes] join_islands (user 2026-11-12): a pad-less island no via reaches is joined to the
/// nearest connected pad of its net by a track ending inside it; off, it is only reported.
#[test]
fn a_pad_less_island_is_joined_to_a_connected_pad() {
    let m = island_model(22.0);
    let s = stitch(&m, &Options::default(), &reg(), &[], &[], None);
    assert_eq!((s.before, s.after, s.island_vias, s.pad_stubs, s.island_joins), (1, 0, 0, 0, 1), "{:?}", s.notes);
    let Item::Track(t) = &s.items[0] else { panic!("{:?}", s.items) };
    let (a, b) = (t.prim.start(), t.prim.end());
    assert_eq!((t.net, t.layer, (a.x, a.y)), (1, 0, (19_000_000, 15_000_000)), "from the pad centre");
    let (bx, by) = (b.x as i64, b.y as i64);
    assert!(bx > 22 * MM && bx < 23 * MM && (by - 15 * MM).abs() < 600_000, "just inside the island, straight across: {b:?}");
    assert!(!s.notes.iter().any(|n| n.starts_with("long island join")), "{:?}", s.notes);
    // The piece reads as where it is, not "near (0, 0)".
    let cut = s.notes.iter().find(|n| n.starts_with("pour piece cut off")).unwrap();
    let at: Vec<f64> = cut.rsplit("near (").next().unwrap().trim_end_matches(')').split(", ").map(|v| v.parse().unwrap()).collect();
    assert!((at[0] - 24.0).abs() < 0.2 && (at[1] - 15.0).abs() < 0.2, "{cut}");
    let off = stitch(&m, &Options { plane_join_islands: false, ..Options::default() }, &reg(), &[], &[], None);
    assert_eq!((off.after, off.items.len()), (1, 0), "{:?}", off.notes);
}

/// The join has no length cutoff: an island farther than the 3 mm scale is still joined (the
/// price grows with the square of the distance, so the nearest site wins) and a warning says so.
#[test]
fn a_far_island_is_joined_with_a_warning() {
    let m = island_model(25.0);
    let s = stitch(&m, &Options::default(), &reg(), &[], &[], None);
    assert_eq!((s.after, s.island_joins), (0, 1), "{:?}", s.notes);
    assert!(s.notes.iter().any(|n| n.starts_with("long island join U1.1")), "{:?}", s.notes);
    use topor_engine::topo::stitch::join_price;
    assert_eq!(join_price(4e6, 0.0), 4e6);
    assert!((join_price(4e6, 6e6) - 20e6).abs() < 1.0, "twice the scale: five times the length");
    assert!(join_price(4e6, 3.1e6) > join_price(4e6, 3.0e6), "graded, no step");
}
