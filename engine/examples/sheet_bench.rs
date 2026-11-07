//! Inflated sketch U0: the static layer sheets of one exported board model, timed, without routing.
//!
//!     cargo run --release --example sheet_bench -- <input.json> [--options options.json] [--style octilinear|smooth] [--full]
//!
//! Prints per signal layer the sheet size, the critical-cut counts (raw / shadowed / kept) and the
//! times; `--full` also walks without shadow pruning (the raw count).

use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::sheet::{LayerSheet, Norm, SheetInput, SheetOpts};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("input.json");
    let flag = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
    let m = BoardModel::from_json(&std::fs::read_to_string(path).expect("read")).expect("model");
    let o = match flag("--options") {
        Some(f) => Options::from_json(&std::fs::read_to_string(&f).expect("read")).expect("options"),
        None => Options::default(),
    };
    let style = flag("--style").unwrap_or(o.draw_style.clone());
    let bo = BuildOpts { existing_vias: true, keepouts: true, plane_layers: o.plane_signals != "forbidden", copper_shapes: true, ..BuildOpts::default() };
    let t = std::time::Instant::now();
    let b = board::build_full(&m, &bo, &[], &[]).expect("projection");
    println!("projection: {} vertices, {} triangles, {:.3} s", b.proj.pos.len(), b.proj.tris.len(), t.elapsed().as_secs_f64());
    let outline = board::outline_segments(&m);
    let full = args.iter().any(|a| a == "--full");
    for l in 0..b.n_signal {
        let terms: Vec<_> = b.proj.obstacles.iter().filter(|ob| ob.net.is_some() && (ob.drilled || ob.layers.has(l))).map(|ob| ob.center).collect();
        let inp = SheetInput { proj: &b.proj, outline: &outline, vias: &[], junctions: &[], terminals: &terms };
        for prune in if full { vec![true, false] } else { vec![true] } {
            let so = SheetOpts { norm: Norm::for_style(&style), radius_nm: o.cut_radius_mm * 1e6, via_steps: 16, prune_walk: prune };
            let t = std::time::Instant::now();
            let sh = LayerSheet::build(&inp, l, &so).expect("sheet");
            let s = sh.stats;
            println!(
                "layer {l} ({}) prune {prune}: {:.3} s (cdt {:.3}, walk {:.3}, shadow {:.3}); features {} vertices {} triangles {}; cuts raw {} (breakpoint {}) shadowed {} kept {} capped walks {}",
                m.layers.iter().find(|x| x.id == b.signal_layers[l]).map_or("?", |x| x.name.as_str()),
                t.elapsed().as_secs_f64(), s.cdt_s, s.walk_s, s.shadow_s, s.features, s.vertices, s.triangles, s.raw, s.breakpoint, s.shadowed, s.kept, s.capped_walks
            );
        }
    }
}
