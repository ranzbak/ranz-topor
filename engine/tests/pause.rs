//! pause-button (user 2026-11-26, docs/dev/design/pause-button.md): the progress dialog's Pause.
//! While paused the engine uses no CPU (every work thread waits at its next stop test) and the
//! run's clock stands still (no paused time counts toward the limit, its grace or the drawing
//! bound). Resume goes on with the same run: the same seed and settings give the same copper with
//! and without pauses. Accept (a stop request) and Cancel (discard) end a pause by themselves.
//!
//! Own test binary: the CPU figures are of the whole process (/proc/self/stat), so the tests here
//! run one at a time (`SERIAL`).

use std::sync::Mutex;
use std::time::{Duration, Instant};
use topor_engine::control::{Progress, RunControl};
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route_with, RouteResult};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn eprom() -> BoardModel {
    BoardModel::from_json(include_str!("fixtures/eprom-to-kernal-ripped.json")).expect("fixture parses")
}

/// Headless, one TopoR loop cycle per run, `runs` runs, realise = "erbs" (the parallel drawing's
/// units); the wall limit is a day: nothing is cut by the clock (a cut run differs by timing).
fn opts(extra: &str) -> Options {
    let mut o: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
        r#"{"wall_clock_s": 86400.0, "opt_loop": "topor", "initial": "once", "search_length": "taut", "topo_rounds": 2, "runs": 2, "realise": "erbs"}"#,
    )
    .expect("base");
    let more: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&format!("{{{}}}", extra.trim_start_matches(','))).expect("extra options");
    o.extend(more);
    Options::from_json(&serde_json::Value::Object(o).to_string()).expect("options parse")
}

/// The copper and the connections.
fn key(r: &RouteResult) -> String {
    format!("{}\n{}", serde_json::to_string(&r.items).expect("items"), serde_json::to_string(&r.connections).expect("connections"))
}

/// CPU seconds of this process so far (all threads; /proc/self/stat utime + stime, 10 ms ticks).
fn cpu_s() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").expect("/proc/self/stat");
    let rest = &s[s.rfind(')').expect("stat") + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let tick = |i: usize| f[i].parse::<f64>().expect("tick");
    (tick(11) + tick(12)) / 100.0
}

/// Seconds after a pause the engine's threads are settled (CPU about 0): generous for a loaded
/// machine and for debug builds (user 2026-11-14: bounds scale with the build profile).
const SETTLE_S: f64 = if cfg!(debug_assertions) { 6.0 } else { 1.2 };
/// The CPU the paused process may still use over one second (cores): the test's own polling.
const IDLE_CORES: f64 = 0.08;

/// What a pause in `pause_when` found: the stage it fell in, the CPU (cores) over 1 s after
/// SETTLE_S, and the run's clock before and after the paused window (it must stand still).
#[derive(Debug, Clone)]
struct Seen {
    stage: String,
    cores: f64,
    clock_still: bool,
    optimize_still: bool,
}

/// Route `m` with `o` on `threads` threads; the controller pauses once in each state `pause_when`
/// names (in order), for `hold`, and measures the CPU there; `toggle_in`: in the drawing, pause and
/// resume `toggles` times (20 ms apart) on top. Returns the result and what each pause saw.
fn run_paused(m: &BoardModel, o: &Options, threads: usize, pause_when: &[&'static str], hold: Duration, toggle_in_drawing: usize) -> (RouteResult, Vec<Seen>) {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let (m2, o2, c2) = (m.clone(), o.clone(), ctl.clone());
    let h = std::thread::spawn(move || pool.install(|| route_with(&m2, &o2, &c2)));
    let mut seen = Vec::new();
    let mut next = 0usize;
    let mut toggled = 0usize;
    let t0 = Instant::now();
    while !h.is_finished() {
        assert!(t0.elapsed().as_secs() < 900, "the run never ended");
        let p: Progress = ctl.snapshot();
        let here = |what: &str| p.stage.contains(what) || p.phase == what;
        if next < pause_when.len() && here(pause_when[next]) {
            assert!(ctl.pause(), "a pause of a running run takes");
            let (clock0, opt0) = (ctl.limit.clock_ns(), ctl.snapshot().optimize_s);
            std::thread::sleep(Duration::from_secs_f64(SETTLE_S));
            let (c0, w0) = (cpu_s(), Instant::now());
            std::thread::sleep(hold.max(Duration::from_secs(1)));
            let cores = (cpu_s() - c0) / w0.elapsed().as_secs_f64();
            let (clock1, opt1) = (ctl.limit.clock_ns(), ctl.snapshot().optimize_s);
            let sp = ctl.snapshot();
            assert!(sp.paused && sp.paused_s > 0.0, "the progress says paused: {} {}", sp.paused, sp.paused_s);
            seen.push(Seen { stage: format!("{} / {}", p.phase, p.stage), cores, clock_still: clock1 == clock0, optimize_still: opt1 == opt0 });
            assert!(ctl.resume(), "resume of a paused run");
            next += 1;
        } else if toggled < toggle_in_drawing && p.stage.contains("drawing the traces") {
            ctl.pause();
            std::thread::sleep(Duration::from_millis(20));
            ctl.resume();
            toggled += 1;
            std::thread::sleep(Duration::from_millis(20));
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let r = h.join().expect("run thread");
    assert!(!ctl.limit.is_paused(), "a pause never outlives its job");
    (r, seen)
}

fn reference(m: &BoardModel, o: &Options, threads: usize) -> String {
    let (r, _) = run_paused(m, o, threads, &[], Duration::ZERO, 0);
    key(&r)
}

/// I1 + I2: pauses in the projection, the topological search (P2), P3/P4, the TopoR loop and the
/// drawing: the engine uses no CPU there, its clock stands still, and the copper is the same as
/// without the pauses.
#[test]
fn a_pause_in_every_phase_frees_the_cpu_and_keeps_the_copper() {
    let _s = serial();
    let m = eprom();
    let o = opts("");
    let want = reference(&m, &o, 4);
    let phases = ["projection of the board", "topological search", "first plan ready", "TopoR loop", "drawing the traces"];
    let (r, seen) = run_paused(&m, &o, 4, &phases, Duration::from_secs(1), 0);
    for s in &seen {
        eprintln!("paused in {}: {:.3} cores, clock still {}, optimize still {}", s.stage, s.cores, s.clock_still, s.optimize_still);
        assert!(s.cores < IDLE_CORES, "paused in {}: the engine still used {:.2} cores {SETTLE_S} s after the pause", s.stage, s.cores);
        assert!(s.clock_still, "paused in {}: the run's clock moved", s.stage);
        assert!(s.optimize_still, "paused in {}: the optimizing seconds moved", s.stage);
    }
    assert!(seen.len() >= 3, "too few phases were reached for a pause: {seen:?}");
    assert!(r.stop_reason != "cancel" && r.stop_reason != "timeout", "the run ended by itself: {}", r.stop_reason);
    assert!(key(&r) == want, "the copper with pauses differs from the copper without them");
}

/// Lead check (2): pause and resume many times while the traces are drawn on lanes (the parallel
/// drawing, ERBS units): no deadlock, the run ends, the same copper.
#[test]
fn many_pauses_during_the_parallel_drawing_keep_the_copper() {
    let _s = serial();
    let m = eprom();
    let o = opts(r#", "runs": 1"#);
    let want = reference(&m, &o, 4);
    let (r, _) = run_paused(&m, &o, 4, &[], Duration::ZERO, 40);
    assert!(key(&r) == want, "the copper with pauses in the drawing differs");
}

/// The run's clock stands still while paused: a 3 s limit with a 4 s pause right after the start
/// still has its time: the limit has not passed when the pause ends, and the optimizing seconds
/// left out the pause.
#[test]
fn paused_time_does_not_count_toward_the_limit() {
    let _s = serial();
    let ctl = RunControl::new();
    ctl.limit.init(3.0);
    let d0 = ctl.limit.deadline().expect("a limit");
    assert!(ctl.pause());
    assert!(!ctl.pause(), "a second pause changes nothing");
    std::thread::sleep(Duration::from_millis(3500));
    assert!(!ctl.limit.passed(), "the limit passed during the pause");
    assert!(ctl.limit.elapsed_s() < 0.5, "the clock moved while paused: {}", ctl.limit.elapsed_s());
    assert!(ctl.limit.paused_s() > 3.0);
    assert!(ctl.resume());
    assert!(!ctl.resume(), "a second resume changes nothing");
    assert!(!ctl.limit.passed());
    assert!(ctl.limit.remaining_s().expect("limit") > 2.4, "{:?}", ctl.limit.remaining_s());
    // the limit's instant is the same on the run's clock: the clock went on from where it stood
    assert_eq!(ctl.limit.deadline().expect("a limit"), d0);
    assert!(ctl.limit.now() < Instant::now() - Duration::from_secs(3));
    std::thread::sleep(Duration::from_millis(3100));
    assert!(ctl.limit.passed(), "after its 3 s on the run's clock the limit passes");
}

/// The grace (`limit_grace_s`) and the drawing bound (`drawing_grace_s`) count on the same clock.
#[test]
fn paused_time_does_not_count_toward_the_grace_or_the_drawing_bound() {
    let _s = serial();
    let ctl = RunControl::new();
    ctl.limit.init(0.2);
    ctl.limit.set_grace(1.0);
    ctl.limit.set_draw_grace(1.0);
    let draw_start = ctl.limit.now();
    std::thread::sleep(Duration::from_millis(300));
    assert!(ctl.limit.passed() && !ctl.limit.grace_over());
    ctl.pause();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!ctl.limit.grace_over(), "the grace ran out during the pause");
    assert!(!ctl.limit.draw_grace_over(draw_start), "the drawing bound ran out during the pause");
    ctl.resume();
    assert!(!ctl.limit.grace_over());
    std::thread::sleep(Duration::from_millis(1100));
    assert!(ctl.limit.grace_over() && ctl.limit.draw_grace_over(draw_start));
}

/// Pause in the TopoR loop, then Accept (a stop request) while paused: the run resumes by itself,
/// stops and draws the best so far, within the cancel bound.
#[test]
fn accept_while_paused_draws_the_best_so_far() {
    let _s = serial();
    let (r, s, ctl) = stop_while_paused(|c| c.request_stop(Some(false)));
    assert!(s < STOP_BOUND_S, "Accept while paused took {s:.2} s");
    assert_eq!(r.stop_reason, "cancel");
    assert!(!r.items.is_empty() && r.routed > 0, "the best so far is drawn: {:?}", r.warnings);
    assert!(!ctl.limit.is_paused());
}

/// Pause, then Cancel (discard) while paused: the threads end within seconds, nothing is drawn.
#[test]
fn cancel_while_paused_discards_within_seconds() {
    let _s = serial();
    let (r, s, ctl) = stop_while_paused(|c| c.request_discard());
    assert!(s < STOP_BOUND_S, "Cancel while paused took {s:.2} s");
    assert!(r.items.is_empty(), "Cancel takes nothing: {} items", r.items.len());
    assert!(!ctl.limit.is_paused());
}

const STOP_BOUND_S: f64 = if cfg!(debug_assertions) { 60.0 } else { 5.0 };

/// The dialog's run (ends only by a stop), paused 0.8 s into its TopoR loop for 1.5 s, then `stop`.
fn stop_while_paused(stop: impl Fn(&RunControl)) -> (RouteResult, f64, RunControl) {
    let m = eprom();
    let o = Options::from_json(r#"{"wall_clock_s": 600.0, "stop_when_stable": false, "finish_when_stable": false, "topo_rounds": 100000, "stable_window": 100000, "loop_stable_window": 100000}"#).expect("options");
    let ctl = RunControl::new();
    ctl.limit.init(o.wall_clock_s);
    let c2 = ctl.clone();
    let h = std::thread::spawn(move || route_with(&m, &o, &c2));
    let t0 = Instant::now();
    while !ctl.snapshot().stage.contains("TopoR loop") {
        assert!(t0.elapsed().as_secs() < 300 && !h.is_finished(), "never reached the loop");
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(800));
    assert!(ctl.pause());
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!h.is_finished(), "a paused run does not end");
    let t = Instant::now();
    stop(&ctl);
    let r = h.join().expect("run thread");
    (r, t.elapsed().as_secs_f64(), ctl)
}

/// Lead check (4): a pause while the run waits past the limit for its step (the soft limit's
/// "time limit reached, waiting for the current step to finish"): in the topological search (the
/// first plan, never cut), in the layer assignment after it (bounded by limit_grace_s) and in the
/// TopoR loop (bounded by limit_grace_s). The dialog's run; when the stage begins, the limit is set
/// to the run's time so far and the run is paused for longer than the grace: no CPU, the clock
/// stands still, and after Resume the grace has not run out (the pause did not use it up).
#[test]
fn a_pause_while_waiting_past_the_limit_uses_no_cpu_and_no_grace() {
    let _g = serial();
    let m = eprom();
    for stage in ["topological search", "first plan ready", "TopoR loop"] {
        let o = Options::from_json(r#"{"wall_clock_s": 600.0, "limit_grace_s": 2.0, "stop_when_stable": false, "finish_when_stable": false, "topo_rounds": 100000, "stable_window": 100000, "loop_stable_window": 100000}"#).expect("options");
        let ctl = RunControl::new();
        ctl.limit.init(o.wall_clock_s);
        ctl.limit.set_grace(o.limit_grace_s);
        let (m2, c2) = (m.clone(), ctl.clone());
        let h = std::thread::spawn(move || route_with(&m2, &o, &c2));
        let t0 = Instant::now();
        while !ctl.snapshot().stage.contains(stage) {
            assert!(t0.elapsed().as_secs() < 300 && !h.is_finished(), "never reached {stage}");
            std::thread::sleep(Duration::from_millis(1));
        }
        ctl.set_time_limit((ctl.limit.elapsed_s() - 0.01).max(0.0));
        assert!(ctl.pause());
        assert!(ctl.limit.passed(), "{stage}: the limit has passed");
        std::thread::sleep(Duration::from_secs_f64(SETTLE_S));
        let (c0, w0, clock0) = (cpu_s(), Instant::now(), ctl.limit.clock_ns());
        std::thread::sleep(Duration::from_secs(3)); // longer than the 2 s grace
        let cores = (cpu_s() - c0) / w0.elapsed().as_secs_f64();
        assert!(cores < IDLE_CORES, "{stage}: {cores:.3} cores while paused past the limit");
        assert_eq!(ctl.limit.clock_ns(), clock0, "{stage}: the clock moved while paused");
        assert!(ctl.resume());
        assert!(!ctl.limit.grace_over(), "{stage}: the pause used up the grace");
        let r = h.join().expect("run thread");
        assert!(r.routed > 0, "{stage}: the run drew its plan ({} routed)", r.routed);
        eprintln!("{stage}: {cores:.3} cores paused, ended {:.1} s after Resume", w0.elapsed().as_secs_f64() - 3.0);
    }
}
