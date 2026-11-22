#![allow(unused_imports, dead_code)]
//! stitch-vias adversarial tests (verifier r1, kept):  the region stitching after routing (classes 2-4, `stitch_regions`) and the
//! lattice / spread rules shared with the cut-off repair (class 1). docs/dev/design/stitch-vias.md.

use topor_engine::geom::quant::{IVec2, TrackPrim};
use topor_engine::model::{BoardModel, Item, MaskOpening, Options, PourFill, Pt, TrackItem};
use topor_engine::pourcheck::check_pour_reach;
use topor_engine::topo::factors::{standard_factors, FactorConfig, Registry};
use topor_engine::topo::stitch::{stitch, stitch_regions};

const MM: i64 = 1_000_000;

/// 40 x 34 mm board, GND (net 1) poured on F.Cu (0) and B.Cu (31) over [2, 32] x [2, 32] mm,
/// "remove islands: always"; THT GND pad U3.1 at (20, 8) joins the layers; signal net 2.
/// `extra_pads`: more pad JSON objects.
fn model(extra_pads: &[String]) -> BoardModel {
    model_with(extra_pads, false)
}

/// [`model`]; `relief`: U3.1 has a thermal connection and the fills carry KiCad's thermal relief hole around U3.1 (a 2.4 mm square, the pad's
/// 1.5 mm disc + a 0.45 mm gap; the spokes' ends lie in the fill), as a real refill does.
fn model_with(extra_pads: &[String], relief: bool) -> BoardModel {
    let mut pads = vec![r#"{ "uuid": "U3", "ref": "U3", "number": "1", "net": 1, "attr": "pth", "center": [20000000, 8000000], "layers": [0, 31],
        "shapes": { "0": { "kind": "circle", "d": 1500000 }, "31": { "kind": "circle", "d": 1500000 } } }"#
        .to_string()];
    pads.extend(extra_pads.iter().cloned());
    let hole = if relief { r#", "holes": [[[18800000, 6800000], [21200000, 6800000], [21200000, 9200000], [18800000, 9200000]]]"# } else { "" };
    let fill = |l: u32| format!(r#"{{ "layer": {l}, "outer": [[2000000, 2000000], [32000000, 2000000], [32000000, 32000000], [2000000, 32000000]]{hole} }}"#);
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
          "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [40000000, 0] }},
            {{ "kind": "seg", "a": [40000000, 0], "b": [40000000, 34000000] }},
            {{ "kind": "seg", "a": [40000000, 34000000], "b": [0, 34000000] }},
            {{ "kind": "seg", "a": [0, 34000000], "b": [0, 0] }} ],
          "pads": [ {} ],
          "pours": [ {{ "uuid": "zz", "net": 1, "layers": [0, 31], "pads": ["U3.1"],
            "vias": [], "clearance": 200000, "min_thickness": 254000, "thermal_gap": 500000,
            "spoke_width": 500000, "pad_connection": {}, "island_removal": 0,
            "fills": [ {}, {} ] }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0 }} }}"#,
        pads.join(","),
        if relief { 1 } else { 2 },
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

/// A closed square of signal track on `layer` from (x0, y0) to (x1, y1) mm (KiCad frame).
fn square(layer: u32, x0: i64, y0: i64, x1: i64, y1: i64) -> Vec<Item> {
    let (a, b, c, d) = ((x0 * MM, y0 * MM), (x1 * MM, y0 * MM), (x1 * MM, y1 * MM), (x0 * MM, y1 * MM));
    vec![seg(2, layer, a, b), seg(2, layer, b, c), seg(2, layer, c, d), seg(2, layer, d, a)]
}

fn reg() -> Registry {
    Registry::from_config(standard_factors(), &FactorConfig::default()).0
}

/// Only the region classes, nothing else changed.
fn opts(dropped: bool, thin_mm: f64, area: bool) -> Options {
    Options { stitch_dropped: dropped, stitch_thin_nm: (thin_mm * 1e6) as i64, stitch_area: area, stitch_lattice: "straight".into(), ..Options::default() }
}

fn vias(items: &[Item]) -> Vec<(i64, i64)> {
    items
        .iter()
        .map(|it| match it {
            Item::Via(v) => (v.at.x, v.at.y),
            _ => panic!("not a via: {it:?}"),
        })
        .collect()
}

fn on_lattice(p: (i64, i64), origin: (i64, i64), pitch: i64) -> bool {
    (p.0 - origin.0).rem_euclid(pitch) == 0 && (p.1 - origin.1).rem_euclid(pitch) == 0
}

// ---- verifier adversarial cases (vfy-stitch-vias r1, 2026-11-22): one graded site price, no lattice-first rule ----

/// A 4 x 4 ball grid of SIG (net 2) SMD balls on F.Cu (pitch 1.5 mm, 0.3 mm), centred on (cx, cy) mm (KiCad frame).
fn bga(cx: f64, cy: f64) -> Vec<String> {
    let mut balls = Vec::new();
    for j in 0..4 {
        for i in 0..4 {
            let (x, y) = (((cx - 2.25 + 1.5 * i as f64) * 1e6) as i64, ((cy - 2.25 + 1.5 * j as f64) * 1e6) as i64);
            balls.push(format!(
                r#"{{ "uuid": "B{i}{j}", "ref": "U9", "number": "{}", "net": 2, "attr": "smd", "center": [{x}, {y}], "layers": [0],
                    "shapes": {{ "0": {{ "kind": "circle", "d": 300000 }} }} }}"#,
                j * 4 + i + 1
            ));
        }
    }
    balls
}

fn in_box(p: (i64, i64), x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    let (x, y) = (p.0 as f64 / 1e6, p.1 as f64 / 1e6);
    x > x0 && x < x1 && y > y0 && y < y1
}

/// V1 (power-via rule, user 2026-11-13: under a BGA a via is a last resort): class 1 (cut-off
/// repair). The B.Cu strip left of a foreign track at x = 7 mm holds U1.1 at (4, 15); its only
/// lattice site (5, 25) (pitch 5 x 40 mm, origin (0, 25)) lies between the balls of a BGA, while
/// legal off-lattice sites next to U1.1 exist. Expected: the one via stands outside the BGA.
#[test]
fn v1_class1_via_not_under_a_bga_when_room_outside() {
    let mut extra = bga(5.0, 25.0);
    extra.push(r#"{ "uuid": "U1", "ref": "U1", "number": "1", "net": 1, "attr": "smd", "center": [4000000, 15000000], "layers": [31],
        "shapes": { "31": { "kind": "circle", "d": 1000000 } } }"#.to_string());
    let mut m = model(&extra);
    m.pours[0].pads.push("U1.1".into());
    let items = [seg(2, 31, (7 * MM, 1_000_000), (7 * MM, 33_000_000))];
    let o = Options { grid_origin_nm: [0, 25 * MM], stitch_min_vias: 1, stitch_per_area: false, stitch_pitch_x_nm: 5 * MM, stitch_pitch_y_nm: 40 * MM, stitch_lattice: "straight".into(), ..Options::default() };
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    let v = vias(&s.items.iter().filter(|i| matches!(i, Item::Via(_))).cloned().collect::<Vec<_>>());
    eprintln!("V1 vias {v:?} notes {:?}", s.notes);
    assert_eq!(s.island_vias, 1, "{:?}", s.notes);
    assert!(v.iter().all(|&p| !in_box(p, 2.0, 22.0, 8.0, 28.0)), "class-1 via under the BGA: {v:?}");
}

/// V1b: the same strip without the BGA. Main placed the class-1 via nearest U1.1 (4, 15); with the
/// lattice first it goes to the one lattice site 10 mm away. Reported, not a pass/fail rule.
#[test]
fn v1b_class1_first_via_distance_to_pad() {
    let mut m = model(&[r#"{ "uuid": "U1", "ref": "U1", "number": "1", "net": 1, "attr": "smd", "center": [4000000, 15000000], "layers": [31],
        "shapes": { "31": { "kind": "circle", "d": 1000000 } } }"#.to_string()]);
    m.pours[0].pads.push("U1.1".into());
    let items = [seg(2, 31, (7 * MM, 1_000_000), (7 * MM, 33_000_000))];
    let o = Options { grid_origin_nm: [0, 25 * MM], stitch_min_vias: 1, stitch_per_area: false, stitch_pitch_x_nm: 5 * MM, stitch_pitch_y_nm: 40 * MM, stitch_lattice: "straight".into(), ..Options::default() };
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    let v = vias(&s.items.iter().filter(|i| matches!(i, Item::Via(_))).cloned().collect::<Vec<_>>());
    eprintln!("V1b vias {v:?}");
    assert_eq!(v.len(), 1);
}

/// V2 (classes 2-4, graded): a dropped island [8, 22] x [12, 26] mm whose only lattice site
/// (15, 20) (pitch 14 x 14 mm, origin (15, 20)) lies under a BGA; legal off-lattice sites outside
/// the BGA exist. Expected: the island still gets a via (it stays) and none under the BGA.
#[test]
fn v2_dropped_island_with_its_lattice_site_under_a_bga_still_stays() {
    let m0 = model(&bga(15.0, 20.0));
    let m = m0;
    let items = square(0, 8, 12, 22, 26);
    let o = Options { grid_origin_nm: [15 * MM, 20 * MM], stitch_pitch_x_nm: 14 * MM, stitch_pitch_y_nm: 14 * MM, ..opts(true, 0.0, false) };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    eprintln!("V2 vias {v:?} notes {:?}", s.notes);
    assert!(v.iter().all(|&p| !in_box(p, 12.0, 17.0, 18.0, 23.0)), "{v:?}");
    assert!(!v.is_empty(), "island dropped although legal sites outside the BGA exist: {:?}", s.notes);
}


/// V2 control: the same island and BGA with the lattice moved so no lattice site fits the island
/// (origin x = 7.5 mm, pitch 14: columns 7.5 / 21.5 are too near the island's edges): only fallback
/// sites, and the island gets its vias outside the BGA. So V2's 0 vias come from the lattice-first rule.
#[test]
fn v2c_control_fallback_sites_outside_the_bga_are_legal() {
    let m = model(&bga(15.0, 20.0));
    let items = square(0, 8, 12, 22, 26);
    let o = Options { grid_origin_nm: [7_500_000, 0], stitch_pitch_x_nm: 14 * MM, stitch_pitch_y_nm: 14 * MM, ..opts(true, 0.0, false) };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    eprintln!("V2c vias {v:?} notes {:?}", s.notes);
    assert!(!v.is_empty() && v.iter().all(|&p| !in_box(p, 12.0, 17.0, 18.0, 23.0)), "{v:?}");
}

// ---- verifier r2 (vfy-stitch-vias 2026-11-22): the two r4 mutant survivors ----

/// priced_out_kept: a dropped island lying wholly under a BGA (balls 1.5 mm pitch around (15, 20);
/// the island [12, 18] x [17, 23] is inside the ball box grown by half a pitch): every site
/// costs 100 via prices, above the value of a via, so classes 2-4 give it no via (it stays
/// dropped). A legal site between the balls exists (no_region_via_under_a_bga), so only the
/// value test keeps it empty.
#[test]
fn vfy_dropped_island_wholly_under_a_bga_gets_no_via() {
    let m = model(&bga(15.0, 20.0));
    let items = square(0, 12, 17, 18, 23);
    let o = Options { stitch_min_vias: 2, ..opts(true, 0.0, false) };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    eprintln!("bga-island vias {v:?} notes {:?}", s.notes);
    assert!(s.dropped[0] >= 1, "{:?}", s.notes);
    assert!(v.is_empty(), "via under a BGA in a dropped island: {v:?}");
}

/// k_always: class 1 (cut-off repair). A foreign B.Cu track (4,1)-(4,20)-(15,20)-(15,33) cuts off a
/// piece: a narrow strip x in [2, 3.675] below y = 20 and a wide part x in [2, 14.675] above it.
/// U1.1 (B.Cu, GND) at (2.85, 10) sits in the strip. Via grid 2 x 0.5 mm from (0, 0): the strip's
/// grid columns x = 2 / 4 lie on its edges, so no grid point of the strip is legal; the strip's
/// raster sites near the pad pay no k·d (no legal grid corner), only the wide part's sites
/// stand on the grid. r2 expected the via near its pad, in the strip, off the grid; round 5 (user
/// 2026-11-22: off the via grid NEVER): the via stands on the nearest legal grid point of the
/// piece, in its wide part.
#[test]
fn vfy_class1_via_stays_on_the_grid_when_no_grid_point_near_its_pad_is_legal() {
    let mut m = model(&[r#"{ "uuid": "U1", "ref": "U1", "number": "1", "net": 1, "attr": "smd", "center": [2850000, 10000000], "layers": [31],
        "shapes": { "31": { "kind": "circle", "d": 1000000 } } }"#.to_string()]);
    m.pours[0].pads.push("U1.1".into());
    let items = [
        seg(2, 31, (4 * MM, MM), (4 * MM, 20 * MM)),
        seg(2, 31, (4 * MM, 20 * MM), (15 * MM, 20 * MM)),
        seg(2, 31, (15 * MM, 20 * MM), (15 * MM, 33 * MM)),
    ];
    let o = Options {
        grid_via_nm: [2 * MM, MM / 2],
        grid_origin_nm: [0, 0],
        stitch_min_vias: 1,
        stitch_per_area: false,
        stitch_pitch_x_nm: 4 * MM,
        stitch_pitch_y_nm: 5 * MM,
        stitch_lattice: "straight".into(),
        ..Options::default()
    };
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    let v = vias(&s.items.iter().filter(|i| matches!(i, Item::Via(_))).cloned().collect::<Vec<_>>());
    eprintln!("k vias {v:?} notes {:?}", s.notes);
    assert_eq!(v.len(), 1, "{:?}", s.notes);
    assert!(v[0].0 % (2 * MM) == 0 && v[0].1 % (MM / 2) == 0, "class-1 via off the via grid: {v:?}");
    assert!(v[0].1 > 20 * MM, "no grid point of the strip is legal: {v:?}");
    // r5c (user: connection copper goes off the grid where it does not fit, as in KiCad): with
    // grid columns only at x = 0 / 20 mm no grid point of the piece is legal; its first via, its
    // only connection, stands off the grid near its pad (graded off_grid price), named in a warning.
    let o2 = Options { grid_via_nm: [20 * MM, MM / 2], ..o.clone() };
    let s = stitch(&m, &o2, &reg(), &items, &[], None);
    let v = vias(&s.items.iter().filter(|i| matches!(i, Item::Via(_))).cloned().collect::<Vec<_>>());
    assert_eq!(v.len(), 1, "{:?}", s.notes);
    assert!(v[0].1 < 20 * MM && v[0].0 < 4 * MM, "the off-grid connection via near its pad: {v:?}");
    assert!(s.notes.iter().any(|n| n.starts_with("stitch via off the via grid at ") && n.contains("pads [U1.1]") && n.contains("the piece's only connection")), "{:?}", s.notes);
}
