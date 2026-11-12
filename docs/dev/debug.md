# Debugging topor

How to look inside a run without KiCad's GUI, and how to turn a user's failed run into something a
developer can reproduce. Every command here was run on the repo at version 0.4.0; run them from the repo
root with the dev venv (`tools/dev-venv.sh`). Boards: only **copies** — snapshots under
`scratch/boards/<board>/<sha12>/` (`.venv/bin/python -m harness.snapshot`) or your own copy; never a file
under `~/work/kicad/projects` (see [CONTRIBUTING.md](../../CONTRIBUTING.md#boards-are-read-only)).

| Tool | Question it answers |
|---|---|
| [`harness.bench`](#benchmark-a-board-harnessbench) | What does topor do with this board end to end (rip, route, DRC, metrics, PNGs)? |
| [`harness.measure`](#board-facts-harnessmeasure) | What is on this board (copper, widths, net classes, rules, original DRC)? |
| [`route_probe`](#engine-only-route_probe) | What does the engine alone do with this model and these options? Where does the time go (`--profile`)? |
| [`gate_diag`](#blocked-or-detouring-connections-gate_diag) | Why is a connection blocked or detouring (`--detour`)? |
| [`harness.progress`](#progress-pngs) | What does the result look like next to the hand routing? |
| [`topor.app --headless`](#headless-runs) | Does the plugin flow (pre-flight, DRC loop, report) work on this board without KiCad? |

## Benchmark a board (`harness.bench`)

```bash
.venv/bin/python -m harness.bench eprom-to-kernal /tmp/topor-bench --router topor --rip all
```

Copies the snapshot, rips the copy's tracks (`--rip all|none|scoped`), exports the model, routes with
topor (`--threads N`), runs `kicad-cli` DRC and prints a ≤ 20-line summary (routed/total, vias, length
outside pads vs hand, detour median / p90 / max, DRC `routing_new` / unconnected). The output directory
keeps `<board>--model.json` (input for the engine tools below), `<board>--metrics.json`, the three DRC
JSONs, `<board>-hand.png` / `-topor.png` / `-compare.png` and `<board>-work/<board>.kicad_pcb` (the routed
copy — open it in KiCad). `--pcb FILE` benchmarks another board file instead of the snapshot.

## Board facts (`harness.measure`)

```bash
.venv/bin/python -m harness.measure --board wiccon-2023
```

≤ 15 lines: source sha256, copper counts and length, widths, net classes, the resolved rules, the original
DRC counts, the wire count. Works on a copy and verifies the source tree is unchanged.

## Engine only (`route_probe`)

```bash
cargo run --release --manifest-path engine/Cargo.toml --example route_probe -- \
    /tmp/topor-bench/eprom-to-kernal--model.json --quiet --profile
```

Runs the engine's `route()` on a model JSON (no Python, no KiCad) and prints routed/total, vias, length,
iterations, `converged` / `timed_out`. Options: `--options '<Options JSON>'` (e.g.
`'{"runs": 1, "seed": 3}'`; the keys are those of `topor_engine.default_options()`),
`--iters N` (iteration cap), `--margin NM`, `--dump out.json` (the result JSON without `runtime_s` /
`threads_used` — diff two dumps to check determinism across thread counts), `--quiet` (no per-connection
lines). **`--profile`** (or `"profile": true` in the options) adds the per-iteration table (search / commit /
metrize / check seconds, searched, re-searched, ripped, overuse) and the search counters (expansions,
candidates, bend queries per run) — the first stop when a board is slow.

## Predicted vs drawn vs KiCad DRC (`harness.predict_vs_drawn`, stage M1 compass)

Every engine result of the topological core carries `compass` (engine `topo/compass.rs`): the plan handed to the
drawing, per violation class (docs/dev/inflated-sketch.md §6.1) its count and its located items (cuts, edge-layers,
via sites, unlayerable crossings, traversals through foreign copper, forced paths; with `[capacity] model = "cuts"`
also the per-edge packer's over-full edge-layers as `model_ref`, the evidence for *model gap*), and per connection the
planned vias, taut length and necked stretches. `toporlib/compass.py` classifies every drawn violation against those
items (first match: via site, unlayerable, foreign copper, over-full (own / nearby), forced path, pad exit, shrinkage,
model gap, pad exit not predicted, else drawing loss), joins KiCad's new routing DRC items by place and builds the
table; the plugin writes it to report.md / result.json, `tools/route_board.sh` to `<name>--compass.json` and the
metrics (`compass`, `compass_line`), `harness.score_runs` appends one row per run to `<root>/compass.tsv`.

    .venv/bin/python -m harness.predict_vs_drawn <route_board out dir | score_runs root> [--cuts DIR] [--json OUT]

`--cuts DIR`: a run with `[capacity] model = "edges"` and `--set capacity.cut_report=DIR` gets the model-gap class from
the cut test (`harness.cutclass`). Shopping list 5.10 (pad exits as plan objects) fills
`topo::compass::pad_exit_items` and the `pad_exit` predicted count; until then that row reads "not predicted".

## Blocked or detouring connections (`gate_diag`)

```bash
cargo run --release --manifest-path engine/Cargo.toml --example gate_diag -- \
    /tmp/topor-bench/eprom-to-kernal--model.json --iters 5 --detour 19
```

(A) searches every connection on the **empty** graph (static routability, estimated length, expansions,
slowest 8); (B) replays the negotiator for `--iters` iterations; (C) for every connection still blocked,
probes which occupancy (vias, direct paths, everything) stands in its way (`--probes` for more). The
connection ids are the `#N` of part (A). **`--detour ID[,ID…]`** dumps, for those connections, the
slot-search path and the topological ghost crossing by crossing (edge ends, length, capacity, need,
fits / passable) and the path the negotiator committed — the tool for "why does this trace go around"
(compare with the detour ratios in the run report). `--skip-a` skips part (A) on big boards.

## Progress PNGs

```bash
.venv/bin/python -m harness.progress --milestone m4 --task B12 --router topor --board wiccon-2023
```

Writes `docs/benchmarks/progress/<YYYY-MM-DD>-m4-<task>-<board>.png`: hand routing left, topor right, both
copper layers, caption with routed/total, vias, DRC, `router_s`, the detour line and the pcb sha12.
Rendered after every task that changes routing results (committed with `git add -f`); never use a test
pattern as a result. `--pcb FILE` renders another board, `--threads N`.

## Headless runs

The plugin flow without KiCad and without a window: `FakeBoard` (a `.kicad_pcb` file instead of the IPC
API) and `ScriptedUi` (answers the dialogs). Pre-flight, the `kicad-cli` DRC loop, the report folder and the
exit code are the real ones.

```bash
TOPOR_REPORT_ROOT=/tmp/topor-reports PYTHONPATH=.:plugin \
    .venv/bin/python -m topor.app --headless <copy>/board.kicad_pcb --model <copy>/model.json [--accept]
```

- `--model` is a BoardModel JSON (from `harness.bench`, or exported as in the next section);
- `--accept` answers the preview with Accept (default Reject); the board **file** is never written —
  `FakeBoard` applies the commit to its in-memory text only;
- `--selection '{"footprints": ["U1", "U2"]}'` simulates a KiCad selection (also `pads` `[["U1","3"]]`,
  `tracks`, `vias` (uuids), `area` `[[x_nm, y_nm], …]`);
- `topor.toml` beside the board is used if present (e.g. `[run] existing_copper = "rip"` for a routed board),
  else the defaults — no `topor.toml` is created;
- prints `topor: <status> routed <n>/<total> report <dir>`; exit code 0 / 1 / 2 as in
  [troubleshoot.md](../user/troubleshoot.md#exit-codes);
- the preview PNG needs a `wx.App`, so headless runs log `preview failed: preview.render needs a wx.App`
  in `log.txt` and add a `PREVIEW_FAILED` warning — expected; the DRC numbers are unaffected.

Net classes come from the model's own `rules`; `topor.app --headless` reports only the *Default* class to the
rule resolver and the priorities. `tools/route_board.sh` passes KiCad's net classes of the board (read with
pcbnew, `harness.route_board.board_netclasses`), as the plugin reads them over IPC, so its priorities match a
plugin run.

## Reproducing a plugin run

A plugin run leaves `log.txt`, `result.json`, `report.md` and `drc.json` in its report folder. The board
copy topor routed (the `save_as` copy in a temporary folder) is deleted when the run ends, so the board
has to come from the user:

1. In KiCad, **File → Save** the board, then copy the whole project folder (`.kicad_pcb`, `.kicad_pro`,
   `.kicad_dru`, `topor.toml`) somewhere else — this is the state topor saw, except for edits made
   after the run.
2. Read the settings of the run from `report.md` (*Settings*: non-default keys, accepted findings) and
   `result.json` (`findings` with code / message / items, `status`, `stop_reason`, `rounds`, `route`); put
   the same values in the copy's `topor.toml` (the options chosen in the dialog are listed in `log.txt`,
   line `decision: overrides {...}`).
3. Export a model of the copy with the harness (pcbnew, dev venv only) — this script works on a further
   copy and writes `model.json` next to it:

   ```python
   # PYTHONPATH=. .venv/bin/python mkmodel.py <copy>/board.kicad_pcb /tmp/repro
   import json, sys
   from pathlib import Path
   from harness.copy import copy_project
   from harness.model import export_model, load_board
   from harness.rip import record_widths
   from harness.rules import resolve_rules
   src, work = Path(sys.argv[1]), Path(sys.argv[2])
   cp = copy_project(src, work)
   b = load_board(cp)
   dru = cp.dru.read_text() if cp.dru else None
   rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
   (work / "model.json").write_text(json.dumps(export_model(b, rules, name=cp.pcb.stem, dru_text=dru)))
   ```

4. Copy `topor.toml` into `/tmp/repro/` and run the [headless](#headless-runs) command on
   `/tmp/repro/board.kicad_pcb` with `--model /tmp/repro/model.json`. Compare `result.json` and `log.txt`
   (connections, DRC rounds, routed/total) with the user's.
5. For engine-level questions run [`route_probe`](#engine-only-route_probe) /
   [`gate_diag`](#blocked-or-detouring-connections-gate_diag) on the same `model.json` (the model holds the
   pre-rip copper; with `existing_copper = "rip"` use the model of
   `.venv/bin/python -m harness.bench myboard /tmp/repro-bench --router topor --rip all --pcb /tmp/repro/board.kicad_pcb`
   instead — exported after the rip; with `--pcb` the board name is a free label).

The IPC model and the harness model of the same board are compared by `tests/plugin/test_ipc_parity.py`
(live, see [gui-test-checklist.md](../user/gui-test-checklist.md#ipc-parity)); differences between a plugin
run and its headless reproduction that go beyond the approximations listed in
[operate.md](../user/operate.md#limitations) are bugs.

## The engine input of a plugin run (`TOPOR_DUMP_ENGINE_INPUT`)

Start KiCad (or `tools/route_board.sh`, or `tools/gui_e2e.sh`) with `TOPOR_DUMP_ENGINE_INPUT=<dir>` in its
environment: every engine call of the run flow writes the exact engine input, `<dir>/model-N.json` and
`<dir>/options-N.json` (N = DRC-loop round), and `log.txt` names the files. Two dumps of the same board (live plugin
and headless harness) are compared field by field by net name; a difference beyond the conservative approximations
of [operate.md](../user/operate.md#limitations) is a bug (K1 found three this way: zone connection codes off by
one, thick copper arcs as dozens of overlapping rings, custom pad arcs flattened inward). The engine alone on a dump:

```bash
PYTHONPATH=<engine site> python3 -c "import topor_engine; print(topor_engine.route(open('model-1.json').read(), open('options-1.json').read())[:200])"
```

## The engine's reading of a board (fail-fast probe, `TOPOR_CDT_NODING`)

Right after the board is read the plugin calls `topor_engine.probe_board(model, options)`
(`engine/src/topo/core.rs` `probe`): the projection a route builds first (one constrained Delaunay
triangulation of every obstacle of all layers), without routing. `log.txt` has the line
`engine probe: ok, <s> s, triangulation <path>`; a failed probe is the `ENGINE_BOARD_UNREADABLE` error and its
`places` name the constraint the triangulation failed on and the nearest copper (owner, layer, net, uuid, KiCad
mm). On a dump:

```bash
cargo run --release --manifest-path engine/Cargo.toml --example probe -- model-1.json --options options-1.json
```

The triangulation (`engine/src/cdt.rs` `Cdt::build_reported`) tries spade's own constraint splitting first; when
spade panics on near-degenerate crossings it nodes the constraints itself (`engine/src/noding.rs`: snap rounding
on a 1 nm grid, exact predicates) and inserts them without crossings; the old reversed-order and 128 nm snap
retries are the last fallback. The path taken and any constraint left out are report lines (`projection: …`).
`TOPOR_CDT_NODING=always` takes the noded path on every board (A/B checks), `TOPOR_CDT_NODING=never` skips it
(the behaviour before it: e.g. to see the fail-fast dialog on `engine/tests/fixtures/bh-nfc-coil-ae1.json`).

## A board routed elsewhere, judged like the harness (`harness.board_metrics`)

```bash
PYTHONPATH=.:plugin .venv/bin/python -m harness.board_metrics <routed.kicad_pcb> --input <stripped input> \
    --hand <hand copy> [--report-dir <topor report folder>] [--out m.json] [--png compare.png]
```

The numbers of `tools/route_board.sh` (unconnected, routing_new, vias; routed/total from the run's `result.json`)
for a board saved by the live plugin, and the hand-vs-result PNG with the open ratsnest in blue.

## KiCad's Cleanup Tracks & Vias replayed (`harness.cleanup_check`)

```bash
.venv/bin/python -m harness.cleanup_check <routed.kicad_pcb> [--only-new <input.kicad_pcb>] [--refill] \
    [--json out.json] [--list N] [--no-per-option]
```

topor's copper must survive KiCad's **Edit → Cleanup Tracks & Vias** with every option on
([operate.md](../user/operate.md#topors-copper-survives-cleanup-tracks--vias)). KiCad's cleaner
(`TRACKS_CLEANER`, `pcbnew/tracks_cleaner.cpp`) is not in its Python API, so this module replays
`TRACKS_CLEANER::CleanupBoard` step by step on an in-memory copy with KiCad's own connectivity and geometry calls:
redundant vias, null and duplicate segments, `mergeCollinearSegments` (including its bounding-box merge rule),
`removeShortingTrackSegments`, `deleteTracksInPads`, then `deleteDanglingTracks` with
`CONNECTIVITY_DATA::TestTrackEndpointDangling` until nothing dangles, applying each change as KiCad does so
cascades count. Every item it would remove or change is one **cleanup-unstable item** (0 is the target); each
names its rule and KiCad function. It also runs every dialog option alone (`per_option`), which separates root
causes from the cascade of the full run. Without `--refill` the zones are judged with the fill stored in the file
(the dialog's default); `--only-new` counts only copper whose uuid is not in the input (topor's).

Checked against the real dialog on qmtech (the plugin's own run, KiCad on Xephyr): the replay's counts per option
match the dialog's within 2 of 105 to 622 items. `tools/route_board.sh` runs it on every routed copy (metrics
`cleanup_unstable`, `cleanup_by_rule`, `cleanup_per_option`, `<name>--cleanup.json`) and `harness.scorecard` shows
the column "cleanup-unstable items". The engine side is `engine/src/topo/tidy.rs` (`[output] cleanup_safe`), the
plugin side `run.commit_fills` (the zones get the fills of the refilled routed copy in the Accept commit); the
mechanism fixtures `fx_cleanup_*` (`tests/test_mechfix.py`) route each class as drawn and with the defaults.

## Live GUI test (`tools/gui_e2e.sh`)

Drives the real plugin in a real KiCad 10 on Xephyr with a separate KiCad home and checks every step; see
[gui-test-checklist.md](../user/gui-test-checklist.md#automated-run).

## Filing a bug

Include:

- what you did and what you expected (one or two sentences);
- the **Copy details** text of the error dialog (code, message, fix, items, log path, versions of engine,
  plugin, kipy, KiCad), or the versions from `report.md` §Versions;
- from the report folder: `log.txt`, `result.json`, `report.md`, `drc.json`, `preview.png`;
- the last entry of `~/.cache/kicad/10.0/topor/topor-crash.log` if the process crashed;
- the board: a copy of the project folder saved as in [Reproducing a plugin run](#reproducing-a-plugin-run),
  step 1 (only if you may share it), and its `topor.toml`;
- `kicad-cli version` and `uname -a`.

The board on disk is never changed by a crash: topor writes only on Accept, as one KiCad commit.
