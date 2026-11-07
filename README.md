# topor — topological curved-trace router for KiCad

topor routes PCB connections as smooth, taut traces (straight segments joined by tangent arcs),
using topological routing: first decide *which side of every obstacle* a wire passes, then pull
each wire tight. It respects KiCad's design rules and checks its output with KiCad's own DRC.

Engine in Rust (`engine/`), packaged as a Python wheel; KiCad 10 plugin in `plugin/` (IPC API);
test and benchmark harness in `harness/`. Linux only for now.

## Status (version 0.4.0, milestone M4b: the plugin)

| Works now, from inside KiCad 10 | Comes in 0.5.0 (milestone M4a: the engine) |
|---|---|
| **Topor: route** button: pre-flight findings (code, cause, fix), options, progress with Cancel, before/after preview, **Accept = one undo step** / Reject = nothing changed | dcsgonebadge 231/231 routed, 0 router DRC, ≤ 10 min (now 148/231 in 60 s) |
| Whole board or a selection (between / touching selected footprints); existing copper kept, or ripped and re-routed; locked copper respected | priorities used for routing order (`@N` markers, `topor_pN` net classes, `[nets] priority` — computed and reported now) |
| Self-documenting `topor.toml` per project, generated [reference](docs/user/configuration.md); every finding names the KiCad menu or the key to change | pad exit stubs for fine pitch, same-net T-junctions, via rules from the board setup, bounded via relaxation, layer costs |
| Result verified by `kicad-cli` DRC on a copy before you decide; report folder (report, preview, DRC, log) per run | engine-side `do_not_touch` / `ignore_pads` options (applied by the plugin's scope now) |
| eprom-to-kernal 27/27 and wiccon-2023 62/62 routed, `routing_new 0`, `unconnected 0` (headless, judged by `kicad-cli`) | eprom / wiccon not worse than now |

The plugin needs a live GUI test before the 0.4.0 gate closes:
[docs/user/gui-test-checklist.md](docs/user/gui-test-checklist.md). Known limits:
[operate.md → Limitations](docs/user/operate.md#limitations).

## Topological core (branch `topor-topo`, in development)

A new engine core, written after TopoR's approach: the board is projected into one triangulation of all pads,
holes, keepouts, copper graphics and the outline; every connection is searched there as a topological path
(which side of each obstacle it passes), its net growing as a tree (a connection may end on a pad or a branch
point of copper it joins, including through-pad junctions); layers are assigned afterwards (Polubasov's via
minimisation), then rip-up and reroute, via reduction, and finally the drawing pulls each wire taut and checks
it against KiCad's rules. Plane pads get a stub and a via into their pour before any of this, so the planner
routes around them (a pour that fills only the pad's own layers gets a track alone into the fill); a pad that
no stub can serve is joined to its net's copper by a signal connection instead; pour pieces the routed copper
cuts off are stitched back with island vias or pad stubs.

What works on this branch:

- graded social factors instead of hard thresholds, each with a weight in `topor.toml [factors]` and a line in
  the run report: via_squeeze, junction_angle, branch, blocks_pad, squeeze, congestion, ignored_rule, and more
  ([factor reference](docs/dev/factors.md));
- keepouts: track keepouts per layer, via keepouts on any copper layer (a through via spans them all), pour
  keepouts in the fill estimate; copper graphics are obstacles on their layer;
- pour-island stitching after routing, and KiCad's minimum connection width respected by vias and stubs;
- layer-aware crossings: the search knows the layers each crossed trace is held to and does not cross where
  no layer assignment can separate the two (`[search] crossing_vias = "via"` also prices the implied via;
  off by default, the crossing factor stands for it);
- net-tree junctions in the layer assignment (two layers): a T junction is one layer domain (Polubasov's tee
  rule: two arms on one layer, the via on the third), and a via next to a junction stands at the junction;
  the junction angle price follows the Steiner-tree length bound (`junction_angle`, ψ curve, 0 from 120°);
- fuse after drawing: a trace that leaves a branch point or a pad right beside another trace of its net
  starts on that trace's copper where the two part (one copper, a T there), when that is cheaper by length
  and junction angle and keeps every clearance (`[search] fuse_branches`, on by default);
- the drawing's exact path has no out-and-back track pieces at its joints any more.

The board ladder at commit `121e3eb` (headless, `tools/route_board.sh`, judged by `kicad-cli` DRC after a zone
refill; "unconnected" counts DRC unconnected items, the hand-routed board's own count in brackets):

| Board | Routed | Unconnected (hand) | New routing DRC errors | Vias |
|---|---|---|---|---|
| eprom-to-kernal | 27/27 | 0 (0) | 0 | 0 |
| wiccon-2023 | 60/60 | 0 (0) | 0 | 23 |
| dcsgonebadge | 189/231 | 47 (4) | 0 | 116 |
| qmtech | 346/426 | 85 (0) | 0 | 195 |

### The topological core inside KiCad (K1)

The **Topor: route** button runs this core in KiCad 10: pre-flight findings (differential pairs announced as
skipped, pads inside track keepouts), a progress dialog that names the stage and round and shows router time and
total time (`[run] timeout_s` is now 600 s, an emergency stop; the `[optimize]` work caps end a run), Cancel at any
time, a preview with the open ratsnest in blue, findings after routing (unrouted connections by reason, pour
islands and stitches, via squeeze, necking, ignored rules, ...; [operate.md](docs/user/operate.md#findings-after-routing)),
Accept as one undo step. Every `topor.toml` setting the core reads reaches it (before K1 the `[optimize]`,
`[factors]` and the other core settings were dropped as "since 0.5.0"); settings of the classic engine say so in
the [reference](docs/user/configuration.md).

Live test, automated: `tools/gui_e2e.sh <board> <out_dir> --from-hand [--harness <metrics.json>]` drives KiCad 10
on a nested X display (Xephyr) with a separate KiCad home, never yours, opens a copy of the hand-routed board,
routes with *Rip up and re-route*, accepts, checks one Ctrl+Z restores the board, saves and judges the result with
KiCad's DRC ([gui-test-checklist.md](docs/user/gui-test-checklist.md#automated-run)). Plugin in KiCad vs the
headless harness on the same board (engine at K1):

| Board | Live plugin: routed / unconnected / routing_new / vias | Harness | Same? |
|---|---|---|---|
| wiccon-2023 | 60/60 / 0 / 0 / 23 | 60/60 / 0 / 0 / 23 | yes |
| dcsgonebadge | 184/231 / 52 / 0 / 110 | 189/231 / 47 / 0 / 116 | no: the copper-graphics polygons differ by < 0.5 µm (both contain KiCad's copper); the plugin's engine input with the harness's polygons routes 189/116 |
| qmtech | 349/426 / 84 / 0 / 206 | 346/426 / 85 / 0 / 195 | no: the net numbering differs (KiCad's API has no net codes, the plugin numbers nets by sorted name); the plugin's engine input renumbered with pcbnew's codes routes 346/195 |

Every other part of the engine input is now identical between the plugin and the harness (rules, widths, net
classes and priorities, pads, pours, zone connections, keepouts, teardrops); the remaining differences are the
router's sensitivity to the copper-graphics vertices and to the net numbering, not plugin errors. The ladder above
is unchanged by K1 (eprom 27/27, wiccon 60/60, dcs 189/231, qmtech 346/426, routing_new 0).



### Iterations like TopoR: "Routes stable", OK, best of N runs (M-B0)

A run no longer stops after a fixed four rounds. Like TopoR it keeps iterating whole-route cycles: every cycle
routes the board again with the passages the drawing found too small lowered and with fluctuating factor weights
(Polubasov 2010) whose width starts wide, narrows every cycle and widens again when the run is stuck with
connections still open (annealing with reheating; `[optimize] fluctuation`, `fluctuation_narrowing`,
`fluctuation_stuck`, `fluctuation_reheat`). Every cycle's variants are pour-stitched and judged (a pour piece left
cut off counts as an error). When the best result gains less than `stable_gain_pct` over `stable_window` cycles the
routes are *stable* (TopoR 6.3 manual p. 73: "Autorouting may be stopped"; not before level 5–10): the progress
dialog says **Routes stable** and its Cancel button reads **OK** — the same action, the best result shown is exactly
the board you get. The run also stops there by itself (`finish_when_stable`, `finish_patience` = 0 since M-B1, user
2026-11-07: "When route stable appears it should stop indeed"; `finish_when_stable = false` keeps iterating for
marginal gains until OK), and headless runs stop at stable. `[optimize] runs` (default 5) seeded runs go on in
parallel on the engine's threads and the best by the objective is offered; `[optimize] rounds` (30) is only a
safety cap and `[run] timeout_s` (now 1800 s) the emergency stop.

Scorecard (KiCad DRC after a refill; connected % of KiCad's own input count, default run and median [range] of
seeds 1–5; `harness/scorecard.py`):

| Board | Before (fixed 4 rounds) | M-B0 (best of 5, until stable) | Router time before → after |
|---|---|---|---|
| eprom-to-kernal | 100 / 100 | 100 / 100 | 0.3 s → 2 s |
| wiccon-2023 | 100 / 100 [98.2–100] | 100 / 100 [100–100] | 1 s → 27 s |
| dcsgonebadge | 77.6 / 76.2 [74.3–78.6] | 80.5 / 79.5 [77.1–80.5] | 53 s → 442 s |
| qmtech | 79.6 / 78.9 [77.5–79.9] | 81.5 / 81.5 [80.6–83.2] | 174 s → 1014 s |

The defaults are provisional; their re-measurement with the M-B1 objective was skipped (user 2026-11-07: next
stage first).

Known limits: the complex boards (bornhack2026, hackerhotel2027, tanmatsu) are not on the ladder yet;
differential pairs are not routed; branching is not complete (the fuse moves copper after drawing but the
planner still reserves room for both traces; re-attaching after changes, junction relaxation toward the
Fermat point, vias as junctions and new branch points are open; the tee rule is two-layer only; a few
side-by-side pairs of one net remain, mostly at adjacent pins of one net and twin plane stubs); pour islands
are stitched afterwards, not prevented; results on the larger rungs move by a few connections with small
changes elsewhere on the board (rip-up order).

### Never drop (M-B1): violations instead of open connections

Like TopoR, a connection is no longer dropped when its drawing finds no legal geometry (a gap too narrow when
drawn, a via without a free site, a crossing that cannot be layered, a branch meeting its trace on another layer).
It stays in the router and is drawn anyway on its least violating geometry (the drawing's attempts, the exact path
with the clearance relaxed by the least amount that gives one, the funnels before the repair; a via goes to its
least violating site). Every rule it breaks is priced very high by the graded `violation` factor (1000 mm per
violation plus 1000 mm per 0.01 mm² of depth × length) and reported with its place, type and cause
(`VIOLATIONS` finding, `violations` in result.json). The best result counts errors first — connections with
violations plus connections without copper ("a design-rule violation cannot be compensated by any reduction of
wire length", Polubasov 2010) — then the objective with the violation price; "Routes stable" and the best of N see
the same count. The violating copper is written to the board; `[optimize] allow_violations = false` keeps the
legal-only output (such connections stay unrouted, as before). The topological core takes no DRC feedback, so the
plugin judges the result with one DRC round.

Scorecard at the defaults (best of 5 runs, measurement runs cut at 600 s of router time; default run and median
[range] of seeds 1–5; `harness/scorecard.py`, KiCad DRC after a refill):

| Board | Connected % M-B0 | Connected % M-B1 | Total length % of hand | Engine violations (connections) | Router time |
|---|---|---|---|---|---|
| eprom-to-kernal | 100 / 100 | 100 / 100 | 95 | 0 | 2 s |
| wiccon-2023 | 100 / 100 | 100 / 100 [100–100] | 89 / 88 | 0 | 24 s |
| dcsgonebadge | 80.5 / 79.5 [77.1–80.5] | 92.4 / 91.0 [90.0–91.4] | 62 / 63 | 236–385 (33–41) | 600 s (cut, all 6) |
| qmtech | 81.5 / 81.5 [80.6–83.2] | 91.4 / 91.6 [90.9–93.3] | 96 / 96 | 445–911 (47–58) | 600 s (cut, all 6) |

The violations are many and some are bad (copper over foreign pads and traces, `short` about half of them): the
drawing still realises a plan whose passages are too narrow or whose crossings are on one layer. Removing them
where they arise — the drawing failure fed back to the planner as a local cut, the crossing rules applied in the
topology — is the next stage (M-B2).

### Failures feed back locally; crossing rules in the planner (M-B2)

A cycle now feeds what went wrong back to the next one *where it happened*, and the planner removes the
crossing patterns that force violations while change is still cheap:

- **Local, fading history.** A violation puts history (a price for that connection only) on the passes of its
  trace near the place, weighted by the distance (1 on it, falling smoothly), not on the whole trace (Dr. CU,
  IEEE TCAD 2020, §IV-C: history only on "the intervals with violations"); a via without a site prices only the
  passages around its interval. Everything carried over fades by `[optimize] history_fade` (0.5) per cycle, and the
  present price of an over-full passage is kept level with the largest history so history cannot dominate
  (BoxRouter 2.0, p. 32:8–9). A passage the drawing found too narrow gets a lowered room (a cut, Haas 2026
  §VI-E); when a cycle comes out worse than the best, the cuts turn into a price (`[optimize] soft_cuts`). The
  drawing's rescue rips blocking traces by a graded price instead of a count, and the traces whose via count rose
  are rerouted first (Polubasov 2010).
- **Crossing rules** (`[optimize] crossing_rules`; `engine/src/topo/xrules.rs`): a trace of another net that wraps
  around a connection's terminal pad (a clinch or half-loop at a terminal, Luzin & Popov 2012 pp. 116–118 — the
  user's qmtech ADV7511 → U4 header case), two traces crossing twice (Luzin 2013 Fig. 16) and a crossing whose
  stretches are both held to one layer (Polubasov 2001) are re-planned pairwise — the terminal's trace first, the
  other one laid on the other side — before and after the rip-up, kept when the plan scores better. The planner's
  score counts crossings that cannot be layered as predicted violations.
- The `VIOLATIONS` finding groups what is left by origin: *pad exit* (a neighbour pin of the connection's own part —
  necking and pad exits, planned in M-C), via without a site, crossing, other copper in a narrow passage, static.

Scorecard (same setup as M-B1: best of 5 runs, cut at 600 s of router time; default / median [range] of seeds 1–5):

| Board | Connected % M-B1 | Connected % M-B2 | Engine violations M-B1 → M-B2 (connections) | shorts | Runs cut at 600 s |
|---|---|---|---|---|---|
| eprom-to-kernal | 100 / 100 | 100 / 100 | 0 → 0 | 0 | 0 / 6 |
| wiccon-2023 | 100 / 100 | 100 / 100 [100–100] | 0 → 0 | 0 | 0 / 6 |
| dcsgonebadge | 92.4 / 91.0 [90.0–91.4] | 90.0 / 91.0 [90.5–92.4] | 236–385 (29–38) → 95–176 (21–25) | 108–177 → 25–74 | 6 → 0 / 6 |
| qmtech | 91.4 / 91.6 [90.9–93.3] | 91.8 / 91.8 [91.1–92.6] | 445–911 (49–58) → 139–280 (32–40) | 234–613 → 35–154 | 6 → 6 / 6 |

Per cycle the errors now fall over the first cycles (median over all runs, dcsgonebadge 56.5 → 49 → 47.5 → 45;
qmtech 94 → 77.5 → 80.5 → 77 → 75); before, the violations of the later cycles grew (dcsgonebadge median 240 → 446).
On dcsgonebadge the default run's 5 more unconnected items are GND pour joins after the refill (stub and zone
pieces), not signal connections: the engine routes 222 of 231 in every run. On qmtech about 40 % of what is left
is at pad exits (M-C). Still open: the crossing rules leave traces of net trees with branch points alone, and a
trace drawn by force can still run over other copper for a millimetre or two.

### Widths, pad exits and via room decided in the plan (M-C)

The planner now decides what the drawing used to find out the hard way (TopoR's way: decide while change is cheap):

- **Pad exits.** A trace next to its own pad needs half its width plus the clearance to the next pin (the drawing's
  packer model); before, the one-plane routing counted no room at all there and planned exits through the 0.2 mm
  gap between fine-pitch pins, which the drawing then clipped (engine test `topo_mc`).
- **Width in the one-plane routing.** Every pass carries a width: the net width, or the minimum width (KiCad minimum
  track width / custom-rule minimum) where the passage is a bottleneck or the pad exit is too tight (TopoR 6.3
  manual p. 56: the minimum width "in bottlenecks unless a different path is available or near pads that are
  narrower than the wire"). A narrowed stretch runs from the nearest pad up to the bottleneck (`[search]
  neck_from_pad`), the search pays the graded *necking* factor once per stretch (TopoR 6.3 pp. 72–73: shrinkages
  are a criterion), and the layer assignment, rip-up and drawing pack the planned widths. The drawing draws the
  full width where it fits and the planned minimum only inside the planned stretches; it no longer narrows on its
  own (`[search] neck_in_drawing`).
- **Via room.** A via the plan puts at a layer change takes its spot: another net's via planned in the same small
  triangle has no site, so layer assignment and rip-up move it or reroute its trace (`[vias] reserve_sites`;
  Polubasov 2001 via intervals).

Scorecard (same setup as M-B2; default / median [range] of seeds 1–5; violations = the engine's report):

| Board | Connected % M-B2 | Connected % M-C | Connections with violations M-B2 → M-C | at a pad exit (connections) | KiCad DRC new | Runs cut at 600 s |
|---|---|---|---|---|---|---|
| eprom-to-kernal | 100 / 100 | 100 / 100 | 0 → 0 | 0 → 0 | 0 | 0 / 6 |
| wiccon-2023 | 100 / 100 | 100 / 100 | 0 → 0 | 0 → 0 | 0 | 0 / 6 |
| dcsgonebadge | 90.0 / 91.0 [90.5–92.4] | 90.5 / 90.0 [89.0–91.0] | 21–25 → 19–23 | 12–15 → 6–14 | 250–525 → 228–596 | 0 → 1 / 6 |
| qmtech | 91.8 / 91.8 [91.1–92.6] | 91.4 / 91.8 [91.4–92.1] | 32–40 → 13–23 | 23–33 → 7–15 | 516–817 → 149–496 | 6 → 6 / 6 |

On qmtech the ADV7511 → FPGA header traces now leave their pins straight out (pins 57–68 without a clip in the
default run). Still open: traces from neighbouring 0.5 mm pins that all turn to 45° right at the row's tips run
closer than their clearance allows (the planner compares passes edge by edge; mechfix `fx_fine_pitch_exit`, xfail),
a via inside a pin ring (qmtech ADV_DVDD → C22 under the chip) is not preferred yet (the connection routes legally
with its via outside the ring), and dcsgonebadge's GND pour joins after the refill still vary with the copper.

## Requirements

- Linux (x86_64), **KiCad 10.0** with `kicad-cli` (tested 10.0.8)
- **Python ≥ 3.11** as KiCad's plugin interpreter (normally `/usr/bin/python3`), with the system wxPython 4.2
- Rust toolchain (stable, tested 1.96) — <https://rustup.rs> — to build the engine wheel
- For development: the system `pcbnew` Python module (harness and tests only, never the plugin)

## User guide

1. [Install](docs/user/install.md) — build, install into KiCad, enable the KiCad API, plugin environment.
2. [Operate](docs/user/operate.md) — first run, findings, options, scope by selection, progress, preview,
   Accept / Reject / undo, report folder, priorities, limitations.
3. [Configuration reference](docs/user/configuration.md) — every `topor.toml` key (generated).
4. [Troubleshooting](docs/user/troubleshoot.md) — one section per message code, exit codes, install problems.
5. [GUI test checklist](docs/user/gui-test-checklist.md) — the manual acceptance test in KiCad.

## Quick start

```bash
git clone git@github.com:ranzbak/ranz-topor.git topor && cd topor
tools/dev-venv.sh          # creates .venv (maturin, pytest; sees the system pcbnew)
tools/build.sh             # builds the engine wheel and installs the plugin into KiCad
```

Then KiCad → **Preferences → Plugins** → tick **Enable KiCad API**, open a **copy** of a board in the PCB
Editor (KiCad creates the plugin environment the first time) and click **Topor: route**. Details:
[install.md](docs/user/install.md).

## Route a board from the command line

The harness never touches your project: it routes a **snapshot copy**.

```bash
# 1. snapshot the source boards listed in harness/paths.py into scratch/boards/<board>/<sha12>/
.venv/bin/python -m harness.snapshot
# 2. rip the copy's tracks, route it with topor, run KiCad DRC, write metrics + images
.venv/bin/python -m harness.bench eprom-to-kernal /tmp/topor-run --router topor --threads 4
```

The output directory holds the routed board in `<board>-work/<board>.kicad_pcb` (open it in KiCad to
inspect), DRC json files (original, ripped, routed), `<board>--metrics.json` (routed, vias, length,
detours, DRC) and comparison PNGs (hand vs topor). More tools (engine probes, headless plugin runs,
reproducing a plugin run): [docs/dev/debug.md](docs/dev/debug.md).

The plugin's own run flow, headless, on a board copy:

```bash
tools/route_board.sh <board|pcb> <out_dir> [--timeout S] [--set section.key=value]
```

It strips the copy's tracks, routes it, runs KiCad DRC after a zone refill and writes `<name>--metrics.json` and
`<name>-compare.png`: the hand-routed board left, topor right, each with its open ratsnest (KiCad's DRC
unconnected items) drawn in blue. Details: [docs/dev/route-board.md](docs/dev/route-board.md).

## Development

[CONTRIBUTING.md](CONTRIBUTING.md): layout, build, tests, read-only boards, configuration and message
catalogues, commit style. Benchmarks and progress images are generated locally into `docs/benchmarks/`
(`harness.bench`, `harness.progress`; not in git) — key numbers are quoted in `docs/progress/STATE.md`.
Design and research library: [docs/](docs/README.md).

Routing mechanisms found on the ladder boards get a tiny synthetic regression layout each
(`harness/mechfix.py`, one function per mechanism, built with pcbnew in seconds), routed end to end with
KiCad DRC by `tests/test_mechfix.py`; they carry the `mechfix` marker and are not in the default run.

```bash
.venv/bin/pytest                                   # unit + integration (default)
.venv/bin/pytest tests/test_mechfix.py -m mechfix  # mechanism layouts, end to end (~2 min)
.venv/bin/pytest tests/gate -m "gate or not gate"  # milestone gates (~10 min)
.venv/bin/pytest tests/scaling -m scaling          # complexity/scaling budget
(cd engine && cargo test --release && cargo clippy --all-targets -- -D warnings)
```

## Repository layout

| Path | What |
|---|---|
| `engine/` | Rust engine: geometry kernel, triangulation, topological search, negotiation, smoothing, checker |
| `plugin/` | KiCad IPC plugin: `plugin.json`, entry point, `topor/` package (flow, dialogs, pre-flight, config), `wheels/`, `requirements.txt` |
| `toporlib/` | pure-Python code shared by plugin and harness (DRC, rules, DRC loop, board-file edits) |
| `harness/` | dev/test only: board snapshot/copy, rip, model export, rules, writer, DRC, render, bench |
| `tools/` | `build.sh`, `dev-venv.sh`, `bench.sh`, `render.sh` |
| `tests/` | pytest suites (unit, `plugin/`, `gate/`, `scaling/`) |
| `docs/` | user guide, developer guide, research library, design, plans, progress notes, benchmarks |

## License

topor is free software, licensed under the **GNU General Public License, version 2 or (at your
option) any later version** (SPDX: `GPL-2.0-or-later`); see [LICENSE](LICENSE).
Copyright (C) 2026 Paul Honig and contributors. This license allows reusing code from other GPL
routers such as the gEDA PCB toporouter (GPL-2.0-or-later); keep their copyright notices when doing so.
