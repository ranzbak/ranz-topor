//! S3b learning loop (`topo::learn`): factor weights fitted to hand routings.
//!
//!     cargo run --release --example learn -- --board NAME HAND.json INPUT.json [--board ...]
//!         [--hold-out NAME] [--eta 5] [--lambda 0.001] [--eps 0.05] [--iters 12] [--margin 2]
//!         [--learn via,crossing,squeeze] [--tier N] [--out learned.json]
//!         [--toml learned.toml] [--stats stats.json] [--stats-only]
//!
//! `HAND.json` is the hand board and `INPUT.json` the same board stripped of tracks and vias
//! (`harness.g0 export`). Trains on every `--board` except `--hold-out`. `--stats` writes, per
//! board (the held-out one included), the hand's and the engine's (default weights, no loss
//! augmentation) factor totals, event counts and loss: the input of the candidate-factor list.
//! The learned weights are written as a `topor.toml [factors]` snippet (`--toml`); they are never
//! applied by default.

use std::collections::BTreeMap;
use topor_engine::model::BoardModel;
use topor_engine::topo::factors::Registry;
use topor_engine::topo::learn::{learn, prepare, route_lite, toml_snippet, LearnOpts, Sample};

fn load(p: &str) -> BoardModel {
    let s = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut boards: Vec<(String, String, String)> = Vec::new();
    let mut o = LearnOpts::default();
    let (mut hold, mut out, mut toml, mut stats) = (None::<String>, None::<String>, None::<String>, None::<String>);
    let mut stats_only = false;
    let mut i = 0;
    let num = |a: &[String], i: usize| -> f64 { a.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or_else(|| panic!("{} needs a number", a[i])) };
    while i < args.len() {
        let a = args[i].as_str();
        let mut n = 2;
        match a {
            "--board" => {
                boards.push((args[i + 1].clone(), args[i + 2].clone(), args[i + 3].clone()));
                n = 4;
            }
            "--hold-out" => hold = Some(args[i + 1].clone()),
            "--eta" => o.eta = num(&args, i),
            "--lambda" => o.lambda = num(&args, i),
            "--eps" => o.eps = num(&args, i),
            "--iters" => o.iters = num(&args, i) as usize,
            "--margin" => o.margin_mm = num(&args, i),
            "--tier" => o.tier = Some(num(&args, i) as u8),
            "--learn" => o.learn = args[i + 1].split(',').map(str::to_string).collect(),
            "--out" => out = Some(args[i + 1].clone()),
            "--toml" => toml = Some(args[i + 1].clone()),
            "--stats" => stats = Some(args[i + 1].clone()),
            "--stats-only" => {
                stats_only = true;
                n = 1;
            }
            _ => panic!("unknown argument {a}"),
        }
        i += n;
    }
    assert!(!boards.is_empty(), "no --board");
    let want = |name: &str| stats.is_some() || hold.as_deref() != Some(name);
    // Prepare the boards in parallel, merged in argument order.
    let samples: Vec<Option<Sample>> = std::thread::scope(|sc| {
        let hs: Vec<_> = boards
            .iter()
            .map(|(name, h, inp)| {
                let go = want(name);
                let tier = o.tier;
                sc.spawn(move || go.then(|| prepare(name, &load(h), &load(inp), tier).unwrap_or_else(|e| panic!("{name}: {e}"))))
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("prepare thread")).collect()
    });
    for s in samples.iter().flatten() {
        let t = &s.hand.totals;
        eprintln!(
            "{}: {} hand wires, {} stretches, {} engine connections ({} skipped)",
            s.name,
            t.wires,
            s.hand.stretches.iter().filter(|x| !x.edges.is_empty()).count(),
            s.conns.len(),
            s.skipped
        );
    }
    if let Some(p) = &stats {
        let base = Registry::standard();
        let runs: Vec<_> = std::thread::scope(|sc| {
            let hs: Vec<_> = samples.iter().flatten().map(|s| sc.spawn(|| route_lite(s, &base, &base, o.tier, 0.0))).collect();
            hs.into_iter().map(|h| h.join().expect("stats thread")).collect()
        });
        let names = Registry::standard().names();
        let j: Vec<_> = samples
            .iter()
            .flatten()
            .zip(&runs)
            .map(|(s, r)| {
                eprintln!("{}: default-weight loss {:.3} ({} of {} hand stretches reproduced), engine vias {}", s.name, r.loss, r.reproduced, r.stretches, r.vias);
                serde_json::json!({"board": s.name, "factors": names, "hand": s.hand.totals, "engine": r})
            })
            .collect();
        std::fs::write(p, serde_json::to_string_pretty(&j).expect("json")).expect("write stats");
    }
    if stats_only {
        return;
    }
    let train: Vec<Sample> = samples.into_iter().flatten().filter(|s| hold.as_deref() != Some(s.name.as_str())).collect();
    let r = learn(&train, &o).unwrap_or_else(|e| panic!("{e}"));
    for (k, it) in r.iterations.iter().enumerate() {
        let ml = it.loss.iter().sum::<f64>() / it.loss.len().max(1) as f64;
        eprintln!("iter {k}: weights {:?} mean aug. loss {ml:.3} step {:.3}", it.weights, it.step);
    }
    println!("learned (train {}, hold-out {}): {:?}{}", r.boards.join(","), hold.as_deref().unwrap_or("-"), r.weights, if r.converged { " (converged)" } else { "" });
    let note = format!("trained on {}; held out {}", r.boards.join(", "), hold.as_deref().unwrap_or("none"));
    if let Some(p) = toml {
        std::fs::write(p, toml_snippet(&r.weights, o.tier, &note)).expect("write toml");
    }
    if let Some(p) = out {
        let w: BTreeMap<_, _> = r.weights.clone();
        let j = serde_json::json!({"result": r, "hold_out": hold, "weights": w});
        std::fs::write(p, serde_json::to_string_pretty(&j).expect("json")).expect("write out");
    }
}
