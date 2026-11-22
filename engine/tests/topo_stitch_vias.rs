//! stitch-vias tests: the region stitching after routing (classes 2-4, `stitch_regions`) and the
//! lattice / spread rules shared with the cut-off repair (class 1). docs/dev/design/stitch-vias.md.

use topor_engine::geom::quant::{IVec2, TrackPrim};
use topor_engine::model::{BoardModel, Item, MaskOpening, Options, PourFill, Pt, TrackItem};
use topor_engine::pourcheck::check_pour_reach;
use topor_engine::topo::factors::{standard_factors, FactorConfig, Registry};
use topor_engine::topo::stitch::{lattice_shift, lattice_xy, lattice_pitch, stitch, stitch_regions, want_vias};

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
    // The fixtures pin straight-lattice positions; the staggered default has its own test.
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

/// Within `tol` of a lattice point: one graded site price (round 4, verifier V1/V2) lets a
/// fallback site a little inside win over a lattice point squeezed at the fill edge.
fn near_lattice(p: (i64, i64), origin: (i64, i64), pitch: i64, tol: i64) -> bool {
    let off = |v: i64, o: i64| {
        let r = (v - o).rem_euclid(pitch);
        r.min(pitch - r)
    };
    (off(p.0, origin.0) as f64).hypot(off(p.1, origin.1) as f64) <= tol as f64
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
    // The lattice points inside (10..20 x 15..25, 9 of them) all fit: every via is on it, or
    // (a lattice point squeezed at the fill edge) less than 0.5 mm inside it.
    assert!(v.iter().all(|&p| near_lattice(p, (0, 0), 5 * MM, MM / 2)), "{v:?}");
    assert!(v.iter().filter(|&&p| on_lattice(p, (0, 0), 5 * MM)).count() >= 5, "{v:?}");
    // Spread: the first two are the farthest pair the farthest-point rule gives (corners).
    let dist = |a: (i64, i64), b: (i64, i64)| (((a.0 - b.0) as f64).powi(2) + ((a.1 - b.1) as f64).powi(2)).sqrt();
    assert!(dist(v[0], v[1]) >= 5.0 * MM as f64 - 1.0, "{v:?}");
    let mut all = items.clone();
    all.extend(s.items.iter().cloned());
    let r1 = check_pour_reach(&m, &all, &[]);
    assert!(r1.dropped.iter().all(|d| d.island.1 != 0), "{:?}", r1.dropped);
    // The summary line plugin/topor/postrun.py parses (STITCH_VIAS; tests/plugin/test_postrun.py).
    assert!(s.notes[0].starts_with("stitching vias (lattice 5.00 x 5.00 mm straight from the grid origin, at least 2 per piece): dropped islands 1 of 1 stitched"), "{}", s.notes[0]);
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
    let m = model(&[]);
    // KiCad frame (1, 2) mm (Options::grid_origin_nm, the one origin source with grid-snap).
    let o = Options { grid_origin_nm: [MM, 2 * MM], ..opts(true, 0.0, false) };
    let items = square(0, 8, 12, 22, 26);
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    // The 6 lattice points inside (x 11, 16, 21 x y 17, 22); the 7th via the area wants is a
    // fallback site off the lattice.
    assert_eq!(v.len(), 7, "{:?}", s.notes);
    assert_eq!(v.iter().filter(|&&p| near_lattice(p, (MM, 2 * MM), 5 * MM, MM / 2)).count(), 6, "{v:?}");
    assert!(v.iter().filter(|&&p| on_lattice(p, (MM, 2 * MM), 5 * MM)).count() >= 4, "{v:?}");
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
    // The same with the thermal relief hole around U3.1 in the fill (as KiCad fills it): U3.1 still
    // anchors the lower part across its thermal gap; only the upper part gets vias.
    let mr = model_with(&[], true);
    let rr = check_pour_reach(&mr, &items, &[]);
    assert!(rr.cuts.is_empty(), "{:?} {:?}", rr.notes, rr.cuts);
    let s = stitch_regions(&mr, &opts(false, 1.0, false), &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert_eq!((s.thin[0], s.thin[1]), (1, 1), "{:?}", s.notes);
    assert!(!v.is_empty() && v.iter().all(|p| p.1 > 20 * MM), "{v:?}");
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
/// grid of a signal net (pitch 1.5 mm, 0.3 mm balls) inside the dropped island leaves room for a
/// legal via on the lattice site (15, 20) between four balls (priced only 4 x 0.15 mm for the
/// near pads); at least 9 vias are wanted, so the run reaches that site and only the BGA price
/// (100 via prices, above the value of a via) keeps it empty.
#[test]
fn no_region_via_under_a_bga() {
    let mut balls = Vec::new();
    for j in 0..4 {
        for i in 0..4 {
            let (x, y) = (12_750_000 + i * 1_500_000, 17_750_000 + j * 1_500_000);
            balls.push(format!(
                r#"{{ "uuid": "B{i}{j}", "ref": "U9", "number": "{}", "net": 2, "attr": "smd", "center": [{x}, {y}], "layers": [0],
                    "shapes": {{ "0": {{ "kind": "circle", "d": 300000 }} }} }}"#,
                j * 4 + i + 1
            ));
        }
    }
    let m = model(&balls);
    let items = square(0, 8, 12, 23, 27);
    let o = Options { stitch_min_vias: 9, ..opts(true, 0.0, false) };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert!(v.len() >= 8, "{:?}", s.notes);
    // BGA area: ball box grown by half a pitch.
    assert!(v.iter().all(|p| !(p.0 > 12_000_000 && p.0 < 18_000_000 && p.1 > 17_000_000 && p.1 < 23_000_000)), "{v:?}");
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
    let straight = Options { stitch_lattice: "straight".into(), ..Options::default() };
    let s = stitch(&m, &straight, &reg(), &items, &[], None);
    let v = vias(&s.items);
    assert!(v.len() >= 2, "{:?}", s.notes);
    assert_eq!(v[0], (5 * MM, 15 * MM), "{v:?}");
    assert_eq!(v[1], (5 * MM, 30 * MM), "{v:?}");
    // stitch_min_vias = 1 and a pitch larger than the strip: one via, as before stitch-vias.
    let o = Options { stitch_min_vias: 1, stitch_pitch_x_nm: 40 * MM, stitch_pitch_y_nm: 40 * MM, ..Options::default() };
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    assert_eq!(s.island_vias, 1, "{:?}", s.notes);
}

/// A solder-mask opening on `layer` over [x0, x1] x [y0, y1] mm (KiCad frame), pushed in the
/// model's math frame.
fn mask(m: &mut BoardModel, layer: u32, x0: i64, y0: i64, x1: i64, y1: i64) {
    let poly = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].iter().map(|&(x, y)| Pt { x: x * MM, y: -y * MM }).collect();
    m.mask_openings.push(MaskOpening { uuid: format!("mask{x0}{y0}"), layer, poly, owner_ref: None });
}

/// F1 (KiCad's solder_mask_bridge): an F.Cu mask opening that exposes the square's signal track
/// (x = 22) and part of the dropped island: keeping the island would expose GND next to the
/// signal in one opening, so the island gets no via and stays dropped (a note says why).
#[test]
fn no_stitch_of_an_island_under_a_foreign_mask_opening() {
    let mut m = model(&[]);
    mask(&mut m, 0, 19, 15, 24, 20);
    let items = square(0, 8, 12, 22, 26);
    let s = stitch_regions(&m, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    assert!(vias(&s.items).is_empty(), "{:?}", s.notes);
    assert!(s.notes.iter().any(|n| n.contains("under a solder-mask opening that exposes another net")), "{:?}", s.notes);
    // The same opening without foreign copper in it (track moved out): the island is stitched.
    let mut m2 = model(&[]);
    mask(&mut m2, 0, 14, 15, 19, 20);
    let s2 = stitch_regions(&m2, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    assert!(!vias(&s2.items).is_empty(), "{:?}", s2.notes);
}

/// F1: a B.Cu mask opening [13, 17] x [17, 23] mm that exposes a short signal track on B.Cu: no
/// stitching via inside it (a through via shows on B.Cu too), though the island's first via
/// would otherwise stand at the lattice site (15, 20) nearest its centre.
#[test]
fn no_stitching_via_in_a_foreign_mask_opening() {
    let mut m = model(&[]);
    let mut items = square(0, 8, 12, 22, 26);
    let free = stitch_regions(&m, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    assert!(vias(&free.items).contains(&(15 * MM, 20 * MM)), "{:?}", free.notes);
    mask(&mut m, 31, 13, 17, 17, 23);
    items.push(seg(2, 31, (13_500_000, 22_500_000), (14_000_000, 22_500_000)));
    let s = stitch_regions(&m, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert!(!v.is_empty(), "{:?}", s.notes);
    assert!(v.iter().all(|p| !(p.0 > 12_500_000 && p.0 < 17_500_000 && p.1 > 16_500_000 && p.1 < 23_500_000)), "{v:?}");
}

/// `stitch_per_area = false`: the dropped island of the first test gets `stitch_min_vias` vias,
/// not one per pitch².
#[test]
fn without_area_scaling_a_piece_gets_min_vias() {
    let m = model(&[]);
    let items = square(0, 8, 12, 22, 26);
    let o = Options { stitch_per_area: false, ..opts(true, 0.0, false) };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    assert_eq!(vias(&s.items).len(), 2, "{:?}", s.notes);
}

/// D2: a stitching via's antipad must not leave a neck narrower than `stitch_thin` in another
/// net's pour. A signal-net plane on In1.Cu over [14.5, 30] x [19, 30] mm (KiCad frame): the
/// lattice site (15, 20), the dropped island's first via otherwise, is 0.5 mm inside the plane's
/// edge, so its antipad (via radius + clearance) would leave no copper there: no via at that
/// site; deeper sites in the plane stay usable.
#[test]
fn no_stitching_via_that_leaves_a_thin_neck_in_another_plane() {
    let mut m = model(&[]);
    let mut l = m.layers[0].clone();
    l.id = 4;
    l.name = "In1.Cu".into();
    m.layers.insert(1, l);
    let mut z = m.pours[0].clone();
    z.uuid = "plane".into();
    z.net = 2;
    z.layers = vec![4];
    z.pads.clear();
    let outer = [(14_500_000, 19_000_000), (30_000_000, 19_000_000), (30_000_000, 30_000_000), (14_500_000, 30_000_000)]
        .iter()
        .map(|&(x, y)| Pt { x, y: -y })
        .collect();
    z.fills = vec![PourFill { layer: 4, outer, holes: Vec::new() }];
    m.pours.push(z);
    let items = square(0, 8, 12, 22, 26);
    let s = stitch_regions(&m, &opts(true, 0.0, false), &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert!(!v.is_empty(), "{:?}", s.notes);
    assert!(!v.contains(&(15 * MM, 20 * MM)), "{v:?}");
    assert!(v.iter().any(|&p| near_lattice(p, (20 * MM, 25 * MM), 100 * MM, MM / 2)), "{v:?}");
}

/// r3 (user 2026-11-22: "x space and y space in mm, straight or staggered"): a dropped island
/// [8, 22] x [12, 27] mm with 7 vias wanted. Staggered 5 x 5 mm from the origin: rows y = 15 and 25
/// (odd rows) are shifted by 2.5 mm, so every via stands on {12.5, 17.5} x {15, 25} or
/// {10, 15, 20} x {20}. Straight 4 x 6 mm (6 vias, no area scaling): x in {12, 16, 20}, y in
/// {18, 24}.
#[test]
fn the_lattice_has_x_and_y_spacing_straight_or_staggered() {
    assert_eq!(Options::default().stitch_lattice, "staggered", "default lattice (user 2026-11-22)");
    let m = model(&[]);
    // y to 27 mm: the y = 25 row is not squeezed at the fill edge (round 4: one graded price).
    let items = square(0, 8, 12, 22, 27);
    let o = Options { stitch_min_vias: 7, stitch_per_area: false, stitch_lattice: Options::default().stitch_lattice, ..opts(true, 0.0, false) };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let mut v = vias(&s.items);
    v.sort_unstable();
    let h = MM / 2;
    let mut want = vec![(25 * h, 15 * MM), (35 * h, 15 * MM), (10 * MM, 20 * MM), (15 * MM, 20 * MM), (20 * MM, 20 * MM), (25 * h, 25 * MM), (35 * h, 25 * MM)];
    want.sort_unstable();
    assert_eq!(v, want, "{:?}", s.notes);
    assert!(s.notes[0].starts_with("stitching vias (lattice 5.00 x 5.00 mm staggered"), "{}", s.notes[0]);
    let o = Options { stitch_min_vias: 6, stitch_per_area: false, stitch_pitch_x_nm: 4 * MM, stitch_pitch_y_nm: 6 * MM, ..opts(true, 0.0, false) };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let mut v = vias(&s.items);
    v.sort_unstable();
    let mut want: Vec<(i64, i64)> = [12, 16, 20].iter().flat_map(|&x| [18, 24].map(|y| (x * MM, y * MM))).collect();
    want.sort_unstable();
    assert_eq!(v, want, "{:?}", s.notes);
}

fn on_grid(p: (i64, i64), origin: (i64, i64), step: i64) -> bool {
    (p.0 - origin.0).rem_euclid(step) == 0 && (p.1 - origin.1).rem_euclid(step) == 0
}

/// Round 4 (user 2026-11-22: "the vias should use that as well"): with a via grid of 10 mil
/// (0.254 mm) from the grid origin (1, 2) mm, the 5 mm pitch becomes 5.08 mm (a note names it),
/// the staggered shift 2.54 mm, and every stitching via of a dropped island stands on the grid
/// (lattice sites and the fallback sites both). stitch_on_grid off: the same island gets vias off
/// the grid (5 mm lattice, raster fallback sites).
#[test]
fn stitching_vias_stand_on_the_via_grid() {
    let m = model(&[]);
    let items = square(0, 8, 12, 22, 26);
    let g = 254_000;
    let o = Options { grid_via_nm: [g, g], grid_origin_nm: [MM, 2 * MM], stitch_lattice: "staggered".into(), ..opts(true, 0.0, false) };
    assert_eq!(lattice_xy(&o), (5_080_000.0, 5_080_000.0));
    assert_eq!(lattice_shift(&o), 2_540_000.0);
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert!(v.len() >= 7, "{:?}", s.notes);
    assert!(v.iter().all(|&p| on_grid(p, (MM, 2 * MM), g)), "{v:?} {:?}", s.notes);
    assert!(s.notes[0].contains("staggered from the grid origin, on the via grid 0.254 x 0.254 mm"), "{}", s.notes[0]);
    assert!(s.notes.iter().any(|n| n == "stitching vias: stitch_pitch_x_mm 5 is not a multiple of the via grid 0.254 mm: the lattice uses 5.08 mm"), "{:?}", s.notes);
    // The track grid stands in for a missing via grid.
    let o2 = Options { grid_via_nm: [0, 0], grid_track_nm: [g, g], ..o.clone() };
    assert_eq!(lattice_xy(&o2), (5_080_000.0, 5_080_000.0));
    // Off: the 0.05 mm rounding, no grid.
    let off = Options { stitch_on_grid: false, ..o.clone() };
    assert_eq!(lattice_xy(&off), (5_000_000.0, 5_000_000.0));
    let s = stitch_regions(&m, &off, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert!(v.iter().any(|&p| !on_grid(p, (MM, 2 * MM), g)), "{v:?}");
}

/// Round 4: no lattice site fits the island (pitch 26 mm from (1, 1) mm: x 1 / 27, y 1 / 27; the
/// island is worth a via at 169 mm² <= its area), so every via is a fallback site; on
/// the 0.25 mm via grid they all stand on grid points (the raster cell centres are 0.025 mm off
/// it). Without the grid at least one is off it.
#[test]
fn fallback_sites_snap_to_the_via_grid() {
    let m = model(&[]);
    let items = square(0, 8, 12, 22, 26);
    let g = 250_000;
    let base = Options { stitch_min_vias: 3, stitch_per_area: false, stitch_pitch_x_nm: 26 * MM, stitch_pitch_y_nm: 26 * MM, grid_origin_nm: [MM, MM], ..opts(true, 0.0, false) };
    let o = Options { grid_via_nm: [g, g], ..base.clone() };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert_eq!(v.len(), 3, "{:?}", s.notes);
    assert!(v.iter().all(|&p| on_grid(p, (MM, MM), g)), "{v:?}");
    let s = stitch_regions(&m, &base, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert_eq!(v.len(), 3, "{:?}", s.notes);
    assert!(v.iter().any(|&p| !on_grid(p, (MM, MM), g)), "{v:?}");
}

/// Round 4 (pins the lattice offset of a staggered lattice, earlier mutant offset_no_stagger): a
/// dropped island [6, 22] x [23, 31] mm holds no point of the 20 x 20 mm lattice (rows y = 20 and
/// 40), so its one via is a fallback site, drawn towards the nearest lattice point by the offset
/// price. Straight, the nearest is (20, 20); staggered, the odd row y = 20 is shifted to x = 10 / 30,
/// so the via moves to the other side of the island.
#[test]
fn the_lattice_offset_follows_the_staggered_rows() {
    let m = model(&[]);
    let items = square(0, 6, 23, 22, 31);
    let base = Options { stitch_min_vias: 1, stitch_per_area: false, stitch_pitch_x_nm: 20 * MM, stitch_pitch_y_nm: 20 * MM, ..opts(true, 0.0, false) };
    let st = stitch_regions(&m, &Options { stitch_lattice: "straight".into(), ..base.clone() }, &reg(), &items, &[], None, &|| false);
    let sg = stitch_regions(&m, &Options { stitch_lattice: "staggered".into(), ..base }, &reg(), &items, &[], None, &|| false);
    let (a, b) = (vias(&st.items), vias(&sg.items));
    eprintln!("straight {a:?} staggered {b:?} {:?}", sg.notes);
    assert_eq!((a.len(), b.len()), (1, 1), "{:?} {:?}", st.notes, sg.notes);
    assert!(a[0].0 > 15 * MM && a[0].1 > 22 * MM, "straight: towards (20, 20) {a:?}");
    assert!(b[0].0 < 15 * MM, "staggered: towards x = 10 {b:?}");
}

/// Round 4b (esp: squeezed grid points lost to the raster point): a dropped strip [8, 22] x
/// [14, 17] mm where a via fits only in the band y in [14.88, 16.12]. On a 1.25 mm grid from
/// (0, 1.15) mm the only grid row in the band is y = 14.90, at its edge (squeezed); the raster
/// points in the band's middle are unsqueezed but up to ~0.6 mm off the grid. The off-grid price
/// k·d outweighs the squeeze difference: the vias stand on the grid (at 4·d they did not).
#[test]
fn a_squeezed_grid_point_beats_the_raster_point_beside_it() {
    let m = model(&[]);
    let items = square(0, 8, 14, 22, 17);
    let g = 1_250_000;
    let org = (0, 1_150_000);
    let o = Options {
        stitch_min_vias: 2,
        stitch_per_area: false,
        stitch_pitch_x_nm: 10 * MM,
        stitch_pitch_y_nm: 10 * MM,
        grid_via_nm: [g, g],
        grid_origin_nm: [org.0, org.1],
        stitch_lattice: "straight".into(),
        ..opts(true, 0.0, false)
    };
    let s = stitch_regions(&m, &o, &reg(), &items, &[], None, &|| false);
    let v = vias(&s.items);
    assert_eq!(v.len(), 2, "{:?}", s.notes);
    assert!(v.iter().all(|&p| on_grid(p, org, g)), "{v:?} {:?}", s.notes);
}
