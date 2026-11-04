//! S2a on one exported board model, without Python:
//!
//!     cargo run --release --example topo_route -- <input.json> [--out result.json] [--timeout S]
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
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    println!(
        "{}: connections {} routed {} failed {} vias {} runtime {:.2} s ({})",
        m.board, r.connections_total, r.routed, r.failed, r.vias, r.runtime_s, r.stop_reason
    );
    let mut why: BTreeMap<String, usize> = BTreeMap::new();
    for c in r.connections.iter().filter(|c| c.status != "routed") {
        let key: String = c.reason.chars().take(70).collect();
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
    for w in r.warnings.iter().take(12) {
        println!("  warning: {w}");
    }
    if let Some(p) = flag("--out") {
        std::fs::write(&p, serde_json::to_string(&r).expect("json")).expect("write");
    }
}
