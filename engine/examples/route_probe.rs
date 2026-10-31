//! Dev tool: run `route()` on a model JSON with a given `Options` JSON and print the outcome
//! per connection plus the layered-search counters.
//!
//! `cargo run --release --example route_probe -- <model.json> [--options '<Options JSON>']
//!     [--iters N] [--margin NM] [--quiet] [--dump out.json]`
//!
//! Engine-only reproduction of a harness run (same `route()` entry point, no Python). The M2
//! iteration-by-iteration replay of the single-layer pipeline (search order, sketch, relax and
//! self-check per wire) was retired with the planar negotiator: the two-layer router keeps one
//! wire per leg and its metrization state is private to `route.rs`.
use std::time::Instant;
use topor_engine::model::{BoardModel, Options};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut path, mut options_json, mut iters, mut margin) = (None, None, None, None);
    let mut quiet = false;
    let mut dump: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--iters" => {
                iters = Some(args[i + 1].parse::<u32>().expect("--iters N"));
                i += 1;
            }
            "--margin" => {
                margin = Some(args[i + 1].parse::<i64>().expect("--margin NM"));
                i += 1;
            }
            "--options" => {
                options_json = Some(args[i + 1].clone());
                i += 1;
            }
            "--quiet" => quiet = true,
            "--dump" => {
                dump = Some(args[i + 1].clone());
                i += 1;
            }
            a => path = Some(a.to_string()),
        }
        i += 1;
    }
    let path = path.expect(
        "usage: route_probe <model.json> [--options JSON] [--iters N] [--margin NM] [--quiet]",
    );
    let json = std::fs::read_to_string(&path).expect("read model");
    let model = BoardModel::from_json(&json).expect("parse model");
    let mut o = match options_json {
        Some(oj) => Options::from_json(&oj).expect("parse --options"),
        None => Options::default(),
    };
    if let Some(n) = iters {
        o.pathfinder_iter_cap = n;
    }
    if let Some(e) = margin {
        o.margin_nm = e;
    }
    topor_engine::search::search_totals_reset();
    let t = Instant::now();
    let r = topor_engine::route::route(&model, &o);
    let wall = t.elapsed().as_secs_f64();
    if let Some(d) = &dump {
        // result JSON without the run-dependent fields (determinism checks across thread counts)
        let mut v = serde_json::to_value(&r).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("runtime_s");
        o.remove("threads_used");
        std::fs::write(d, serde_json::to_string(&v).unwrap()).expect("write dump");
    }
    println!(
        "route(): {:.3}s wall, runtime_s {:.3}, routed {} / {}, vias {}, sites {}, iterations {}, converged {}, timed_out {}",
        wall,
        r.runtime_s,
        r.routed,
        r.connections_total,
        r.vias,
        r.sites,
        r.iterations,
        r.converged,
        r.timed_out
    );
    println!(
        "  length {:.3} mm, outside pads {:.3} mm, by layer {:?}, zero-slack {} {:?}",
        r.total_length_nm as f64 * 1e-6,
        r.length_outside_pads_nm as f64 * 1e-6,
        r.length_by_layer_nm,
        r.zero_slack,
        r.zero_slack_wires
            .iter()
            .map(|w| &w.name)
            .collect::<Vec<_>>()
    );
    for w in &r.warnings {
        println!("  warning: {w}");
    }
    if !quiet {
        for c in &r.connections {
            println!(
                "  #{} {} {} -> {}: {} {} vias {} layers {:?} {:.3} mm {}",
                c.id,
                c.name,
                c.from.label,
                c.to.label,
                c.status,
                c.reason,
                c.vias,
                c.layers,
                c.length_nm as f64 * 1e-6,
                c.pattern
            );
        }
    }
    let tot = topor_engine::search::search_totals();
    for (name, (ns, n, e, c, b)) in ["search", "ghost"].iter().zip(tot) {
        println!(
            "  {name}: {:.3}s in {n} runs, expansions {e} ({:.0}/run), candidates {c} ({:.0}/run), bend queries {b} ({:.0}/cand-per-query), {:.2} ms/run",
            ns as f64 * 1e-9,
            e as f64 / n.max(1) as f64,
            c as f64 / n.max(1) as f64,
            c as f64 / b.max(1) as f64,
            ns as f64 * 1e-6 / n.max(1) as f64
        );
    }
}
