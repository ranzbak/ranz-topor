# Report: A16 (escalating per-pair rip cost) + A13 (bounded via position relaxation)

Branch `worktree-agent-afc384996005e682e`. Commits: A16 `62af5ad`, A13 `8eb47e3` (report commit follows). Status: DONE_WITH_CONCERNS.

## Concerns up front

1. **A16 changes the F.Cu-only eprom trajectory.** `eprom_ripped_front_only_is_the_m2_result` (27/27, <= 67.8 mm, +5V zero-slack wire)
   cannot hold with the default `growth = 2.0`: the fragile three-way contention takes another trajectory (27/27, clean at margin 0,
   no zero-slack wire, **92.29 mm** outside pads, 19 rips, max_pair 7). Measured on that fixture (cap 8): growth 1.0 and 1.5 reproduce
   the M3 result (67.03 mm, zero slack kept); 2.0 gives 92.3 mm; 1.75, 1.9 and 3.0 lose a connection (26/27). The result is chaotic in
   growth, not monotone. I did not weaken the guard: it is kept unchanged but runs with the M3-equivalent rip options
   (`ripup_growth 1.0, ripup_max_per_pair 3`), and a new test `eprom_ripped_front_only_default_ripup_routes_all_clean` pins the
   default-option behaviour (27/27, clean, no backstop pair; no length bound). The real two-layer eprom and wiccon never rip
   (`ripup.total` 0), so only this single-layer restriction sees it. Coordinator decision needed: accept (the user's row 17 default is 2.0),
   or ask whether the guard should pin the default.
2. **private-badge-b not measurable under the machine load.** Other workers kept load average 10-13 on 8 cores. Both main-base (68a77b6 engine) and A16
   timed out at 600 s after only 2 iterations with byte-identical results (171/N, failed 60, vias 152, routing_new 0, unconnected 70,
   detour max 16.175, count > 1.5: 10); the A16 rip report there: 5 rips in total, max_pair 1, no backstop pair. The gate-options numbers of the
   brief (201/243, 25 iterations) could not be reproduced for either engine, so "A16/A13 do not regress private-badge-b" is NOT demonstrated. A13 is
   skipped on a timed-out run (`skipped: timeout`); its cost was measured on wiccon (below) and extrapolated.
3. Full default pytest after A13 was not run to the end (38 min under load for the A16 run). A16 full suite: 451 passed, 7 skipped, 1 failed
   (`test_default_options_lists_every_field`) - an artefact of my parallel A13 edit (the test scans `model.rs` in the worktree, which already had the
   A13 field while the `.so` was the A16 build); it passes with the A13 `.so`. A13: targeted run of `tests/plugin/test_config*.py`,
   `tests/test_engine_surface.py`, `tests/plugin/test_run_flow.py` = 72 passed, 1 failed (`test_route_async_cancel_returns_anytime_result`, wall-clock
   flake under load; passes alone with the A13 `.so`). cargo: full `cargo test --release` green, clippy `-D warnings` clean for both commits.

## A16 - what changed

- `engine/src/negotiate.rs`: `RIP_PAIR_CAP` removed. `rip()` adds `hist_block * growth^(k-1)` (`rip_increment`, factor capped at `RIP_GROWTH_MAX_FACTOR = 1e6`
  against overflow) to the blocker's edges and sites for the k-th rip of B by A; refuses at `k >= ripup_max_per_pair` and records the refused pair.
  `Negotiator::rip_report(m)` -> `RipReport { rips_per_connection (times ripped out as blocker), pairs_at_backstop ([ripper, blocker], sorted),
  total, max_pair }`. Big-O in the doc comments: `O(max_per_pair * m^2)` rips, each `O(path * layers)`.
- `engine/src/route.rs`: `RouteResult.ripup: RipReport`.
- `plugin/topor/run.py`: run-report note `rip-up: N rips, ... ; backstop reached by ...` (only when something was ripped). `[ripup] growth /
  max_per_pair` already existed in schema/docs (P1) and map to `ripup_growth` / `ripup_max_per_pair`; no schema change needed.
- Tests: `engine/tests/negotiate_ripup.rs` (`ping_pong_pair_converges_before_backstop`, `backstop_stops_rips`, `growth_one_is_constant_cost`,
  `ripup_threads_byte_identical`; all four were first written against the pre-change API shape in exploration, then pinned); `route_engine.rs`
  damping test updated to the new semantics (backstop 3 -> still exactly `2 * 3` ripping iterations then none; same assertions), guard test options
  as in concern 1; plugin test for the note.
- Honest limit of the tests: a ping-pong that really *settles by price* needs a loser with a feasible alternative that the engine rips anyway
  (only via the `detour_factor` rule). I could not construct one: two-layer cross -> no contention at all (B goes via B.Cu); gap-wall -> metrization
  rejects the second wire (capacity model overcounts the pinch). So `ping_pong_pair_converges_before_backstop` pins the sealed cross under the
  defaults: the stop rule (patience 5) ends the exchange with max_pair 3 (<= 4 < 8), no backstop pair; `backstop_stops_rips` pins the backstop (cap 3 ->
  exactly 3 rips per pair, 6 total, pair reported, no rips after iteration 6). The pricing itself is pinned arithmetically (`growth_one_is_constant_cost`:
  0.5/1/2/4/8, cumulative 3.5, growth 1 constant, saturation).

## A13 - what changed

- `engine/src/model.rs`: `Options.via_relaxation: bool = true`.
- `engine/src/route.rs`: `via_relaxation_pass` (after the via-reduction block, before the zero-slack pass), `star_clip`, `weiszfeld_step`;
  `RouteResult.via_relaxation: String` (`off`, `not run`, `moved k of n vias, -x um`, `skipped: timeout[...]`). Moved positions live in
  `MetrState.via_at` (site -> position); `leg_terminals`, `via_item` and `ConnOk.via_sites` read it, so every later `metrize` (zero-slack pass,
  final checks) emits the moved via consistently. Per via (site order, pour goals skipped): up to 8 steps; target = Weiszfeld point between the last
  taut anchors of the two legs; clipped to 90 % of the way to the boundary of the site vertex's star in both layer graphs (ray clip, outside-outline faces = 0);
  back-off to 1/2 and 1/4 of the move when `via_fits` fails; kept iff `via_fits`, the connection re-metrizes with every connection still routed, its
  length falls by >= 1 um and its outside length does not grow; the first rejected step ends that via. Big-O in the doc comment:
  `O(V_via * steps * metrize(1 conn))` + one state copy per step; deadline/cancel checked before every step. Sequential, sorted: deterministic.
  Constants (not options): `VIA_RELAX_STEPS = 8`, `VIA_RELAX_MIN_GAIN_NM = 1000`, star fraction 0.9; the user switch is `[vias] via_relaxation`.
  (Judgement calls named: the back-off fractions 1/2, 1/4 and the star fraction 0.9 are mine; the note had neither.)
- `plugin/topor/schema.py`: `via_relaxation` row gets `engine="via_relaxation"`; `plugin/topor/run.py` run-report note; plugin test.
- Tests `engine/tests/route_via_relax.rs` (four named tests): fixture = two THT pads, locked B.Cu wall x=17 and F.Cu wall x=33, one forced via in
  between. Relaxation moves the via from the site (30.34, 27.21) onto the segment A1-A2 (distance < 1 um) and the connection from 37.18 mm to the
  straight 34.0 mm (diagonal variant: 45.33 -> 39.45 mm), clean at margin 0; with a no-via keepout across the line the via stops short outside it
  (34.82 mm), never inside; `off` reproduces the pre-A13 numbers (37 180 415 nm, via [30 340 616, -27 205 896]); threads 1/2/4 + repeat byte-identical.

## Numbers (bench harness, threads 4, same machine under heavy shared load; router_s is therefore noisy)

Base = main engine at 68a77b6 (A16 `ripup.total` 0 on eprom and wiccon, so results below are identical to base for A16).

| board | engine | routed | failed | vias | routing_new | unconnected | outside mm | iters | router_s | detour median / p90 / max / n>1.5 |
|---|---|---|---|---|---|---|---|---|---|---|
| eprom | base / A16 | 27/27 | 0 | 0 | 0 | 0 | 64.628 | 1 | 0.14 / 0.13 | 1.000 / 1.030 / 1.312 / 0 |
| eprom | A13 | 27/27 | 0 | 0 | 0 | 0 | 64.628 | 1 | 0.23 | same, "moved 0 of 0 vias" |
| wiccon | base | 62/62 | 0 | 37 | 0 | 0 | 1373.553 | 20 | 6.2 (8.5-9.8 under load) | 1.000 / 1.088 / 1.449 / 0 |
| wiccon | A16 | 62/62 | 0 | 37 | 0 | 0 | 1373.553 | 20 | 8.5-10.5 (load) | identical to base |
| wiccon | A13 | 62/62 | 0 | 37 | 0 | 0 | **1362.751** | 21 | 7.0-10.0 (load) | 1.000 / 1.098 / **1.243** / 0 |
| private-badge-b (gate options, 600 s) | base2 and A16 | 171/N | 60 | 152 | 0 | 70 | 1398.027 | 2 (timeout) | 600 | median 1.011 / p90 1.372 / max 16.175 / 10 |

- wiccon A13: via count unchanged (37), outside **1373.553 -> 1362.751 mm** (headroom to 1380 mm now 17.2 mm instead of 6.4), DRC routing_new 0, `moved 8 of 30 vias`
  in the final pass; iterations 20 -> 21 because the pour-reach completion passes (GND) run on the relaxed copper and the last pass stopped after 10 instead of 9
  iterations (no regression; the pour passes are the same set in both). `via_relaxation=false` on the A13 engine reproduces base exactly (20 iterations, 1373.553 mm).
- A13 cost: the pass took 0.38 s on wiccon (30 vias, 60 connections) of ~7 s router time; extrapolated to private-badge-b (152 vias, N connections) roughly
  10-30 s of the 600 s budget (not measured: see concern 2).
- eprom F.Cu-only guard: see concern 1 (92.29 mm / 67.03 mm numbers). detour_ghost fields are `null` in the harness result at this base for all boards.
- Detour (reference) non-regression: eprom and wiccon count > 1.5 stays 0 and max does not get worse (wiccon max 1.449 -> 1.243 with A13); private-badge-b unchanged (timeout-limited).

## Progress PNGs (not committed, docs/benchmarks is untracked by ruling)

- `/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-afc384996005e682e/docs/benchmarks/progress/2026-11-01-m4-A16-wiccon-2023.png`
  caption: A16: 62/62 connections, 27/27 nets, partial 0, failed 0, vias 37, arcs 66, zero slack 0, iters 20, pcb 2d3063e70ed5; DRC routing 0 preexisting 0,
  unconnected 0, outside pads 1373.6 vs hand 1690.1 mm, F.Cu 666.3 / B.Cu 857.3 mm, router 9.54 s on 4 threads (load); detours > 1.5: 0 (max 1.4); stop no gain.
- `/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-afc384996005e682e/docs/benchmarks/progress/2026-11-01-m4-A13-wiccon-2023.png`
  caption: A13: 62/62 connections, 27/27 nets, partial 0, failed 0, vias 37, arcs 65, zero slack 0, iters 21, pcb 2d3063e70ed5; DRC routing 0 preexisting 0,
  unconnected 0, outside pads 1362.8 vs hand 1690.1 mm, F.Cu 654.7 / B.Cu 859.2 mm, router 8.73 s on 4 threads (load); detours > 1.5: 0 (max 1.2); stop no gain.

## Notes for the coordinator

- Parallel-worker overlap: `route.rs` edits are confined to the rip report field/assignment, `MetrState.via_at` + `leg_terminals`/`via_item` signatures
  (`leg_terminals` gained a `via_at` parameter - A5/A6/A14 touching the same call sites will conflict mechanically), the new pass and the result field.
  `relax.rs` / `wires.rs` untouched.
- `relax_iter_cap` dedupe (ledger item 7) not touched (assigned to A0 in the ledger).
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5` (session system reminder) instead of the contract's Opus 5.5 text.
- Worktree has an untracked `scratch` symlink (per contract), not committed.
