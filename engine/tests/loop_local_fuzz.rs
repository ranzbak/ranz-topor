//! loop-local (fix round 2, 2026-11-18): dense random boards through the TopoR loop with the
//! local judge and `TOPO_LOCALCHECK` (the loop-local-verify fuzz, `tools/llv/llv_fuzz.rs` on
//! branch loop-local-verify, cut to the seeds that matter). On a dense board a move's placing
//! undoes double crossings (`State::uncross_from_touched`) and lays the wires that took part anew
//! (`State::relay`): wires besides the move's own die, and the survivors change their pass order.
//! The judge must see both:
//! - seed 1000: a relay inside a move kills a held wire; a judge that keeps partner links to it
//!   panics later in `moves::flips` (the bornhack2026 symptom of 5e542178);
//! - seed 1446: the swaps change held-held crossings with path and passes unchanged; a judge that
//!   does not lay those wires out again leaves one-layer crossings unpriced (89 in 47 moves on
//!   dfd8bc72).
//!
//! Each fixture must run without a panic, with 0 partner faults, 0 unpriced held-held crossings
//! and 0 stale held events (a held wire's event at a crossing with a wire the move killed). `dense_board_scan` (ignored; `LOOPLOCAL_FUZZ=first,count`) runs a seed range (seeds
//! 1000-1449 are the verifier's range, ~5 s each in release). Run in release with debug assertions:
//! `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true cargo test --release --test loop_local_fuzz`.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;

const MM: f64 = 1e6;

/// A board `w` x `h` mm with `n_layers` copper layers (2, or 4 with In1.Cu and In2.Cu), SMD rect
/// pads `(ref, net, x, y, w, h, layer)` (layer 0 = F.Cu, 2 = B.Cu, 99 = both) and nets
/// 1..=`nets` (as tests/topo_d6b.rs).
fn board_layers(w: f64, h: f64, nets: usize, pads: &[(&str, i32, f64, f64, f64, f64, u32)], n_layers: usize) -> BoardModel {
    let layers = if n_layers == 4 {
        r#"{"id":0,"name":"F.Cu","kind":"signal"},{"id":4,"name":"In1.Cu","kind":"signal"},{"id":6,"name":"In2.Cu","kind":"signal"},{"id":2,"name":"B.Cu","kind":"signal"}"#
    } else {
        r#"{"id":0,"name":"F.Cu","kind":"signal"},{"id":2,"name":"B.Cu","kind":"signal"}"#
    };
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .map(|&(r, net, x, y, w, h, l)| {
            let ls: Vec<u32> = if l == 99 { vec![0, 2] } else { vec![l] };
            let shape = format!(r#"{{"kind":"rect","w":{},"h":{}}}"#, p(w), p(h));
            let shapes: Vec<String> = ls.iter().map(|l| format!(r#""{l}":{shape}"#)).collect();
            let ls: Vec<String> = ls.iter().map(|l| l.to_string()).collect();
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[{}],"shapes":{{{}}}}}"#,
                p(x),
                p(y),
                ls.join(","),
                shapes.join(",")
            )
        })
        .collect();
    let nets: Vec<String> = (1..=nets).map(|i| format!(r#"{{"code":{i},"name":"N{i}"}}"#)).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{layers}],
            "nets":[{}],
            "pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        nets.join(","),
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// A dense random board (the loop-local-verify fuzz with 24-36 nets, 2026-11-18): 24-36 nets of
/// 2 or 3 pads (1 x 1 mm, F.Cu or B.Cu), at least 2.4 mm apart, on 30 x 20 mm.
fn random_board(seed: u64, n_layers: usize) -> BoardModel {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let (w, h) = (30.0, 20.0);
    let nets = rng.gen_range(24..=36);
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let mut pads: Vec<(String, i32, f64, f64, f64, f64, u32)> = Vec::new();
    for net in 1..=nets {
        let k = if rng.gen_bool(0.3) { 3 } else { 2 };
        for j in 0..k {
            let mut tries = 0;
            loop {
                tries += 1;
                let (x, y) = (rng.gen_range(1.5..w - 1.5), rng.gen_range(1.5..h - 1.5));
                if pts.iter().all(|&(a, b)| (a - x).hypot(b - y) > 2.4) || tries > 500 {
                    pts.push((x, y));
                    let l = if rng.gen_bool(0.5) { 0 } else { 2 };
                    pads.push((format!("P{net}_{j}"), net, (x * 10.0).round() / 10.0, (y * 10.0).round() / 10.0, 1.0, 1.0, l));
                    break;
                }
            }
        }
    }
    let refs: Vec<(&str, i32, f64, f64, f64, f64, u32)> = pads.iter().map(|p| (p.0.as_str(), p.1, p.2, p.3, p.4, p.5, p.6)).collect();
    board_layers(w, h, nets as usize, &refs, n_layers)
}

fn opts(seed: u64) -> Options {
    let s = format!(r#"{{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 4, "runs": 1, "seed": {seed}, "loop_judge": "local"}}"#);
    Options::from_json(&s).unwrap()
}

/// The number before `key` in the judge line.
fn num_before(l: &str, key: &str) -> Option<usize> {
    let i = l.find(key)?;
    l[..i].trim_end().rsplit(' ').next()?.trim_start_matches('(').parse().ok()
}

/// The dense fixtures `seeds` through the loop (`TOPO_LOCALCHECK` on): per seed a panic, partner
/// faults, unpriced held-held crossings or stale held events are failures; asserts there are none.
fn run_seeds(seeds: &[u64]) {
    // SAFETY: a debug switch the engine reads; every test of this binary wants it on.
    unsafe {
        std::env::set_var("TOPO_LOCALCHECK", "1");
    }
    let mut bad = Vec::new();
    for &s in seeds {
        let nl = if s % 2 == 0 { 2 } else { 4 };
        let m = random_board(s, nl);
        let o = opts(s);
        let t0 = std::time::Instant::now();
        let r = std::panic::catch_unwind(|| {
            let ctl = RunControl::new();
            ctl.limit.init(o.wall_clock_s);
            route_with(&m, &o, &ctl)
        });
        let dt = t0.elapsed().as_secs_f64();
        match r {
            Err(e) => {
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                println!("FUZZ seed {s} layers {nl}: PANIC {msg} ({dt:.1} s)");
                bad.push((s, "panic"));
            }
            Ok(r) => {
                let l = r.warnings.iter().find(|w| w.starts_with("TopoR loop judge")).cloned().unwrap_or_default();
                let ch = num_before(&l, " wires a move changed").unwrap_or(0);
                let pf = num_before(&l, " partner faults").unwrap_or(usize::MAX);
                let up = num_before(&l, " unpriced held-held crossings").unwrap_or(usize::MAX);
                let se = num_before(&l, " stale held events").unwrap_or(usize::MAX);
                println!("FUZZ seed {s} layers {nl}: changed {ch} partner faults {pf} unpriced {up} stale {se} ({dt:.1} s)");
                if se != 0 {
                    bad.push((s, "stale held events"));
                }
                if pf != 0 {
                    bad.push((s, "partner faults"));
                }
                if up != 0 {
                    bad.push((s, "unpriced held-held crossings"));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

/// Seed 1000: a relay inside a move kills a held wire (the bornhack2026 panic without the fix;
/// stale held events when the judge does not lay the killed wire's partners out again).
#[test]
fn dense_board_relay_inside_a_move_keeps_every_link() {
    run_seeds(&[1000]);
}

/// Seed 1446: the placing's double-crossing swaps change held-held crossings; each is priced.
#[test]
fn dense_board_uncross_swaps_leave_no_crossing_unpriced() {
    run_seeds(&[1446]);
}

/// A scan: `LOOPLOCAL_FUZZ=first,count` (ignored by default; `-- --ignored`).
#[test]
#[ignore]
fn dense_board_scan() {
    let seeds: Vec<u64> = match std::env::var("LOOPLOCAL_FUZZ").ok().and_then(|v| v.split_once(',').map(|(a, b)| (a.parse::<u64>(), b.parse::<u64>()))) {
        Some((Ok(a), Ok(n))) => (a..a + n).collect(),
        _ => (1000..1010).collect(),
    };
    run_seeds(&seeds);
}
