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
    let r = route(&m, &[]);
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
}

/// S1 target (design M3, F1; fails on 7513b3db): what the plan hands the drawing predicts the
/// drawn violations of P. Found with the plan report (S1 diagnosis): P's last via interval lies in
/// traversal 7 between its crossings with F4 (event s 0.44) and B2 (s 0.65); the site
/// ([`end_site`]) is chosen on the pass-point polyline, where the two crossings are ~1 mm apart, at
/// (11.27, 6.50); on the taut lines the drawing realises, F4 is at x 10 and B2 at x 11, so the
/// point lies past B2 and the 1.0 mm gap holds no 0.9 mm via with P's clearances (0.5 mm free):
/// predicted 0, drawn 2 shorts with B2 / B3 and 4 clearance items.
#[test]
#[ignore]
fn thick_dive_grouping_predicts_what_it_draws() {
    let m = thick_dive_grouping();
    let r = route(&m, &[]);
    let p = plan(&r);
    let drawn = r.violations.iter().filter(|v| v.conn == 0).count();
    let pred: u64 = p["conns"]["0"]["predicted"].as_object().unwrap().values().map(|v| v.as_u64().unwrap()).sum();
    assert!(drawn == 0 || pred > 0, "P: drawn {drawn} violations, predicted {pred}: {}", p["conns"]["0"]);
}
