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
ratio and ladder warnings when the engine report carries them, and the runtime.

Detour (`toporlib.detour.detour_stats`): per connection, routed length / ghost length (`median`, `p90`, `max`,
`count_gt_1_5`, `top10`), unchanged; beside it `trees`: per net with two or more pad-to-pad connections, the drawn
tree length over the Euclidean MST of its pad positions (Du & Hwang 1990: a Steiner tree is at least sqrt(3)/2 of the
MST), with its own `n`, `max`, `count_gt_1_5`, `top10`. Nets with an unrouted connection get no tree row; their routed
connections still count per connection. Compare scorecards on both: the per-connection ratio over-states a tree branch
that ends at another pad of its net (wiccon cycles GPIO0 4.53 per connection, tree 1.68), the tree ratio hides a
single wandering branch (qmtech JS2_5 5.69 per connection, tree 2.79). On `topor-topo` before S2 the engine
route is a placeholder: every connection fails with `NO_ENGINE`, the tool reports 0 % routed.

`tools/classic_reference.sh <out_dir>` builds main @ 64b3979 in a temporary worktree, runs it once on the gate boards
with the same tool (`TOPOR_TREE` selects the code tree), writes `classic-reference.{md,json}` and removes the worktree.
The two USB crops are engine models without a `.kicad_pcb`, so they are run engine-only (no KiCad DRC).

## Layers, planes and capacity (S2b)

Any number of signal layers (>= 2) is routed. Two layers keep the exact Polubasov solve of S2a; with three or
more projection layers `topo/layers.rs::assign_n` colours the wires greedily in wire order and then repeats alpha-beta
swaps: for every pair of layers the events labelled with one of the two are re-solved exactly (Polubasov reductions,
every other label fixed) and the swap is kept when the cost drops. A through via blocks every layer.

`[planes] signals = "costly"` (default) appends the plane layers (non-routable copper layers) to the projection
as layers after the signal layers; a stretch there pays `plane_signal` per mm (on top of length) and half of
`plane_cut` per change into or out of the plane. `"forbidden"` leaves the planes out. The report carries a
`planes (...)` warning (signal mm on plane layers, plane cuts of the drawn copper) and a `capacity:` warning: edges that
fail the layer-subset (Hall) check over the allowed masks before P3, and edge-layers that are overfull when packed
after P3 (the drawing rejects a wire that does not fit; nothing DRC-violating is emitted).

Rule areas that forbid tracks close their layers in the projection (P1), so P2 routes around them.

`python3 tools/samelayer.py <topo_route --out json>` is an independent count of different-net crossings on
one layer. `topo_route` takes `--plane-signals costly|forbidden`.

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
- `TOPO_PLANDUMP=<file>`: the plan handed to the drawing (`<file>.r<round>`, JSON): per wire its net,
  connection, taut points, the layering's stretches and events, its passes (edge ends, room per layer, need)
  and crossings, in KiCad mm. Compare with `TOPO_DUMP` to tell a plan error from a drawing error. Note: the
  `[optimize] runs` seeded runs write the same files (the last one wins), so set `--set optimize.runs=1`
  to read one run's plan and drawing together; join wires by net and end points.
- `TOPO_LANEDBG=1`: per pass of every octilinear redraw, the edge, the S2 interval, the tight span,
  the bounds used, the planned-room bounds of the undrawn neighbours (`planned_room` factor) and the
  lane (wires, spacing, drawn crossings).

Without Python: `cargo run --release --example topo_route -- <input.json>` on a `harness.g0 export` model.
Settings that steer it: `[search] drawing_feedback`, `feedback_max_rip`, `[vias] offset_sites`.

## Validity first (M0v)

A pattern or board run can only judge the router when the router worked to exactly the rules the expected answer was
built for (user 2026-11-12: "the test needs to be valid in order to be able to judge the algorithm"). Every
`route_board.sh` run therefore starts its summary with a `VALIDITY:` line and writes `<name>--validity.json`
(`harness/validity.py`): the rules of the model the engine received (board minimums after fab floors, per-net width /
clearance / the via sizes the router may place, clearance regions of custom rules, `[search] margin_um`) against

- the pattern's `expected.json` `rules_assumed` (schema in the `harness/validity.py` docstring; also checked against
  the board files, so a declaration the `.kicad_pro` / `.kicad_dru` do not carry is caught), or
- without one, the board's own files resolved with `[rules] fab_profile = "board"` (real boards: the hand layout's
  rules).

A pattern's reference board (the hand copy) must also use only what the router may place (via sizes, track widths
at or above its floor). Custom-rule parts topor cannot model (a track width tied to a courtyard, refused rules) make
the run INVALID unless the declaration lists them in `not_needed`. INVALID = the run's faults are not router faults;
metrics carry `valid` / `validity`, the scorecard lists invalid runs under its table.

Route patterns with `--set rules.fab_profile=board` (and the options their `rules_assumed` declares, e.g.
`--set vias.size='"board_min"'`): the default `jlcpcb_1oz` raises Board Setup values below the JLCPCB 1 oz minimums
and adds the profile's smaller vias, which most patterns do not assume. Check a pattern without routing:

    PYTHONPATH=$PWD:$PWD/plugin .venv/bin/python -m harness.validity audit <pattern dir>... --set rules.fab_profile=board [--md]
    PYTHONPATH=$PWD:$PWD/plugin .venv/bin/python -m harness.validity show <route_board out dir>
