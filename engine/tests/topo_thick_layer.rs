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
    let (w, h) = (n(16.0), n(14.0));
    let sig = serde_json::json!({"clearance": n(0.15), "width": n(0.2), "via_d": n(0.6), "via_drill": n(0.3), "netclass": "Default", "width_source": "default", "min_width": 0});
    let pwr = serde_json::json!({"clearance": n(0.15), "width": n(1.0), "via_d": n(0.9), "via_drill": n(0.5), "netclass": "Power", "width_source": "netclass", "min_width": 0});
    let mut nets = vec![serde_json::json!({"code": 1, "name": "P"})];
    let mut pads = vec![pth(1, "PA", 1, 1.0, 7.0, 1.4, 0.7), pth(2, "PB", 1, 15.0, 7.0, 1.4, 0.7)];
    let mut per_net = serde_json::Map::new();
    per_net.insert("1".into(), pwr);
    for (i, &(name, x, l)) in SIGNALS.iter().enumerate() {
        let code = i as i32 + 2;
        nets.push(serde_json::json!({"code": code, "name": name}));
        pads.push(smd(10 + 2 * i, &format!("{name}T"), code, x, 1.0, l));
        pads.push(smd(11 + 2 * i, &format!("{name}B"), code, x, 13.0, l));
        per_net.insert(code.to_string(), sig.clone());
    }
    let j = serde_json::json!({
        "units": "nm", "board": "thick_dive_grouping",
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
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0]))]);
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
    // ω off (P weaves, as main) and the default end-site test alone (`site_lanes` off).
    let r = route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0]))]);
    let p = plan(&r);
    let drawn = r.violations.iter().filter(|v| v.conn == 0).count();
    let pred: u64 = p["conns"]["0"]["predicted"].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum();
    assert!(drawn == 0 || pred > 0, "P: drawn {drawn} violations, predicted {pred}: {}", p["conns"]["0"]);
}

/// M3 (`[vias] site_planned`): P's vias between B1 / F3 and between F4 / B2 sit in 1.0 mm gaps
/// between the crossers' taut lines, too narrow for the 0.9 mm via with clearances (1.4 mm): with
/// the option both are vias without a site, deficit = (1.4 - 1.0) / 2 = 0.2 mm each (the
/// lane room, not the static room, which has space); without it (the static room alone, main)
/// every via has a site and the plan predicts nothing. ω off (`priority_weight [0, 0]`), so P
/// keeps its 3 vias.
#[test]
fn thick_dive_grouping_lane_sites() {
    let m = thick_dive_grouping();
    let sites = |on: bool| {
        let r = route(&m, &[("via_site_planned", serde_json::json!(on)), ("via_site_lanes", serde_json::json!(on)), ("via_priority_weight", serde_json::json!([0.0, 0.0]))]);
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

/// S2 (M1, `[vias] priority_weight`): with ω (default [1, 0.5]: P's vias weigh 1 + 4 + 0.5 = 5.5)
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

/// S3 (M2.3, `[vias] pair_one_side`): with ω off (every via alike) the layering weaves P with 3
/// vias; the one-side candidate holds P's whole run to one end layer and lays its crossers on
/// that layer again (they dive), which the board's evaluation keeps (the 2 vias without a site,
/// predicted errors, go): P fewer vias, no drawn violation. Off: main's weave.
///
/// S3 TARGET, FAILS (ignored): the candidate is tried (`TOPO_LOOPDBG`: "pair move wire 0 events
/// 2..=6 onto 1: wires [3, 6, 7] laid again, eval 2173.268 -> 2173.268") but the crossers cannot
/// go round (P's pads at the outline), they are laid again in place, and the board's layering
/// after the move is free again: it weaves P as before (ω off, and P3's `site_amount` sees only
/// the static room, not M3's lanes). Next: the move's layering holds the wire's events to `t`.
#[test]
#[ignore]
fn thick_dive_grouping_one_side_move() {
    let m = thick_dive_grouping();
    let o = |on: bool| route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_pair_one_side", serde_json::json!(on))]);
    let (on, off) = (o(true), o(false));
    let (pon, poff) = (per_net(&on), per_net(&off));
    assert_eq!(poff[&1].0, 3, "off: P weaves: {poff:?}");
    assert!(pon[&1].0 < 3, "one side: P fewer vias: {pon:?}");
    assert_eq!(on.violations.len(), 0, "{:?}", on.violations);
}

/// M3 with `site_lanes` off (the default) and ω off: only the end site (via 3, between F4 and B2
/// in one traversal) is measured on the taut tracks; it has no room there (predicted), so the
/// TopoR loop reroutes P (its via 3 moves above the crossers' ends of room): 1 drawn clearance
/// item (B2 against P, 0.05 mm) instead of main's 6 (2 shorts). Without the option: main's 6.
#[test]
fn thick_dive_grouping_end_sites_only() {
    let m = thick_dive_grouping();
    let o = |on: bool| route(&m, &[("via_priority_weight", serde_json::json!([0.0, 0.0])), ("via_site_planned", serde_json::json!(on))]);
    let (on, off) = (o(true), o(false));
    let shorts = |r: &RouteResult| r.violations.iter().filter(|v| v.kind == "short").count();
    assert_eq!((off.violations.len(), shorts(&off)), (6, 2), "main: {:?}", off.violations);
    assert!(on.violations.len() < off.violations.len() && shorts(&on) == 0, "{:?}", on.violations);
}
