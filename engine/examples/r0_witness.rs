//! R0 witness test on one board:
//!
//!     cargo run --release --example r0_witness -- <hand.json> [--g0 <g0.json>] [--drc <drc.json>]
//!         [--out r0.json]
//!
//! `hand.json` is the untouched board from `harness.g0 export`, `g0.json` the G0 result of the
//! same board, `drc.json` the `kicad-cli pcb drc --format json` report of the hand board.
//! Prints the metric block; `--out` writes the JSON that `harness.r0 render` draws.

use topor_engine::geom::Vec2;
use topor_engine::model::BoardModel;
use topor_engine::topo::stats;
use topor_engine::topo::witness::{run, G0In, WitnessOpts};

const CLEARANCE_TYPES: [&str; 8] = [
    "clearance", "shorting_items", "tracks_crossing", "hole_clearance", "copper_edge_clearance",
    "track_width", "via_diameter", "annular_width",
];

/// Positions (math frame, nm) of the copper-geometry violations of a KiCad DRC json.
fn drc_spots(path: &str) -> Vec<[f64; 2]> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect("drc json")).expect("drc json");
    let mut out = Vec::new();
    for x in v["violations"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        if !x["type"].as_str().is_some_and(|t| CLEARANCE_TYPES.contains(&t)) {
            continue;
        }
        for it in x["items"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            if let (Some(px), Some(py)) = (it["pos"]["x"].as_f64(), it["pos"]["y"].as_f64()) {
                out.push([px * 1e6, -py * 1e6]);
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut hand, mut g0, mut drc, mut out) = (None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--g0" => g0 = Some(args[i + 1].clone()),
            "--drc" => drc = Some(args[i + 1].clone()),
            "--out" => out = Some(args[i + 1].clone()),
            a => {
                hand = Some(a.to_string());
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    let hp = hand.expect("hand.json");
    let s = std::fs::read_to_string(&hp).unwrap_or_else(|e| panic!("{hp}: {e}"));
    let m = BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{hp}: {e}"));
    let g: Option<G0In> = g0.map(|p| serde_json::from_str(&std::fs::read_to_string(&p).expect("g0 json")).expect("g0 json"));
    let spots = drc.map(|p| drc_spots(&p)).unwrap_or_default();
    let w = run(&m, g.as_ref(), &spots, &WitnessOpts::default()).unwrap_or_else(|e| panic!("r0: {e}"));
    let s = &w.summary;
    for (c, k) in w.chains.iter().zip(&w.walked) {
        if let Some(er) = &k.err {
            let p = c.pts[0];
            println!("  failed chain net {} at ({:.2},{:.2}) mm: {er}", c.net, p.x / 1e6, -p.y / 1e6);
        }
    }
    let c = &s.counts;
    println!("{}: {} signal layers, {} chains ({} failed), {} passes", s.board, s.signal_layers, s.chains, s.chains_failed, s.passes);
    println!(
        "  mapped {:.3} % of {:.0} mm signal copper (+{:.2} mm inside nodes, {:.2} mm duplicate); degenerate {}; non-signal-layer copper {:?} mm",
        s.mapped_pct, s.signal_len_mm, s.in_node_len_mm, s.duplicate_len_mm, s.degenerate, s.nonsignal_len_mm
    );
    println!(
        "  violations: edge fit {}, layer combo {}, allowed {}, pair fit {}, same-layer crossings {}; hand DRC {} ({} spots), hand below rule {}",
        c.edge_fit, c.layer_combo, c.allowed, c.pair_fit, c.same_layer, c.hand_drc, s.hand_drc_spots, c.hand_below_rule
    );
    println!("  fit violations remaining under the proposed model (end clearances, room + inflation): {}", c.fit_remaining_after_fix);
    println!(
        "  {} edges and {} pair cuts ({} with traffic) checked; tight passes {}; G0 under the pair cuts: {:?}",
        s.edge_cuts_checked, s.pair_cuts, s.pair_cuts_with_traffic, s.tight_passes, s.g0_pair_violations
    );
    println!(
        "  topology: {} wires ({} excluded), state {}, crossings {} (same-layer {}, ambiguous {})",
        s.topo_wires, s.topo_excluded, s.state_check, s.crossings, s.same_layer_crossings, s.ambiguous_crossings
    );
    if s.p3_ran {
        println!(
            "  P3 witness: {} vias vs hand {} signal ({} total), unlayerable {}, same-layer {}",
            s.p3_vias, s.hand_signal_vias, s.hand_vias, s.p3_unlayerable, s.p3_same_layer
        );
    }
    println!(
        "  build {:.2} s, walk {:.2} s, fit {:.2} s, topology {:.2} s, total {:.2} s",
        s.build_s, s.walk_s, s.fit_s, s.topo_s, s.runtime_s
    );
    if let Some(p) = out {
        let st = stats::collect(&m, &w, g.as_ref());
        let j = serde_json::json!({ "summary": w.summary, "violations": w.violations, "stats": st });
        std::fs::write(&p, serde_json::to_string(&j).expect("json")).expect("write out");
    }
    let _ = Vec2 { x: 0.0, y: 0.0 };
}
