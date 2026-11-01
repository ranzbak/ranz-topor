//! M4b Task P1: stop rule, timeout disable, cancel and progress (`RunControl`).
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{no_gain, route, route_with, RouteResult};

const MM: f64 = 1_000_000.0;

/// The timing tests (cancel after 2 s, progress polling) must not compete for the cores with
/// each other: they run one at a time.
static HEAVY: Mutex<()> = Mutex::new(());

fn heavy() -> std::sync::MutexGuard<'static, ()> {
    HEAVY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Plus-shaped board whose two nets seal each other (see `route_engine.rs`): A and B rip each
/// other out every iteration until the damping cap, then nothing improves. Unroutable, one
/// routed, one blocked, churn at the start.
fn sealed_cross_model() -> BoardModel {
    let pts: [(f64, f64); 12] = [
        (20.5, 2.0),
        (23.5, 2.0),
        (23.5, 20.5),
        (42.0, 20.5),
        (42.0, 23.5),
        (23.5, 23.5),
        (23.5, 42.0),
        (20.5, 42.0),
        (20.5, 23.5),
        (2.0, 23.5),
        (2.0, 20.5),
        (20.5, 20.5),
    ];
    let segs: Vec<String> = (0..12)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % 12]);
            format!(
                r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
                (a.0 * MM) as i64,
                (-a.1 * MM) as i64,
                (b.0 * MM) as i64,
                (-b.1 * MM) as i64
            )
        })
        .collect();
    let rect = |r: &str, n: &str, net: i32, x: f64, y: f64, w: f64, h: f64| {
        format!(
            r#"{{ "uuid": "{r}{n}", "ref": "{r}", "number": "{n}", "net": {net},
              "center": [{}, {}], "layers": [0],
              "shapes": {{ "0": {{ "kind": "rect", "w": {}, "h": {} }} }} }}"#,
            (x * MM) as i64,
            (-y * MM) as i64,
            (w * MM) as i64,
            (h * MM) as i64
        )
    };
    let pads = [
        rect("A", "1", 1, 4.0, 22.0, 1.0, 4.0),
        rect("A", "2", 1, 40.0, 22.0, 1.0, 4.0),
        rect("B", "1", 2, 22.0, 4.0, 4.0, 1.0),
        rect("B", "2", 2, 22.0, 40.0, 4.0, 1.0),
    ];
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }}, {{ "code": 2, "name": "N2" }} ],
          "pads": [ {} ], "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

/// Two-layer synthetic board: `n` two-pad nets on a 3 mm grid, partners within 9 mm (seeded
/// LCG), the second pad of every pair on B.Cu only, so every connection needs a via. Same
/// construction as `tests/scaling/synth.py`; self-contained and deterministic.
fn synth_two_layer(n: usize) -> BoardModel {
    let pitch = 3_000_000i64;
    let cols = ((2 * n) as f64).sqrt().ceil() as usize;
    let mut rows = cols;
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    let (cells, pairs) = loop {
        let cells: Vec<(i64, i64)> = (0..cols * rows)
            .map(|i| ((i % cols) as i64, (i / cols) as i64))
            .collect();
        let mut order: Vec<usize> = (0..cells.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, next() % (i + 1));
        }
        let mut free = vec![true; cells.len()];
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for &i in &order {
            if pairs.len() == n {
                break;
            }
            if !free[i] {
                continue;
            }
            free[i] = false;
            let near: Vec<usize> = (0..cells.len())
                .filter(|&j| {
                    free[j] && {
                        let (dx, dy) = ((cells[i].0 - cells[j].0) as f64, (cells[i].1 - cells[j].1) as f64);
                        (dx * dx + dy * dy).sqrt() * pitch as f64 <= 9.0 * MM
                    }
                })
                .collect();
            if near.is_empty() {
                continue;
            }
            let j = near[next() % near.len()];
            free[j] = false;
            pairs.push((i, j));
        }
        if pairs.len() == n {
            break (cells, pairs);
        }
        rows += 1;
    };
    let mut pads = Vec::new();
    let (mut lo, mut hi) = ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
    for (net, &(i, j)) in pairs.iter().enumerate() {
        let net = net + 1;
        for (k, c) in [i, j].into_iter().enumerate() {
            let lay = if k == 1 { 31 } else { 0 };
            let (x, y) = (cells[c].0 * pitch, cells[c].1 * pitch);
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
            pads.push(format!(
                r#"{{ "uuid": "p{net}-{k}", "ref": "P{net}", "number": "{}", "net": {net},
                  "center": [{x}, {y}], "layers": [{lay}],
                  "shapes": {{ "{lay}": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
                k + 1
            ));
        }
    }
    let m = 2_500_000i64;
    let (x0, y0, x1, y1) = (lo.0 - m, lo.1 - m, hi.0 + m, hi.1 + m);
    let c = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let outline: Vec<String> = (0..4)
        .map(|k| {
            format!(
                r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
                c[k].0,
                c[k].1,
                c[(k + 1) % 4].0,
                c[(k + 1) % 4].1
            )
        })
        .collect();
    let nets: Vec<String> = (1..=n)
        .map(|k| format!(r#"{{ "code": {k}, "name": "N{k}" }}"#))
        .collect();
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }}, {{ "id": 31, "name": "B.Cu" }} ],
          "nets": [ {} ], "pads": [ {} ], "outline": [ {} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000,
              "via_d": 600000, "via_drill": 300000 }}, "min_clearance": 200000,
            "hole_clearance": 350000, "edge_clearance": 250000,
            "drill_list": [300000], "routing_vias": [300000] }} }}"#,
        nets.join(","),
        pads.join(","),
        outline.join(",")
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}"))
}

fn opts(wall: f64) -> Options {
    Options {
        wall_clock_s: wall,
        threads: 2,
        ..Options::default()
    }
}

fn clean(m: &BoardModel, r: &RouteResult) {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn stop_rule_no_gain_terminates_without_timeout() {
    let m = sealed_cross_model();
    let t = Instant::now();
    let r = route(&m, &opts(0.0));
    assert!(t.elapsed() < Duration::from_secs(60));
    assert_eq!((r.routed, r.failed), (1, 1), "{:?}", r.warnings);
    assert_eq!(r.stop_reason, "no gain", "{:?}", r.warnings);
    assert!(r.iterations <= 30, "{}", r.iterations);
    assert!(!r.timed_out && !r.converged);
    clean(&m, &r);
}

#[test]
fn stop_rule_counts_newly_routed_as_progress() {
    let o = Options::default(); // patience 5, min_gain_pct 1.0
    let flat = |n: usize| vec![(3usize, 1000.0f64); n];
    // nothing routed, no gain over the last 5 iterations: stop (needs patience + 1 entries)
    assert!(!no_gain(&flat(5), &o));
    assert!(no_gain(&flat(6), &o));
    // a connection newly routed within the window is progress even if the cost went up
    let mut h = flat(6);
    h[5] = (4, 1500.0);
    assert!(!no_gain(&h, &o));
    // no new connection but a 2 % cost gain is progress; 0.5 % is not
    let mut h = flat(6);
    h[5] = (3, 980.0);
    assert!(!no_gain(&h, &o));
    h[5] = (3, 995.0);
    assert!(no_gain(&h, &o));
    // a routed count that grew before the window does not count
    let mut h = flat(7);
    h[0] = (2, 1000.0);
    assert!(no_gain(&h, &o));
}

#[test]
fn timeout_zero_is_disabled() {
    let _g = heavy();
    let n = if cfg!(debug_assertions) { 10 } else { 40 };
    let m = synth_two_layer(n);
    let r = route(&m, &opts(0.0));
    assert!(!r.timed_out, "{:?}", r.warnings);
    assert_eq!(r.routed, n, "{:?}", r.warnings);
    assert!(r.stop_reason == "converged" || r.stop_reason == "no gain", "{}", r.stop_reason);
    // a positive, already expired budget still times out
    let r = route(&m, &opts(1e-6));
    assert!(r.timed_out);
    assert_eq!(r.stop_reason, "timeout");
}

/// Cancel during a run returns the best result so far. Uncancelled this board routes for about
/// 12 s in a release build (2 threads). The cancel is placed by the run's progress, not by the
/// wall clock: once iteration 1 has ended (a best result exists). A fixed 2 s sleep (before TE)
/// landed in the setup or inside iteration 1 on a loaded machine, before anything was
/// committed, and then nothing could be returned. "Promptly" is measured from the cancel.
#[test]
#[cfg_attr(debug_assertions, ignore = "timing test (cancel after iteration 1): release build only")]
fn route_async_cancel_returns_anytime_result() {
    let _g = heavy();
    let m = synth_two_layer(400);
    let ctl = RunControl::default();
    let c2 = ctl.clone();
    let canceller = std::thread::spawn(move || {
        // capped poll (10 min); the cancel instant, `None` when the run ended first
        for _ in 0..600_000 {
            let p = c2.snapshot();
            if p.phase == "done" {
                return None;
            }
            if p.iteration >= 1 {
                c2.cancel.store(true, Ordering::Relaxed);
                return Some(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    });
    let r = route_with(&m, &opts(0.0), &ctl);
    let done = Instant::now();
    let at = canceller.join().unwrap().expect("the run ended before iteration 1 was seen");
    assert_eq!(r.stop_reason, "cancel", "{:?}", r.warnings);
    assert!(r.timed_out);
    assert!(r.routed >= 1, "routed {}", r.routed);
    // stop_reason "cancel" means the run was still going at the cancel; it returns promptly after
    let after = done.saturating_duration_since(at);
    assert!(after < Duration::from_secs(8), "{after:?} after the cancel");
    clean(&m, &r);
    assert_eq!(ctl.snapshot().phase, "done");
}

#[test]
fn progress_is_monotone() {
    let _g = heavy();
    let m = synth_two_layer(if cfg!(debug_assertions) { 40 } else { 300 });
    let ctl = RunControl::default();
    let c2 = ctl.clone();
    let poller = std::thread::spawn(move || {
        let mut seen = Vec::new();
        loop {
            let p = c2.snapshot();
            let done = p.phase == "done";
            seen.push(p);
            if done {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    let r = route_with(&m, &opts(0.0), &ctl);
    let seen = poller.join().unwrap();
    assert!(seen.len() >= 2);
    for w in seen.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        assert!(b.iteration >= a.iteration, "{a:?} -> {b:?}");
        assert!(b.routed >= a.routed, "{a:?} -> {b:?}");
        assert!(b.total >= a.total, "{a:?} -> {b:?}");
        assert!(b.vias >= a.vias, "{a:?} -> {b:?}");
        assert!(b.elapsed_s >= a.elapsed_s, "{a:?} -> {b:?}");
    }
    let last = seen.last().unwrap();
    assert_eq!((last.routed, last.total), (r.routed, r.connections_total));
    assert_eq!(last.iteration, r.iterations);
}
