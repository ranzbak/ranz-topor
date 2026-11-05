//! S2a on one exported board model, without Python:
//!
//!     cargo run --release --example topo_route -- <input.json> [--out result.json] [--timeout S] [--plane-signals costly|forbidden] [--opt NAME=JSON ...]
//!
//! `input.json` is the board model with tracks and vias removed (`harness.g0 export`). Prints the
//! routed count, the failure reasons and the detour ratios; `--out` writes the `RouteResult` JSON.

use std::collections::BTreeMap;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("input.json");
    let flag = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
    let s = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let m = BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut o = Options::default();
    if let Some(t) = flag("--timeout") {
        o.wall_clock_s = t.parse().expect("--timeout S");
    }
    if let Some(v) = flag("--plane-signals") {
        o.plane_signals = v;
    }
    for (i, a) in args.iter().enumerate() {
        if a == "--factor" {
            let v = args.get(i + 1).expect("--factor NAME=WEIGHT");
            let (n, w) = v.split_once('=').expect("--factor NAME=WEIGHT");
            o.factors.weight.insert(n.to_string(), w.parse().expect("weight"));
        }
    }
    // --opt NAME=JSON: any engine option (e.g. --opt moves=false --opt fluctuation=0).
    for (i, a) in args.iter().enumerate() {
        if a == "--opt" {
            let v = args.get(i + 1).expect("--opt NAME=JSON");
            let (n, j) = v.split_once('=').expect("--opt NAME=JSON");
            let mut val = serde_json::to_value(&o).expect("options json");
            val[n] = serde_json::from_str(j).unwrap_or_else(|_| serde_json::Value::String(j.to_string()));
            o = serde_json::from_value(val).expect("option value");
        }
    }
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    println!(
        "{}: connections {} routed {} failed {} vias {} runtime {:.2} s ({})",
        m.board, r.connections_total, r.routed, r.failed, r.vias, r.runtime_s, r.stop_reason
    );
    let mut why: BTreeMap<String, usize> = BTreeMap::new();
    for c in r.connections.iter().filter(|c| c.status != "routed") {
        let key: String = if let Some(i) = c.reason.find(" against ") {
            let kind: String = c.reason[i + 9..].chars().take_while(|&ch| ch != ':').collect();
            format!("drawing: clearance against {kind}")
        } else if c.reason.starts_with("pad ") || c.reason.starts_with("via ") {
            c.reason.split(':').nth(1).unwrap_or("").chars().take(60).collect::<String>()
        } else {
            c.reason.chars().take(70).collect()
        };
        *why.entry(key).or_default() += 1;
    }
    for (k, v) in &why {
        println!("  {v:5}  {k}");
    }
    let mut ratios: Vec<f64> = r
        .connections
        .iter()
        .filter(|c| c.status == "routed")
        .filter_map(|c| c.ghost_len_nm.filter(|&g| g > 0).map(|g| c.length_nm as f64 / g as f64))
        .collect();
    ratios.sort_by(f64::total_cmp);
    let q = |f: f64| ratios.get(((ratios.len() as f64 - 1.0) * f).round() as usize).copied().unwrap_or(0.0);
    println!("  detour (vs straight pad distance) median {:.3} p90 {:.3} max {:.3}", q(0.5), q(0.9), q(1.0));
    let mut top: Vec<_> = r.connections.iter().filter(|c| c.status == "routed" && c.ghost_len_nm.unwrap_or(0) > 0).collect();
    top.sort_by(|a, b| {
        let f = |c: &&topor_engine::route::ConnReport| c.length_nm as f64 / c.ghost_len_nm.unwrap_or(1) as f64;
        f(b).total_cmp(&f(a))
    });
    for c in top.iter().take(8) {
        println!(
            "  top detour {:.2}: {} {} -> {} len {:.2} mm straight {:.2} mm vias {}",
            c.length_nm as f64 / c.ghost_len_nm.unwrap_or(1) as f64,
            c.name,
            c.from.label,
            c.to.label,
            c.length_nm as f64 / 1e6,
            c.ghost_len_nm.unwrap_or(0) as f64 / 1e6,
            c.vias
        );
    }
    for w in r.warnings.iter().filter(|w| !w.contains("skipped") && !w.contains("island")).take(12) {
        println!("  warning: {w}");
    }
    if let Some(p) = flag("--out") {
        std::fs::write(&p, serde_json::to_string(&r).expect("json")).expect("write");
    }
}
