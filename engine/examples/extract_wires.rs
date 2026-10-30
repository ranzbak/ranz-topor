//! Dev tool: count the wires the smoother would see in a model JSON.
//!
//! `cargo run --release --example extract_wires -- <model.json>` (JSON from
//! `harness.model.export_model` / `write_model_json`). Prints the wire count, skipped items with
//! reasons and the total hand length in mm. Not part of the PyO3 surface.
use std::collections::{BTreeMap, BTreeSet};
use topor_engine::model::{BoardModel, ExistingCopper, Options};
use topor_engine::wires::extract_wires;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: extract_wires <model.json>");
    let text = std::fs::read_to_string(&path).expect("read model json");
    let model = BoardModel::from_json(&text).expect("parse model");
    let opts = Options {
        existing_copper: ExistingCopper::Smooth,
        ..Options::default()
    };
    let e = extract_wires(&model, &opts);
    let hand_mm: f64 = e.wires.iter().map(|w| w.hand_len).sum::<f64>() / 1e6;
    let nets: BTreeSet<i32> = e.wires.iter().map(|w| w.net).collect();
    println!("wires: {}", e.wires.len());
    println!("nets_with_wires: {}", nets.len());
    let mut per_net: BTreeMap<&str, usize> = BTreeMap::new();
    for w in &e.wires {
        let name = model
            .nets
            .iter()
            .find(|n| n.code == w.net)
            .map_or("?", |n| n.name.as_str());
        *per_net.entry(name).or_default() += 1;
    }
    let multi: Vec<String> = per_net
        .iter()
        .filter(|(_, &c)| c > 1)
        .map(|(n, c)| format!("{n}={c}"))
        .collect();
    println!("nets_with_several_wires: {}", multi.join(" "));
    println!("skipped: {}", e.skipped.len());
    for (uuid, reason) in &e.skipped {
        println!("  skipped {uuid}: {reason}");
    }
    println!("hand_len_mm: {hand_mm:.3}");
}
