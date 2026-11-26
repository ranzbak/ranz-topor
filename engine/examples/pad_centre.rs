//! draw-via-neck r6 board-level check of the pad-centre invariant (user 2026-11-26: every trace
//! connects to the MIDDLE of its pad) on a routed board's copper, without routing:
//!
//!     cargo run --release --example pad_centre -- <model.json> [--json OUT]
//!
//! `model.json` is the engine model of the ROUTED board (`harness.model.export_model` on the
//! routed copy: its tracks and vias are the model's kept copper). Every kept track and via is
//! judged as drawn copper by `topo::tidy::pad_end_breaches` (the engine test's check). Prints one
//! `BREACH` line per entry (kind, pad, net name, layer, offset mm, flags E = edge-clear,
//! B = blocked centre, H = a centre-distance hint: KiCad's Cleanup keeps such ends (r7), so a hint, not a failure) and a `SUMMARY` line; `--json` writes
//! the same list. O(pad_end_breaches).

use topor_engine::geom::quant::{IVec2, TrackPrim};
use topor_engine::model::{BoardModel, Item, Pt, TrackItem, ViaItem};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("model.json");
    let s = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut m = BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{path}: {e}"));
    // math frame (model) -> KiCad frame (items)
    let k = |q: IVec2| IVec2 { x: q.x, y: -q.y };
    let mut items: Vec<Item> = Vec::new();
    for t in std::mem::take(&mut m.kept) {
        let prim = match t.prim {
            TrackPrim::Seg { a, b } => TrackPrim::Seg { a: k(a), b: k(b) },
            TrackPrim::Arc { start, mid, end } => TrackPrim::Arc { start: k(start), mid: k(mid), end: k(end) },
        };
        items.push(Item::Track(TrackItem { net: t.net, layer: t.layer, width: t.width, prim, group: None, wire: None }));
    }
    for v in std::mem::take(&mut m.vias) {
        items.push(Item::Via(ViaItem { kind: "via".into(), net: v.net, at: Pt { x: v.at.x, y: -v.at.y }, d: v.d, drill: v.drill, layers: v.layers, group: None, wire: None }));
    }
    let name = |n: i32| m.nets.iter().find(|x| x.code == n).map(|x| x.name.clone()).unwrap_or_default();
    let bs = topor_engine::topo::tidy::pad_end_breaches(&m, &items);
    let mut out = Vec::new();
    for b in &bs {
        let flags = format!("{}{}{}", if b.edge { "E" } else { "" }, if b.centre_blocked { "B" } else { "" }, if b.hint() { "H" } else { "" });
        println!("BREACH {:?} {} {} L{} off {:.4} mm at ({:.4},{:.4}) {}", b.kind, b.pad, name(b.net), b.layer, b.off / 1e6, b.at.0 as f64 / 1e6, b.at.1 as f64 / 1e6, flags);
        out.push(serde_json::json!({"kind": format!("{:?}", b.kind), "pad": b.pad, "net": name(b.net), "layer": b.layer, "off_mm": b.off / 1e6,
            "at_mm": [b.at.0 as f64 / 1e6, b.at.1 as f64 / 1e6], "edge": b.edge, "centre_blocked": b.centre_blocked, "counts": b.hint(), "hint": b.hint()}));
    }
    let n = bs.iter().filter(|b| b.hint()).count();
    println!("SUMMARY breaches {n} (centre-distance hints, not failures: KiCad keeps them) listed {} (tee {}, edge {}, blocked {})", bs.len(), bs.iter().filter(|b| b.kind == topor_engine::topo::tidy::PadEndKind::Tee).count(), bs.iter().filter(|b| b.edge).count(), bs.iter().filter(|b| b.centre_blocked).count());
    if let Some(i) = args.iter().position(|a| a == "--json") {
        let f = args.get(i + 1).expect("--json OUT");
        std::fs::write(f, serde_json::to_string_pretty(&serde_json::json!({"breaches": n, "items": out})).unwrap()).unwrap_or_else(|e| panic!("{f}: {e}"));
    }
}
