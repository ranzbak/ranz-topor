//! fuse-octi r4: the fuse pass's order and the split guard of the polish, on tiny synthetic boards
//! (built here; PROPOSED patterns, text in the r4 report). One layer (B.Cu closed), one run, one
//! round: deterministic.
//!
//! M2, fixture `fuse_shared_pad`: net N has three pads, P (25, 15), A to the west and B to the north
//! of it, so both wires of the tree end at P (P-A west along y = 15 / 17, P-B 45 degrees up to
//! (22, 12), then north) and either may fuse its end at P onto the other's copper, but not both
//! (`fused_at`: no cycle). A onto B (A leaves B's copper at (22, 12)) is the larger legal gain in both
//! variants; in `long_a` (A at (5, 17): the A wire is the longer one, the higher wire id) the small
//! one (B onto A at (22, 15)) has the lower id, in `short_a` (A at (11, 17), B at (22, 1): the A
//! wire is the shorter one, the lower id) the small one (B at P, 0.19 mm) has the higher id. Both
//! must take the large fuse: an id order fails `long_a`, a smallest-first order fails `short_a`.
//!
//! H1, fixture `fuse_free_trunk`: net N has pads A (5, 5), B (25, 15), C (11, 25), drawn with the
//! `any_angle` factor at 0 so the drawing leaves straight free-angle legs; the branch from A fuses
//! onto the free-angle trunk B-C at (16.486, 21.081), and the octilinear polish redraws that trunk
//! segment. Expected (KiCad's connectivity is the spec): the foot stays on the trunk's copper (it
//! is a vertex of the redrawn trunk) and all copper is octilinear; with the test hook "polish drops
//! feet" (the r3 bug: no fixed vertices) the split guard refuses every shape of the trunk segment,
//! so the net stays one piece with the trunk left free-angle.

use topor_engine::control::RunControl;
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::RouteResult;

const MM: i64 = 1_000_000;

fn pad(r: &str, net: i32, x: f64, y: f64, s: f64) -> serde_json::Value {
    let n = |v: f64| (v * MM as f64).round() as i64;
    serde_json::json!({
        "uuid": format!("00000000-0000-4000-8000-{:012}", (x * 1000.0) as i64 * 100_000 + (y * 1000.0) as i64),
        "ref": r, "number": "1", "net": net, "attr": "smd",
        "center": [n(x), n(y)], "rotation_deg": 0.0, "layers": [0],
        "shapes": {"0": {"kind": "rect", "w": n(s), "h": n(s)}},
        "drill": null, "local_clearance": null, "net_tie_group": null, "spoke_angle_deg": 45.0
    })
}

/// The fixture board (KiCad frame, mm) with the pads in `order`.
fn model(pads: Vec<serde_json::Value>) -> BoardModel {
    let (w, h) = (30 * MM, 30 * MM);
    let cls = serde_json::json!({"clearance": 200000, "width": 250000, "via_d": 600000, "via_drill": 300000, "netclass": "Default", "width_source": "default", "min_width": 0});
    let j = serde_json::json!({
        "units": "nm", "board": "fuse_shared_pad",
        "layers": [{"id": 0, "name": "F.Cu", "kind": "signal", "routable": true}, {"id": 2, "name": "B.Cu", "kind": "signal", "routable": true}],
        "nets": [{"code": 1, "name": "N"}],
        "pads": pads,
        "vias": [], "kept": [], "copper_shapes": [], "holes": [],
        "outline": [
            {"kind": "seg", "a": [0, 0], "b": [w, 0]}, {"kind": "seg", "a": [w, 0], "b": [w, h]},
            {"kind": "seg", "a": [w, h], "b": [0, h]}, {"kind": "seg", "a": [0, h], "b": [0, 0]}
        ],
        "keepouts": [{"name": "", "layers": [2], "poly": [[0, 0], [w, 0], [w, h], [0, h]], "no_tracks": true, "no_vias": true, "no_pour": false, "no_pads": false, "no_footprints": false}],
        "net_ties": [], "pours": [], "teardrops": [], "dangling_ends": [],
        "rules": {"min_clearance": 200000, "min_track_width": 250000, "min_via_diameter": 400000, "min_via_drill": 300000, "min_annular": 50000, "hole_clearance": 250000, "via_hole_clearance": 250000, "hole_to_hole": 250000, "edge_clearance": 500000, "max_error": 5000, "min_connection": 0, "profile": "board", "predefined_vias": [], "default": cls, "per_net": {"1": cls}, "floors_applied": [], "fallbacks": [], "warnings": [], "ignored_checks": [], "clearance_regions": [], "outline_half_stroke": 25000}
    });
    BoardModel::from_json(&j.to_string()).unwrap()
}

fn route(m: &BoardModel, extra: &[(&str, serde_json::Value)]) -> RouteResult {
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    topor_engine::route::route_with(m, &o, &RunControl::new())
}

fn tracks(r: &RouteResult) -> Vec<String> {
    r.items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            format!("({:.3},{:.3})-({:.3},{:.3})", a.x as f64 / 1e6, a.y as f64 / 1e6, b.x as f64 / 1e6, b.y as f64 / 1e6)
        })
        .collect()
}

fn line<'a>(r: &'a RouteResult, start: &str) -> Option<&'a str> {
    r.warnings.iter().map(|s| s.as_str()).find(|s| s.starts_with(start))
}


/// Track segments off the 0/45/90 degree grid (the harness judge's rule), as length mm.
fn free_angle(r: &RouteResult) -> Vec<f64> {
    r.items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .filter_map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            let (dx, dy) = (((b.x - a.x) as f64).abs(), ((b.y - a.y) as f64).abs());
            (dx > 1e3 && dy > 1e3 && (dx - dy).abs() > 0.01 * dx.max(dy) + 1e3).then(|| dx.hypot(dy) / 1e6)
        })
        .collect()
}

/// How far (mm) the nearest track of the result passes from the point (x, y) mm (KiCad frame).
fn copper_near(r: &RouteResult, x: f64, y: f64) -> f64 {
    let p = Vec2 { x: x * 1e6, y: y * 1e6 };
    r.items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            let (a, b) = (Vec2 { x: a.x as f64, y: a.y as f64 }, Vec2 { x: b.x as f64, y: b.y as f64 });
            let ab = b - a;
            let u = ((p - a).dot(ab) / ab.len2().max(1.0)).clamp(0.0, 1.0);
            (a + ab * u - p).len() / 1e6
        })
        .fold(f64::INFINITY, f64::min)
}

/// The copper pieces of net 1 in the result and its dangling track ends (tracks and the 1 mm pads; KiCad's anchor rule,
/// [`topor_engine::topo::netconn`]).
fn pieces(r: &RouteResult, pads: &[(f64, f64)]) -> (usize, usize) {
    use topor_engine::topo::netconn::{conn, Piece, Term};
    use topor_engine::topo::state::LayerMask;
    type Runs = Vec<(usize, Vec<Vec2>)>;
    let runs: Vec<(Runs, f64)> = r
        .items
        .iter()
        .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
        .filter(|t| t.net == 1)
        .map(|t| {
            let (a, b) = (t.prim.start(), t.prim.end());
            (vec![(0usize, vec![Vec2 { x: a.x as f64, y: a.y as f64 }, Vec2 { x: b.x as f64, y: b.y as f64 }])], t.width as f64 / 2.0)
        })
        .collect();
    let pcs: Vec<Piece> = runs.iter().map(|(r, hw)| Piece { runs: r, vias: vec![], half_w: *hw }).collect();
    let polys: Vec<(Vec2, Vec<Vec2>)> = pads
        .iter()
        .map(|&(x, y)| {
            let c = Vec2 { x: x * 1e6, y: y * 1e6 };
            let h = 0.5e6;
            (c, vec![Vec2 { x: c.x - h, y: c.y - h }, Vec2 { x: c.x + h, y: c.y - h }, Vec2 { x: c.x + h, y: c.y + h }, Vec2 { x: c.x - h, y: c.y + h }])
        })
        .collect();
    let ts: Vec<Term> = polys.iter().map(|(c, p)| Term { poly: p, layers: LayerMask(1), center: *c }).collect();
    let c = conn(&pcs, &ts);
    (c.n(), c.dangling.len())
}

fn shared_pad(a: (f64, f64), b: (f64, f64)) -> BoardModel {
    model(vec![pad("P", 1, 25.0, 15.0, 1.0), pad("A", 1, a.0, a.1, 1.0), pad("B", 1, b.0, b.1, 1.0)])
}

/// M2: the large legal fuse goes first whatever the wire ids (both variants of `fuse_shared_pad`).
#[test]
fn the_large_legal_fuse_at_a_shared_pad_goes_first_whatever_the_wire_ids() {
    // long_a: the A wire has the higher id. Unfused, A runs west along y = 15 from P; the small fuse
    // (B onto A) would start B at (22, 15) on it. The large one starts A at (22, 12) on B's copper.
    let pads = [(25.0, 15.0), (5.0, 17.0), (22.0, 4.0)];
    let r = route(&shared_pad(pads[1], pads[2]), &[]);
    let l = line(&r, "fuse after drawing").expect("fuse line");
    assert!(l.starts_with("fuse after drawing: 1 trace ends moved") && l.contains("1.8 mm saved"), "{l}");
    assert!(copper_near(&r, 15.0, 15.0) > 1.0 && copper_near(&r, 22.0, 12.0) < 1e-3, "A leaves B at (22, 12): {:?}", tracks(&r));
    assert_eq!(pieces(&r, &pads), (1, 0), "{:?}", tracks(&r));
    let off = route(&shared_pad(pads[1], pads[2]), &[("fuse_branches", serde_json::json!(false))]);
    assert!(copper_near(&off, 15.0, 15.0) < 1e-3, "unfused, A runs along y = 15: {:?}", tracks(&off));
    // short_a: the A wire has the lower id; the small fuse (B at P, 0.19 mm) has the higher. Unfused,
    // A runs along y = 17 from (23, 17); the large fuse starts A at (22, 12) on B's copper.
    let pads = [(25.0, 15.0), (11.0, 17.0), (22.0, 1.0)];
    let r = route(&shared_pad(pads[1], pads[2]), &[]);
    let l = line(&r, "fuse after drawing").expect("fuse line");
    assert!(l.starts_with("fuse after drawing: 1 trace ends moved") && l.contains("1.8 mm saved"), "{l}");
    assert!(copper_near(&r, 18.0, 17.0) > 1.0 && copper_near(&r, 16.0, 12.0) < 1e-3, "A leaves B at (22, 12): {:?}", tracks(&r));
    assert_eq!(pieces(&r, &pads), (1, 0), "{:?}", tracks(&r));
    let off = route(&shared_pad(pads[1], pads[2]), &[("fuse_branches", serde_json::json!(false))]);
    assert!(copper_near(&off, 18.0, 17.0) < 1e-3, "unfused, A runs along y = 17: {:?}", tracks(&off));
}

fn route_h(m: &BoardModel, extra: &[(&str, serde_json::Value)], hooks: &'static [&'static str]) -> RouteResult {
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    let mut ctl = RunControl::new();
    ctl.set_test_hook(move |pt| hooks.contains(&pt));
    topor_engine::route::route_with(m, &o, &ctl)
}

const FREE_TRUNK: [(f64, f64); 3] = [(5.0, 5.0), (25.0, 15.0), (11.0, 25.0)];

fn free_trunk(hooks: &'static [&'static str]) -> RouteResult {
    let m = model(FREE_TRUNK.iter().zip(["A", "B", "C"]).map(|(&(x, y), n)| pad(n, 1, x, y, 1.0)).collect());
    // `via_reduce` off: the variant before the via reduction (no fuse, no polish) is not a
    // candidate, so the result is the fused and polished copper.
    route_h(&m, &[("factors", serde_json::json!({"weight": {"any_angle": 0.0}})), ("via_reduce", serde_json::json!(false))], hooks)
}

/// H1 part 1 (qmtech JS2_9 r3, main's JS1_2): the polish keeps a fused foot on a free-angle trunk
/// segment as a vertex, so both redrawn pieces end on it.
#[test]
fn the_polish_keeps_the_foot_of_a_fused_branch_on_its_free_angle_trunk() {
    let r = free_trunk(&[]);
    let l = line(&r, "fuse after drawing").expect("fuse line");
    assert!(l.starts_with("fuse after drawing: 1 trace ends moved"), "the fixture reaches the fuse: {l}");
    assert!(copper_near(&r, 16.486, 21.081) < 1e-3, "{:?}", tracks(&r));
    assert_eq!(pieces(&r, &FREE_TRUNK), (1, 0), "{:?}", tracks(&r));
    assert!(free_angle(&r).is_empty(), "the trunk is polished in two pieces: {:?}", tracks(&r));
    assert!(line(&r, "split guard").is_none(), "{:?}", line(&r, "split guard"));
}

/// H1 part 2: the split guard. With the polish's fixed vertices taken away (hook "polish drops
/// feet", the r3 bug) every elbow and Z of the trunk segment drops the foot; the guard refuses
/// them, the net stays one piece and the report says so.
#[test]
fn the_split_guard_refuses_a_polish_that_drops_a_branch_foot() {
    let r = free_trunk(&["polish drops feet"]);
    assert_eq!(pieces(&r, &FREE_TRUNK), (1, 0), "{:?}", tracks(&r));
    let l = line(&r, "split guard after drawing: ").expect("split guard line");
    assert!(l.starts_with("split guard after drawing: 0 fuse and 4 polish steps not taken"), "{l}");
    assert!(!l.contains("engine bug"), "{l}");
    assert!(!free_angle(&r).is_empty(), "the trunk segment under the foot stays as drawn: {:?}", tracks(&r));
}

/// H1 part 2, the self-check (rule 12: every path): with the guard off as well, the dropped foot
/// dangles, and the self-check after the steps names the net as an engine bug.
#[test]
fn the_split_self_check_names_a_net_the_steps_split() {
    let r = free_trunk(&["polish drops feet", "split guard off"]);
    // The branch still crosses the elbow of the trunk (KiCad joins crossing copper: one piece), but
    // its foot is left dangling (KiCad Cleanup "dangling track").
    assert_eq!(pieces(&r, &FREE_TRUNK), (1, 1), "{:?}", tracks(&r));
    let l = line(&r, "split guard after drawing: ").expect("split guard line");
    assert!(l.starts_with("split guard after drawing: 0 fuse and 0 polish steps not taken") && l.ends_with("; engine bug: 1 net with copper apart or a new dangling end after the steps after the drawing: N"), "{l}");
}

/// The fixtures' copper for the approval PNGs (fuse-octi r5, shopping-list rule: patterns are
/// user-approved first): `TOPO_FIXTURE_DUMP=<dir> cargo test --release --test topo_fuse_order --
/// --ignored dump` writes `<dir>/<fixture>.json` (pads, tracks fused and with fuse_branches off,
/// KiCad frame mm); scratch tooling draws them with the expected answer.
#[test]
#[ignore]
fn dump_the_fixtures_for_the_approval_pngs() {
    let Ok(dir) = std::env::var("TOPO_FIXTURE_DUMP") else { return };
    let segs = |r: &RouteResult| -> Vec<[f64; 4]> {
        r.items
            .iter()
            .filter_map(|i| if let Item::Track(t) = i { Some(t) } else { None })
            .map(|t| {
                let (a, b) = (t.prim.start(), t.prim.end());
                [a.x as f64 / 1e6, a.y as f64 / 1e6, b.x as f64 / 1e6, b.y as f64 / 1e6]
            })
            .collect()
    };
    let off = [("fuse_branches", serde_json::json!(false))];
    for (name, pads) in [("fuse_shared_pad-long_a", [(25.0, 15.0), (5.0, 17.0), (22.0, 4.0)]), ("fuse_shared_pad-short_a", [(25.0, 15.0), (11.0, 17.0), (22.0, 1.0)])] {
        let m = shared_pad(pads[1], pads[2]);
        let (r, u) = (route(&m, &[]), route(&m, &off));
        let j = serde_json::json!({"pads": pads, "names": ["P", "A", "B"], "fused": segs(&r), "unfused": segs(&u), "lines": r.warnings.iter().filter(|w| w.starts_with("fuse")).collect::<Vec<_>>()});
        std::fs::write(format!("{dir}/{name}.json"), j.to_string()).unwrap();
    }
    let r = free_trunk(&[]);
    let g = free_trunk(&["polish drops feet"]);
    let j = serde_json::json!({"pads": FREE_TRUNK, "names": ["A", "B", "C"], "fused": segs(&r), "guarded": segs(&g), "lines": r.warnings.iter().filter(|w| w.starts_with("fuse") || w.starts_with("octi") || w.starts_with("split")).collect::<Vec<_>>()});
    std::fs::write(format!("{dir}/fuse_free_trunk.json"), j.to_string()).unwrap();
}

/// r5 verifier L4 (survivor best_of_trunk_min): three wires end at pad P (fixture
/// `fuse_shared_pad` with a third pad C at (25, 27), PROPOSED pattern). A's wire end at P has two
/// trunks: B's copper (best legal gain 4.22, A leaves it at (22, 12)) and C's (1.08). The end's key
/// is the best over its trunks, so A fuses onto B; a key from the worse trunk (1.08) would let B's
/// end (3.95, onto A's copper at (22, 15)) go first instead.
#[test]
fn an_end_with_two_trunks_is_keyed_by_its_best_trunk() {
    let pads = [(25.0, 15.0), (5.0, 17.0), (22.0, 4.0), (25.0, 27.0)];
    let m = model(pads.iter().zip(["P", "A", "B", "C"]).map(|(&(x, y), n)| pad(n, 1, x, y, 1.0)).collect());
    let r = route(&m, &[]);
    let l = line(&r, "fuse after drawing").expect("fuse line");
    assert!(l.starts_with("fuse after drawing: 1 trace ends moved") && l.contains("1.8 mm saved"), "{l}");
    assert!(copper_near(&r, 15.0, 12.0) < 1e-3 && copper_near(&r, 15.0, 15.0) > 1.0, "A leaves B's copper at (22, 12): {:?}", tracks(&r));
    assert_eq!(pieces(&r, &pads), (1, 0), "{:?}", tracks(&r));
}
