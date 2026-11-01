//! A13: bounded via position relaxation (design note `m4-capacity-steiner-design.md` §5).
//!
//! Fixture: a 50 x 40 mm two-layer board, two through-hole pads A1 / A2 of one net, a locked
//! B.Cu wall at x = 17 mm and a locked F.Cu wall at x = 33 mm (both y 2..38). The connection
//! must run on F.Cu across the B.Cu wall and on B.Cu across the F.Cu wall, so it has exactly one
//! via between x = 17 and x = 33 mm. Any point of the segment A1-A2 inside that window is an
//! optimal via position (length = |A1 A2|); the negotiation puts the via on a face-incentre site
//! of the triangulation, off that segment. The relaxation pass moves it onto the segment.

use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

const MM: f64 = 1_000_000.0;

/// Through-hole round pad of net `net` at `(x, y)` mm (y up), d 1 mm.
fn pad(r: &str, net: i32, x: f64, y: f64) -> String {
    format!(
        r#"{{ "uuid": "{r}", "ref": "{r}", "number": "1", "net": {net}, "center": [{}, {}],
          "layers": [0, 31], "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }},
          "31": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
        (x * MM) as i64,
        (-y * MM) as i64
    )
}

fn seg(a: (i64, i64), b: (i64, i64)) -> String {
    format!(
        r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
        a.0 * 1_000_000,
        a.1 * 1_000_000,
        b.0 * 1_000_000,
        b.1 * 1_000_000
    )
}

fn wall(u: &str, layer: u32, x: f64) -> String {
    format!(
        r#"{{ "uuid": "{u}", "net": 3, "layer": {layer}, "width": 250000, "locked": true,
          "prim": {{ "kind": "seg", "a": [{}, -2000000], "b": [{}, -38000000] }} }}"#,
        (x * MM) as i64,
        (x * MM) as i64
    )
}

/// The board with A1 at `a`, A2 at `b` (mm, y up) and `keepouts` (JSON objects).
fn board(a: (f64, f64), b: (f64, f64), keepouts: &[String]) -> BoardModel {
    board_with(a, b, keepouts, true)
}

/// [`board`] with or without the two walls.
fn board_with(a: (f64, f64), b: (f64, f64), keepouts: &[String], walls: bool) -> BoardModel {
    let outline = [
        ((0, 0), (50, 0)),
        ((50, 0), (50, -40)),
        ((50, -40), (0, -40)),
        ((0, -40), (0, 0)),
    ]
    .iter()
    .map(|&(p, q)| seg(p, q))
    .collect::<Vec<_>>()
    .join(",");
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }}, {{ "code": 3, "name": "N3" }} ],
          "pads": [ {}, {} ],
          "kept": [ {}{}{} ], "keepouts": [ {} ], "outline": [ {outline} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pad("A1", 1, a.0, a.1),
        pad("A2", 1, b.0, b.1),
        if walls { wall("wf", 0, 33.0) } else { String::new() },
        if walls { "," } else { "" },
        if walls { wall("wb", 31, 17.0) } else { String::new() },
        keepouts.join(",")
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

fn opts(relax: bool, threads: usize) -> Options {
    Options {
        wall_clock_s: 600.0,
        threads,
        via_relaxation: relax,
        ..Options::default()
    }
}

/// No-via keepout rectangle on both layers (mm).
fn no_via_box(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let p = |x: f64, y: f64| format!("[{}, {}]", (x * MM) as i64, (-y * MM) as i64);
    format!(
        r#"{{ "name": "nv", "layers": [0, 31], "no_vias": true,
          "poly": [{}, {}, {}, {}] }}"#,
        p(x0, y0),
        p(x1, y0),
        p(x1, y1),
        p(x0, y1)
    )
}

/// The single via of the single connection, mm (y up).
fn via_mm(r: &RouteResult) -> (f64, f64) {
    assert_eq!(r.connections.len(), 1);
    assert_eq!(r.connections[0].status, "routed", "{:?}", r.connections[0]);
    let v = &r.connections[0].via_sites;
    assert_eq!(v.len(), 1, "{v:?}");
    (v[0][0] as f64 / MM, -v[0][1] as f64 / MM)
}

/// Distance of `p` from the line through `a` and `b`.
fn off_line(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / dx.hypot(dy)
}

fn clean(m: &BoardModel, r: &RouteResult) {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

fn strip(r: &RouteResult) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    let o = v.as_object_mut().unwrap();
    o.remove("runtime_s");
    o.remove("threads_used");
    o.remove("profile");
    serde_json::to_string(&v).unwrap()
}

/// Force balance: the via of a two-anchor connection ends on the segment between its legs'
/// anchors (the Weiszfeld fixed set), inside the window between the walls, the connection
/// shrinks to the straight distance, and the result is clean at margin 0. The via was off the
/// segment before (it sat on a site).
#[test]
fn via_moves_toward_force_balance_within_face_star() {
    for (a, b) in [((8.0, 20.0), (42.0, 20.0)), ((8.0, 10.0), (42.0, 30.0))] {
        let m = board(a, b, &[]);
        let off = route(&m, &opts(false, 1));
        let on = route(&m, &opts(true, 1));
        let (v0, v1) = (via_mm(&off), via_mm(&on));
        assert_eq!(off.via_relaxation, "off");
        assert!(on.via_relaxation.starts_with("moved 1 of 1 vias"), "{}", on.via_relaxation);
        assert!(off_line(v0, a, b) > 1.0, "the site is off the line: {v0:?}");
        assert!(off_line(v1, a, b) < 0.001, "{v1:?} not on {a:?} - {b:?}");
        assert!(v1.0 > 17.5 && v1.0 < 32.5, "{v1:?}");
        let straight = (b.0 - a.0).hypot(b.1 - a.1) * MM;
        let (l0, l1) = (off.connections[0].length_nm as f64, on.connections[0].length_nm as f64);
        assert!(l1 < l0 - 1.0 * MM, "{l0} -> {l1}");
        assert!((l1 - straight).abs() < 0.01 * MM, "{l1} vs straight {straight}");
        assert!(on.length_outside_pads_nm <= off.length_outside_pads_nm);
        assert_eq!((on.routed, on.failed, on.vias), (1, 0, 1));
        clean(&m, &on);
    }
}

/// A no-via keepout across the straight line: the target position is not allowed, the via stays
/// out of the keepout (it may stop short of it, never inside), the result is clean and no
/// longer than without relaxation.
#[test]
fn via_relaxation_rejects_move_into_keepout() {
    let a = (8.0, 20.0);
    let b = (42.0, 20.0);
    let ko = no_via_box(18.0, 17.0, 32.0, 23.0);
    let m = board(a, b, &[ko]);
    let off = route(&m, &opts(false, 1));
    let on = route(&m, &opts(true, 1));
    let (v0, v1) = (via_mm(&off), via_mm(&on));
    for v in [v0, v1] {
        assert!(!(v.0 > 17.0 && v.0 < 33.0 && v.1 > 16.5 && v.1 < 23.5), "via inside keepout: {v:?}");
    }
    assert!(on.connections[0].length_nm <= off.connections[0].length_nm);
    // the free board reaches the straight distance, the keepout one does not
    let free = route(&board(a, b, &[]), &opts(true, 1));
    assert!(on.connections[0].length_nm > free.connections[0].length_nm + 500_000);
    // it moved, but only up to the keepout (back-off), not into it
    assert!(on.via_relaxation.starts_with("moved 1 of 1"), "{}", on.via_relaxation);
    assert!(v1.1 >= 23.5 && v1.1 < v0.1, "{v0:?} -> {v1:?}");
    assert_eq!(on.routed, 1);
    clean(&m, &on);
}

/// `via_relaxation = false`: no pass runs, the status says `off`, the vias stay on their
/// sites (the numbers of the pre-A13 engine on this fixture), and a board without vias is
/// identical with the pass on or off.
#[test]
fn via_relaxation_off_matches_previous() {
    let m = board((8.0, 20.0), (42.0, 20.0), &[]);
    let off = route(&m, &opts(false, 1));
    assert_eq!(off.via_relaxation, "off");
    assert_eq!(off.connections[0].length_nm, 37_180_415);
    assert_eq!(off.connections[0].via_sites, vec![[30_340_616, -27_205_896]]);
    // default options enable it (older option JSON without the key too)
    assert!(Options::default().via_relaxation);
    assert!(Options::from_json("{}").unwrap().via_relaxation);
    assert!(!Options::from_json(r#"{ "via_relaxation": false }"#).unwrap().via_relaxation);
    // a board without vias: identical but for the status line
    let free = board_with((8.0, 20.0), (42.0, 20.0), &[], false);
    let (a, b) = (route(&free, &opts(false, 1)), route(&free, &opts(true, 1)));
    assert_eq!(a.vias, 0);
    assert_eq!(b.via_relaxation, "moved 0 of 0 vias, -0 um");
    let norm = |r: &RouteResult| strip(r).replace(&r.via_relaxation, "");
    assert_eq!(norm(&a), norm(&b));
}

/// Threads 1 / 2 / 4 and repeats: byte-identical results including the moved via.
#[test]
fn via_relaxation_threads_byte_identical() {
    let m = board((8.0, 10.0), (42.0, 30.0), &[]);
    let base = route(&m, &opts(true, 1));
    assert!(base.via_relaxation.starts_with("moved 1 of 1"), "{}", base.via_relaxation);
    let want = strip(&base);
    for t in [1usize, 2, 4] {
        for _ in 0..2 {
            assert_eq!(strip(&route(&m, &opts(true, t))), want, "threads {t}");
        }
    }
}
