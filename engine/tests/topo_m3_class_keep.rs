//! Round i (M3): the length factor's sequential competitor keeps the plan's class: an inner wire
//! drawn first whose sequential shortcut would cross the outer wire's ERBS keeps its own ERBS
//! (qmtech ADV_CLK / ADV_DE). Fixture: `harness/radii_patterns.py` `inner_first_wrap`.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};

/// (name, length mm, violations) per connection, and the warnings, routed with `realise = "erbs"`,
/// `erbs_margin` 0 (every ERBS longer than its band meets a sequential competitor), the test
/// hooks `hooks` on.
fn route(hooks: &'static [&'static str]) -> (Vec<(String, f64, usize)>, Vec<String>) {
    let path = std::env::var("TOPO_FIXTURE").unwrap_or_else(|_| format!("{}/tests/fixtures/m3/model-inner_first_wrap.json", env!("CARGO_MANIFEST_DIR")));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    val["realise"] = serde_json::json!("erbs");
    val["erbs_margin"] = serde_json::json!(0.0);
    let o: Options = serde_json::from_value(val).unwrap();
    let mut ctl = RunControl::new();
    ctl.set_test_hook(move |p| hooks.contains(&p));
    let r = topor_engine::route::route_with(&m, &o, &ctl);
    assert_eq!(r.routed, r.connections_total, "every connection routes");
    (r.connections.iter().map(|c| (c.name.clone(), c.length_nm as f64 / 1e6, c.violations)).collect(), r.warnings.clone())
}

#[test]
fn inner_wire_keeps_its_erbs_against_a_shortcut_across_the_outer() {
    let (got, warn) = route(&[]);
    if std::env::var_os("TOPO_FIXTURE").is_some() {
        let (off, woff) = route(&["erbs class keep off"]);
        eprintln!("ON {got:?}\n{:?}\nOFF {off:?}\n{:?}", warn.iter().filter(|w| w.contains("ERBS") || w.contains("erbs")).collect::<Vec<_>>(), woff.iter().filter(|w| w.contains("ERBS") || w.contains("erbs")).collect::<Vec<_>>());
    }
    assert!(got.iter().all(|c| c.2 == 0), "no violation: {got:?}");
}
