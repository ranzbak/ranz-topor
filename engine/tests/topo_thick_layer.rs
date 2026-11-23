//! thick-layer (docs/dev/design/thick-layer.md): a wide trace among thin crossers.
//!
//! Fixture `thick_dive_grouping` (synthetic, built here; PROPOSED pattern, PNG in the thick-layer
//! report; the qmtech VIN5V F3.2-F1.2 mechanism, reports/alt-route-probe-20261122-2225.md): P (1.0 mm,
//! via 0.9/0.5, clearance 0.15) runs from a PTH pad at (1, 7) to one at (15, 7), both against the
//! outline (no way round its ends); seven 0.2 mm signals run from y 1 to y 13 between SMD pads that
//! fix their layers, order F1 F2 . B1 F3 F4 B2 B3 at x 3, 4, 8, 9, 10, 11, 12. Expected (optimal):
//! P on B.Cu from its left pad, one via near x 6.85, F.Cu to the right pad; F3 / F4 jog left over B1
//! and cross P inside its B.Cu stretch. 1 via, 0 violations.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::RouteResult;

const MM: f64 = 1e6;

fn n(v: f64) -> i64 {
    (v * MM).round() as i64
}

fn uuid(k: usize) -> String {
    format!("00000000-0000-4000-8000-{k:012}")
}

fn pth(k: usize, r: &str, net: i32, x: f64, y: f64, d: f64, drill: f64) -> serde_json::Value {
    let s = serde_json::json!({"kind": "oval", "w": n(d), "h": n(d)});
    serde_json::json!({"uuid": uuid(k), "ref": r, "number": "1", "net": net, "attr": "pth", "center": [n(x), n(y)], "rotation_deg": 0.0,
        "layers": [0, 2], "shapes": {"0": s, "2": s}, "drill": {"w": n(drill), "h": n(drill)}, "local_clearance": null, "net_tie_group": null})
}

fn smd(k: usize, r: &str, net: i32, x: f64, y: f64, layer: u32) -> serde_json::Value {
    let mut shapes = serde_json::Map::new();
    shapes.insert(layer.to_string(), serde_json::json!({"kind": "rect", "w": n(0.5), "h": n(1.0)}));
    serde_json::json!({"uuid": uuid(k), "ref": r, "number": "1", "net": net, "attr": "smd", "center": [n(x), n(y)], "rotation_deg": 0.0,
        "layers": [layer], "shapes": shapes, "drill": null, "local_clearance": null, "net_tie_group": null})
}

/// The signals of `thick_dive_grouping`: (name, x, layer id).
const SIGNALS: [(&str, f64, u32); 7] = [("F1", 3.0, 0), ("F2", 4.0, 0), ("B1", 8.0, 2), ("F3", 9.0, 0), ("F4", 10.0, 0), ("B2", 11.0, 2), ("B3", 12.0, 2)];

pub fn thick_dive_grouping() -> BoardModel {
    thick_dive_grouping_with(&SIGNALS)
}

/// [`thick_dive_grouping`] with other signals (name, x, layer id).
fn thick_dive_grouping_with(signals: &[(&str, f64, u32)]) -> BoardModel {
    thick_dive_grouping_layers(signals, 2, false)
}

/// [`thick_dive_grouping_with`] on 2 copper layers (F.Cu, B.Cu) or 4 (F.Cu, In1.Cu, In2.Cu, B.Cu;
/// `p_smd`: P's pads are SMD on F.Cu, else PTH with a ring on every layer; the signals' SMD pads stay on their layer (F 0, In1 4, In2 6, B 2)).
fn thick_dive_grouping_layers(signals: &[(&str, f64, u32)], layers: usize, p_smd: bool) -> BoardModel {
    let (w, h) = (n(16.0), n(14.0));
    let sig = serde_json::json!({"clearance": n(0.15), "width": n(0.2), "via_d": n(0.6), "via_drill": n(0.3), "netclass": "Default", "width_source": "default", "min_width": 0});
    let pwr = serde_json::json!({"clearance": n(0.15), "width": n(1.0), "via_d": n(0.9), "via_drill": n(0.5), "netclass": "Power", "width_source": "netclass", "min_width": 0});
    let mut nets = vec![serde_json::json!({"code": 1, "name": "P"})];
    let mut pads = vec![pth(1, "PA", 1, 1.0, 7.0, 1.4, 0.7), pth(2, "PB", 1, 15.0, 7.0, 1.4, 0.7)];
    if p_smd {
        // P's pads are SMD on F.Cu: P starts and ends on F.Cu and may leave it only by vias.
        pads = vec![smd_rect(1, "PA", 1, 1.0, 7.0, 1.4, 1.4), smd_rect(2, "PB", 1, 15.0, 7.0, 1.4, 1.4)];
    } else if layers == 4 {
        for p in pads.iter_mut() {
            let s = p["shapes"]["0"].clone();
            p["layers"] = serde_json::json!([0, 4, 6, 2]);
            p["shapes"]["4"] = s.clone();
            p["shapes"]["6"] = s;
        }
    }
    let layer_json = if layers == 4 {
        serde_json::json!([{"id": 0, "name": "F.Cu", "kind": "signal", "routable": true}, {"id": 4, "name": "In1.Cu", "kind": "signal", "routable": true}, {"id": 6, "name": "In2.Cu", "kind": "signal", "routable": true}, {"id": 2, "name": "B.Cu", "kind": "signal", "routable": true}])
    } else {
        serde_json::json!([{"id": 0, "name": "F.Cu", "kind": "signal", "routable": true}, {"id": 2, "name": "B.Cu", "kind": "signal", "routable": true}])
    };
    let mut per_net = serde_json::Map::new();
    per_net.insert("1".into(), pwr);
    for (i, &(name, x, l)) in signals.iter().enumerate() {
        let code = i as i32 + 2;
        nets.push(serde_json::json!({"code": code, "name": name}));
        pads.push(smd(10 + 2 * i, &format!("{name}T"), code, x, 1.0, l));
        pads.push(smd(11 + 2 * i, &format!("{name}B"), code, x, 13.0, l));
        per_net.insert(code.to_string(), sig.clone());
    }
    let j = serde_json::json!({
        "units": "nm", "board": "thick_dive_grouping",
        "layers": layer_json,
        "nets": nets, "pads": pads,
        "vias": [], "kept": [], "copper_shapes": [], "holes": [],
        "outline": [
            {"kind": "seg", "a": [0, 0], "b": [w, 0]}, {"kind": "seg", "a": [w, 0], "b": [w, h]},
            {"kind": "seg", "a": [w, h], "b": [0, h]}, {"kind": "seg", "a": [0, h], "b": [0, 0]}
        ],
        "keepouts": [], "net_ties": [], "pours": [], "teardrops": [], "dangling_ends": [],
        "rules": {"min_clearance": n(0.15), "min_track_width": n(0.2), "min_via_diameter": n(0.4), "min_via_drill": n(0.3), "min_annular": n(0.05), "hole_clearance": n(0.25), "via_hole_clearance": n(0.25), "hole_to_hole": n(0.25), "edge_clearance": n(0.3), "max_error": 5000, "min_connection": 0, "profile": "board", "predefined_vias": [], "default": sig, "per_net": per_net, "floors_applied": [], "fallbacks": [], "warnings": [], "ignored_checks": [], "clearance_regions": [], "outline_half_stroke": 25000}
    });
    BoardModel::from_json(&j.to_string()).expect("fixture parses")
}

fn smd_rect(k: usize, r: &str, net: i32, x: f64, y: f64, w: f64, h: f64) -> serde_json::Value {
    let mut v = smd(k, r, net, x, y, 0);
    v["shapes"]["0"] = serde_json::json!({"kind": "rect", "w": n(w), "h": n(h)});
    v
}

/// Fixture `wide_one_side` (PROPOSED pattern, design M1; scratch/boards/thick-layer/pat/wide_one_side.py
/// r2): P (1.0 mm, via 0.9/0.5) between SMD pads on F.Cu at (1, 7) and (15, 7); S1-S5 (0.2 mm)
/// top to bottom between PTH pads (1.0/0.5) at x 3.5, 5, 6.5, 8, 9.5 (either layer); S6 top to
/// bottom between SMD pads on F.Cu at x 12, so S6 or P must change layer where they cross.
/// Expected: P straight on F.Cu with 0 vias, S1-S5 straight on B.Cu, S6 dives under P with 2
/// small vias; 86.0 mm of track in all (14 + 6 x 12).
pub fn wide_one_side() -> BoardModel {
    let (w, h) = (n(16.0), n(14.0));
    let sig = serde_json::json!({"clearance": n(0.15), "width": n(0.2), "via_d": n(0.6), "via_drill": n(0.3), "netclass": "Default", "width_source": "default", "min_width": 0});
    let pwr = serde_json::json!({"clearance": n(0.15), "width": n(1.0), "via_d": n(0.9), "via_drill": n(0.5), "netclass": "Power", "width_source": "netclass", "min_width": 0});
    let mut nets = vec![serde_json::json!({"code": 1, "name": "P"})];
    let mut pads = vec![smd_rect(1, "PA", 1, 1.0, 7.0, 1.4, 1.4), smd_rect(2, "PB", 1, 15.0, 7.0, 1.4, 1.4)];
    let mut per_net = serde_json::Map::new();
    per_net.insert("1".into(), pwr);
    for (i, x) in [3.5, 5.0, 6.5, 8.0, 9.5].into_iter().enumerate() {
        let code = i as i32 + 2;
        nets.push(serde_json::json!({"code": code, "name": format!("S{}", i + 1)}));
        pads.push(pth(10 + 2 * i, &format!("S{}T", i + 1), code, x, 1.0, 1.0, 0.5));
        pads.push(pth(11 + 2 * i, &format!("S{}B", i + 1), code, x, 13.0, 1.0, 0.5));
        per_net.insert(code.to_string(), sig.clone());
    }
    nets.push(serde_json::json!({"code": 7, "name": "S6"}));
    pads.push(smd(30, "S6T", 7, 12.0, 1.0, 0));
    pads.push(smd(31, "S6B", 7, 12.0, 13.0, 0));
    per_net.insert("7".into(), sig.clone());
    let j = serde_json::json!({
        "units": "nm", "board": "wide_one_side",
        "layers": [{"id": 0, "name": "F.Cu", "kind": "signal", "routable": true}, {"id": 2, "name": "B.Cu", "kind": "signal", "routable": true}],
        "nets": nets, "pads": pads,
        "vias": [], "kept": [], "copper_shapes": [], "holes": [],
        "outline": [
            {"kind": "seg", "a": [0, 0], "b": [w, 0]}, {"kind": "seg", "a": [w, 0], "b": [w, h]},
            {"kind": "seg", "a": [w, h], "b": [0, h]}, {"kind": "seg", "a": [0, h], "b": [0, 0]}
        ],
        "keepouts": [], "net_ties": [], "pours": [], "teardrops": [], "dangling_ends": [],
        "rules": {"min_clearance": n(0.15), "min_track_width": n(0.2), "min_via_diameter": n(0.4), "min_via_drill": n(0.3), "min_annular": n(0.05), "hole_clearance": n(0.25), "via_hole_clearance": n(0.25), "hole_to_hole": n(0.25), "edge_clearance": n(0.3), "max_error": 5000, "min_connection": 0, "profile": "board", "predefined_vias": [], "default": sig, "per_net": per_net, "floors_applied": [], "fallbacks": [], "warnings": [], "ignored_checks": [], "clearance_regions": [], "outline_half_stroke": 25000}
    });
    BoardModel::from_json(&j.to_string()).expect("fixture parses")
}

/// Per net: (vias, track length mm) of a route.
fn per_net(r: &RouteResult) -> std::collections::BTreeMap<i32, (usize, f64)> {
    let mut m = std::collections::BTreeMap::new();
    for it in &r.items {
        let v = serde_json::to_value(it).unwrap();
        let net = v["net"].as_i64().unwrap() as i32;
        let e = m.entry(net).or_insert((0usize, 0.0f64));
        if v.get("d").is_some() {
            e.0 += 1;
        } else if v["prim"]["kind"] == "seg" {
            let (a, b) = (&v["prim"]["a"], &v["prim"]["b"]);
            let d = ((a[0].as_f64().unwrap() - b[0].as_f64().unwrap()).powi(2) + (a[1].as_f64().unwrap() - b[1].as_f64().unwrap()).powi(2)).sqrt();
            e.1 += d / MM;
        }
    }
    m
}

/// Diagnosis print (run with --nocapture): wide_one_side per net, ω off and on.
#[test]
#[ignore]
fn diag_wide_one_side() {
    let m = wide_one_side();
    for pw in [serde_json::json!([0.0, 0.0]), serde_json::json!([1.0, 0.5])] {
        let r = route(&m, &[("via_priority_weight", pw.clone())]);
        println!("priority_weight {pw}: per net (vias, mm) {:?}; violations {}", per_net(&r), r.violations.len());
        for it in &r.items {
            println!("item {}", serde_json::to_string(it).unwrap());
        }
    }
}

fn route(m: &BoardModel, extra: &[(&str, serde_json::Value)]) -> RouteResult {
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["wall_clock_s"] = serde_json::json!(600.0);
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().expect("pool");
    pool.install(|| {
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        topor_engine::route::route_with(m, &o, &ctl)
    })
}

fn plan(r: &RouteResult) -> serde_json::Value {
    serde_json::from_str(r.plan.as_deref().expect("plan")).expect("json")
}

/// Diagnosis print (run with --nocapture): what the plan of P says and what was drawn.
#[test]
#[ignore]
fn diag_thick_dive_grouping() {
    let m = thick_dive_grouping();
    let sp = std::env::var("DIAG_SITE_PLANNED").is_ok();
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(sp))]);
    let p = plan(&r);
    for w in p["wires"].as_array().unwrap().iter().filter(|w| w["net_name"] == "P") {
        println!("P wire: layers_now {} vias {} crossings {}", w["layers_now"], w["vias"], w["crossings"]);
        println!("P events {} passes-at {:?}", w["events"], w["passes"].as_array().unwrap().iter().map(|p| p["at"].clone()).collect::<Vec<_>>());
    }
    println!("conns {}", p["conns"]);
    println!("predicted {} violations {}", p["predicted"], p["predicted_violations"]);
    for v in &r.violations {
        println!("drawn violation: {}", serde_json::to_string(v).unwrap());
    }
    println!("vias {} routed {}/{}", r.vias, r.routed, r.connections_total);
    for it in &r.items {
        println!("item {}", serde_json::to_string(it).unwrap());
    }
    for w in r.warnings.iter().filter(|w| w.contains("ERBS") || w.contains("relocat") || w.contains("via site")) {
        println!("warning: {w}");
    }
}

/// S1 target (design M3, F1; failed on 7513b3db, passes with `[vias] site_planned`): what the plan hands the drawing predicts the
/// drawn violations of P. Found with the plan report (S1 diagnosis): P's last via interval lies in
/// traversal 7 between its crossings with F4 (event s 0.44) and B2 (s 0.65); the site
/// ([`end_site`]) is chosen on the pass-point polyline, where the two crossings are ~1 mm apart, at
/// (11.27, 6.50); on the taut lines the drawing realises, F4 is at x 10 and B2 at x 11, so the
/// point lies past B2 and the 1.0 mm gap holds no 0.9 mm via with P's clearances (0.5 mm free):
/// predicted 0, drawn 2 shorts with B2 / B3 and 4 clearance items.
#[test]
fn thick_dive_grouping_predicts_what_it_draws() {
    let m = thick_dive_grouping();
    // ω off (P weaves, as main) and the end-site test alone (`site_lanes` off), the crossers'
    // taut tracks as walls (`site_slack` 0); the slack case (round 4: the drawing realises it)
    // is `thick_dive_grouping_predicts_what_it_draws_with_slack`.
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(true)), ("via_site_slack", serde_json::json!(0.0))]);
    let p = plan(&r);
    let drawn = r.violations.iter().filter(|v| v.conn == 0).count();
    let pred: u64 = p["conns"]["0"]["predicted"].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum();
    assert!(drawn == 0 || pred > 0, "P: drawn {drawn} violations, predicted {pred}: {}", p["conns"]["0"]);
}

/// M3 (`[vias] site_planned`, `site_slack` 0 = taut tracks as walls): P's vias between B1 / F3
/// and between F4 / B2 sit in 1.0 mm gaps between the crossers' taut lines, too narrow for the 0.9 mm via with clearances (1.4 mm): with
/// the option both are vias without a site, deficit = (1.4 - 1.0) / 2 = 0.2 mm each (the
/// lane room, not the static room, which has space); without it (the static room alone, main)
/// every via has a site and the plan predicts nothing. ω off (`priority_weight [0, 0]`), so P
/// keeps its 3 vias.
#[test]
fn thick_dive_grouping_lane_sites() {
    let m = thick_dive_grouping();
    let sites = |on: bool| {
        let r = route(&m, &[("via_site_planned", serde_json::json!(on)), ("via_site_lanes", serde_json::json!(on)), ("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_slack", serde_json::json!(0.0))]);
        let p = plan(&r);
        let w = p["wires"].as_array().unwrap().iter().find(|w| w["net_name"] == "P").unwrap().clone();
        w["vias"].as_array().unwrap().iter().map(|v| (v["site"].as_bool().unwrap(), v["deficit"].as_f64().unwrap())).collect::<Vec<_>>()
    };
    let on = sites(true);
    assert_eq!(on.len(), 3, "{on:?}");
    assert!(on[0].0 && on[0].1 == 0.0, "via 0 (between F2 and B1, 4 mm) has a site: {on:?}");
    for v in &on[1..] {
        assert!(!v.0 && (v.1 - 0.2).abs() < 0.05, "1 mm gap: no site, deficit ~0.2 mm: {on:?}");
    }
    let off = sites(false);
    assert!(off.iter().all(|v| v.0 && v.1 == 0.0), "static room alone: every via has a site: {off:?}");
}

/// S2 (M1, `[vias] priority_weight`): with ω ([1, 0.5]: P's vias weigh 1 + 4 + 0.5 = 5.5)
/// the layering keeps P on one layer and lets the thin crossers on that layer dive (2 small vias
/// each, ω 1): P 0 vias, 0 drawn violations. With ω 1 for every via (main) P weaves with 3 vias,
/// two of them in 1 mm gaps (the S1 items).
#[test]
fn thick_dive_grouping_wide_keeps_its_layer() {
    let m = thick_dive_grouping();
    let r = route(&m, &[("via_priority_weight", serde_json::json!([1.0, 0.5]))]);
    let pn = per_net(&r);
    assert_eq!(pn[&1].0, 0, "P keeps its layer: {pn:?}");
    assert_eq!(r.violations.len(), 0, "{:?}", r.violations);
    let r0 = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0]))]);
    assert_eq!(per_net(&r0)[&1].0, 3, "ω 1: P weaves (main): {:?}", per_net(&r0));
}

/// M1: ω = 1 + a_w · max(0, w / w_def − 1) + a_t when wider than the Default width; 1 at or
/// below it, and for a_w = a_t = 0; grows with the width (graded).
#[test]
fn omega_formula() {
    use topor_engine::topo::layers::Omega;
    let o = Omega { a_w: 1.0, a_t: 0.5, w_def: 0.2e6 };
    assert_eq!(o.of_width(0.2e6), 1.0);
    assert_eq!(o.of_width(0.15e6), 1.0);
    assert!((o.of_width(1.0e6) - 5.5).abs() < 1e-12);
    assert!((o.of_width(0.3e6) - 2.0).abs() < 1e-12);
    assert!(o.of_width(0.5e6) < o.of_width(0.6e6));
    assert_eq!(Omega { a_w: 0.0, a_t: 0.0, w_def: 0.2e6 }.of_width(1.0e6), 1.0);
    assert_eq!(Omega { a_w: 1.0, a_t: 0.5, w_def: 0.0 }.of_width(1.0e6), 1.0);
}

/// S3 (M2.3, `[vias] pair_one_side` with `[vias] pair_hold`, lead 2026-11-23): the one-side move
/// holds P's run on its end layer through the layering that follows the move, as a graded release
/// price per event off that layer (not a lock). Round 2 (no hold): the layering after the move was
/// free and wove P again at once, so the move changed nothing. Measured on the layering right
/// after each one-side move (metrics `pair_move_kinds.one_side.left_changes`, P's layer changes
/// summed over the tries): a strong hold (10 via prices per event) leaves none, no hold leaves
/// the weave. The board's evaluation still decides: here (ω off, static via sites) the held
/// result has more vias (5 against the weave's 3, LOOPDBG "eval 173.268 -> 223.268") and is
/// rightly not kept; the expected 1-via answer needs F3 / F4 to change their topology (jog over
/// B1), not a layer move.
#[test]
fn thick_dive_grouping_one_side_move() {
    let m = thick_dive_grouping();
    let left = |hold: f64| {
        let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_pair_one_side", serde_json::json!(true)), ("via_pair_hold", serde_json::json!(hold))]);
        let pm = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).expect("plan metrics");
        let v: serde_json::Value = serde_json::from_str(pm.trim_start_matches("plan metrics:").trim()).expect("json");
        let os = &v["loop"]["pair_move_kinds"]["one_side"];
        (os["tried"].as_u64().unwrap_or(0), os["left_changes"].as_u64().unwrap_or(0))
    };
    let (free, held) = (left(0.0), left(10.0));
    assert!(free.0 > 0 && held.0 > 0, "one-side moves tried: free {free:?} held {held:?}");
    assert!(free.1 > 0, "no hold: the layering weaves P again: {free:?}");
    assert_eq!(held.1, 0, "hold: P stays on its side in the move's layering: {held:?}");
}

/// M3 end sites (`site_planned` on, `site_lanes` off, `site_slack` 0) and ω off: only the end site (via 3, between F4 and B2
/// in one traversal) is measured on the taut tracks; it has no room there (predicted), so the
/// TopoR loop reroutes P (its via 3 moves above the crossers' ends of room): 1 drawn clearance
/// item (B2 against P, 0.05 mm) instead of main's 6 (2 shorts). Without the option: main's 6.
#[test]
fn thick_dive_grouping_end_sites_only() {
    let m = thick_dive_grouping();
    let o = |on: bool| route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(on)), ("via_site_slack", serde_json::json!(0.0))]);
    let (on, off) = (o(true), o(false));
    let shorts = |r: &RouteResult| r.violations.iter().filter(|v| v.kind == "short").count();
    assert_eq!((off.violations.len(), shorts(&off)), (6, 2), "main: {:?}", off.violations);
    assert!(on.violations.len() < off.violations.len() && shorts(&on) == 0, "{:?}", on.violations);
}

/// M3 slack (`[vias] site_slack`, default 1): each crosser of P's 1 mm gaps has 1 mm to its
/// planned neighbour (0.65 mm of room past the spacing), and the via needs 0.2 mm on each side:
/// with the slack every via of P has a site and no deficit.
#[test]
fn thick_dive_grouping_lane_sites_with_slack() {
    let m = thick_dive_grouping();
    let r = route(&m, &[("via_site_planned", serde_json::json!(true)), ("via_site_lanes", serde_json::json!(true)), ("via_priority_weight", serde_json::json!([0.0, 0.0]))]);
    let p = plan(&r);
    let w = p["wires"].as_array().unwrap().iter().find(|w| w["net_name"] == "P").unwrap().clone();
    let v: Vec<(bool, f64)> = w["vias"].as_array().unwrap().iter().map(|v| (v["site"].as_bool().unwrap(), v["deficit"].as_f64().unwrap())).collect();
    assert!(!v.is_empty() && v.iter().all(|x| x.0 && x.1 == 0.0), "{v:?}");
}

/// M3 slack, drawn half (round 4): with the slack the plan predicts every via of P a site (the
/// crossers move 0.2 mm aside), and the drawing now realises it. The via relocation
/// ([`viaopt::relocate_vias`]) bounds each via by the crossers' crossings farthest out and measures
/// its gap against the allowed crosser position farthest from it (`refine::shifted_lanes_of`, the
/// plan's model; a crosser stays between the via and the window's other end), so the planned
/// sites are no longer tight, and the via pair's room counts only the crossers whose taut track
/// runs between its two vias (B2 / B3 lie beyond via 2 in its own traversal). ERBS then bends B1,
/// F3, F4 and B2 round the placed vias. Round 3 (taut walls): VIAMISS on vias 1 and 2, 2 shorts
/// with B2 / B3 and 4 clearance items, predicted 0. Now: predicted = drawn = 0, no short.
#[test]
fn thick_dive_grouping_predicts_what_it_draws_with_slack() {
    let m = thick_dive_grouping();
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(true))]);
    let p = plan(&r);
    let drawn = r.violations.iter().filter(|v| v.conn == 0).count();
    let pred: u64 = p["conns"]["0"]["predicted"].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum();
    assert!(drawn == 0 || pred > 0, "P: drawn {drawn} violations, predicted {pred}: {}", p["conns"]["0"]);
    assert_eq!(r.violations.iter().filter(|v| v.kind == "short").count(), 0, "{:?}", r.violations);
    assert!(r.violations.is_empty(), "{:?}", r.violations);
    assert_eq!(per_net(&r)[&1].0, 3, "P weaves with its 3 planned vias: {:?}", per_net(&r));
}

/// thick-layer r3 (`[search] free_crossing`, design addendum r3): S1-S5 have drilled pads at both
/// ends, so crossing P costs them no via (they take B.Cu). With the full crossing price (main)
/// S1 / S2 go round P's pad (91.41 mm in all, round 1); with `free_crossing = 0` every wire runs
/// straight: 86.00 mm (14 + 6 x 12), P 0 vias, S6 the only diver (2 vias), no violation.
#[test]
fn wide_one_side_free_crossing() {
    let m = wide_one_side();
    let r = route(&m, &[("free_crossing", serde_json::json!(0.0))]);
    let pn = per_net(&r);
    let total: f64 = pn.values().map(|v| v.1).sum();
    assert_eq!(r.violations.len(), 0, "{:?}", r.violations);
    assert_eq!(pn[&1].0, 0, "P keeps F.Cu: {pn:?}");
    assert!((total - 86.0).abs() < 0.05, "86.00 mm, no detour: {total:.3} {pn:?}");
    assert_eq!(pn.values().map(|v| v.0).sum::<usize>(), 2, "S6 dives with 2 vias: {pn:?}");
}

/// The same with the full crossing price (main, `free_crossing = 1`): the detour this fixes.
#[test]
fn wide_one_side_full_crossing_detours() {
    let m = wide_one_side();
    let r = route(&m, &[("free_crossing", serde_json::json!(1.0))]);
    let total: f64 = per_net(&r).values().map(|v| v.1).sum();
    assert!(total > 86.5, "main detours: {total:.3}");
}


/// M3 slack, the plan's end site (round 4): P's via 2 (between F4 at x 10 and B2 at x 11, one
/// traversal) is planned between the two crossers. A crosser that bounds the window keeps the
/// window's other end on the via's side (`refine::Frame::end_site_lane`): with the window's middle
/// as the reference (round 3) a candidate past B2 counted B2 at its left-shifted lane, beyond the
/// via, and the plan put the site at x 12.08, past B2 and B3.
#[test]
fn thick_dive_grouping_end_site_between_its_crossers() {
    let m = thick_dive_grouping();
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(true))]);
    let p = plan(&r);
    let w = p["wires"].as_array().unwrap().iter().find(|w| w["net_name"] == "P").unwrap().clone();
    let at: Vec<f64> = w["vias"].as_array().unwrap().iter().map(|v| v["at"][0].as_f64().unwrap()).collect();
    assert_eq!(at.len(), 3, "{at:?}");
    assert!(at[2] > 10.0 && at[2] < 11.0, "via 2 between F4 (x 10) and B2 (x 11): {at:?}");
}

/// Diagnosis print: [`thick_dive_grouping_with`] signals from `DIAG_SIGNALS` ("name:x:layer,...").
#[test]
#[ignore]
fn diag_thick_dive_grouping_with() {
    let spec = std::env::var("DIAG_SIGNALS").unwrap_or_default();
    let owned: Vec<(String, f64, u32)> = spec.split(',').filter(|s| !s.is_empty()).map(|s| {
        let f: Vec<&str> = s.split(':').collect();
        (f[0].to_string(), f[1].parse().unwrap(), f[2].parse().unwrap())
    }).collect();
    let sig: Vec<(&str, f64, u32)> = owned.iter().map(|x| (x.0.as_str(), x.1, x.2)).collect();
    let m = thick_dive_grouping_with(&sig);
    let win = std::env::var("DIAG_WINDOW").unwrap_or_else(|_| "farthest".into());
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(true)), ("via_site_window", serde_json::json!(win))]);
    let p = plan(&r);
    for w in p["wires"].as_array().unwrap().iter().filter(|w| w["net_name"] == "P") {
        println!("P vias {}", w["vias"]);
    }
    println!("conn0 {}", p["conns"]["0"]);
    for v in &r.violations {
        println!("drawn violation: {} {} {}", v.kind, v.name, v.against);
    }
    for it in r.items.iter().map(|it| serde_json::to_value(it).unwrap()).filter(|v| v.get("d").is_some()) {
        println!("via {}", it["at"]);
    }
}

/// M3 slack, a via past a crosser's taut crossing. TARGET, FAILS (ignored; round 4 finding): F4
/// at x 10.6 and B2 at x 11.0 with B3 at 11.35 (B2 has no room to its right), F3 at 9 (F4 has
/// 1.25 mm to its left). P's via 2 fits only at x <= 10.3, west of F4's taut track: F4 must bend
/// 0.3+ mm west round it. The plan (window to the crossing farthest out) predicts a site and 0
/// violations; the drawing places the via, but ERBS takes the via's side from the geometry, so
/// F4 passes it on the wrong side or squeezes (clearance with F3 and P). Realisable only when the
/// window stays at the taut crossings (crossers bend away from the via, not past it) or ERBS
/// takes the via's side from the plan; proposal in the round 4 report.
#[test]
#[ignore = "plan window past the taut crossing is not realised by ERBS (round 4 report)"]
fn thick_dive_grouping_via_past_taut_crossing() {
    let m = thick_dive_grouping_with(&[("F1", 3.0, 0), ("F2", 4.0, 0), ("B1", 8.0, 2), ("F3", 9.0, 0), ("F4", 10.6, 0), ("B2", 11.0, 2), ("B3", 11.35, 2)]);
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(true))]);
    let p = plan(&r);
    let pred: u64 = p["conns"].as_object().unwrap().values().map(|c| c["predicted"].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum::<u64>()).sum();
    assert!(r.violations.is_empty() || pred > 0, "drawn {} violations, predicted {pred}: {:?}", r.violations.len(), r.violations.iter().map(|v| (&v.name, &v.kind, &v.against)).collect::<Vec<_>>());
}

/// `[vias] site_window = "taut"` (round 4): the fixture of
/// [`thick_dive_grouping_via_past_taut_crossing`] with the via window at the bounding crossers'
/// taut crossings (they bend away from the via only, never past it). With "farthest" the plan puts
/// P's via 2 at x 10.17, west of F4's taut track (x 10.6), which the drawing does not realise
/// (ERBS passes F4 on the wrong side). With "taut" no planned via of P lies between F3 (x 9) and
/// F4's taut track: the loop routes P another way. (That way still draws one clearance item with
/// B2 that the plan does not predict: a separate drawing gap, not a via site.)
#[test]
fn thick_dive_grouping_taut_window_keeps_vias_off_the_crossers_path() {
    let m = thick_dive_grouping_with(&[("F1", 3.0, 0), ("F2", 4.0, 0), ("B1", 8.0, 2), ("F3", 9.0, 0), ("F4", 10.6, 0), ("B2", 11.0, 2), ("B3", 11.35, 2)]);
    let xs = |win: &str| {
        let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(true)), ("via_site_window", serde_json::json!(win))]);
        let p = plan(&r);
        let w = p["wires"].as_array().unwrap().iter().find(|w| w["net_name"] == "P").unwrap().clone();
        w["vias"].as_array().unwrap().iter().map(|v| v["at"][0].as_f64().unwrap()).collect::<Vec<f64>>()
    };
    let (taut, far) = (xs("taut"), xs("farthest"));
    assert!(taut.iter().all(|&x| !(9.0..10.6).contains(&x)), "taut: {taut:?}");
    assert!(far.iter().any(|&x| (9.0..10.6).contains(&x)), "farthest (the unrealised site): {far:?}");
}

/// `site_window = "taut"` keeps the base fixture's result: P's vias sit inside the taut windows
/// there, so the drawn slack test holds as with "farthest".
#[test]
fn thick_dive_grouping_taut_window_draws_clean() {
    let m = thick_dive_grouping();
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(true)), ("via_site_window", serde_json::json!("taut"))]);
    assert!(r.violations.is_empty(), "{:?}", r.violations);
}

/// Fixture for the drawn half of `[vias] site_window` (r5 F2): F4 at x 10.3, B2 at 11.0, B3 at 11.6.
/// F4's taut track is a way west of where "farthest" puts the window's end for P's via 2; the
/// plan's site and the via relocation both read the flag.
fn window_fixture() -> BoardModel {
    thick_dive_grouping_with(&[("F1", 3.0, 0), ("F2", 4.0, 0), ("B1", 8.0, 2), ("F3", 9.0, 0), ("F4", 10.3, 0), ("B2", 11.0, 2), ("B3", 11.6, 2)])
}

/// The drawn vias and the planned via x of P (both sorted by x), under a window mode.
fn window_vias(m: &BoardModel, win: &str) -> (Vec<f64>, Vec<f64>) {
    let r = route(m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(true)), ("via_site_window", serde_json::json!(win))]);
    let p = plan(&r);
    let w = p["wires"].as_array().unwrap().iter().find(|w| w["net_name"] == "P").unwrap().clone();
    let mut planned: Vec<f64> = w["vias"].as_array().unwrap().iter().map(|v| v["at"][0].as_f64().unwrap()).collect();
    planned.sort_by(|a, b| a.total_cmp(b));
    let mut drawn: Vec<f64> = r.items.iter().map(|it| serde_json::to_value(it).unwrap()).filter(|v| v.get("d").is_some()).map(|v| v["at"][0].as_f64().unwrap() / MM).collect();
    drawn.sort_by(|a, b| a.total_cmp(b));
    (drawn, planned)
}

/// r5 F2 (verifier mutants rl_win_far, rl_win_taut, pl_alt_taut): both modes of `site_window`
/// reach the drawing's via relocation (`viaopt::relocate_vias`) and the plan's site search
/// (`refine::Frame` window). "taut": the plan keeps via 2 on the near side of F4's taut track
/// (x < 10.6) and the drawn via 2 stays there too (x 10.3). "farthest": the window ends at the
/// crossing farthest out and the drawing puts via 2 past B3 (x > 12.5: the unrealised site the
/// option exists for). A relocation that read the other mode's window, or a plan that took the
/// shifted lanes in "taut" (planned x 10.97), fails one of these.
#[test]
fn thick_dive_grouping_site_window_reaches_the_drawing() {
    let m = window_fixture();
    let (drawn_t, plan_t) = window_vias(&m, "taut");
    let (drawn_f, _) = window_vias(&m, "farthest");
    assert_eq!((drawn_t.len(), drawn_f.len(), plan_t.len()), (3, 3, 3), "taut {drawn_t:?} farthest {drawn_f:?} plan {plan_t:?}");
    assert!(plan_t[2] > 10.0 && plan_t[2] < 10.6, "taut plan: via 2 west of F4's taut track (10.6): {plan_t:?}");
    assert!(drawn_t[2] > 10.0 && drawn_t[2] < 10.6, "taut drawing: via 2 stays at F4's side: {drawn_t:?}");
    assert!(drawn_f[2] > 12.5, "farthest drawing: via 2 past the crossers: {drawn_f:?}");
}

/// 4-layer variant of `thick_dive_grouping` (r5 F3, verifier mutants om_n_off, os_hold_n_off:
/// every other thick-layer fixture has 2 layers, so the N-layer layering `assign_n` was never
/// reached): P (PTH pads, a ring on every layer) crosses thin signals whose SMD pads pin them to
/// F.Cu (x 3), In1.Cu, In2.Cu and B.Cu: whatever layer P takes, it crosses a signal on it.
fn four_layer(signals: &[(&str, f64, u32)]) -> BoardModel {
    thick_dive_grouping_layers(signals, 4, false)
}

/// r5 F3, ω in the N-layer layering (`assign_n`, `Costs::trans_w`): P (ω 5.5) crossing one thin
/// signal on each of F.Cu, In1.Cu, In2.Cu, B.Cu. Without ω P takes one via to get past the signal
/// on its layer (1 via, thin signals 0); with `priority_weight [1, 0.5]` the layering lets the
/// thin signal dive instead (2 small vias) and P keeps its layer (0 vias).
#[test]
fn four_layer_wide_keeps_its_layer() {
    let m = four_layer(&[("F1", 3.0, 0), ("I1", 5.0, 4), ("I2", 7.0, 6), ("B1", 9.0, 2)]);
    let off = per_net(&route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0]))]));
    let on = per_net(&route(&m, &[("via_priority_weight", serde_json::json!([1.0, 0.5]))]));
    let thin = |pn: &std::collections::BTreeMap<i32, (usize, f64)>| pn.iter().filter(|(k, _)| **k != 1).map(|(_, v)| v.0).sum::<usize>();
    assert_eq!((off[&1].0, thin(&off)), (1, 0), "ω 1: P pays the via: {off:?}");
    assert_eq!(on[&1].0, 0, "ω: P keeps its layer: {on:?}");
    assert!(thin(&on) > 0, "ω: a thin signal dives: {on:?}");
}

/// r5 F3, the one-side hold in the N-layer layering (`Costs::unary`): P crossing two thin signals
/// on each of the 4 layers, `pair_one_side` on. Metrics `pair_move_kinds.one_side.left_changes`
/// (P's layer changes in the layering right after each one-side move): without a hold the
/// layering weaves P again, with a strong hold none is left (as `thick_dive_grouping_one_side_move`
/// on 2 layers).
#[test]
fn four_layer_one_side_hold() {
    let m = four_layer(&[("F1", 3.0, 0), ("I1", 4.0, 4), ("I2", 5.0, 6), ("B1", 6.0, 2), ("F2", 8.0, 0), ("I3", 9.0, 4), ("I4", 10.0, 6), ("B2", 11.0, 2)]);
    let left = |hold: f64| {
        let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_pair_one_side", serde_json::json!(true)), ("via_pair_hold", serde_json::json!(hold))]);
        let pm = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).expect("plan metrics");
        let v: serde_json::Value = serde_json::from_str(pm.trim_start_matches("plan metrics:").trim()).expect("json");
        let os = &v["loop"]["pair_move_kinds"]["one_side"];
        (os["tried"].as_u64().unwrap_or(0), os["left_changes"].as_u64().unwrap_or(0))
    };
    let (free, held) = (left(0.0), left(10.0));
    assert!(free.0 > 0 && held.0 > 0, "one-side moves tried: free {free:?} held {held:?}");
    assert!(free.1 > 0, "no hold: the layering weaves P again: {free:?}");
    assert_eq!(held.1, 0, "hold: P stays on its side in the move's layering: {held:?}");
}
