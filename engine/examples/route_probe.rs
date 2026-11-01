//! Dev tool: run `route()` on a model JSON with a given `Options` JSON and print the outcome
//! per connection plus the layered-search counters.
//!
//! `cargo run --release --example route_probe -- <model.json> [--options '<Options JSON>']
//!     [--iters N] [--margin NM] [--quiet] [--profile] [--dump out.json]`
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
    let (mut quiet, mut profile) = (false, false);
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
            "--profile" => profile = true,
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
    o.profile |= profile; // or `"profile": true` in --options
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
    if let Some(p) = &r.profile {
        println!("  iter  search_s  commit_s metrize_s  check_s searched re-searched revalid ripped paths overuse     ok blocked mfail   vias   cost_mm best");
        for it in p {
            println!(
                "  {:>4} {:>9.3} {:>9.3} {:>9.3} {:>8.3} {:>8} {:>11} {:>7} {:>6} {:>5} {:>7} {:>6} {:>7} {:>5} {:>6} {:>9.1} {}",
                it.iteration,
                it.search_s,
                it.commit_s,
                it.metrize_s,
                it.check_s,
                it.searched,
                it.re_searched,
                it.revalidated,
                it.ripped,
                it.routed,
                it.overuse_edges,
                it.ok,
                it.blocked,
                it.metrize_failed,
                it.vias,
                it.cost_nm * 1e-6,
                if it.best { "*" } else { "" }
            );
            if !it.fail_classes.is_empty() || !it.blocked_classes.is_empty() {
                println!(
                    "        mfail {:?} | blocked {:?}",
                    it.fail_classes, it.blocked_classes
                );
            }
        }
        let sum = |f: fn(&topor_engine::route::IterProfile) -> f64| {
            p.iter().map(f).fold(0.0, |a, b| a + b)
        };
        println!(
            "  total search {:.3}s commit {:.3}s metrize {:.3}s (check {:.3}s) of {:.3}s",
            sum(|i| i.search_s),
            sum(|i| i.commit_s),
            sum(|i| i.metrize_s),
            sum(|i| i.check_s),
            wall
        );
    }
    let tot = topor_engine::search::search_totals();
    for (name, (ns, n, e, c, b, sk, mh)) in ["search", "ghost"].iter().zip(tot) {
        println!(
            "  {name}: {:.3}s in {n} runs, expansions {e} ({:.0}/run), candidates {c} ({:.0}/run), bend queries {b} ({:.0}/cand-per-query), skipped sub-chords {sk}, memo hits {mh}, {:.2} ms/run",
            ns as f64 * 1e-9,
            e as f64 / n.max(1) as f64,
            c as f64 / n.max(1) as f64,
            c as f64 / b.max(1) as f64,
            ns as f64 * 1e-6 / n.max(1) as f64
        );
    }
}
