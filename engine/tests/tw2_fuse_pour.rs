//! TW2 fuse pour check ([`topor_engine::topo::fusepour`]): the fuse and the octilinear polish after
//! drawing are judged by the pour stitching too, so they never leave more pour pieces cut off
//! than the copper without their moves (qmtech 2026-11-18: the reduced, fused and polished copper
//! left 21 GND pieces against 20 as drawn, one error more, so the table dropped the via reduction).
//!
//! Fixture: the esp USB crop (`fixtures/esp-rust-board-usb-crop.json`) with a GND pour over the
//! whole board on both layers added in code (the crop has no pours), so the fuse moves of its
//! nets meet a pour. No new pattern.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::RouteResult;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::draw::{track_item, Drawer, WireOut};
use topor_engine::topo::state::State;

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

/// `(n, k)` of the line's "<n> of <k> moves undone".
fn undone(l: &str) -> (usize, usize) {
    let t = l.split(" moves undone").next().unwrap();
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
    let (without, now) = (num(" and the octilinear polish, "), num(", now "));
    assert!(now <= without, "{l}");
    let (_, k) = undone(l);
    assert!(k >= 1, "the crop fuses trace ends: {l}");
    // Every polished wire is a move of the check too.
    let pol: usize = line(&r, "octilinear polish after drawing: ").and_then(|p| p.split("drawing: ").nth(1)?.split(' ').next()?.parse().ok()).expect("polish line");
    let pol_moves: usize = l.split(", polish ").nth(1).and_then(|t| t.split(" of ").nth(1)?.split([';', ')']).next()?.parse().ok()).unwrap_or_else(|| panic!("{l}"));
    assert_eq!(pol == 0, pol_moves == 0, "{pol} polished segments: {l}");
    // Off: no check, the fuse as before.
    let off = route(&m, &[("fuse_pour_check", serde_json::json!(false))], &[]);
    assert!(line(&off, "fuse pour check: ").is_none());
}

/// Hook "fuse pour undo all": every move is undone, so the copper is the copper with the fuse
/// off: the drawn wires, their outcomes and the drawing's index all come back (the steps after
/// the check see the same copper as without the fuse). Style smooth: no polish, whose moves a
/// run with the fuse off would make too.
#[test]
fn undoing_every_fuse_move_gives_the_copper_without_the_fuse() {
    let m = crop_with_gnd_pour();
    let smooth = ("draw_style", serde_json::json!("smooth"));
    let all = route(&m, std::slice::from_ref(&smooth), &["fuse pour undo all"]);
    let l = line(&all, "fuse pour check: ").expect("fuse pour check line");
    let (n, k) = undone(l);
    assert!(k >= 1 && n == k, "{l}");
    let f = line(&all, "fuse after drawing: ").expect("fuse line");
    assert!(f.starts_with("fuse after drawing: 0 trace ends moved"), "the fuse line counts the moves kept: {f}");
    assert!(!all.warnings.iter().any(|w| w.contains("not in the drawing's index")), "{:?}", all.warnings.iter().filter(|w| w.starts_with("fuse")).collect::<Vec<_>>());
    let off = route(&m, &[smooth, ("fuse_branches", serde_json::json!(false))], &[]);
    let diag = |r: &RouteResult| -> Vec<String> { r.warnings.iter().filter(|w| w.starts_with("fuse") || w.starts_with("via reduction") || w.starts_with("octilinear polish") || w.starts_with("TopoR loop (") || w.starts_with("pour stitching")).map(|w| w.chars().take(300).collect()).collect() };
    assert_eq!(all.total_length_nm, off.total_length_nm, "undo all:\n{:#?}\nfuse off:\n{:#?}", diag(&all), diag(&off));
    assert_eq!(copper(&all), copper(&off));
    // Octilinear: the polish line counts the polish moves kept (verify V5), here none.
    let oct = route(&m, &[], &["fuse pour undo all"]);
    let pl = line(&oct, "octilinear polish after drawing: ").expect("polish line");
    assert!(pl.starts_with("octilinear polish after drawing: 0 segments") && pl.contains(": 0.0 mm"), "{pl}");
}

/// Verify V6: the check's stitching stops with the time limit (hook "fuse pour cut": cut at
/// once); a cut stitching decides nothing, even where every move would be undone.
#[test]
fn a_cut_stitching_undoes_nothing() {
    let m = crop_with_gnd_pour();
    let r = route(&m, &[], &["fuse pour undo all", "fuse pour cut"]);
    let l = line(&r, "fuse pour check: ").expect("fuse pour check line");
    assert!(l.contains("cut its stitching") && undone(l).0 == 0, "{l}");
}

/// Verify V6, the limit path: a hard limit that passes while the stitching of the check begins
/// cuts it (hook "fuse pour check begin" reaches the limit, after the 1 s floor every round gets),
/// even where every move would be undone, and the report names the stitching as the step cut
/// (not the step before it).
#[test]
fn a_hard_limit_reached_at_the_start_of_the_check_cuts_its_stitching() {
    let m = crop_with_gnd_pour();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["topo_rounds"] = serde_json::json!(1);
    val["timeout_mode"] = serde_json::json!("hard");
    val["wall_clock_s"] = serde_json::json!(if cfg!(debug_assertions) { 900.0 } else { 120.0 });
    let o: Options = serde_json::from_value(val).unwrap();
    let mut ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let lim = ctl.limit.clone();
    let t0 = std::time::Instant::now();
    ctl.set_test_hook(move |pt| {
        match pt {
            "fuse pour undo all" => return true,
            "fuse pour check begin" => {
                // The limit acts once a round's 1 s floor has passed (core.rs `floor`).
                let left = std::time::Duration::from_millis(1200).saturating_sub(t0.elapsed());
                std::thread::sleep(left);
                lim.reach();
            }
            _ => {}
        }
        false
    });
    let r = topor_engine::route::route_with(&m, &o, &ctl);
    let l = line(&r, "fuse pour check: ").expect("fuse pour check line");
    assert!(l.contains("cut its stitching") && undone(l).0 == 0, "{l}");
    let lr = r.limit.as_ref().expect("limit report");
    assert!(lr.reached && lr.cut_steps.contains(&"stitching".to_string()), "{lr:?}");
}

/// Verify V10: `[search] fuse_pour_rounds`. Hook "fuse pour worse" judges the first round worse and
/// seeds it with the first kept fuse move: with the default rounds only that move (and its
/// dependents) is undone; with 0 every move is undone at once.
#[test]
fn fuse_pour_rounds_zero_undoes_every_move_at_once() {
    let m = crop_with_gnd_pour();
    let d = route(&m, &[], &["fuse pour worse"]);
    let (n, k) = undone(line(&d, "fuse pour check: ").expect("check line"));
    assert!(n >= 1 && n < k, "rounds 2: only the nearest moves ({n} of {k})");
    let z = route(&m, &[("fuse_pour_rounds", serde_json::json!(0))], &["fuse pour worse"]);
    let (n0, k0) = undone(line(&z, "fuse pour check: ").expect("check line"));
    assert!(k0 == k && n0 == k0, "rounds 0: every move ({n0} of {k0})");
}

/// `(n)` of the line's "<n> by the re-check of the kept moves".
fn rechecked(l: &str) -> usize {
    l.split(" by the re-check of the kept moves").next().and_then(|t| t.rsplit(' ').next()).and_then(|x| x.parse().ok()).unwrap_or_else(|| panic!("{l}"))
}

/// Verify V3: after a partial undo the moves kept are checked again against the copper that came
/// back, and a clashing one is undone too. Hooks: "fuse pour partial undo" undoes the first fuse
/// move whatever the counts; "fuse pour clash" reports the first kept fuse move's wire as clashing
/// once. The re-check undoes it (and nothing is left stale in the drawing's index).
#[test]
fn a_kept_move_that_clashes_with_the_restored_copper_is_undone_too() {
    let m = crop_with_gnd_pour();
    let r = route(&m, &[], &["fuse pour partial undo", "fuse pour clash"]);
    let l = line(&r, "fuse pour check: ").expect("fuse pour check line");
    let (n, k) = undone(l);
    assert!(rechecked(l) >= 1 && n >= 2 && n < k, "{l}");
    assert!(line(&r, "fuse pour self-check: ").is_none(), "{:?}", line(&r, "fuse pour self-check: "));
    // Without the clash hook the partial undo re-checks the kept moves and finds them clear.
    let p = route(&m, &[], &["fuse pour partial undo"]);
    let lp = line(&p, "fuse pour check: ").expect("fuse pour check line");
    assert_eq!(rechecked(lp), 0, "{lp}");
}

/// The re-check's test itself ([`Drawer::wire_clear`]): a wire's tracks against the rest of the
/// drawing, its own tracks skipped.
#[test]
fn a_wire_is_clear_only_while_no_foreign_track_comes_too_close() {
    const MM: f64 = 1e6;
    let mut v: serde_json::Value = serde_json::from_str(include_str!("fixtures/esp-rust-board-usb-crop.json")).unwrap();
    v["pours"] = serde_json::json!([]);
    let m = BoardModel::from_json(&v.to_string()).unwrap();
    let b = board::build(&m, &BuildOpts::default()).unwrap();
    let st = State::new(&b.proj);
    let o = Options::default();
    let ck = Checker::new(&m, &o);
    let mut dr = Drawer::new(&m, &o, &b, &st, &ck);
    // An empty spot of the crop: (139, 119.5) mm is 5 mm from the nearest pad.
    let (a, z) = (Vec2 { x: 139.0 * MM, y: -119.0 * MM }, Vec2 { x: 139.0 * MM, y: -120.0 * MM });
    let w = 0.15 * MM;
    let mine = track_item(1, 0, w, a, z, None);
    let h = dr.commit(&mine);
    let wo = WireOut { items: vec![mine.clone()], handles: vec![h], ..WireOut::default() };
    let free = dr.wire_clear(&wo);
    assert!(free, "the probe track itself must be legal on the empty corner");
    // A foreign track 0.05 mm beside it: not clear; taken away again: clear.
    let off = Vec2 { x: 0.2 * MM, y: 0.0 };
    let other = track_item(2, 0, w, a + off, z + off, None);
    let ho = dr.commit(&other);
    assert!(!dr.wire_clear(&wo));
    dr.uncommit(ho);
    assert!(dr.wire_clear(&wo));
    // Its own tracks never count against it (the same track committed twice is its own).
    let h2 = dr.commit(&mine);
    let both = WireOut { items: vec![mine.clone(), mine], handles: vec![h, h2], ..WireOut::default() };
    assert!(dr.wire_clear(&both));
}
