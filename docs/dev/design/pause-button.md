# Pause button (branch pause-button, 2026-11-26)

User request: a Pause button in the progress dialog, next to Accept and Cancel. Pause frees the CPU
mid-run. Resume goes on with the same run. Paused time counts toward no timer. The copper is the
same with and without a pause.

## How the plugin runs the engine
The engine runs in the plugin's own Python process: `topor_engine.route_async` is a pyo3 call, and
`RouteJob` is a Rust `Job` thread plus its rayon, crew and `thread::scope` workers. The wx progress
dialog runs on the main thread of that same process (run.py polls `job.progress()` every 0.25 s).
So process signals are ruled out: SIGSTOP stops the whole process, and the dialog with it (no
Resume, no Cancel). No other process uses CPU during the engine phase. The kicad-cli phases
(pre-flight DRC, the DRC loop, preview) are short subprocesses of their own: there Pause stops the
running `kicad-cli` with SIGSTOP and Resume continues it with SIGCONT (never the plugin process
itself). Its `kicad_cli_timeout_s` excludes the stopped time. Between two steps (no engine job, no
kicad-cli running) the button is greyed out.

## Mechanism: a cooperative pause gate in the run's clock
- `TimeLimit` (shared by every run of a call, already reachable from every stop test: `RunControl`,
  the `STOP` thread-local, `ErbsOpts::limit`) gets a pause gate. It is one `AtomicU64` with a
  frozen bit, plus a `Mutex`/`Condvar`.
- `pause()` freezes the clock and sets the gate. `resume()` thaws the clock and wakes the waiters.
  Both are O(1) and idempotent.
- `hold()` is the fast path: one relaxed load. The slow path is `wait_timeout(100 ms)` while the
  gate is paused.
- `hold()` is called at the stop tests the work threads already run: `RunControl::is_cancelled`,
  `is_discarded`, `limit_reached`, `over_limit`, `control::stop_requested` (P3/P4 moves and
  refinements), `control::cancelled` (the projection's `with_cancel`, which then also carries the
  limit) and `ErbsOpts::expired` (the drawing).
- `hold()` is never called inside `TimeLimit`'s own reads or under the progress lock. The Python
  poll uses `snapshot`, which reads the flags without `hold()`, so the dialog never blocks.
- Pool workers between two checks finish their short unit and go idle. This is the same latency
  that Cancel relies on (M0 stop latency, tests/cancel_latency.rs). The test measures it: process
  CPU about 0 within ~1 s.
- A stop request ends the pause. Accept (`request_stop`) and Cancel (`request_discard`) call
  `resume()` first, so Accept draws the best result and Cancel abandons the job within seconds,
  as today. Job end and drop also resume, so a pause can never outlive its job.

## How the timers freeze
- The run's clock becomes `clock_ns = wall_ns - paused_ns`, and stands still while the gate is
  frozen. The state lives in one atomic: either the frozen clock value or the paused total. A
  reader never sees a torn pair.
- `TimeLimit::now()` = `start + clock`. Every `start.elapsed()` and `Instant::now()` inside
  `TimeLimit` uses it: deadline, passed, elapsed_s/optimize_s, drawing_s, the grace and the
  draw-grace bounds, the cap, harden, the soft-step wait.
- The core's own instants that are compared with limit instants switch to `ctl.limit.now()`:
  - route_round `t0`, its 1 s `floor` and `limit_cut`;
  - the TopoR loop's `t_loop`, `loop_deadline` and `loop_over`;
  - the drawing's `t_draw` (draw_start of the draw-grace bound);
  - ERBS `expired`'s `now`.
  The core's other `Instant` deadlines (stitch, reduce, pourcheck) get `None` from the core.
  Per-stage `.elapsed()` used only for profile and log figures stays on the wall clock.
- The loop's own clocks are work counts (cycles, re-lays), not time, so nothing else in the core
  decides by time.
- Plugin side: `Progress` gains `paused` and `paused_s` (the job's total, from the engine).
  - run.py counts router time as `router_before + (now - job_t0) - paused_s`, so the live limit
    it hands the job (`engine_limit`) stays on the same clock as the job.
  - The CPU clock (`time.process_time`) does not move while paused.
  - The total wall time of the run keeps counting.

## UI
- `[Pause]` sits left of `[Cancel] [Accept]`. It is enabled only while an engine job runs
  (`Progress.pausable`).
- Pressing it toggles the label between Pause and Resume.
- While paused, the phase line says "Paused: the router uses no CPU and the time limit stands still.
  Resume goes on with the run". The elapsed figure stands still; the "ends" (Optimizing ends) time moves later by the paused time.
- `UiPort.pause_wanted()` is optional; headless UIs have none, and ScriptedUi gets a script for
  tests. run.py syncs it to `job.pause()`/`job.resume()` at each poll and writes a log line.

## Invariants and tests
- I1, same copper: paused or not, same seed and settings give the same copper. Nothing decides by
  wall time when no limit binds.
- I2, no CPU and no timer while paused.
- I3, a stop always ends the pause.
- I4, the dialog never blocks.

Tests:
- engine/tests/pause.rs, its own binary:
  - the copper hash with and without pauses at several phases (projection, P2, P3/P4, loop,
    drawing) on eprom;
  - process CPU about 0 over 1 s after a 1 s settle;
  - `optimize_s` and the deadline frozen while paused, so a 2 s limit with a 3 s pause still
    stops by run time;
  - Accept and Cancel while paused, within the cancel-latency bound.
- control.rs unit tests for the clock algebra.
- pytest (ScriptedUi plus the real engine): router_s leaves out the pause; Accept and Cancel work
  while paused; the dialog's label toggles.
- The GUI check uses Xephyr :6 (`gui_e2e.py --pause-at S --resume-after S`), with screenshots
  of the dialog running, paused and resumed.
