//! TW2 fuse pour check ([`topor_engine::topo::fusepour`]): the fuse after drawing is judged by the
//! pour stitching too, so it never leaves more pour pieces cut off than the copper without it
//! (qmtech 2026-11-18: two fuse moves of 0.4 mm gain each left a GND piece beside the ADV7511
//! and a capacitor pad cut off, one error more than the copper as drawn).
//!
//! Fixture: the esp USB crop (`fixtures/esp-rust-board-usb-crop.json`) with a GND pour over the
//! whole board on both layers added in code (the crop has no pours), so the fuse moves of its
//! nets meet a pour. No new pattern.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::RouteResult;

fn crop_with_gnd_pour() -> BoardModel {
    let mut v: serde_json::Value = serde_json::from_str(include_str!("fixtures/esp-rust-board-usb-crop.json")).expect("fixture parses");
    let gnd = v["nets"].as_array().unwrap().iter().find(|n| n["name"] == "GND").expect("GND net")["code"].as_i64().unwrap();
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for s in v["outline"].as_array().unwrap() {
        for k in ["a", "b"] {
            let (x, y) = (s[k][0].as_i64().unwrap(), s[k][1].as_i64().unwrap());
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    let m = 500_000; // inside the edge clearance
    let ring = serde_json::json!([[x0 + m, y0 + m], [x1 - m, y0 + m], [x1 - m, y1 - m], [x0 + m, y1 - m]]);
    let pads: Vec<String> = v["pads"].as_array().unwrap().iter().filter(|p| p["net"].as_i64() == Some(gnd)).map(|p| format!("{}.{}", p["ref"].as_str().unwrap(), p["number"].as_str().unwrap())).collect();
    v["pours"] = serde_json::json!([{
        "uuid": "tw2-gnd", "net": gnd, "layers": [0, 2], "pads": pads, "vias": [],
        "clearance": 200000, "min_thickness": 250000, "thermal_gap": 500000, "spoke_width": 500000, "pad_connection": 2,
        "fills": [{"layer": 0, "outer": ring}, {"layer": 2, "outer": ring}]
    }]);
    BoardModel::from_json(&v.to_string()).expect("model with the pour parses")
}

fn route(m: &BoardModel, extra: &[(&str, serde_json::Value)], hooks: &'static [&'static str]) -> RouteResult {
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    val["wall_clock_s"] = serde_json::json!(if cfg!(debug_assertions) { 900.0 } else { 120.0 });
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    let mut ctl = RunControl::new();
    ctl.set_test_hook(move |pt| hooks.contains(&pt));
    topor_engine::route::route_with(m, &o, &ctl)
}

fn line<'a>(r: &'a RouteResult, start: &str) -> Option<&'a String> {
    r.warnings.iter().find(|w| w.starts_with(start))
}

/// `(of the line "<n> of <k> fuse moves undone")`.
fn undone(l: &str) -> (usize, usize) {
    let t = l.split(" fuse moves undone").next().unwrap();
    let mut w = t.rsplit(' ');
    let k = w.next().unwrap().parse().unwrap();
    let _ = w.next();
    let n = w.next().unwrap().parse().unwrap();
    (n, k)
}

/// Copper of the routed connections, sorted (KiCad frame), stitching left out.
fn copper(r: &RouteResult) -> Vec<String> {
    let mut v: Vec<String> = r
        .items
        .iter()
        .zip(&r.item_conn)
        .filter(|(_, &c)| c != usize::MAX)
        .map(|(it, _)| match it {
            Item::Track(t) => format!("T {} {} {:?}", t.net, t.layer, t.prim),
            Item::Via(v) => format!("V {} {:?}", v.net, v.at),
        })
        .collect();
    v.sort();
    v
}

#[test]
fn the_fuse_never_leaves_more_pour_pieces_cut_off() {
    let m = crop_with_gnd_pour();
    let r = route(&m, &[], &[]);
    let l = line(&r, "fuse pour check: ").unwrap_or_else(|| panic!("no fuse pour check line; {:?}", r.warnings.iter().filter(|w| w.starts_with("fuse")).collect::<Vec<_>>()));
    let num = |after: &str| -> usize { l.split(after).nth(1).and_then(|t| t.split(|c: char| !c.is_ascii_digit()).next()).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("{after}: {l}")) };
    let (without, now) = (num(" cut off in the fused copper, "), num(", now "));
    assert!(now <= without, "{l}");
    let (_, k) = undone(l);
    assert!(k >= 1, "the crop fuses trace ends: {l}");
    // Off: no check, the fuse as before.
    let off = route(&m, &[("fuse_pour_check", serde_json::json!(false))], &[]);
    assert!(line(&off, "fuse pour check: ").is_none());
}

/// Hook "fuse pour undo all": every move is undone, so the copper is the copper with the fuse
/// off: the drawn wires, their outcomes and the drawing's index all come back (the steps after
/// the fuse, the octilinear polish among them, see the same copper as without it).
#[test]
fn undoing_every_fuse_move_gives_the_copper_without_the_fuse() {
    let m = crop_with_gnd_pour();
    let all = route(&m, &[], &["fuse pour undo all"]);
    let l = line(&all, "fuse pour check: ").expect("fuse pour check line");
    let (n, k) = undone(l);
    assert!(k >= 1 && n == k, "{l}");
    let f = line(&all, "fuse after drawing: ").expect("fuse line");
    assert!(f.starts_with("fuse after drawing: 0 trace ends moved"), "the fuse line counts the moves kept: {f}");
    assert!(!all.warnings.iter().any(|w| w.contains("not in the drawing's index")), "{:?}", all.warnings.iter().filter(|w| w.starts_with("fuse")).collect::<Vec<_>>());
    let off = route(&m, &[("fuse_branches", serde_json::json!(false))], &[]);
    let diag = |r: &RouteResult| -> Vec<String> { r.warnings.iter().filter(|w| w.starts_with("fuse") || w.starts_with("via reduction") || w.starts_with("octilinear polish") || w.starts_with("TopoR loop (") || w.starts_with("pour stitching")).map(|w| w.chars().take(300).collect()).collect() };
    assert_eq!(all.total_length_nm, off.total_length_nm, "undo all:\n{:#?}\nfuse off:\n{:#?}", diag(&all), diag(&off));
    assert_eq!(copper(&all), copper(&off));
}
