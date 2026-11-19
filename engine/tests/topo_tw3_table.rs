//! TW3 TABLE (docs/dev/design/tw3-table.md; topor-way plan stage TW3): one variant table for all
//! runs, periodic restarts on the re-lay work clock, the four bests drawn and compared at 100 %
//! drawn (TopoR 6.3 manual pp. 72-73; Polubasov 2010 «Многовариантная оптимизация»).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

fn crop(name: &str) -> BoardModel {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    BoardModel::from_json(&std::fs::read_to_string(path).expect("fixture")).expect("fixture parses")
}

/// The TopoR method's settings, headless; `extra` adds keys (`, "key": value ...`).
fn topor(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut"}"#).expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
}

fn run(m: &BoardModel, o: &Options, threads: usize) -> RouteResult {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
    pool.install(|| {
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        route_with(m, o, &ctl)
    })
}

/// The copper of a result (segments, arcs, vias) as text.
fn copper(r: &RouteResult) -> String {
    serde_json::to_string(&r.items).expect("items")
}

fn line<'r>(r: &'r RouteResult, start: &str) -> &'r str {
    r.warnings.iter().find(|w| w.starts_with(start)).map(|s| s.as_str()).unwrap_or_else(|| panic!("{start}: {:?}", r.warnings))
}

#[test]
fn the_four_bests_are_drawn_once_each_and_listed() {
    let m = eprom();
    let r = run(&m, &topor(r#", "runs": 3, "restart_relays": 60, "topo_rounds": 12"#), 2);
    assert!(!r.bests.is_empty(), "{:?}", r.warnings);
    let roles: Vec<String> = r.bests.iter().flat_map(|b| b["roles"].as_array().expect("roles").iter().map(|x| x.as_str().unwrap().to_string())).collect();
    for role in ["best", "errors", "shortest", "vias"] {
        assert_eq!(roles.iter().filter(|x| *x == role).count(), 1, "role {role} held by exactly one drawn row: {roles:?}");
    }
    let rows: Vec<&str> = r.bests.iter().map(|b| b["row"].as_str().unwrap()).collect();
    let mut dedup = rows.clone();
    dedup.sort_unstable();
    dedup.dedup();
    assert_eq!(dedup.len(), rows.len(), "each distinct row drawn once: {rows:?}");
    // eprom: every run has the same pre-loop state, so a row is drawn by the run it came from
    assert!(r.bests.iter().all(|b| b["run"] == b["origin_run"]), "origin run first: {:?}", r.bests);
    assert_eq!(r.bests.iter().filter(|b| b["applied"].as_bool() == Some(true)).count(), 1);
    assert!(line(&r, "four bests of the variant table drawn").contains("APPLIED"));
    assert!(line(&r, "TopoR variant table (one for all 3 runs").contains("offers"));
    // candidates = 1: the overall best only
    let one = run(&m, &topor(r#", "runs": 3, "restart_relays": 60, "topo_rounds": 12, "draw_candidates": 1"#), 2);
    assert_eq!(one.bests.len(), 1);
    assert!(one.bests[0]["roles"].as_array().unwrap().iter().any(|x| x == "best"));
}

#[test]
fn the_result_does_not_depend_on_the_thread_count() {
    // commit order by cycle id: 1 and 4 threads, the same table, restarts, bests and copper
    let m = eprom();
    let o = topor(r#", "runs": 3, "restart_relays": 60, "topo_rounds": 12"#);
    let a = run(&m, &o, 1);
    let b = run(&m, &o, 4);
    assert_eq!(copper(&a), copper(&b));
    assert_eq!(a.bests, b.bests);
    assert_eq!(line(&a, "TopoR variant table"), line(&b, "TopoR variant table"));
    let restarts = |r: &RouteResult| r.warnings.iter().filter(|w| w.starts_with("TopoR loop restarts")).cloned().collect::<Vec<_>>();
    assert_eq!(restarts(&a), restarts(&b));
}

/// sync-wait (docs/dev/design/sync-wait.md): the fair slots and the lagged table views keep the
/// result a function of the seeds: 1, 2 and 4 threads (1, 2 and 4 compute slots for 3 runs: the runs
/// take turns at every cycle boundary with fewer slots than runs) give the same copper, table and
/// restarts for every `[optimize] table_lag`; lag 2 restarts from older tables than lag 0.
#[test]
fn the_result_does_not_depend_on_the_thread_count_at_any_table_lag() {
    let m = eprom();
    let restarts = |r: &RouteResult| r.warnings.iter().filter(|w| w.starts_with("TopoR loop restarts")).cloned().collect::<Vec<_>>();
    let mut by_lag = Vec::new();
    for lag in [0usize, 2] {
        let o = topor(&format!(r#", "runs": 3, "restart_relays": 60, "topo_rounds": 12, "table_lag": {lag}"#));
        let a = run(&m, &o, 1);
        for t in [2usize, 4] {
            let b = run(&m, &o, t);
            assert_eq!(copper(&a), copper(&b), "lag {lag}: 1 vs {t} threads");
            assert_eq!(a.bests, b.bests, "lag {lag}: 1 vs {t} threads");
            assert_eq!(line(&a, "TopoR variant table"), line(&b, "TopoR variant table"), "lag {lag}: 1 vs {t} threads");
            assert_eq!(restarts(&a), restarts(&b), "lag {lag}: 1 vs {t} threads");
        }
        assert!(!restarts(&a).is_empty(), "lag {lag}: the runs restart through the table (premise)");
        by_lag.push(restarts(&a));
    }
    assert_ne!(by_lag[0], by_lag[1], "lag 2 reads older tables: other restarts (premise: the option is used)");
}

#[test]
fn with_one_run_the_four_bests_are_drawn_one_after_another() {
    // condition (1) of the lead: runs = 1 draws every distinct row of the four bests itself
    let m = crop("esp-rust-board-usb-crop.json");
    let r = run(&m, &topor(r#", "runs": 1, "restart_relays": 200, "topo_rounds": 16"#), 1);
    let n_roles: usize = r.bests.iter().map(|b| b["roles"].as_array().unwrap().len()).sum();
    assert_eq!(n_roles, 4, "{:?}", r.bests);
    assert!(r.bests.iter().all(|b| b["run"] == 1));
}

/// TW3 D2 (lead 2026-11-19): a run whose loop ends at its re-lay budget (`loop_relays`) says so
/// in its stop reason ("re-lay budget", a work cap like rounds, never "round cap" or "stable"),
/// and the loop line names it.
#[test]
fn a_loop_ended_by_its_relay_budget_reports_it() {
    let r = run(&eprom(), &topor(r#""runs": 1, "loop_relays": 40, "topo_rounds": 100000"#), 1);
    assert_eq!(r.stop_reason, "re-lay budget", "{:?}", r.warnings);
    assert!(line(&r, "TopoR loop (").contains("ended by re-lay budget"), "{}", line(&r, "TopoR loop ("));
    assert!(!r.timed_out);
}
