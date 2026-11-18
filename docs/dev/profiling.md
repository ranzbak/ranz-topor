# Profiling a run (TW0)

One command gives, for any board run, the per-stage time table, the loop split, cycles and re-lays per 600 s of loop
time, and (optional) flame graphs. The engine counters are off unless `TOPO_PROF` is set, so a normal run pays one
relaxed load per hook site.

## Use

    tools/prof_run.sh <board|pcb> [out_dir] [--flame] [route_board options]
    # e.g.  tools/prof_run.sh esp-rust-board /tmp/prof-esp --timeout 600

- `<board>` is a `harness.paths.BENCH_BOARDS` key or a `.kicad_pcb` path (read only).
- The run is `tools/route_board.sh` with `TOPO_PROF=1`, `optimize.rounds=1`, `optimize.runs=1`, `--no-png`. The stage
  windows are a wall-clock partition of one run, so several parallel runs (`optimize.runs > 1`) are not supported.
- Output in `<out_dir>`: `prof.md` (the table), `prof.json` (a sidecar `"metrics"` object `{"<board> s0": {...}}`, paste
  it into `reports/<branch>.json`), `run/<board>--metrics.json` (everything, incl. `profile`), and with `--flame`
  `flame-stages.svg` (root frame = stage), `flame-run.svg`, `flame-loop.svg`.
- Only the table from an existing run: `python3 tools/prof_table.py <run dir | X--metrics.json | result.json>`.
- Any plugin run with `TOPO_PROF=1` in the environment also writes the profile: `result.json` key `profile` (engine
  counters plus `python.{before_s, job_s, after_s, total_s}`).

Timing numbers that decide something come from an exclusive ccx43 (`hz.sh job --exclusive ...`), one board at a time.
Locally the tool is a smoke test (load changes every second).

## Reading the table

Rows partition the run's wall time: python before the engine, the engine stages (plane stubs, projection, P2-P4, TopoR
loop, via/branch relocation, drawing + feedback, via reduction, tail), the engine job's own overhead, python after the
engine. The shares sum to 100 % (the `sum of the rows` line). "of it" rows are parts of the row above: the loop split
(judge, search, layering, crossing, pair, cycle setup, other; laps on the loop thread, they add up to the loop row) and
the python split from `log.txt` (baseline DRC, DRC after, refill, commit). The counter keys of the loop (`rr_viol_after`,
`assign_with`, `refine`, `search_tree`, ...) are inclusive and summed over all threads: they overlap each other and can
exceed the wall time inside the refine lanes. Per 600 s of loop time: cycles and re-lays (`reroute` calls = Sp::reroute).

## Flame graphs

`--flame` needs `perf`, `rustfilt` and `inferno` (`cargo install inferno --locked`) on PATH and an engine built with
frame pointers and debug info:

    CARGO_PROFILE_RELEASE_DEBUG=true RUSTFLAGS="-C force-frame-pointers=yes" \
      maturin build --release --manifest-path engine/Cargo.toml --features python
    pip install --no-deps --target <site> <wheel>; export TOPOR_ENGINE_SITE=<site>

(`echo -1 | sudo tee /proc/sys/kernel/perf_event_paranoid` on the VM.) The samples are mapped to stages by the
CLOCK_MONOTONIC stamps of the stage windows (`perf record -k mono`). perf's `period` is not nanoseconds on the VMs: the
tool counts samples (CPU s = samples / 499).

## For contributors: adding a counter

`engine/src/topo/prof.rs` is the only module. A timed key: add a variant to `K` and its name to `NAMES` (same order),
then `let _pg = super::prof::g(super::prof::K::MyKey);` at the top of the function. A stage mark ends the window its label
names (`prof::mark("label")`; the label to stage map is `stage_of`, covered by a unit test). Keep call sites one line;
nothing in the hooks may read or change routing state. `engine/tests/topo_prof.rs` checks that counters on give the same
copper and that the windows cover the run.
