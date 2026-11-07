//! The plugin's fail-fast probe on one exported board model, without Python:
//!
//!     cargo run --release --example probe -- <model.json> [--options options.json]
//!
//! Prints the probe JSON (`topo::core::probe`: the projection a route builds first, no routing).
use topor_engine::model::{BoardModel, Options};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("model.json");
    let s = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let m = BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{path}: {e}"));
    let o = match args.iter().position(|a| a == "--options").and_then(|i| args.get(i + 1)) {
        Some(f) => Options::from_json(&std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{f}: {e}"))).expect("options json"),
        None => Options::default(),
    };
    let p = topor_engine::topo::core::probe(&m, &o);
    println!("{}", serde_json::to_string_pretty(&p).expect("json"));
}
