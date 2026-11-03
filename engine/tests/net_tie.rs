//! Task NT: net-tie DRC semantics as KiCad 10 applies them, pinned on the engine's `Checker`
//! (the self-check every emitted item passes) and on the router.
//!
//! KiCad evidence (kicad-cli 10.0.8, `pcb drc --severity-all`, task NT, 2026-11-01; re-run with
//! `.venv/bin/python tools/net_tie_drc_probe.py <out_dir>`): a small board of 3-pad net-tie
//! footprints (`net_tie_pad_groups "1,2"`, pads 1.0 × 1.5 mm rect, pad 1 = net A at x −1.3, pad 2
//! = net B at x 0, pad 3 = untied net C at x +1.3, copper bridge `fp_poly` x −0.9..−0.4, y ±0.3 as
//! in `Jumper:SolderJumper-3_P1.3mm_Bridged12`, the footprint of dcsgonebadge JP1), every track
//! starting in a pad of its own net (an unanchored track touching a single pad takes that pad's
//! net in KiCad's net propagation and proves nothing):
//!
//! | case     | geometry                                                         | KiCad 10        |
//! |----------|------------------------------------------------------------------|-----------------|
//! | 1/20/21/22 | net-B track touches tie pad 1 (net A) outside pad 2            | shorting_items  |
//! | 8        | net-B track 0.1 mm from tie pad 1, outside pad 2                 | clearance       |
//! | 31/32    | tie pads 0.1 apart (< clearance), B inside pad 2 within clearance of pad 1 | clearance |
//! | 33       | same pads, B 0.3 mm from pad 1                                   | none            |
//! | 24       | overlapping tie pads, B track ends inside pad 2 in the overlap   | none            |
//! | 35       | overlapping tie pads, B inside pad 2 only, 0.1 mm from pad 1     | none            |
//! | 23       | overlapping tie pads, B touches pad 1 from the far side          | shorting_items  |
//! | 25       | net-A track touches the tie's copper bridge away from both pads  | none            |
//! | 26       | untied net-C track touches the bridge                            | shorting_items  |
//! | 27       | net-A track ends in pad 1, overlapping the bridge inside pad 1   | none            |
//! | 7        | untied net-C track into tie pad 2                                | shorting_items  |
//!
//! i.e. KiCad's `DRC_ENGINE::IsNetTieExclusion`: a collision of a track of net N with copper of a
//! net-tie footprint is allowed only where the collision lies inside a tie-group pad of net N of
//! that footprint; the footprint's net-less copper graphics are allowed for the tied nets. A tie
//! pad is therefore NOT exempt for the other tied net's tracks (the dcsgonebadge JP1.1 ↔
//! Net-(J3-Pin_22) short of the A5 report, concern 3).
use topor_engine::check::Checker;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options, TrackItem};

const MM: f64 = 1_000_000.0;
const A: i32 = 1;
const B: i32 = 2;
const C: i32 = 3;

fn nm(v: f64) -> i64 {
    (v * MM).round() as i64
}

/// Pad JSON, KiCad frame mm. `group`: net-tie group index.
fn pad(
    r: &str,
    n: &str,
    net: i32,
    (x, y): (f64, f64),
    (w, h): (f64, f64),
    group: Option<usize>,
) -> String {
    let g = group.map_or(String::new(), |g| format!(r#", "net_tie_group": {g}"#));
    format!(
        r#"{{ "uuid": "{r}{n}", "ref": "{r}", "number": "{n}", "net": {net}, "attr": "smd",
            "center": [{}, {}], "layers": [0],
            "shapes": {{ "0": {{ "kind": "rect", "w": {}, "h": {} }} }}{g} }}"#,
        nm(x),
        nm(y),
        nm(w),
        nm(h)
    )
}

/// Footprint JP at (20, 20): tie pads 1 (A) / 2 (B) at `xs[0]`, `xs[1]`, untied pad 3 (C) at
/// `xs[2]` (offsets, mm), optional bridge `(x0, x1, y0, y1)` (offsets), plus `extra` pads.
fn tie_model(xs: [f64; 3], bridge: Option<(f64, f64, f64, f64)>, extra: &[String]) -> BoardModel {
    let (cx, cy) = (20.0, 20.0);
    let mut pads = vec![
        pad("JP", "1", A, (cx + xs[0], cy), (1.0, 1.5), Some(0)),
        pad("JP", "2", B, (cx + xs[1], cy), (1.0, 1.5), Some(0)),
        pad("JP", "3", C, (cx + xs[2], cy), (1.0, 1.5), None),
    ];
    pads.extend(extra.iter().cloned());
    let shapes = bridge.map_or(String::new(), |(x0, x1, y0, y1)| {
        let p = |x: f64, y: f64| format!("[{}, {}]", nm(cx + x), nm(cy + y));
        format!(
            r#""copper_shapes": [ {{ "uuid": "bridge", "net": 0, "layer": 0, "owner_ref": "JP",
                "poly": [ {}, {}, {}, {} ] }} ],"#,
            p(x0, y0),
            p(x1, y0),
            p(x1, y1),
            p(x0, y1)
        )
    });
    let seg = |a: (i64, i64), b: (i64, i64)| {
        format!(
            r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
            a.0 * 1_000_000,
            a.1 * 1_000_000,
            b.0 * 1_000_000,
            b.1 * 1_000_000
        )
    };
    let outline = [
        seg((0, 0), (40, 0)),
        seg((40, 0), (40, 40)),
        seg((40, 40), (0, 40)),
        seg((0, 40), (0, 0)),
    ]
    .join(",");
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu", "routable": true }} ],
            "nets": [ {{ "code": 1, "name": "A" }}, {{ "code": 2, "name": "B" }}, {{ "code": 3, "name": "C" }} ],
            "pads": [ {} ], {shapes}
            "net_ties": [ {{ "ref": "JP", "groups": [["1", "2"]], "nets": [[1, 2]] }} ],
            "outline": [ {outline} ],
            "rules": {{ "default": {{ "clearance": 200000, "width": 200000 }}, "min_clearance": 0,
                "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pads.join(",")
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

/// dcsgonebadge JP1 geometry: gapped tie pads (0.3 mm), copper bridge between them.
fn gapped() -> BoardModel {
    tie_model([-1.3, 0.0, 1.3], Some((-0.9, -0.4, -0.3, 0.3)), &[])
}

/// Same, the bridge extended to y −1.5 (away from both pads).
fn long_bridge() -> BoardModel {
    tie_model([-1.3, 0.0, 1.3], Some((-0.9, -0.4, -1.5, 0.3)), &[])
}

/// Overlapping tie pads (pad 1 x −0.9..0.1, pad 2 x −0.1..0.9), no bridge.
fn overlapping() -> BoardModel {
    tie_model([-0.4, 0.4, 2.0], None, &[])
}

/// Track of `net`, KiCad-frame offsets (mm) from the footprint origin (20, 20).
fn track(net: i32, a: (f64, f64), b: (f64, f64), w: f64) -> Item {
    Item::Track(TrackItem {
        net,
        layer: 0,
        width: nm(w),
        prim: TrackPrim::Seg {
            a: [nm(20.0 + a.0) as i32, nm(20.0 + a.1) as i32].into(),
            b: [nm(20.0 + b.0) as i32, nm(20.0 + b.1) as i32].into(),
        },
        group: None,
        wire: None,
    })
}

fn against(m: &BoardModel, it: Item) -> Vec<String> {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let mut v: Vec<String> = Checker::new(m, &o)
        .check_items(&[it])
        .violations
        .into_iter()
        .map(|v| v.against)
        .collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn other_tied_net_touching_a_gapped_tie_pad_is_a_violation() {
    let m = gapped();
    // case 1 (dcsgone JP1): from pad 2's centre across the bridge into pad 1
    assert_eq!(
        against(&m, track(B, (0.0, 0.0), (-1.6, 0.0), 0.2)),
        ["pad:JP:1"]
    );
    // case 20 / 22: from the far side to pad 1's centre / edge
    assert_eq!(
        against(&m, track(B, (-4.0, 0.0), (-1.3, 0.0), 0.2)),
        ["pad:JP:1"]
    );
    assert_eq!(
        against(&m, track(B, (-4.0, 0.0), (-1.6, 0.0), 0.2)),
        ["pad:JP:1"]
    );
    // case 8: 0.1 mm from pad 1 (clearance 0.2), never touching
    assert_eq!(
        against(&m, track(B, (-3.0, 0.95), (0.0, 0.95), 0.2)),
        ["pad:JP:1"]
    );
    // case 7: untied net C into pad 2
    assert_eq!(
        against(&m, track(C, (1.3, 0.0), (0.3, 0.0), 0.2)),
        ["pad:JP:2"]
    );
}

#[test]
fn tied_net_may_touch_the_tie_footprint_copper_bridge() {
    let m = long_bridge();
    // case 25: net A touches the bridge away from both pads; case 27: A ends in pad 1 over the bridge
    assert!(against(&m, track(A, (-0.65, -4.0), (-0.65, -1.3), 0.1)).is_empty());
    assert!(against(&gapped(), track(A, (-4.0, 0.0), (-0.9, 0.0), 0.2)).is_empty());
    // case 26: the untied net C may not
    assert_eq!(
        against(&m, track(C, (-0.65, -4.0), (-0.65, -1.3), 0.1)),
        ["shape:bridge"]
    );
}

#[test]
fn overlapping_tie_pads_allow_the_collision_inside_the_own_pad_only() {
    let m = overlapping();
    // case 24: B ends inside pad 2, in the overlap with pad 1
    assert!(against(&m, track(B, (0.05, -4.0), (0.05, 0.0), 0.2)).is_empty());
    // case 35: B inside pad 2 only, 0.1 mm from pad 1
    assert!(against(&m, track(B, (0.3, -4.0), (0.3, 0.0), 0.2)).is_empty());
    // case 23: B touches pad 1 from the far side (outside pad 2)
    assert_eq!(
        against(&m, track(B, (-4.0, 0.3), (-0.85, 0.3), 0.2)),
        ["pad:JP:1"]
    );
}

#[test]
fn tie_pads_closer_than_clearance_are_not_exempt() {
    // case 31: pads 0.1 mm apart (< 0.2 clearance), B inside pad 2 within clearance of pad 1
    let m = tie_model([-0.55, 0.55, 2.5], None, &[]);
    assert_eq!(
        against(&m, track(B, (0.10, -4.0), (0.10, 0.0), 0.2)),
        ["pad:JP:1"]
    );
    // case 33: 0.3 mm away is clean
    assert!(against(&m, track(B, (0.35, -4.0), (0.35, 0.0), 0.2)).is_empty());
}

