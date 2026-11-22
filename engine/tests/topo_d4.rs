//! D4 tests (pour stitching, own-net junctions).

use topor_engine::geom::quant::{IVec2, TrackPrim};
use topor_engine::model::{BoardModel, Item, Options, TrackItem};
use topor_engine::pourcheck::check_pour_reach;
use topor_engine::topo::factors::{standard_factors, FactorConfig, Registry};
use topor_engine::topo::stitch::{lattice_pitch, stitch, want_vias};

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
    // The straight 5 mm lattice this test pins (the default is staggered since 2026-11-22).
    let o = Options { stitch_lattice: "straight".into(), ..Options::default() };
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    // stitch-vias: at least 2, one per pitch² of the piece (131.6 mm² / 25 mm² -> 6); round 5:
    // capped at the legal lattice points of the piece (the x = 5 column).
    let want = want_vias(before.cuts[0].area, lattice_pitch(&o), o.stitch_min_vias);
    assert_eq!(want, 6);
    assert_eq!((s.before, s.after, s.pad_stubs), (1, 0, 0), "{:?}", s.notes);
    assert!(s.island_vias >= 2 && s.island_vias <= want, "{:?}", s.notes);
    let Item::Via(v) = &s.items[0] else { panic!("{:?}", s.items) };
    assert_eq!(v.net, 1);
    // All on the 5 mm lattice from the grid origin (0, 0), left of the cut.
    for it in &s.items {
        let Item::Via(v) = it else { panic!("{it:?}") };
        assert!(v.at.x % (5 * MM) == 0 && v.at.y % (5 * MM) == 0 && v.at.x < 7 * MM, "{:?}", v.at);
    }
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
    // Round 5: the 3 legal points of the staggered lattice (rows y = 10, 20, 30 at x = 5; the
    // shifted rows fall at x = 2.5 / 7.5, outside the site area), never a raster point.
    assert_eq!((s.before, s.after, s.island_vias), (1, 0, 3), "{:?}", s.notes);
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

/// The join's island end keeps the D4 gap to the net's other copper (own_copper_ok, the sliver
/// rule of stitch vias): a GND via stands in the island right where the shortest join would end;
/// the join ends clear of it (or would graze it into a sliver KiCad flags).
#[test]
fn an_island_join_ends_clear_of_own_net_vias() {
    use topor_engine::model::{Pt, ViaItem};
    let m = island_model(22.0);
    let via = Item::Via(ViaItem { kind: "via".into(), net: 1, at: Pt { x: 22_600_000, y: 15_000_000 }, d: 600_000, drill: 300_000, layers: vec![], group: None, wire: None });
    let s = stitch(&m, &Options::default(), &reg(), &[via], &[], None);
    assert_eq!(s.island_joins, 1, "{:?}", s.notes);
    let Item::Track(t) = &s.items[0] else { panic!("{:?}", s.items) };
    let b = t.prim.end();
    let d = ((b.x as f64 - 22.6e6).powi(2) + (b.y as f64 - 15e6).powi(2)).sqrt();
    assert!(d >= 0.3e6 + 0.125e6, "the join end keeps off the via: {b:?}, {:.3} mm", d / 1e6);
}

/// GND (net 1) poured on F.Cu and B.Cu; GND SMD pad C1.1 (F.Cu, 0.6 mm round) at (10, 15) mm, boxed in
/// on F.Cu by four net-2 tracks: a pocket left -0.595 / right +0.725 mm of the pad centre, from 2 mm
/// above it to 1.2 mm below. Its F.Cu piece is cut off and too narrow for an island via; a pad stub
/// to a 0.6 mm via in the B.Cu fill fits only with the via centre 0.03..0.10 mm right of the pad
/// centre (side clearances) and about 0.6 mm or more above it (off the pad), a window between the
/// coarse lattice's directions (90 degrees: x 0; 78.75 degrees: x >= 0.137 at its first legal ring). THT GND pad U3.1 at (5, 15) holds the main fill.
fn boxed_model() -> BoardModel {
    BoardModel::from_json(BOXED).unwrap()
}

const BOXED: &str = r#"{ "layers": [ { "id": 0, "name": "F.Cu" }, { "id": 31, "name": "B.Cu" } ],
          "nets": [ { "code": 1, "name": "GND" }, { "code": 2, "name": "SIG" } ],
          "outline": [ { "kind": "seg", "a": [0, 0], "b": [20000000, 0] },
            { "kind": "seg", "a": [20000000, 0], "b": [20000000, 30000000] },
            { "kind": "seg", "a": [20000000, 30000000], "b": [0, 30000000] },
            { "kind": "seg", "a": [0, 30000000], "b": [0, 0] } ],
          "pads": [ { "uuid": "C1", "ref": "C1", "number": "1", "net": 1, "attr": "smd", "center": [10000000, 15000000], "layers": [0], "shapes": { "0": { "kind": "circle", "d": 600000 } } },
                    { "uuid": "U3", "ref": "U3", "number": "1", "net": 1, "attr": "pth", "center": [5000000, 15000000], "layers": [0, 31], "shapes": { "0": { "kind": "circle", "d": 1000000 }, "31": { "kind": "circle", "d": 1000000 } } } ],
          "pours": [ { "uuid": "zz", "net": 1, "layers": [0, 31], "pads": ["C1.1", "U3.1"],
            "vias": [], "clearance": 200000, "min_thickness": 254000, "thermal_gap": 500000,
            "spoke_width": 500000, "pad_connection": 2,
            "fills": [ { "layer": 0, "outer": [[2000000, 2000000], [18000000, 2000000], [18000000, 28000000], [2000000, 28000000]] },
                       { "layer": 31, "outer": [[2000000, 2000000], [18000000, 2000000], [18000000, 28000000], [2000000, 28000000]] } ] } ],
          "rules": { "default": { "clearance": 200000, "width": 250000 }, "min_clearance": 0 } }"#;

fn boxed_items() -> Vec<Item> {
    let (l, r, t, b) = (10 * MM - 595_000, 10 * MM + 725_000, 13 * MM, 16_200_000);
    vec![seg(2, 0, (l, t), (r, t)), seg(2, 0, (r, t), (r, b)), seg(2, 0, (r, b), (l, b)), seg(2, 0, (l, b), (l, t))]
}

/// radii-m3-clear r2 (qmtech erbs C27.1): the pad stub's fine lattice finds the narrow site the
/// coarse one steps over; the coarse lattice alone leaves the island.
#[test]
fn a_boxed_pad_gets_its_stub_from_the_fine_lattice() {
    use topor_engine::topo::stitch::{FINE_POINTS_MAX, STUB_LATTICES};
    let m = boxed_model();
    let items = boxed_items();
    let before = check_pour_reach(&m, &items, &[]);
    assert_eq!(before.cuts.len(), 1, "{:?}", before.notes);
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    assert_eq!((s.before, s.after, s.island_vias, s.pad_stubs), (1, 0, 0, 1), "{:?}", s.notes);
    assert_eq!((s.fine_cuts, s.fine_stubs), (1, 1), "{:?}", s.notes);
    assert!(s.fine_points > 0 && s.fine_points <= FINE_POINTS_MAX + STUB_LATTICES[1].1);
    let v = s.items.iter().find_map(|it| if let Item::Via(v) = it { Some(v) } else { None }).expect("a stub via");
    let dx = (v.at.x as i64 - 10 * MM) as f64 / 1e6;
    assert!((0.02..=0.11).contains(&dx) && v.at.y < 15 * MM - 600_000, "{:?} dx {dx}", v.at);
    assert!(s.notes.iter().any(|n| n.starts_with("fine stub lattice: 1 cut piece(s) searched, 1 stub(s) found")), "{:?}", s.notes);
    let mut all = items.clone();
    all.extend(s.items.iter().cloned());
    assert!(check_pour_reach(&m, &all, &[]).cuts.is_empty());
}

/// The fine lattice runs only after the coarse passes are over: a second cut-off GND pad C2.1 in a
/// roomy net-2 box (an island via fits) is fixed before the boxed C1.1 gets its fine stub, whichever
/// of the two pieces comes first (C2.1 below and above C1.1).
#[test]
fn the_fine_lattice_waits_for_the_coarse_passes() {
    let u3 = r#"{ "uuid": "U3", "ref": "U3""#;
    assert_eq!(BOXED.matches(u3).count(), 1);
    let mut seen = Vec::new();
    for y in [22_000_000i64, 8_000_000] {
        let c2 = format!(r#"{{ "uuid": "C2", "ref": "C2", "number": "1", "net": 1, "attr": "smd", "center": [14000000, {y}], "layers": [0], "shapes": {{ "0": {{ "kind": "circle", "d": 600000 }} }} }}"#);
        let js = BOXED.replacen(u3, &format!("{c2},\n                    {u3}"), 1).replacen(r#""pads": ["C1.1", "U3.1"]"#, r#""pads": ["C1.1", "U3.1", "C2.1"]"#, 1);
        let m = BoardModel::from_json(&js).unwrap_or_else(|e| panic!("{e:?}\n{js}"));
        assert_eq!(m.pads.len(), 3);
        let (l, r, t, b) = (12_500_000, 15_500_000, y - 1_500_000, y + 1_500_000);
        let mut items = boxed_items();
        items.extend([seg(2, 0, (l, t), (r, t)), seg(2, 0, (r, t), (r, b)), seg(2, 0, (r, b), (l, b)), seg(2, 0, (l, b), (l, t))]);
        let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
        assert_eq!((s.before, s.after, s.fine_stubs), (2, 0, 1), "{:?}", s.notes);
        let at = |pad: &str| s.notes.iter().position(|n| (n.starts_with("stitch via") || n.starts_with("pad stub")) && n.contains(pad)).unwrap_or_else(|| panic!("{pad}: {:?}", s.notes));
        assert!(at("C2.1") < at("pad stub C1.1"), "C2.1 at y {y}: {:?}", s.notes);
        let cuts = check_pour_reach(&m, &items, &[]).cuts;
        seen.push(cuts.iter().position(|c| c.pads.iter().any(|&p| m.pads[p].ref_ == "C1")).unwrap());
    }
    assert!(seen.contains(&0), "C1.1's piece comes first in one of the two layouts: {seen:?}");
}

/// A stub the coarse lattice finds is the one it found before the fine lattice existed: the fine
/// pass does not run (the D4 fixture's island via and a roomy boxed pad).
#[test]
fn a_coarse_stub_never_reaches_the_fine_lattice() {
    let m = model();
    let items = [seg(2, 31, (7 * MM, 1_000_000), (7 * MM, 33_000_000))];
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    assert_eq!((s.fine_cuts, s.fine_points, s.fine_stubs), (0, 0, 0), "{:?}", s.notes);
    assert!(!s.notes.iter().any(|n| n.starts_with("fine stub lattice")));
}
