//! limit-wait r3 (throw-away, from the r2 verifier's vfy_lw3.rs): phase sweep of TIME_LIMIT_WAIT. A thread reaches the
//! limit at a chosen time, a poller records every stage change (snapshot().open_steps()), and the
//! engine's LimitReport + "time limit wait:" line are written as one JSON line per run to $VFY_OUT;
//! tools/vfy_lw3_check.py judges them. #[ignore]: run explicitly with --ignored.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

fn fixture() -> (String, BoardModel) {
    let name = std::env::var("VFY_FIX").unwrap_or_else(|_| "eprom-to-kernal-ripped".into());
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    (name, BoardModel::from_json(&std::fs::read_to_string(&path).expect("fixture")).expect("fixture parses"))
}

fn env_usize(k: &str, d: usize) -> usize {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn opts(rounds: usize, runs: usize, extra: &str) -> Options {
    let cap = if extra.contains("timeout_overrun_s") { "" } else { r#""timeout_overrun_s": "none", "# };
    let lp = if extra.contains("opt_loop") { "" } else { r#""opt_loop": "topor", "initial": "once", "# };
    Options::from_json(&format!(
        r#"{{"wall_clock_s": 86400.0, {cap}{lp}"search_length": "taut", "runs": {runs}, "topo_rounds": {rounds}, "stop_when_stable": false, "finish_when_stable": false{extra}}}"#
    ))
    .expect("options parse")
}

struct Out {
    r: RouteResult,
    /// (t since the limit's clock start, open steps) at every change seen by the poller
    stages: Vec<(f64, String)>,
    t_lim: Option<f64>,
    t_end: f64,
}

fn run(m: &BoardModel, o: &Options, delay: Option<Duration>) -> Out {
    let ctl = RunControl::new();
    let done = Arc::new(AtomicBool::new(false));
    let stages = Arc::new(Mutex::new(Vec::<(f64, String)>::new()));
    let poll = {
        let (ctl, done, stages) = (ctl.clone(), done.clone(), stages.clone());
        std::thread::spawn(move || {
            let mut last = String::from("\u{0}");
            while !done.load(Ordering::SeqCst) {
                let s = ctl.snapshot().open_steps();
                if s != last {
                    stages.lock().unwrap().push((ctl.limit.elapsed_s(), s.clone()));
                    last = s;
                }
                std::thread::sleep(Duration::from_micros(150));
            }
        })
    };
    let lim = delay.map(|d| {
        let (l, done) = (ctl.limit.clone(), done.clone());
        std::thread::spawn(move || {
            std::thread::sleep(d);
            if !done.load(Ordering::SeqCst) {
                l.reach();
            }
        })
    });
    let r = route_with(m, o, &ctl);
    let t_end = ctl.limit.elapsed_s();
    done.store(true, Ordering::SeqCst);
    poll.join().unwrap();
    if let Some(h) = lim {
        h.join().unwrap();
    }
    let t_lim = ctl.limit.seconds().filter(|&s| s < 86000.0);
    let stages = stages.lock().unwrap().clone();
    Out { r, stages, t_lim, t_end }
}

fn emit(name: &str, fix: &str, variant: &str, delay: Option<f64>, o: &Out) {
    let r = &o.r;
    let wait = r.warnings.iter().filter(|w| w.starts_with("time limit wait:")).cloned().collect::<Vec<_>>();
    let ll = r.warnings.iter().find(|w| w.starts_with("TopoR loop (")).cloned().unwrap_or_default();
    let l = r.limit.as_ref();
    let line = serde_json::json!({
        "name": name, "fix": fix, "variant": variant, "delay": delay, "t_lim": o.t_lim, "t_end": o.t_end,
        "stages": o.stages, "wait_lines": wait, "loop_line": ll,
        "reached": l.map(|l| l.reached), "step": l.map(|l| l.step.clone()), "wait_s": l.map(|l| l.wait_s),
        "grace_s": l.and_then(|l| l.grace_s), "draw_grace_s": l.and_then(|l| l.draw_grace_s), "draw_grace_hit": l.map(|l| l.draw_grace_hit), "grace_hit": l.map(|l| l.grace_hit), "cycle_dropped": l.map(|l| l.cycle_dropped),
        "cut": l.map(|l| l.cut.clone()), "cut_steps": l.map(|l| l.cut_steps.clone()), "cap_hit": l.map(|l| l.cap_hit),
        "mode": l.map(|l| l.mode.clone()), "overrun_s": l.map(|l| l.overrun_s), "routed": r.routed, "total": r.connections_total,
        "warnings_time": r.warnings.iter().filter(|w| w.contains("time limit") || w.contains("TIME_") || w.contains("drawing was cut")).cloned().collect::<Vec<_>>(),
    });
    let path = std::env::var("VFY_OUT").unwrap_or_else(|_| "lw3_sweep.jsonl".into());
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("out");
    writeln!(f, "{line}").expect("write");
}

/// Limit reached at N times spread over a no-limit run, plus the middle of every stage seen, for
/// the variants default grace / 0 / unbounded / hard / soft with a 20 ms overrun cap / 2 runs.
#[test]
#[ignore]
fn vfy_phase_sweep() {
    let (fix, m) = fixture();
    let rounds = env_usize("VFY_ROUNDS", 2);
    let n = env_usize("VFY_N", 30);
    let variants: Vec<(&str, usize, &str)> = vec![
        ("dflt", 1, ""),
        ("g0", 1, r#", "limit_grace_s": 0"#),
        ("gm1", 1, r#", "limit_grace_s": -1"#),
        ("hard", 1, r#", "timeout_mode": "hard""#),
        ("cap20", 1, r#", "timeout_overrun_s": 0.02"#),
        ("runs2", 2, ""),
        ("runs2g0", 2, r#", "limit_grace_s": 0"#),
        ("gm5", 1, r#", "limit_grace_s": -5"#),
        ("dg0", 1, r#", "drawing_grace_s": 0"#),
        ("cyc", 1, r#", "opt_loop": "cycles""#),
    ];
    let only = std::env::var("VFY_VARIANTS").ok();
    for (tag, runs, extra) in variants {
        if only.as_ref().is_some_and(|o| !o.split(',').any(|x| x == tag)) {
            continue;
        }
        let o = opts(rounds, runs, extra);
        let base = run(&m, &o, None);
        emit(&format!("{tag}_none"), &fix, tag, None, &base);
        let total = base.t_end;
        let mut delays: Vec<f64> = (1..=n).map(|i| total * (i as f64 - 0.5) / n as f64).collect();
        // the middle of every stage of the reference run (and 0.3 ms after each change)
        for w in base.stages.windows(2) {
            delays.push((w[0].0 + w[1].0) / 2.0);
            delays.push(w[1].0 + 0.0003);
        }
        if let Some(last) = base.stages.last() {
            delays.push((last.0 + total) / 2.0);
        }
        for (i, d) in delays.iter().enumerate() {
            let out = run(&m, &o, Some(Duration::from_secs_f64(d.max(0.0))));
            emit(&format!("{tag}_{i}"), &fix, tag, Some(*d), &out);
        }
    }
}
