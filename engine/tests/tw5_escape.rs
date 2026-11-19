//! TW5 ESCAPE (docs/dev/design/tw5-escape.md) on the fit fixtures (engine/tests/fixtures/fit):
//! (e) the pre-loop P4 rounds run only with `[optimize] preloop_p4 = true`; the layering escape on
//! a 4-layer board with a full top layer and a free inner layer (inner_layer_room).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn model(name: &str) -> BoardModel {
    let path = format!("{}/tests/fixtures/fit/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json");
    BoardModel::from_json(&doc["model"].to_string()).expect("model")
}

fn opts(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "topo_rounds": 4, "runs": 1, "seed": 1}"#).unwrap();
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{extra}}}")).unwrap();
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).unwrap()
}

fn route(m: &BoardModel, o: &Options) -> RouteResult {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    route_with(m, o, &ctl)
}

fn layers_of(r: &RouteResult) -> String {
    let mut v: Vec<String> = r.connections.iter().map(|c| format!("{} {} vias {}", c.name, c.layers.join("/"), c.vias)).collect();
    v.sort();
    v.join("; ")
}

/// TW5 (e), D9: with the TopoR loop the negotiated rip-up rounds before it are folded into the
/// loop with `preloop_p4 = false` (TW5); the default `true` runs them as before TW5. inner_layer_room is
/// over-full on F.Cu when every wire takes the top layer (5 wires, room for 2), so the rounds have
/// work to do there.
#[test]
fn preloop_p4_rounds_run_only_when_asked() {
    let m = model("inner_layer_room");
    let p4 = |r: &RouteResult| r.warnings.iter().find(|w| w.starts_with("P4 negotiated rip-up")).cloned();
    let on = route(&m, &opts(""));
    let off = route(&m, &opts(r#""preloop_p4": false"#));
    eprintln!("default (preloop_p4 = true): {:?}\n  {}\npreloop_p4 = false: {:?}\n  {}", p4(&on), layers_of(&on), p4(&off), layers_of(&off));
    assert!(p4(&off).is_none(), "preloop_p4 = false: no P4 rounds before the loop: {:?}", p4(&off));
    assert!(p4(&on).is_some(), "default (preloop_p4 = true): the P4 rounds run on an over-full board: {:?}", on.warnings);
}


/// TW5 (f) `[optimize] loop_p4`, the in-loop text of the escapes' warning.
fn loop_p4_text(r: &RouteResult) -> String {
    let t = r.warnings.iter().find(|w| w.starts_with("TopoR loop escapes")).and_then(|w| w.split("in-loop P4 rounds ").nth(1)).unwrap_or("");
    t.split("; states after").next().unwrap_or("").to_string()
}

/// TW5 `[optimize] escape_offer`: the offers' text of the escapes' warning.
fn offer_text(r: &RouteResult) -> String {
    r.warnings.iter().find(|w| w.starts_with("TopoR loop escapes")).and_then(|w| w.split("rounds offered to the variant table ").nth(1)).unwrap_or("").to_string()
}

fn vias_of(r: &RouteResult) -> usize {
    r.connections.iter().map(|c| c.vias).sum()
}

/// TW5 (f) `[optimize] loop_p4` (docs/dev/design/tw5-escape.md (f)): with the pre-loop P4 rounds
/// off (D9) the loop's single-wire moves keep every wire of inner_layer_room on the full F.Cu
/// (room for 2 of 5): no single wire gains by taking two vias while the others hold the room
/// (Polubasov 2010 fig. 7, a local minimum). One negotiated round per cycle inside the loop rips
/// the over-full cut's wires together, and wires move to In1 (Polubasov 2010 fig. 6a-c: a wire
/// first takes two vias for the others). Seeds 0-5 (16 cycles): without the rounds no wire leaves
/// F.Cu; with them at least two do on every seed, with fewer drawn errors, and some seed reaches
/// the answer (three wires on In1, 6 vias, no error). Since TW3 (shared table, hybrid restarts)
/// which seeds reach the answer within 16 cycles changed (before: seeds 2 and 3; the planner
/// predicts more violations for the 6-via state than for the 4-via one, tw5 report).
#[test]
fn loop_p4_rounds_fold_the_preloop_rounds_into_the_loop() {
    let m = model("inner_layer_room");
    let errors = |r: &RouteResult| r.variants.first().map_or(usize::MAX, |v| v.errors);
    let (mut answer, mut better) = (0, 0);
    for seed in 0u64..6 {
        let base = format!(r#""topo_rounds": 16, "stable_patience": 16, "seed": {seed}, "preloop_p4": false"#);
        let e = route(&m, &opts(&base));
        let f = route(&m, &opts(&format!(r#"{base}, "loop_p4": true"#)));
        // loop_p4 also turns the loop history on: the history alone is the fair reference.
        let h = route(&m, &opts(&format!(r#"{base}, "loop_history": true"#)));
        eprintln!("seed {seed}\n  off: vias {} errors {} {}\n  history only: vias {} errors {} {}\n  on:  vias {} errors {} {} | {}", vias_of(&e), errors(&e), layers_of(&e), vias_of(&h), errors(&h), layers_of(&h), vias_of(&f), errors(&f), layers_of(&f), loop_p4_text(&f));
        assert_eq!(loop_p4_text(&e), "off", "loop_p4 off: the warning says so");
        let t = loop_p4_text(&f);
        let taken: usize = t.split(' ').next().and_then(|x| x.parse().ok()).unwrap_or(0);
        assert!(taken >= 1 && t.contains(" taken (") && t.contains("undone (a wire found no path)") && t.contains("undone (worse, escape_take \"not_worse\")") && t.contains("waited for the work budget"), "loop_p4 on: at least one round taken, every count named: {t}");
        assert_eq!(vias_of(&e), 0, "seed {seed}: without the in-loop rounds no wire leaves F.Cu: {}", layers_of(&e));
        assert!(vias_of(&f) >= 4 && errors(&f) < errors(&e), "seed {seed}: with them two wires or more reach In1, fewer drawn errors: {} errors {} vs {}", layers_of(&f), errors(&f), errors(&e));
        // The round itself, not the history it switches on: it rips wires (a "no rip" round is the history alone) and
        // is no worse than the history alone on any seed.
        let ripped: usize = t.split('(').nth(1).and_then(|x| x.split(' ').next()).and_then(|x| x.parse().ok()).unwrap_or(0);
        assert!(ripped >= 1, "seed {seed}: the rounds rip wires: {t}");
        assert!(vias_of(&f) >= vias_of(&h) && errors(&f) <= errors(&h), "seed {seed}: the rounds are no worse than the history alone: {} errors {} vs {} errors {}", vias_of(&f), errors(&f), vias_of(&h), errors(&h));
        better += usize::from(vias_of(&f) > vias_of(&h) || errors(&f) < errors(&h));
        answer += usize::from(vias_of(&f) == 6 && errors(&f) == 0);
    }
    assert!(better >= 1, "on some seed the rounds beat the history alone");
    assert!(answer >= 1, "some seed reaches the answer (three wires on In1, no error)");
}

/// TW5 (f): a round in which a wire finds no path is undone, never dropped (costly, not failed):
/// with the tests' forced failure of every round's second placing (`loop_p4_test_fail`), every
/// round is undone and every connection stays routed, on the layering the loop had.
#[test]
fn loop_p4_round_without_a_path_is_undone() {
    let m = model("inner_layer_room");
    let base = r#""topo_rounds": 6, "stable_patience": 6, "seed": 2, "preloop_p4": false, "loop_p4": true"#;
    let r = route(&m, &opts(&format!(r#"{base}, "loop_p4_test_fail": true"#)));
    let t = loop_p4_text(&r);
    eprintln!("{t}\n{}", layers_of(&r));
    assert!(t.starts_with("0 taken") && !t.contains(", 0 undone (a wire found no path)"), "every round undone as without a path: {t}");
    assert_eq!((r.routed, r.failed), (5, 0), "no connection dropped: {}", layers_of(&r));
    assert!(r.connections.iter().all(|c| !c.layers.is_empty()), "every connection drawn: {}", layers_of(&r));
}

/// TW5 (f): the rounds draw on the escapes' work budget (`escape_share`, lead 2026-11-19: a stuck
/// board cannot spend the loop on them): at share 0.05 fewer rounds run than at 0.5, some wait for
/// the budget, and the measured share stays near the bound.
#[test]
fn loop_p4_rounds_are_throttled_by_the_escape_budget() {
    let m = model("inner_layer_room");
    let run = |share: f64| route(&m, &opts(&format!(r#""topo_rounds": 16, "stable_patience": 16, "seed": 2, "preloop_p4": false, "loop_p4": true, "escape_share": {share}"#)));
    let nums = |t: &str| -> Vec<usize> { t.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect() };
    let share_of = |r: &RouteResult| -> f64 {
        let w = r.warnings.iter().find(|w| w.starts_with("TopoR loop escapes")).cloned().unwrap_or_default();
        w.split("work share of the escapes ").nth(1).and_then(|x| x.split(' ').next()).and_then(|x| x.parse().ok()).unwrap_or(f64::NAN)
    };
    let (lo, hi) = (run(0.05), run(0.5));
    let (tl, th) = (loop_p4_text(&lo), loop_p4_text(&hi));
    // "<taken> taken (<ripped> wires ripped), <lost> undone (...), <worse> undone (...), <waited> waited ..."
    let (nl, nh) = (nums(&tl), nums(&th));
    let ran = |n: &[usize]| n[0] + n[2] + n[3];
    eprintln!("share 0.05: {tl} (share {} %)\nshare 0.5: {th} (share {} %)", share_of(&lo), share_of(&hi));
    assert!(ran(&nl) < ran(&nh), "a smaller budget runs fewer rounds: {tl} | {th}");
    assert!(nl[4] >= 1, "share 0.05: rounds wait for the budget: {tl}");
    assert!(share_of(&lo) <= 10.0, "share 0.05: the measured share stays near the bound: {} %", share_of(&lo));
}

/// TW5 (f): with `escape = "layering"` on too the rounds still run (a shared budget was spent by the
/// escapes inside the sweep and `due()` was false after it: 0 rounds, every one waited): the two get
/// half of `escape_share` each.
#[test]
fn loop_p4_rounds_run_beside_the_layering_escapes() {
    let m = model("inner_layer_room");
    let base = r#""topo_rounds": 16, "stable_patience": 16, "seed": 2, "preloop_p4": false, "loop_p4": true"#;
    let both = route(&m, &opts(&format!(r#"{base}, "escape": "layering""#)));
    let t = loop_p4_text(&both);
    let w = both.warnings.iter().find(|w| w.starts_with("TopoR loop escapes")).cloned().unwrap_or_default();
    eprintln!("{t}\n{w}");
    let ran: usize = t.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).filter_map(|x| x.parse::<usize>().ok()).collect::<Vec<_>>().iter().enumerate().filter(|(i, _)| [0, 2, 3].contains(i)).map(|(_, n)| n).sum();
    assert!(ran >= 1, "with the layering escapes on, in-loop rounds run: {t}");
    let escapes: usize = w.split("layering escape \"layering\": ").nth(1).and_then(|x| x.split(' ').next()).and_then(|x| x.parse().ok()).unwrap_or(0);
    assert!(escapes >= 1, "the escapes run beside the rounds: {w}");
}

/// TW5 (f): a round is kept like the loop's other whole-board moves: `escape_take = "always"`
/// takes every round (none undone as worse), `"not_worse"` (default) undoes the ones that make the
/// board worse (on inner_layer_room with the pre-loop rounds on, every round is worse than the
/// state they left: all undone). Deterministic: the same options give the same copper.
#[test]
fn loop_p4_rounds_are_taken_as_escape_take_says() {
    let m = model("inner_layer_room");
    let base = r#""topo_rounds": 6, "stable_patience": 6, "seed": 0, "loop_p4": true"#;
    let nw = route(&m, &opts(base));
    let al = route(&m, &opts(&format!(r#"{base}, "escape_take": "always""#)));
    let count = |t: &str, key: &str| -> usize { t.split(", ").find(|x| x.contains(key)).and_then(|x| x.trim_start_matches(|c: char| !c.is_ascii_digit()).split(' ').next()).and_then(|x| x.parse().ok()).unwrap_or(usize::MAX) };
    let (tn, ta) = (loop_p4_text(&nw), loop_p4_text(&al));
    eprintln!("not_worse: {tn}\nalways: {ta}");
    assert_eq!(count(&ta, "worse"), 0, "always: no round undone as worse: {ta}");
    assert!(count(&ta, "taken") >= 1, "always: rounds taken: {ta}");
    assert!(count(&tn, "worse") >= 1, "not_worse: the worse rounds undone: {tn}");
    let again = route(&m, &opts(base));
    assert_eq!(layers_of(&again), layers_of(&nw), "deterministic per seed");
    assert_eq!(loop_p4_text(&again), tn);
}

/// TW5 `[optimize] escape_offer` (lead 2026-11-19: "escapes offer their result to the table; the
/// table keeps the bests"): the state after a taken in-loop P4 round goes to the run's archive
/// and the shared table before the cycle's own state (Polubasov 2010 «Многовариантная
/// оптимизация»: the table keeps the variants optimal for some criterion). Two runs share the
/// table: on, states are offered and the runs' restarts go on from such a row; off, none is
/// offered and no row comes from one. Deterministic: the same options give the same copper.
#[test]
fn escape_offer_puts_the_in_loop_round_states_into_the_table() {
    let m = model("inner_layer_room");
    let base = r#""topo_rounds": 16, "stable_patience": 16, "seed": 2, "runs": 2, "preloop_p4": false, "loop_p4": true"#;
    let on = route(&m, &opts(base));
    let off = route(&m, &opts(&format!(r#"{base}, "escape_offer": false"#)));
    let labels = |r: &RouteResult| r.variants.iter().map(|v| v.label.clone()).collect::<Vec<_>>();
    // the restarts go on from rows of the shared table, named by their origin
    let restarts = |r: &RouteResult| r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).and_then(|w| w.split(" restarts from the archive (").nth(1)).and_then(|x| x.split(");").next()).unwrap_or("").to_string();
    let (t_on, t_off) = (offer_text(&on), offer_text(&off));
    eprintln!("on: {t_on}\n  rows {:?}\n  {}\noff: {t_off}\n  rows {:?}\n  {}", labels(&on), layers_of(&on), labels(&off), layers_of(&off));
    let offered: usize = t_on.split(' ').next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let archived: usize = t_on.split("(").nth(1).and_then(|x| x.split(' ').next()).and_then(|x| x.parse().ok()).unwrap_or(0);
    assert!(archived >= 1, "on: a round's state is taken into the run's own archive: {t_on}");
    assert!(offered >= 1 && t_on.contains("taken into this run's archive") && t_on.contains("not offered: covered by its rows"), "on: states offered, every count named: {t_on}");
    assert_eq!(t_off, "off", "off: the warning says so");
    assert!(restarts(&on).contains("in-loop P4 round"), "on: a restart goes on from a table row that came from a round's state: {}", restarts(&on));
    assert!(!restarts(&off).contains("in-loop P4 round") && !labels(&off).iter().any(|l| l.contains("in-loop P4 round")), "off: no row from a round: {} | {:?}", restarts(&off), labels(&off));
    let again = route(&m, &opts(base));
    assert_eq!((layers_of(&again), offer_text(&again)), (layers_of(&on), t_on), "deterministic per seed");
}
