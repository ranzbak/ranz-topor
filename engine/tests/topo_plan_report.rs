//! thick-layer F4 (`[output] plan_report`, `topo/plandump.rs`): the result carries the plan
//! handed to the drawing of the row it drew (`RouteResult::plan`), so a user's run can be traced
//! plan vs drawing without a re-route (alt-route-probe 2026-11-22 R4: the user's qmtech row came
//! out of 32 runs with restarts and a wall stop and could not be replayed).
//!
//! Checked on the eprom fixture with the TopoR loop and 3 runs (the variant table's four bests
//! drawn, TW3): the plan's row is the applied row of `bests` for every pick (also when the applied
//! row is not the overall best's run result), its connections are the result's, its per-wire vias
//! add up to the compass's planned vias per connection, its crossings are symmetric and carry the
//! layers of both wires, and the report changes no copper (on / off, and the C0 `stop_after =
//! "plan"` plan equals the drawn round's).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

fn topor(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "runs": 3, "restart_relays": 60, "topo_rounds": 12}"#).expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
}

fn run(m: &BoardModel, o: &Options) -> RouteResult {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().expect("pool");
    pool.install(|| {
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        route_with(m, o, &ctl)
    })
}

fn plan(r: &RouteResult) -> serde_json::Value {
    serde_json::from_str(r.plan.as_deref().expect("plan report on by default")).expect("plan is JSON")
}

#[test]
fn the_plan_is_the_applied_rows_plan_for_every_pick() {
    let m = eprom();
    for pick in ["best", "vias", "shortest"] {
        let r = run(&m, &topor(&format!(r#", "pick": "{pick}""#)));
        let p = plan(&r);
        assert_eq!(p["format"], "topor-plan");
        assert_eq!(p["version"], 1);
        let applied: Vec<&serde_json::Value> = r.bests.iter().filter(|b| b["applied"].as_bool() == Some(true)).collect();
        assert_eq!(applied.len(), 1, "{pick}: {:?}", r.bests);
        assert_eq!(p["row"]["label"], applied[0]["row"], "{pick}: the plan belongs to the applied row");
        assert_eq!(p["row"]["loop"], "topor");
        // the compass of the result is the same plan's (one round built both)
        let cp = r.compass.as_ref().expect("compass");
        assert_eq!(p["predicted_violations"].as_u64(), Some(cp.predicted_violations as u64), "{pick}");
    }
}

#[test]
fn the_plan_lists_the_connections_vias_and_crossings_of_the_result() {
    let m = eprom();
    let r = run(&m, &topor(""));
    let p = plan(&r);
    let cp = r.compass.as_ref().expect("compass");
    let wires = p["wires"].as_array().expect("wires");
    assert!(!wires.is_empty());
    let layers = p["layers"].as_array().expect("layers").len();
    assert!(layers >= 2, "{:?}", p["layers"]);
    // per connection: the compass's planned vias = the vias of its wires in the plan
    for c in &cp.conns {
        let v: usize = wires.iter().filter(|w| w["conn"].as_u64() == Some(c.conn as u64)).map(|w| w["vias"].as_array().unwrap().len()).sum();
        assert_eq!(v, c.vias, "conn {}", c.conn);
        assert_eq!(p["conns"][c.conn.to_string()]["vias"].as_u64(), Some(c.vias as u64));
    }
    // every routed connection of the result is a connection of the plan
    for cr in r.connections.iter().filter(|c| c.status == "routed" && c.length_nm > 0 && c.stub_nm == 0) {
        assert!(wires.iter().any(|w| w["conn"].as_u64() == Some(cr.id as u64)), "conn {} not in the plan", cr.id);
    }
    let by_wire = |id: u64| wires.iter().find(|w| w["wire"].as_u64() == Some(id));
    let mut n_x = 0;
    for w in wires {
        let me = w["wire"].as_u64().unwrap();
        let nl = w["layers_now"].as_array().unwrap().len();
        for v in w["vias"].as_array().unwrap() {
            let (a, b, k) = (v["iv"][0].as_u64().unwrap(), v["iv"][1].as_u64().unwrap(), v["k"].as_u64().unwrap());
            assert!(a <= k && k <= b.max(a), "via traversal inside its interval: {v}");
            assert!(v["deficit"].as_f64().unwrap() >= 0.0);
        }
        for x in w["crossings"].as_array().unwrap() {
            n_x += 1;
            assert!((x["k"].as_u64().unwrap() as usize) < nl);
            let o = by_wire(x["other"].as_u64().unwrap()).expect("the other wire is in the plan");
            // symmetric: the other wire lists this crossing back, with the layers swapped
            let back = o["crossings"].as_array().unwrap().iter().find(|y| y["other"].as_u64() == Some(me) && y["k"] == x["other_k"] && y["other_k"] == x["k"]).unwrap_or_else(|| panic!("crossing of {me} not listed back: {x}"));
            assert_eq!(back["layers"], x["other_layers"]);
            assert_eq!(back["other_layers"], x["layers"]);
            assert!(!x["layers"].as_array().unwrap().is_empty());
        }
        for ps in w["passes"].as_array().unwrap() {
            assert!(ps["pos"].as_u64().unwrap() < ps["n"].as_u64().unwrap(), "pass position inside its edge's order: {ps}");
        }
    }
    assert!(n_x > 0, "eprom has crossings");
}

#[test]
fn the_plan_report_changes_no_copper() {
    let m = eprom();
    let on = run(&m, &topor(""));
    let off = run(&m, &topor(r#", "plan_report": false"#));
    assert!(off.plan.is_none());
    assert_eq!(serde_json::to_string(&on.items).unwrap(), serde_json::to_string(&off.items).unwrap());
    assert_eq!(on.bests, off.bests);
    let j = serde_json::to_value(&off).unwrap();
    assert!(j.get("plan").is_none(), "off: no plan key in the result JSON");
    // cycles loop, one run, one round: the plan before the drawing (C0 stop_after = plan) is the
    // plan the full round drew
    let cyc = |extra: &str| Options::from_json(&format!(r#"{{"runs": 1, "wall_clock_s": 600.0, "topo_rounds": 1, "opt_loop": "cycles"{extra}}}"#)).expect("options");
    let a = run(&m, &cyc(""));
    let s = run(&m, &cyc(r#", "stop_after": "plan""#));
    let (pa, ps) = (plan(&a), plan(&s));
    assert_eq!(pa["wires"], ps["wires"]);
    assert_eq!(pa["row"]["label"], serde_json::Value::Null);
    assert_eq!(pa["row"]["round"], 1);
}
