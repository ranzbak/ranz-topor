//! site-price-cost (docs/dev/design/site-price-cost.md): the TopoR loop's measured escape budget
//! (`[optimize] escape_budget = "cost"`). Its own test binary: it turns the global profiler on
//! (`prof::set_on`) and reads the run's `work` rows, which no other test may write meanwhile.

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;
use topor_engine::topo::escape::cost;
use topor_engine::topo::prof;

const MM: f64 = 1e6;

/// A 30 x 20 mm 4-layer board with the crossing fan of tests/loop_local.rs (`fan_pads` + F3).
fn fan4() -> BoardModel {
    let pads: &[(&str, i32, f64, f64, u32)] = &[
        ("A1", 1, 2.0, 6.0, 0),
        ("A2", 1, 28.0, 6.0, 0),
        ("C1", 3, 2.0, 14.0, 2),
        ("C2", 3, 28.0, 14.0, 0),
        ("E1", 5, 2.0, 10.0, 0),
        ("E2", 5, 28.0, 11.0, 2),
        ("B1", 2, 9.0, 1.5, 2),
        ("B2", 2, 10.0, 18.5, 2),
        ("F1", 6, 16.0, 1.5, 0),
        ("F2", 6, 15.0, 18.5, 2),
        ("G1", 7, 22.0, 1.5, 0),
        ("G2", 7, 23.0, 18.5, 0),
        ("D1", 4, 2.0, 7.5, 0),
        ("D2", 4, 28.0, 7.5, 0),
        ("F3", 6, 26.0, 4.0, 0),
    ];
    let p = |v: f64| (v * MM) as i64;
    let pad = |r: &str, net: i32, x: f64, y: f64, s: f64, l: u32| {
        format!(
            r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,"layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}}}}"#,
            p(x),
            p(y),
            p(s),
            p(s)
        )
    };
    let mut ps: Vec<String> = pads.iter().map(|&(r, n, x, y, l)| pad(r, n, x, y, 1.0, l)).collect();
    // A foreign pad on each layer in the middle (no via site there).
    ps.push(pad("K0", 8, 12.5, 10.0, 2.0, 0));
    ps.push(pad("K1", 8, 19.5, 8.0, 2.0, 2));
    let c = [(0.0, 0.0), (30.0, 0.0), (30.0, 20.0), (0.0, 20.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let nets: Vec<String> = (1..=8).map(|i| format!(r#"{{"code":{i},"name":"N{i}"}}"#)).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":4,"name":"In1.Cu","kind":"signal"}},{{"id":6,"name":"In2.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{}],"pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        nets.join(","),
        ps.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// The TW5 loop of tests/loop_local.rs (`loop_opts` + `TW5`, local judge) with `extra` fields.
fn opts(extra: &str) -> Options {
    let s = format!(
        r#"{{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 3, "runs": 1, "seed": 3,
            "loop_judge": "local", "oscillation": "bounded", "escape": "layering", "escape_take": "always", "relay_count_fade": 0.8,
            "loop_history": true, "float_wire": true, "preloop_p4": false{extra}}}"#
    );
    Options::from_json(&s).unwrap()
}

/// One run: its plan metrics "loop" -> "escape" record and the profile's work rows.
fn run(m: &BoardModel, o: &Options) -> (serde_json::Value, Vec<serde_json::Value>) {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let r = route_with(m, o, &ctl);
    let line = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).expect("plan metrics line");
    let v: serde_json::Value = serde_json::from_str(line.trim_start_matches("plan metrics:").trim()).expect("plan metrics JSON");
    let p = prof::report().expect("profile");
    (v["loop"]["escape"].clone(), p["work"].as_array().cloned().unwrap_or_default())
}

/// The measured budget prices each sweep re-lay by the cost terms of its counts (fixed part per pass,
/// local layering evaluations, judged wires), and both throttle totals are the sums of the escapes'
/// and re-lays' own counter changes; the escapes stay within their share of that cost. With
/// "units" the totals are wire units, not the counter.
#[test]
fn the_cost_budget_counts_each_step_by_its_cost_terms() {
    prof::set_on(true);
    let m = fan4();
    let near = |a: f64, b: f64, tol: f64| (a - b).abs() <= tol;
    let f = |r: &serde_json::Value, k: &str| r[k].as_f64().unwrap_or_else(|| panic!("{k} in {r}"));
    let (e, rows) = run(&m, &opts(""));
    assert_eq!(e["budget"], "cost", "{e}");
    let esc: Vec<_> = rows.iter().filter(|r| r["kind"] == "escape").collect();
    let rel: Vec<_> = rows.iter().filter(|r| r["kind"] == "relay").collect();
    assert!(!esc.is_empty() && !rel.is_empty(), "escapes {} re-lays {}", esc.len(), rel.len());
    assert_eq!(esc.len() as f64, f(&e, "escapes"), "{e}");
    for r in &rel {
        let want = cost::relay(f(r, "passes") as usize) + cost::local(f(r, "evals") as usize, f(r, "local_wires") as usize);
        // A re-lay whose judge fell back to the whole board also pays a global layering + measure.
        assert!(near(f(r, "cost"), want, 1e-9) || f(r, "cost") > want, "re-lay row {r}: cost terms {want}");
    }
    assert!(rel.iter().any(|r| near(f(r, "cost"), cost::relay(f(r, "passes") as usize) + cost::local(f(r, "evals") as usize, f(r, "local_wires") as usize), 1e-9)));
    let w = e["work_units"].as_array().expect("work_units");
    let (ew, rw) = (w[0].as_f64().unwrap(), w[1].as_f64().unwrap());
    let (se, sr): (f64, f64) = (esc.iter().map(|r| f(r, "cost")).sum(), rel.iter().map(|r| f(r, "cost")).sum());
    assert!(near(ew, se, 0.051) && near(rw, sr, 0.051), "work_units {w:?} vs the rows' cost {se} {sr}");
    assert!(se / (se + sr) <= 0.35 + 0.05, "escape share of the counted cost {}", se / (se + sr));
    assert!(sr > 0.0 && f(&e, "relay_wires") > 0.0 && !near(sr, f(&e, "relay_wires"), 0.051), "{e}");
    // "units": the re-lays in wire units (the counter still runs for the rows).
    let (u, rows_u) = run(&m, &opts(r#", "escape_budget": "units""#));
    let sr_u: f64 = rows_u.iter().filter(|r| r["kind"] == "relay").map(|r| f(r, "cost")).sum();
    let wu = u["work_units"].as_array().expect("work_units");
    assert!(near(wu[1].as_f64().unwrap(), f(&u, "relay_wires"), 1e-9) && !near(wu[1].as_f64().unwrap(), sr_u, 0.051), "{u}");
    prof::set_on(false);
}
