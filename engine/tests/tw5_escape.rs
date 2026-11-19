//! TW5 ESCAPE (docs/dev/design/tw5-escape.md) on the fit fixtures (engine/tests/fixtures/fit):
//! (e) the pre-loop P4 rounds run only with `[optimize] preloop_p4 = true`; the layering escape on
//! a 4-layer board with a full top layer and a free inner layer (inner_layer_room).

use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn model(name: &str) -> BoardModel {
    let path = format!("{}/tests/fixtures/fit/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("json");
    BoardModel::from_json(&doc["model"].to_string()).expect("model")
}

fn opts(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "topo_rounds": 4, "runs": 1, "seed": 1}"#).unwrap();
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{extra}}}")).unwrap();
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).unwrap()
}

fn route(m: &BoardModel, o: &Options) -> RouteResult {
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    route_with(m, o, &ctl)
}

fn layers_of(r: &RouteResult) -> String {
    let mut v: Vec<String> = r.connections.iter().map(|c| format!("{} {} vias {}", c.name, c.layers.join("/"), c.vias)).collect();
    v.sort();
    v.join("; ")
}

/// TW5 (e), D9: with the TopoR loop the negotiated rip-up rounds before it are folded into the
/// loop with `preloop_p4 = false` (TW5); the default `true` runs them as before TW5. inner_layer_room is
/// over-full on F.Cu when every wire takes the top layer (5 wires, room for 2), so the rounds have
/// work to do there.
#[test]
fn preloop_p4_rounds_run_only_when_asked() {
    let m = model("inner_layer_room");
    let p4 = |r: &RouteResult| r.warnings.iter().find(|w| w.starts_with("P4 negotiated rip-up")).cloned();
    let on = route(&m, &opts(""));
    let off = route(&m, &opts(r#""preloop_p4": false"#));
    eprintln!("default (preloop_p4 = true): {:?}\n  {}\npreloop_p4 = false: {:?}\n  {}", p4(&on), layers_of(&on), p4(&off), layers_of(&off));
    assert!(p4(&off).is_none(), "preloop_p4 = false: no P4 rounds before the loop: {:?}", p4(&off));
    assert!(p4(&on).is_some(), "default (preloop_p4 = true): the P4 rounds run on an over-full board: {:?}", on.warnings);
}
