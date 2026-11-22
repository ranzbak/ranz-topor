//! pad-pour-via tests: the pour-reach estimate's thermal spokes as KiCad 10 draws them (reliefs,
//! test points, spoke width, spoke survival, facing spokes) and the dog-bone stub for a pour pad
//! whose spokes are all cut. docs/dev/design/pad-pour-via.md.

use topor_engine::geom::quant::{IVec2, TrackPrim};
use topor_engine::model::{BoardModel, Item, Options, TrackItem};
use topor_engine::pourcheck::check_pour_reach;
use topor_engine::topo::factors::{standard_factors, FactorConfig, Registry};
use topor_engine::topo::stitch::stitch;

const MM: f64 = 1e6;

fn pad(r: &str, num: &str, net: i32, x: f64, y: f64, w: f64, h: f64, layers: &str) -> String {
    let shapes: Vec<String> = layers.split(',').map(|l| format!(r#""{}": {{ "kind": "rect", "w": {}, "h": {} }}"#, l.trim(), (w * MM) as i64, (h * MM) as i64)).collect();
    let attr = if layers.contains(',') { "pth" } else { "smd" };
    format!(
        r#"{{ "uuid": "{r}{num}", "ref": "{r}", "number": "{num}", "net": {net}, "attr": "{attr}", "center": [{}, {}], "layers": [{layers}], "shapes": {{ {} }} }}"#,
        (x * MM) as i64,
        (y * MM) as i64,
        shapes.join(",")
    )
}

/// 20 x 30 mm board, GND (net 1) poured solid on F.Cu (0) and B.Cu (31) over [2, 18] x [2, 28] mm,
/// thermal connection (gap 0.5, spoke 0.5, min width 0.254), clearance 0.2, width 0.25, signal net
/// 2; a THT GND pad H.1 at (4, 4) holds the main fill on both layers. `pads`: more pads,
/// `pour_pads` their labels in the pour, `min_track_mm` the board's minimum track width.
fn board(pads: &[String], pour_pads: &[&str], min_track_mm: f64) -> BoardModel {
    board_z(pads, pour_pads, min_track_mm, 0.2)
}

/// [`board`] with the pour's clearance `zclear_mm`.
fn board_z(pads: &[String], pour_pads: &[&str], min_track_mm: f64, zclear_mm: f64) -> BoardModel {
    let mut all = vec![pad("H", "1", 1, 4.0, 4.0, 1.5, 1.5, "0, 31")];
    all.extend(pads.iter().cloned());
    let names: Vec<String> = std::iter::once("\"H.1\"".to_string()).chain(pour_pads.iter().map(|p| format!("\"{p}\""))).collect();
    let fill = |l: u32| format!(r#"{{ "layer": {l}, "outer": [[2000000, 2000000], [18000000, 2000000], [18000000, 28000000], [2000000, 28000000]] }}"#);
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "GND" }}, {{ "code": 2, "name": "SIG" }} ],
          "outline": [ {{ "kind": "seg", "a": [0, 0], "b": [20000000, 0] }},
            {{ "kind": "seg", "a": [20000000, 0], "b": [20000000, 30000000] }},
            {{ "kind": "seg", "a": [20000000, 30000000], "b": [0, 30000000] }},
            {{ "kind": "seg", "a": [0, 30000000], "b": [0, 0] }} ],
          "pads": [ {} ],
          "pours": [ {{ "uuid": "zz", "net": 1, "layers": [0, 31], "pads": [{}],
            "vias": [], "clearance": {}, "min_thickness": 254000, "thermal_gap": 500000,
            "spoke_width": 500000, "pad_connection": 1,
            "fills": [ {}, {} ] }} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0, "min_track_width": {} }} }}"#,
        all.join(","),
        names.join(", "),
        (zclear_mm * MM) as i64,
        fill(0),
        fill(31),
        (min_track_mm * MM) as i64
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e:?}\n{s}"))
}

fn seg(net: i32, layer: u32, w: f64, a: (f64, f64), b: (f64, f64)) -> Item {
    let q = |p: (f64, f64)| IVec2 { x: (p.0 * MM).round() as i32, y: (p.1 * MM).round() as i32 };
    Item::Track(TrackItem { net, layer, width: (w * MM) as i64, prim: TrackPrim::Seg { a: q(a), b: q(b) }, group: None, wire: None })
}

fn via(net: i32, x: f64, y: f64, d: f64) -> Item {
    let js = format!(r#"[{{ "kind": "via", "net": {net}, "at": [{}, {}], "d": {}, "drill": {}, "layers": [], "group": null }}]"#, (x * MM) as i64, (y * MM) as i64, (d * MM) as i64, (d * MM / 2.0) as i64);
    Item::list_from_json(&js).unwrap().remove(0)
}

fn reg() -> Registry {
    Registry::from_config(standard_factors(), &FactorConfig::default()).0
}

fn cut(m: &BoardModel, items: &[Item], label: &str) -> bool {
    check_pour_reach(m, items, &[]).cuts.iter().any(|c| c.pads.iter().any(|&p| format!("{}.{}", m.pads[p].ref_, m.pads[p].number) == label))
}

/// Four net-2 bars (1 mm long, 0.25 wide) across the spokes of GND pad G.1 (0.3 x 1.2 at (10, 15)),
/// centred `d` mm beyond the pad's edge.
fn bars(d: f64) -> Vec<Item> {
    let (cx, cy, hx, hy) = (10.0, 15.0, 0.15, 0.6);
    vec![
        seg(2, 0, 0.25, (cx + hx + d, cy - 0.5), (cx + hx + d, cy + 0.5)),
        seg(2, 0, 0.25, (cx - hx - d, cy - 0.5), (cx - hx - d, cy + 0.5)),
        seg(2, 0, 0.25, (cx - 0.5, cy + hy + d), (cx + 0.5, cy + hy + d)),
        seg(2, 0, 0.25, (cx - 0.5, cy - hy - d), (cx + 0.5, cy - hy - d)),
    ]
}

/// KiCad 10 knocks out the relief (pad + thermal gap) and draws a spoke only where its test point
/// (bbox + gap + max_error + min_width/2 = 0.632 mm past the edge) lies in the opened fill. Bars
/// centred 0.885 mm past each edge cover the test points (their knockouts span 0.56..1.21 mm) and
/// leave a 0.56 mm ring of pre-route fill around the pad that joins the main fill around the bars'
/// ends: without the relief knockout that ring joins the pad (the old spokes of the pre-route fill).
#[test]
fn spokes_whose_test_points_are_covered_are_not_drawn() {
    let m = board(&[pad("G", "1", 1, 10.0, 15.0, 0.3, 1.2, "0")], &["G.1"], 0.09);
    assert!(!cut(&m, &[], "G.1"), "an open pad keeps its spokes");
    assert!(cut(&m, &bars(0.885), "G.1"), "all four test points covered: no spoke");
}

/// The other side of the knife edge: bars 1.3 mm past the edges leave the test points in solid fill.
#[test]
fn spokes_whose_test_points_are_in_the_fill_are_drawn() {
    let m = board(&[pad("G", "1", 1, 10.0, 15.0, 0.3, 1.2, "0")], &["G.1"], 0.09);
    assert!(!cut(&m, &bars(1.3), "G.1"));
}

/// KiCad draws no spokes at all when the spoke width, clamped to the pad's minor size, is below the
/// zone's minimum width ("Cannot create stubs having a width < zone min thickness"): a 0.2 mm wide
/// pad in open fill is cut off.
#[test]
fn a_pad_narrower_than_the_min_width_gets_no_spokes() {
    let m = board(&[pad("G", "1", 1, 10.0, 15.0, 0.2, 1.2, "0")], &["G.1"], 0.09);
    assert!(cut(&m, &[], "G.1"));
    let m = board(&[pad("G", "1", 1, 10.0, 15.0, 0.3, 1.2, "0")], &["G.1"], 0.09);
    assert!(!cut(&m, &[], "G.1"));
}

/// A spoke whose test point is in the fill but that foreign copper cuts on its way to the pad is
/// gone after the clearance holes and the opening (KiCad `fillCopperZone`): a net-2 via in the
/// relief right above the pad cuts the top spoke; the other three are covered by bars.
#[test]
fn a_spoke_cut_inside_the_relief_does_not_join() {
    let m = board(&[pad("G", "1", 1, 10.0, 15.0, 0.3, 1.2, "0")], &["G.1"], 0.09);
    let mut items: Vec<Item> = bars(0.885).into_iter().take(3).collect();
    assert!(!cut(&m, &items, "G.1"), "the top spoke alone joins");
    items.push(seg(2, 0, 0.1, (9.0, 14.1), (11.0, 14.1)));
    assert!(cut(&m, &items, "G.1"), "a track across the top spoke inside the relief cuts it");
}

/// KiCad 10 keeps two facing spokes of two thermal pads of the net whose test points lie in each
/// other ("hit-test against other spokes"; qmtech C31.2 / X2.2): G.1 is boxed in except towards
/// G.2, 1.6 mm above it, which joins the main fill.
#[test]
fn facing_spokes_join_two_pads() {
    let pads = [pad("G", "1", 1, 10.0, 15.0, 0.9, 0.9, "0"), pad("G", "2", 1, 10.0, 13.4, 0.9, 0.9, "0")];
    let m = board(&pads, &["G.1", "G.2"], 0.09);
    let (cx, cy) = (10.0, 15.0);
    // Bars 0.89 mm past G.1's left, right and bottom edges, and two bars on either side of the gap.
    let items = vec![
        seg(2, 0, 0.25, (cx + 0.45 + 0.885, cy - 1.0), (cx + 0.45 + 0.885, cy + 1.0)),
        seg(2, 0, 0.25, (cx - 0.45 - 0.885, cy - 1.0), (cx - 0.45 - 0.885, cy + 1.0)),
        seg(2, 0, 0.25, (cx - 1.0, cy + 0.45 + 0.885), (cx + 1.0, cy + 0.45 + 0.885)),
    ];
    assert!(!cut(&m, &items, "G.1"), "{:?}", check_pour_reach(&m, &items, &[]).notes);
}

/// A 0.5 mm-pitch row: GND pad G.1 between net-2 pads whose 0.25 mm fanouts run 2.8 mm out on
/// both sides of its axis channel (pour clearance 0.25 mm: the channel's fill, 0.25 mm, is below the
/// minimum width); a net-2 track across the row's other side covers the bottom spoke. The spokes are cut; the radial pad stub (at most 3 mm from the centre) finds no via site
/// (the first is about 3.18 mm out, past the fanouts' ends); the dog-bone runs out along the pad
/// axis and puts its via there, in the B.Cu fill.
fn row() -> (BoardModel, Vec<Item>) {
    row_fan(0.25, &[])
}

/// [`row`] with fanouts `fan` mm wide and `extra` more pads.
fn row_fan(fan: f64, extra: &[String]) -> (BoardModel, Vec<Item>) {
    let mut pads = vec![pad("G", "1", 1, 10.0, 15.0, 0.3, 1.2, "0")];
    let mut items = Vec::new();
    for (k, x) in [9.0, 9.5, 10.5, 11.0].into_iter().enumerate() {
        pads.push(pad("S", &format!("{}", k + 1), 2, x, 15.0, 0.3, 1.2, "0"));
        items.push(seg(2, 0, fan, (x, 15.0), (x, 15.0 - 2.8)));
    }
    pads.extend(extra.iter().cloned());
    items.push(seg(2, 0, 0.25, (8.0, 16.24), (12.0, 16.24)));
    (board_z(&pads, &["G.1"], 0.09, 0.25), items)
}

#[test]
fn a_boxed_row_pad_gets_a_dog_bone_along_its_axis() {
    let (m, items) = row();
    assert!(cut(&m, &items, "G.1"));
    let o = Options::default();
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    assert_eq!(s.pad_stubs, 0, "{:?}", s.notes);
    let note = s.notes.iter().find(|n| n.starts_with("dog-bone G.1")).unwrap_or_else(|| panic!("{:?}", s.notes));
    let v = s.items.iter().find_map(|it| if let Item::Via(v) = it { Some(v.clone()) } else { None }).expect("a via");
    assert_eq!(v.at.x, 10_000_000, "{note}");
    // Past the radial stub's reach (STUB_MAX_NM, 3 mm from the centre), clear of the fanouts' ends.
    assert!(v.at.y < 15_000_000 - 3_000_000, "{note}");
    let mut all = items.clone();
    all.extend(s.items.iter().cloned());
    assert!(!cut(&m, &all, "G.1"), "{note}");
    // Off: no dog-bone, the pad stays cut with the pad-stub note.
    let s = stitch(&m, &Options { stitch_dogbone: false, ..o.clone() }, &reg(), &items, &[], None);
    assert!(!s.notes.iter().any(|n| n.starts_with("dog-bone")), "{:?}", s.notes);
}

/// A net-2 via on the axis just past the fanouts' ends leaves no straight via site; the dog-bone
/// turns (45 or 90 degrees) past it.
#[test]
fn a_blocked_axis_turns_the_dog_bone() {
    let (m, mut items) = row();
    items.push(via(2, 10.0, 15.0 - 2.8 - 1.125, 0.6));
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    let note = s.notes.iter().find(|n| n.starts_with("dog-bone G.1")).unwrap_or_else(|| panic!("{:?}", s.notes));
    assert!(note.contains("degree bend"), "{note}");
}

/// No fit: a net-2 bar right past the fanouts' ends closes the channel; the warning names the pad
/// and the nets that boxed it in.
#[test]
fn no_dog_bone_fit_names_the_pad() {
    let (m, mut items) = row();
    items.push(seg(2, 0, 0.25, (7.0, 15.0 - 3.0), (13.0, 15.0 - 3.0)));
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    assert!(!s.notes.iter().any(|n| n.starts_with("dog-bone")), "{:?}", s.notes);
    let w = s.notes.iter().find(|n| n.starts_with("no dog-bone via fits for pad G.1")).unwrap_or_else(|| panic!("{:?}", s.notes));
    assert!(w.contains("SIG"), "{w}");
    assert_eq!(s.dogbone_failed, 1);
}

fn first_via(s: &topor_engine::topo::stitch::StitchOut) -> (f64, f64) {
    s.items.iter().find_map(|it| if let Item::Via(v) = it { Some((v.at.x as f64 / MM, v.at.y as f64 / MM)) } else { None }).expect("a via")
}

fn dog_note(s: &topor_engine::topo::stitch::StitchOut) -> String {
    s.notes.iter().find(|n| n.starts_with("dog-bone G.1")).unwrap_or_else(|| panic!("{:?}", s.notes)).clone()
}

/// 0.4 mm fanouts leave the axis channel room for a 0.2 mm stub only: the dog-bone necks below the
/// net width (0.25 mm) to fit.
#[test]
fn a_narrow_channel_necks_the_dog_bone() {
    let (m, items) = row_fan(0.4, &[]);
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    let note = dog_note(&s);
    assert!(note.contains("0.188 mm wide") || note.contains("0.125 mm wide") || note.contains("0.090 mm wide"), "{note}");
}

/// An existing GND via on the axis just past the fanouts' ends: the dog-bone ends on it (shared
/// via, no new via; a new one would sit farther out, clear of it).
#[test]
fn the_dog_bone_shares_an_existing_via() {
    let (m, mut items) = row();
    items.push(via(1, 10.0, 11.6, 0.6));
    let s = stitch(&m, &Options::default(), &reg(), &items, &[], None);
    let note = dog_note(&s);
    assert!(note.contains("shared via") && note.contains("(10.000, 11.600)"), "{note}");
    assert!(!s.items.iter().any(|it| matches!(it, Item::Via(_))), "{note}");
}

/// Power-via rule: under a BGA a dog-bone via costs 100 via prices. With footprint B (two B.Cu
/// corner pads spanning the axis past the fanouts) declared a BGA, the via leaves that area.
#[test]
fn a_dog_bone_via_keeps_out_of_a_bga() {
    let bga = [pad("B", "1", 2, 9.2, 10.4, 0.1, 0.1, "31"), pad("B", "2", 2, 10.8, 12.0, 0.1, 0.1, "31")];
    let (m, items) = row_fan(0.25, &bga);
    let free = first_via(&stitch(&m, &Options::default(), &reg(), &items, &[], None));
    assert!((9.2..=10.8).contains(&free.0) && (10.4..=12.0).contains(&free.1), "without the BGA the axis via: {free:?}");
    let o = Options { plane_bga_refs: vec!["B".into()], ..Options::default() };
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    let v = first_via(&s);
    assert!(!((9.2..=10.8).contains(&v.0) && (10.4..=12.0).contains(&v.1)), "{v:?} {}", dog_note(&s));
}

/// With `[planes] stitch_on_grid` the via prefers the user's grid at a graded price (0.5 mm,
/// origin y 0.25): the nearest legal site 11.8 is 0.05 mm off it, the grid point 11.75 one lattice
/// step farther out wins; the pad end stays at the pad centre.
#[test]
fn a_dog_bone_via_prefers_the_grid() {
    let (m, items) = row();
    let off = first_via(&stitch(&m, &Options { stitch_on_grid: false, ..Options::default() }, &reg(), &items, &[], None));
    assert_eq!(off.0, 10.0);
    assert!((off.1 - 11.8).abs() < 1e-6, "{off:?}");
    // off_grid weighted 2 mm per via fully off the grid (default 0.15 mm: a 0.05 mm detour would not pay).
    let factors = FactorConfig { weight: [("off_grid".to_string(), 2.0)].into_iter().collect(), ..FactorConfig::default() };
    let o = Options { stitch_on_grid: true, grid_via_nm: [500_000, 500_000], grid_origin_nm: [0, 250_000], factors, ..Options::default() };
    let s = stitch(&m, &o, &reg(), &items, &[], None);
    let v = first_via(&s);
    assert!((v.0 - 10.0).abs() < 1e-6 && (v.1 - 11.75).abs() < 1e-6, "{v:?} {}", dog_note(&s));
}
