//! pad-pour-via dev aid: replay the pour stitching (D4 class 1 + stitch-vias classes 2-4) on a
//! finished run's routed copper, in seconds, without routing again:
//!
//!     cargo run --release --example stitch_replay -- <dump_dir> [--pads REF.NUM,REF.NUM] [--opt NAME=JSON ...]
//!
//! `<dump_dir>` is written by `tools/stitch_replay_dump.py` (model.json, options.json, items.json,
//! routed_pads.json). Prints the stitching notes and, for every `--pads` pad, its pour-reach
//! verdict (which component its spokes join) before the stitching.

use topor_engine::geom::Vec2;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::pourcheck::check_pour_reach;
use topor_engine::topo::factors::{standard_factors, Registry};
use topor_engine::topo::stitch::{stitch_regions, stitch_until};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(args.first().expect("dump_dir"));
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    let m = BoardModel::from_json(&read("model.json")).expect("model.json");
    let mut o = Options::from_json(&read("options.json")).expect("options.json");
    for (i, a) in args.iter().enumerate() {
        if a == "--opt" {
            let v = args.get(i + 1).expect("--opt NAME=JSON");
            let (n, j) = v.split_once('=').expect("--opt NAME=JSON");
            let mut val = serde_json::to_value(&o).expect("options json");
            val[n] = serde_json::from_str(j).unwrap_or_else(|_| serde_json::Value::String(j.to_string()));
            o = serde_json::from_value(val).expect("option value");
        }
    }
    let items = Item::list_from_json(&read("items.json")).expect("items.json");
    let rp: Vec<serde_json::Value> = serde_json::from_str(&read("routed_pads.json")).expect("routed_pads.json");
    let mut routed_pads: Vec<usize> = rp
        .iter()
        .filter_map(|t| {
            let label = t["label"].as_str()?;
            let at = Vec2 { x: t["at"][0].as_f64()?, y: -t["at"][1].as_f64()? };
            (0..m.pads.len())
                .filter(|&i| format!("{}.{}", m.pads[i].ref_, m.pads[i].number) == label)
                .min_by(|&a, &b| (m.pads[a].center.to_vec2() - at).len().total_cmp(&(m.pads[b].center.to_vec2() - at).len()).then(a.cmp(&b)))
        })
        .collect();
    routed_pads.sort_unstable();
    routed_pads.dedup();
    let reg = Registry::from_config(standard_factors(), &o.factors).0;
    if let Some(i) = args.iter().position(|a| a == "--pads") {
        let want: Vec<&str> = args[i + 1].split(',').collect();
        let reach = check_pour_reach(&m, &items, &routed_pads);
        for w in want {
            let cut = reach.cuts.iter().find(|c| c.pads.iter().any(|&p| format!("{}.{}", m.pads[p].ref_, m.pads[p].number) == w));
            match cut {
                Some(c) => println!("PAD {w}: cut off, piece of {} island(s) {:.3} mm², {} pad(s)", c.islands.len(), c.area / 1e12, c.pads.len()),
                None => println!("PAD {w}: not in a cut piece (main component, or not a pour pad)"),
            }
        }
    }
    let t = std::time::Instant::now();
    let s = stitch_until(&m, &o, &reg, &items, &routed_pads, None, &|| false);
    println!("class 1: before {} after {}; island vias {} pad stubs {} joins {} ({:.2} s)", s.before, s.after, s.island_vias, s.pad_stubs, s.island_joins, t.elapsed().as_secs_f64());
    for n in &s.notes {
        println!("  {n}");
    }
    let mut all = items.clone();
    all.extend(s.items.iter().cloned());
    let r = stitch_regions(&m, &o, &reg, &all, &routed_pads, None, &|| false);
    for n in &r.notes {
        println!("  {n}");
    }
}
