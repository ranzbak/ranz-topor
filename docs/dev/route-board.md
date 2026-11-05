# Routing a board headless (S0 tools)

`tools/route_board.sh <board|pcb> <out_dir> [--timeout S] [--set run.key=value] [--whole-project] [--name N]`

1. Copies the project twice into `<out_dir>`: `<name>-hand/` (untouched reference) and `<name>-input/` (tracks, vias
   and teardrop zones removed; zones and planes kept). `harness/strip.py` does this and can be run alone:
   `.venv/bin/python -m harness.strip <board|pcb> <out_dir>`. The source tree is hashed before and after; it is never
   written.
2. Builds the engine model of the stripped copy and runs `topor.run.run` (the plugin's run flow) on a `FakeBoard`
   that accepts the result. The engine is whatever `topor_engine` the venv has; `TOPOR_ENGINE_SITE=<dir>` puts another
   one (a wheel installed with `pip install --target <dir>`) in front.
3. Writes `<name>-topor/` (routed copy), `<name>--metrics.json`, `<name>-compare.png` (hand left, topor right).

Metrics come from `kicad-cli pcb drc --refill-zones` on the routed copy: unconnected items, routing_new (routing-type
violations the stripped input does not already have), vias; plus the run's routed/total connection count, the detour
ratio and ladder warnings when the engine report carries them, and the runtime. On `topor-topo` before S2 the engine
route is a placeholder: every connection fails with `NO_ENGINE`, the tool reports 0 % routed.

`tools/classic_reference.sh <out_dir>` builds main @ 64b3979 in a temporary worktree, runs it once on the gate boards
with the same tool (`TOPOR_TREE` selects the code tree), writes `classic-reference.{md,json}` and removes the worktree.
The two USB crops are engine models without a `.kicad_pcb`, so they are run engine-only (no KiCad DRC).

## Drawing diagnosis (S2c)

The topological core's report carries one `drawing yield:` warning: wires that entered drawing, wires drawn,
wires lost at the via stage, rejected runs by class (`packing`: a pass lies on an edge whose demand exceeds its
room, `repair`, `chord`), runs accepted by the exact path (`topo/exact.rs`) or by S2a's funnel+repair fallback,
and the feedback counts. Environment variables of the engine (debugging only, no effect on the result):

- `TOPO_DIAG=<file>`: one JSON row per rejected run (wire, net, layer, width, passes, class, violated object,
  gap and requirement in nm, location in mm, endpoints).
- `TOPO_DIAG_DEEP=1`: also re-run the exact search against the static board alone (`static_infeasible`);
  slow.
- `TOPO_DUMP=<file>`: the drawn copper, pad polygons and the failed runs' polylines (for `plotfail`-style plots).
- `TOPO_VIADBG=1`: why a via interval had no site.

Without Python: `cargo run --release --example topo_route -- <input.json>` on a `harness.g0 export` model.
Settings that steer it: `[search] drawing_feedback`, `feedback_max_rip`, `[vias] offset_sites`.
