//! pause-button probe (debugging tool, docs/dev/design/pause-button.md): how soon a Pause frees the
//! CPU in every phase of a real board's run.
//!
//!   cargo run --release --example pause_probe -- <model.json> <options.json> [every_s] [hold_s] [loop_s]
//!
//! Routes the model (the plugin's engine input, `TOPOR_DUMP_ENGINE_INPUT`) as the progress dialog
//! does (no stop at stable), on RAYON_NUM_THREADS threads. Every `every_s` (default 4) it pauses
//! for `hold_s` (default 2.5) and samples the process CPU (/proc/self/stat) every 50 ms. One line per
//! pause: the stage, the latency (seconds from the pause until the CPU over 200 ms falls under 0.1
//! cores and stays there) and the cores used from 1 s after the pause to its end. After `loop_s`
//! (default 40) seconds in the TopoR loop it presses Accept (`request_stop`) so the drawing, via
//! reduction and pour stitching run and are probed too. A pause still busy 1 s in sends this process
//! SIGUSR1 (under gdb: `thread apply all bt` shows where the threads run without a stop test).

use std::time::{Duration, Instant};
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::route_with;

fn cpu_s() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").expect("/proc/self/stat");
    let rest = &s[s.rfind(')').expect("stat") + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    (f[11].parse::<f64>().expect("utime") + f[12].parse::<f64>().expect("stime")) / 100.0
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = BoardModel::from_json(&std::fs::read_to_string(&a[1]).expect("model")).expect("model parses");
    let mut ov: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[2]).expect("options")).expect("options json");
    let every: f64 = a.get(3).map_or(4.0, |x| x.parse().expect("every_s"));
    let hold: f64 = a.get(4).map_or(2.5, |x| x.parse().expect("hold_s"));
    let loop_s: f64 = a.get(5).map_or(40.0, |x| x.parse().expect("loop_s"));
    ov["stop_when_stable"] = serde_json::json!(false);
    ov["finish_when_stable"] = serde_json::json!(false);
    let o = Options::from_json(&ov.to_string()).expect("options parse");
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    let t0 = Instant::now();
    let h = std::thread::spawn(move || route_with(&m, &o, &c2));
    let (mut last, mut in_loop, mut accepted) = (Instant::now(), None::<Instant>, false);
    println!("t_s\tstage\tlatency_s\tcores_after_1s\tclock_still");
    while !h.is_finished() {
        let p = ctl.snapshot();
        if p.stage.contains("TopoR loop") && in_loop.is_none() {
            in_loop = Some(Instant::now());
        }
        if !accepted && in_loop.is_some_and(|t| t.elapsed().as_secs_f64() >= loop_s) {
            accepted = true;
            println!("{:.1}\tAccept pressed", t0.elapsed().as_secs_f64());
            ctl.request_stop(Some(false));
        }
        if last.elapsed().as_secs_f64() >= every {
            let stage = format!("{} / {}", p.phase, p.stage);
            ctl.pause();
            let (tp, clock0) = (Instant::now(), ctl.limit.clock_ns());
            let mut samples: Vec<(f64, f64)> = vec![(0.0, cpu_s())];
            let mut signalled = false;
            while tp.elapsed().as_secs_f64() < hold && !h.is_finished() {
                std::thread::sleep(Duration::from_millis(50));
                let t = tp.elapsed().as_secs_f64();
                samples.push((t, cpu_s()));
                if !signalled && t >= 1.0 {
                    let k = samples.iter().rposition(|s| s.0 <= t - 0.2).unwrap_or(0);
                    let cores = (samples.last().unwrap().1 - samples[k].1) / (t - samples[k].0);
                    if cores > 0.1 {
                        signalled = true;
                        let _ = std::process::Command::new("kill").args(["-USR1", &std::process::id().to_string()]).status();
                    }
                }
            }
            // latency: the first sample from which every 200 ms window stays under 0.1 cores
            let rate = |i: usize| {
                let j = samples.iter().position(|s| s.0 >= samples[i].0 + 0.2)?;
                Some((samples[j].1 - samples[i].1) / (samples[j].0 - samples[i].0))
            };
            let n = samples.len();
            let mut lat = None;
            for (i, smp) in samples.iter().enumerate() {
                if (i..n).all(|k| rate(k).is_none_or(|r| r < 0.1)) {
                    lat = Some(smp.0);
                    break;
                }
            }
            let k1 = samples.iter().position(|s| s.0 >= 1.0).unwrap_or(n - 1);
            let after = if n - 1 > k1 { (samples[n - 1].1 - samples[k1].1) / (samples[n - 1].0 - samples[k1].0) } else { f64::NAN };
            let still = ctl.limit.clock_ns() == clock0;
            println!("{:.1}\t{stage}\t{}\t{after:.3}\t{still}", t0.elapsed().as_secs_f64(), lat.map_or("never".into(), |l| format!("{l:.2}")));
            ctl.resume();
            last = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let r = h.join().expect("run");
    println!("done in {:.1} s: {} / {} routed, stop {}", t0.elapsed().as_secs_f64(), r.routed, r.connections_total, r.stop_reason);
}
