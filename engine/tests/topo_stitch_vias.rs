//! stitch-vias tests: the region stitching after routing (classes 2-4, `stitch_regions`) and the
//! lattice / spread rules shared with the cut-off repair (class 1). docs/dev/design/stitch-vias.md.

use topor_engine::geom::quant::{IVec2, TrackPrim};
use topor_engine::model::{BoardModel, Item, Options, TrackItem};
use topor_engine::pourcheck::check_pour_reach;
use topor_engine::topo::factors::{standard_factors, FactorConfig, Registry};
use topor_engine::topo::stitch::{lattice_pitch, stitch, stitch_regions, want_vias};

const MM: i64 = 1_000_000;

/// 40 x 34 mm board, GND (net 1) poured on F.Cu (0) and B.Cu (31) over [2, 32] x [2, 32] mm,
/// "remove islands: always"; THT GND pad U3.1 at (20, 8) joins the layers; signal net 2.
/// `extra_pads`: more pad JSON objects.
fn model(extra_pads: &[String]) -> BoardModel {
    let mut pads = vec![r#"{ "uuid": "U3", "ref": "U3", "number": "1", "net": 1, "attr": "pth", "center": [20000000, 8000000], "layers": [0, 31],
        "shapes": { "0": { "kind": "circle", "d": 1500000 }, "31": { "kind": "circle", "d": 1500000 } } }"#
        .to_string()];
    pads.extend(extra_pads.iter().cloned());
    let fill = |l: u32| format!(r#"{{ "layer": {l}, "outer": [[2000000, 2000000], [32000000, 2000000], [32000000, 32000000], [2000000, 32000000]] }}"#);
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
            "spoke_width": 500000, "pad_connection": 2, "island_removal": 0,
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
    Options { stitch_dropped: dropped, stitch_thin_nm: (thin_mm * 1e6) as i64, stitch_area: area, ..Options::default() }
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

/// Class 2: a closed F.Cu signal square [8, 22] x [12, 26] mm leaves a pad-less F.Cu island that
/// KiCad's island removal ("always") would drop. It gets want_vias(area) vias, spread, inside
/// it, on the 5 mm lattice where a lattice site is legal; afterwards it is in the main
/// component. Off, nothing.
#[test]
fn a_dropped_island_gets_spread_lattice_vias_and_stays() {
    let m = model(&[]);
    let items = square(0, 8, 12, 22, 26);
    let r0 = check_pour_reach(&m, &items, &[]);
    assert!(r0.cuts.is_empty(), "{:?}", r0.notes);
    let d: Vec<_> = r0.dropped.iter().filter(|d| d.island.1 == 0).collect();
    assert_eq!(d.len(), 1, "{:?}", r0.dropped);
    let o = opts(true, 0.0, false);
    let want = want_vias(d[0].area, lattice_pitch(&o), o.stitch_min_vias);
    assert!(want >= 7, "{want} {}", d[0].area);
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert_eq!((s.dropped[0], s.dropped[1], v.len()), (1, 1, want), "{:?}", s.notes);
    for p in &v {
        assert!(p.0 > 8 * MM && p.0 < 22 * MM && p.1 > 12 * MM && p.1 < 26 * MM, "{p:?}");
    }
    // The lattice points inside (10..20 x 15..25, 9 of them) all fit: every via is on it.
    assert!(v.iter().all(|&p| on_lattice(p, (0, 0), 5 * MM)), "{v:?}");
    // Spread: the first two are the farthest pair the farthest-point rule gives (corners).
    let dist = |a: (i64, i64), b: (i64, i64)| (((a.0 - b.0) as f64).powi(2) + ((a.1 - b.1) as f64).powi(2)).sqrt();
    assert!(dist(v[0], v[1]) >= 5.0 * MM as f64 - 1.0, "{v:?}");
    let mut all = items.clone();
    all.extend(s.items.iter().cloned());
    let r1 = check_pour_reach(&m, &all, &[]);
    assert!(r1.dropped.iter().all(|d| d.island.1 != 0), "{:?}", r1.dropped);
    // The summary line plugin/topor/postrun.py parses (STITCH_VIAS; tests/plugin/test_postrun.py).
    assert!(s.notes[0].starts_with("stitching vias (pitch 5.00 mm from the grid origin, at least 2 per piece): dropped islands 1 of 1 stitched"), "{}", s.notes[0]);
    assert!(s.notes[0].ends_with(&format!("; {want} vias in all")), "{}", s.notes[0]);
    assert!(s.notes[1].starts_with("stitch (dropped island): GND on F.Cu "), "{}", s.notes[1]);
    let off = stitch_regions(&m, &opts(false, 0.0, false), &reg(), &items, &[], None, &|| false);
    assert!(off.items.is_empty(), "{:?}", off.notes);
}

/// Never dangling: with B.Cu closed under the same square too, the inner pieces on both layers
/// are dropped and no via finds solid main pour on another layer: no via.
#[test]
fn no_via_without_solid_pour_on_another_layer() {
    let m = model(&[]);
    let mut items = square(0, 8, 12, 22, 26);
    items.extend(square(31, 7, 11, 23, 27));
    let s = stitch_regions(&m, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    assert!(s.items.is_empty(), "{:?}", s.notes);
    assert_eq!(s.dropped[1], 0);
}

/// The lattice starts at the board's grid origin.
#[test]
fn the_lattice_follows_the_grid_origin() {
    let mut m = model(&[]);
    // KiCad frame (1, 2) mm; the model holds the math frame.
    m.grid_origin = [MM, -2 * MM];
    let items = square(0, 8, 12, 22, 26);
    let s = stitch_regions(&m, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    // The 6 lattice points inside (x 11, 16, 21 x y 17, 22) come first; the 7th via the area
    // wants is a fallback site off the lattice.
    assert_eq!(v.len(), 7, "{:?}", s.notes);
    assert!(v[..6].iter().all(|&p| on_lattice(p, (MM, 2 * MM), 5 * MM)), "{v:?}");
    assert!(!on_lattice(v[6], (MM, 2 * MM), 5 * MM), "{v:?}");
}

/// Small dropped islands (under pitch²/4) are not worth a via.
#[test]
fn a_small_dropped_island_gets_no_via() {
    let m = model(&[]);
    let items = square(0, 10, 14, 12, 16);
    let s = stitch_regions(&m, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    assert!(s.items.is_empty(), "{:?}", s.notes);
    assert_eq!(s.dropped[0], 0);
}

/// Class 3: two F.Cu signal tracks across the pour at y = 20 mm leave a 0.75 mm neck of fill
/// between their ends (x 16 .. 17.4 mm). The part above (y 20..32) holds no via or THT pad of
/// GND: it gets vias; the part below holds U3.1 and gets none. With stitch_thin_mm = 0 nothing.
#[test]
fn a_part_behind_a_thin_neck_gets_vias() {
    let m = model(&[]);
    let items = vec![seg(2, 0, (MM, 20 * MM), (16 * MM, 20 * MM)), seg(2, 0, (17_400_000, 20 * MM), (33 * MM, 20 * MM))];
    let r0 = check_pour_reach(&m, &items, &[]);
    assert!(r0.cuts.is_empty() && r0.dropped.iter().all(|d| d.island.1 != 0), "{:?}", r0.notes);
    let s = stitch_regions(&m, &opts(false, 1.0, false), &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert_eq!(s.thin[1], 1, "{:?}", s.notes);
    assert!(v.len() >= 2, "{:?}", s.notes);
    assert!(v.iter().all(|p| p.1 > 20 * MM), "{v:?}");
    let off = stitch_regions(&m, &opts(false, 0.0, false), &reg(), &items, &[], None, &|| false);
    assert!(off.items.is_empty());
    // A neck wider than the threshold (3 mm gap): no thin part.
    let wide = vec![seg(2, 0, (MM, 20 * MM), (16 * MM, 20 * MM)), seg(2, 0, (19 * MM, 20 * MM), (33 * MM, 20 * MM))];
    let s = stitch_regions(&m, &opts(false, 1.0, false), &reg(), &wide, &[], None, &|| false);
    assert!(s.items.is_empty(), "{:?}", s.notes);
}

/// Class 4 (option): area stitching puts a via on lattice sites at least one pitch from every
/// via of the net (U3.1 counts), none closer than the pitch to each other.
#[test]
fn area_stitching_fills_the_lattice_at_the_pitch() {
    let m = model(&[]);
    let s = stitch_regions(&m, &opts(false, 0.0, true), &reg(), &[], &[], None, &|| false);
    let v = vias(&s.items);
    assert!(v.len() >= 10, "{:?}", s.notes);
    assert!(v.iter().all(|&p| on_lattice(p, (0, 0), 5 * MM)));
    for (i, a) in v.iter().enumerate() {
        assert!((((a.0 - 20 * MM) as f64).hypot((a.1 - 8 * MM) as f64)) >= 5.0e6, "{a:?} near U3.1");
        for b in &v[i + 1..] {
            assert!((((a.0 - b.0) as f64).hypot((a.1 - b.1) as f64)) >= 5.0e6 - 1.0, "{a:?} {b:?}");
        }
    }
    assert!(stitch_regions(&m, &Options::default(), &reg(), &[], &[], None, &|| false).items.is_empty());
}

/// Power-via rule (user 2026-11-13): no stitching via of classes 2-4 under a BGA. A 4 x 4 ball
/// grid of a signal net (pitch 1 mm) inside the dropped island: no via in its area.
#[test]
fn no_region_via_under_a_bga() {
    let mut balls = Vec::new();
    for j in 0..4 {
        for i in 0..4 {
            let (x, y) = (14 * MM + i * MM, 18 * MM + j * MM);
            balls.push(format!(
                r#"{{ "uuid": "B{i}{j}", "ref": "U9", "number": "{}", "net": 2, "attr": "smd", "center": [{x}, {y}], "layers": [0],
                    "shapes": {{ "0": {{ "kind": "circle", "d": 400000 }} }} }}"#,
                j * 4 + i + 1
            ));
        }
    }
    let m = model(&balls);
    let items = square(0, 8, 12, 22, 26);
    let s = stitch_regions(&m, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert!(!v.is_empty(), "{:?}", s.notes);
    // BGA area: ball box grown by half a pitch.
    assert!(v.iter().all(|p| !(p.0 > 13_500_000 && p.0 < 17_500_000 && p.1 > 17_500_000 && p.1 < 21_500_000)), "{v:?}");
}

/// Class 1 (cut-off repair) spreads its vias: the second via is the lattice site farthest from
/// the first. B.Cu cut by a foreign track at x = 7 mm; the strip left of it holds the SMD pad
/// U1.1 on B.Cu at (4, 15).
#[test]
fn cut_off_repair_spreads_its_vias() {
    let m = model(&[r#"{ "uuid": "U1", "ref": "U1", "number": "1", "net": 1, "attr": "smd", "center": [4000000, 15000000], "layers": [31],
        "shapes": { "31": { "kind": "circle", "d": 1000000 } } }"#
        .to_string()]);
    let mut m = m;
    m.pours[0].pads.push("U1.1".into());
    let items = [seg(2, 31, (7 * MM, 1_000_000), (7 * MM, 33_000_000))];
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    let v = vias(&s.items);
    assert!(v.len() >= 2, "{:?}", s.notes);
    assert_eq!(v[0], (5 * MM, 15 * MM), "{v:?}");
    assert_eq!(v[1], (5 * MM, 30 * MM), "{v:?}");
    // stitch_min_vias = 1 and a pitch larger than the strip: one via, as before stitch-vias.
    let o = Options { stitch_min_vias: 1, stitch_pitch_nm: 40 * MM, ..Options::default() };
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    assert_eq!(s.island_vias, 1, "{:?}", s.notes);
}
