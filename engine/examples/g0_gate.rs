//! G0 flat-plane gate on one board:
//!
//!     cargo run --release --example g0_gate -- <input.json> [<hand.json>] [--out g0.json]
//!         [--cross MM] [--via MM] [--steiner MM] [--round-steps N] [--forced MM] [--smd-over MM]
//!
//! `input.json` is the board model with tracks and vias removed, `hand.json` the untouched board
//! (both from `harness.g0 export`). Prints the metrics; `--out` writes the JSON that
//! `harness.g0 render` draws.

use topor_engine::model::BoardModel;
use topor_engine::topo::gate::{run, G0Opts};

fn load(p: &str) -> BoardModel {
    let s = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let mut out = None;
    let mut o = G0Opts::default();
    let mut i = 0;
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).expect("flag value").clone();
        match args[i].as_str() {
            "--out" => out = Some(val(i)),
            "--cross" => o.init.cross_mm = val(i).parse().expect("--cross MM"),
            "--forced" => o.init.forced_layer_mm = val(i).parse().expect("--forced MM"),
            "--smd-over" => o.init.smd_over_mm = val(i).parse().expect("--smd-over MM"),
            "--via" => o.layers.via_mm = val(i).parse().expect("--via MM"),
            "--round-steps" => o.build.round_steps = val(i).parse().expect("--round-steps N"),
            "--steiner" => o.build.steiner_mm = val(i).parse().expect("--steiner MM"),
            a => {
                pos.push(a.to_string());
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    let input = load(pos.first().expect("input.json"));
    let hand = pos.get(1).map(|p| load(p));
    let r = run(&input, hand.as_ref(), &o).unwrap_or_else(|e| panic!("g0: {e}"));
    let s = &r.summary;
    println!(
        "{}: nets {} (+{} poured), connections {} (routed {}, failed {}), overflows {}",
        s.board, s.nets, s.poured_nets, s.connections, s.routed, s.failed, s.overflows
    );
    println!(
        "  crossings {} -> {} ({} swaps); domain graph {}n/{}e -> {}n/{}e ({:.1} % reduced)",
        s.crossings_before, s.crossings_after, s.swaps, s.graph_nodes, s.graph_edges,
        s.reduced_nodes, s.reduced_edges, s.reduction_pct
    );
    println!(
        "  vias {} ({} without site), unlayerable {}, same-layer {}; hand {} vias ({} signal)",
        s.vias, s.vias_no_site, s.unlayerable, s.same_layer, s.hand_vias, s.hand_signal_vias
    );
    println!(
        "  detour median {:.2} p90 {:.2} max {:.2}, {} wires > 1.5; plane stub vias {}",
        s.detour_median, s.detour_p90, s.detour_max, s.detours_over_1_5, s.plane_stub_vias
    );
    println!(
        "  {} triangles; build {:.2} s, P2 {:.2} s, P3 {:.2} s, total {:.2} s",
        s.tris, s.build_s, s.p2_s, s.p3_s, s.runtime_s
    );
    if let Some(p) = out {
        std::fs::write(&p, serde_json::to_string(&r).expect("json")).expect("write out");
    }
}
