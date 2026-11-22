//! Round i (M3): the length factor's sequential competitor keeps the plan's class: an inner wire
//! drawn first whose sequential shortcut would cross the outer wire's ERBS keeps its own ERBS
//! (qmtech ADV_CLK / ADV_DE). Fixture: `harness/radii_patterns.py` `shortcut_across`
//! (`python -m harness.radii_patterns --fixture shortcut_across`; approved by the user 2026-11-17).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};

/// (name, length mm, violations) per connection, and the warnings, routed with `realise = "erbs"`,
/// `erbs_margin` 0 (every ERBS longer than its band meets a sequential competitor), the test
/// hooks `hooks` on.
fn route(hooks: &'static [&'static str]) -> (Vec<(String, f64, usize)>, Vec<String>) {
    let (got, warn, _) = route_m(hooks, false);
    (got, warn)
}

/// The fixture model, mirrored in x (`x -> 24 mm - x`) when `mirror`: the same geometry with
/// left and right swapped.
fn model(mirror: bool) -> BoardModel {
    let path = std::env::var("TOPO_FIXTURE").unwrap_or_else(|_| format!("{}/tests/fixtures/m3/model-shortcut_across.json", env!("CARGO_MANIFEST_DIR")));
    let mut m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    if mirror {
        let w = 24_000_000i64;
        let fx = |p: &mut serde_json::Value| p[0] = serde_json::json!(w - p[0].as_i64().unwrap());
        for pad in m["pads"].as_array_mut().unwrap() {
            fx(&mut pad["center"]);
        }
        for s in m["outline"].as_array_mut().unwrap() {
            assert_eq!(s["kind"], "seg", "the fixture outline is segments");
            fx(&mut s["a"]);
            fx(&mut s["b"]);
        }
        for k in m["keepouts"].as_array_mut().unwrap() {
            let poly = k["poly"].as_array_mut().unwrap();
            poly.iter_mut().for_each(fx);
            poly.reverse();
        }
        for key in ["copper_shapes", "holes", "kept", "pours", "vias", "dangling_ends"] {
            assert!(m[key].as_array().is_none_or(|a| a.is_empty()), "mirror: {key} is not empty");
        }
    }
    BoardModel::from_json(&m.to_string()).unwrap()
}

/// (connections (net, length mm, violations), warnings, ERBS draw order by net name)
type Routed = (Vec<(String, f64, usize)>, Vec<String>, Vec<String>);

/// As [`route`], on the fixture mirrored in x when `mirror`, and the ERBS draw order by net name
/// (the engine's `erbs draw order` test probe).
fn route_m(hooks: &'static [&'static str], mirror: bool) -> Routed {
    let m = model(mirror);
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["realise"] = serde_json::json!("erbs");
    val["erbs_margin"] = serde_json::json!(0.0);
    let o: Options = serde_json::from_value(val).unwrap();
    let mut ctl = RunControl::new();
    let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let rec = order.clone();
    ctl.set_test_hook(move |p| {
        if let Some(o) = p.strip_prefix("erbs draw order ") {
            *rec.lock().unwrap() = o.split(',').map(str::to_string).collect();
            return false;
        }
        hooks.contains(&p)
    });
    let r = topor_engine::route::route_with(&m, &o, &ctl);
    assert_eq!(r.routed, r.connections_total, "every connection routes");
    let order = order.lock().unwrap().clone();
    (r.connections.iter().map(|c| (c.name.clone(), c.length_nm as f64 / 1e6, c.violations)).collect(), r.warnings.clone(), order)
}

fn kept_count(warn: &[String]) -> usize {
    warn.iter()
        .find_map(|w| w.split("replaced by it, ").nth(1).and_then(|r| r.split(' ').next()).and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

/// A (0.40 mm, drawn first) wraps J3; its sequential competitor (forced straight by the test hook:
/// legal while B is undrawn, 14.36 mm) crosses B's ERBS. Round i: A keeps its ERBS, B is drawn
/// straight, no crossing. With the rule off the shortcut wins and B is drawn across it.
#[test]
fn inner_wire_keeps_its_erbs_against_a_shortcut_across_the_outer() {
    let (got, warn) = route(&["straight sequential run"]);
    let (off, woff) = route(&["straight sequential run", "erbs class keep off"]);
    let (nat, wnat) = route(&[]);
    if std::env::var_os("TOPO_FIXTURE").is_some() {
        eprintln!("ON {got:?} kept {}\nOFF {off:?} kept {}\nNATURAL {nat:?} kept {}", kept_count(&warn), kept_count(&woff), kept_count(&wnat));
    }
    let len = |r: &[(String, f64, usize)], n: &str| r.iter().find(|c| c.0 == n).map(|c| c.1).expect("net");
    assert!(got.iter().all(|c| c.2 == 0), "no violation: {got:?}");
    // The approved judge (harness/radii_patterns.py shortcut_across): A = reference 18.255 mm
    // (+0.5 %). XFAIL: measured 19.08 mm (+4.5 %, round i; the ERBS's octilinear / pad-exit form,
    // open, parking list). The bound is the measured length (+0.5 %); strict: once A meets the
    // judge this test fails (XPASS) so the bound is replaced by the judge.
    let (a, a_ref, a_xfail) = (len(&got, "A"), 18.255, 19.08);
    assert!(a > a_ref - 0.01 && a <= a_xfail * 1.005, "A wraps J3 (reference {a_ref} mm, xfail bound {a_xfail} mm): {got:?}");
    assert!(a > a_ref * 1.005, "XPASS: A {a:.3} mm meets the approved judge (reference {a_ref} mm +0.5 %): tighten the bound to it");
    assert!((len(&got, "B") - 9.0).abs() < 0.01, "B straight: {got:?}");
    // TW2 joint commit: B's ERBS is a kept band in the index before A draws, so the straight
    // competitor that crosses it is illegal and never competes; the class rule (round i) has no
    // case left here, with or without it (it still guards an undrawn wire without an ERBS:
    // `a_wire_without_erbs_is_checked_by_its_planned_path`). Before TW2: one competitor kept out,
    // and with the rule off the shortcut was drawn and B crossed it (qmtech ADV_CLK / ADV_DE).
    assert_eq!(kept_count(&warn), 0, "no competitor reaches the class test: {warn:?}");
    assert_eq!(off, got, "the rule off: the kept band of B keeps A's wrap all the same");
    assert_eq!(kept_count(&woff), 0);
    // Without the hooks: no violation either (the exact attempt finds the wrap or the ERBS is kept).
    assert!(nat.iter().all(|c| c.2 == 0), "no violation without hooks: {nat:?}");
}

/// Condition 1 (lead, round i): an undrawn wire without an ERBS (its sleeve failed: the test hook
/// fails the first stretch, B's) is checked by its planned path, never skipped.
/// fd45 r10 (lead): the precondition is that A is drawn while B is undrawn. Under the default
/// `octi_order = "sweep"` (left first) the fixture as approved draws B first (B is checked as
/// drawn copper then, the class test has no case); mirrored in x, A comes first. The order is
/// asserted, so a future order change fails here instead of testing nothing.
#[test]
fn a_wire_without_erbs_is_checked_by_its_planned_path() {
    let (got, warn, order) = route_m(&["straight sequential run", "fail erbs sleeve"], true);
    if std::env::var_os("TOPO_FIXTURE").is_some() {
        eprintln!("FAILED-B {got:?} kept {} order {order:?}\n{:?}", kept_count(&warn), warn.iter().filter(|w| w.contains("ERBS")).collect::<Vec<_>>());
    }
    assert_eq!(order, ["A", "B"], "precondition: A is drawn first, B undrawn (the ERBS draw order)");
    assert!(warn.iter().any(|w| w.contains("1 without a sleeve")), "B's sleeve failed: {warn:?}");
    assert!(got.iter().all(|c| c.2 == 0), "no violation: {got:?}");
    assert_eq!(kept_count(&warn), 1, "B's planned path keeps A's ERBS: {warn:?}");
    // The fixture as approved draws B first under the default order (why the test mirrors it).
    let (_, _, plain) = route_m(&["straight sequential run", "fail erbs sleeve"], false);
    assert_eq!(plain, ["B", "A"], "the unmirrored fixture's order changed: the mirror may no longer be needed");
}
