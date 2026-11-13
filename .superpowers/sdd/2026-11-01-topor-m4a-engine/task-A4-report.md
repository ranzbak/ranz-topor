# Task A4 report: own-net / same-net capacity, wall-aware (M4-D1)

Status: **DONE_WITH_CONCERNS**. The capacity rule is in. All guards are green. private-badge-b failures drop from 67 to 52
(the diagnosis expected 25–35 fewer, we got 15 fewer). The gate is not reached; A4 alone was not expected to reach it.
The rest of the failures are the classes of other tasks, see the per-mechanism table below.

Worktree base: main `cba0e2a` (fast-forwarded from `68a77b6`). Machine load during the runs: 23–34 on 8 cores, so
wall times are inflated 2–3×. The counts are exact and deterministic.

## What changed, per file

- `engine/src/graph.rs`
  - `RouteGraph::fits_for(e, net, w, c)`, `fits_for_at`, `fits_for_with(.., wall)` and `fits_empty_with` (the static
    fit), plus `fit_parts` / `fit_parts_empty` (`FitParts {lo, hi, need, wall, c_wall}`). This is the §1 rule:
    - `E_v = 0` for an own end or a free end.
    - `K_v` credit at an own end.
    - `g(x) = 0` between strands of the same net.
    - An own end with a foreign strand on the edge keeps its gap.
    - An edge with no own end and no same-net strand is evaluated with the literal M3 `fits_at` code, so it is
      bit-identical. A test checks this.
  - `overuse(e)` uses the same per-slot accounting. Same-net neighbours charge 0, and own ends charge 0 plus a credit.
    Without the case it falls back to the literal M3 code (`overuse_m3`).
  - Wall term `wall_free_len(e, net, c, lo, hi)` returns the longest free piece of `[lo, hi]`. It uses the walls of
    `walls_near[e]` (CSR layout, static, shared via `Arc`; build `O(E·q)`). It skips walls ending at `a`/`b`, same-net
    and tie walls, and DRC spots that do not list the net. Each wall's blocked set is the stadium/disc interval
    (`stadium_interval`). Arcs are split into chords with the sagitta added.
  - New `transit_fits(f, e1, e2, net, w, c, eps)`, the transit pinch (see deviation 3).
  - `strand_net` per strand id. `insert_path_net` records it, `remove_conn` forgets it. `UNKNOWN_NET` (plain
    `insert`) is always treated as foreign.
  - `CapacityMode {own_net, wall_aware, transit}` field `cap_mode`.
  - `GEdge.len`.
- `engine/src/search.rs`
  - `Search::fits` / `hard` / `soft_blocked` / `step_cost` now use the net-aware rule. The wall term is memoised per
    search (`wall_memo`).
  - `push_state` refuses a face transit that `transit_fits` rejects (memo `transit_memo`).
  - `revalidate_path` uses `fits_for` plus `transit_fits`, so it stays conservative.
- `engine/src/multigraph.rs`: `insert_path` records the connection net (`insert_path_net`); new `set_capacity_mode`.
- `engine/src/route.rs`
  - Sets the capacity mode from `Options`.
  - The zero-slack retry also tries connections that failed (not permanently), ahead of the length candidates
    (`Options.zero_slack_failed`, see deviation 4).
- `engine/src/obstacle.rs`: `LayerObstacles::query_grow()` (the set's cell). The wall lists need it.
- `engine/src/model.rs`: new options `expert.own_net_capacity`, `expert.wall_aware_capacity`, `expert.transit_pinch`
  and top-level `zero_slack_failed`. All default to true.
- `plugin/topor/schema.py`: the four settings, `since=V5` (`[search] zero_slack_failed`, `[expert] own_net_capacity`,
  `wall_aware_capacity`, `transit_pinch`). `docs/user/configuration.md` was regenerated (`docs_gen --write`).
- `engine/examples/gate_diag.rs`
  - Applies the capacity mode.
  - Prints `fits_for` instead of the net-blind `fits`.
  - Copies `route()`'s rule of no via sites on a single routable layer without a pour target. Without it, the
    front-only replay was wrong.
- Tests
  - New `engine/tests/graph_ownnet.rs` (7):
    - `own_tip_edge_fits_own_net` (row 1, U3.1: 0.0/0.905 → 0.6/0.505; long side 1.325).
    - `two_same_net_strands_share_one_tip_edge` (rows 3/4: 0.625 / 0.41; overuse 0 vs M3 > 0; a foreign strand
      cancels the credit).
    - `own_end_edge_against_outline_wall_stays_unfit` (row 7, see deviation 2).
    - `foreign_edges_keep_the_m3_rule` (bit-identical fit and overuse).
    - `fine_pitch_tip_fits_between_neighbours` (U4 tip at 0.5 mm pitch: fits at 0.2 mm, not at 0.5 mm).
    - `transit_between_pad_and_outline_is_pinched`.
    - `front_only_plus5v_needs_zero_slack_for_failed`.
  - `engine/tests/search_layers.rs`: `ghost_is_never_stricter_than_search` case (1) flips to "fits, ghost not soft"
    (net-blind `fits` stays false; `fits_for` own net is true and foreign net false; the ghost has no soft element).
  - `tests/test_engine_surface.py`: the pinned default dict gets the new keys. This is a spec change, not a relaxed
    bound.

## Deviations from the design note §1 (with numbers)

1. **`K_v = min(r_v, w_min/2)` instead of `r_v`.** The strand's centreline must cross the segment `ab`, so its copper
   reaches at most `w/2` past an own vertex.
   - The numbers are identical on every design row, because there `r ≤ w/2`.
   - THT and via ends differ: U2.28 with r 0.8 gets 0.2 instead of 0.8.
2. **The wall term is measured in copper terms: blocked radius `r_W + clr(W)`, not `r_W + clr + w/2 + ε`.** The note
   compares a centreline-blocked set with the copper demand `need = w + ε`, which counts the width twice.
   - Measured on private-badge-b: the U4 tip `e1249` (0.25 mm pad, 0.5 mm pitch, 0.275 mm to each neighbour) came out unfit
     at 0.2 mm (free 0.14 < 0.205).
   - In copper terms it fits (`fine_pitch_tip_fits_between_neighbours`).
   - The row-7 test uses an outline notch whose apex is 0.1 mm below two same-net pads. The edge is M3-fit by accident
     (cap 1.0 ≥ 0.655). It fits without the wall term (usable 1.4). With the wall term the free piece is
     0.2101 < 0.255, so it is unfit.
   - The wall term applies whenever an own end gives its gap back (`E_v = 0`), not only when `K > 0`. Rect pads
     have r 0 but still drop the gap.
3. **The wall term does not protect the M2 guard; a transit pinch was added.** The F6 failure repeated exactly:
   `eprom_ripped_front_only_is_the_m2_result` went to 26/27 with "wall edge grazed mid-span by 455000 nm", with or
   without the wall term.
   - I logged the failing leg (`route.rs` debug, removed). +5V U2.28→J1.2 goes around via 816a. That via sits 0.25 mm
     of copper from the right board edge.
   - The crossed edges are via→outline corner (5 mm long) and U1.21→via. They are foreign-foreign, M3-fit and legal
     in the vertex-only model. The pinch is between the via and the outline interior, which no edge has.
   - The new `transit_fits` checks, for a move from crossing `e1` to crossing `e2` of face `f`: a foreign wall inside
     the wedge of `f` at the shared vertex `o`, closer than `r_o + gap_o + w + ε + r_W + clr(W)`, refuses the move.
     - The check is skipped for an own `o` and for walls through `o`'s centre.
     - It is static and memoised per search. `revalidate_path` applies it too.
   - It is a refusal, not a price. Pricing it as overuse left the result identical (26/27). The geometry cannot
     realise such a transit, and the margin-0 zero-slack search re-tests at its own margin.
   - Option `[expert] transit_pinch` (true).
4. **The zero-slack retry also takes failed connections (`[search] zero_slack_failed`, true).** Even with deviation 3
   the front-only run stayed 26/27, now failing at "pad:U1:23 <-> pad:U2:26".
   - Per-iteration profile: the M3 run and this run are identical up to iteration 4. Then they diverge, because M3
     charged overuse on +5V's own tips.
   - M3 reached 27 ok only at iteration 7, with +5V routed as a 140.7 mm detour, which made it a zero-slack
     candidate. That was luck. The M3 retry only ever considered routed connections.
   - Its only real corridor is the top-edge zero-slack corridor, 0.9 mm wide for a 0.905 mm need.
   - With failed connections as candidates (sorted first, `gain = MAX`) the guard is **27/27, 67.034 mm, +5V
     zero-slack**. That is identical to the M3 result. `front_only_plus5v_needs_zero_slack_for_failed` pins the
     setting: off gives 26.
5. **`terminal_open` is unchanged.** The note asks for a width fit there. The default search crosses unfit edges as
   overuse, so a static block on fit would mark the eprom +5V pinch "dead".
6. **Interface name.** The plan's `RouteGraph.wall_r: Vec<f64>` became `walls_near` (per edge, static) plus a
   per-search memo, as the note's later text says. The value depends on net, w and c, so it cannot be one per-edge
   number.

Judgement calls, all Options and schema Settings with defaults:
- `expert.own_net_capacity = true`
- `expert.wall_aware_capacity = true`
- `expert.transit_pinch = true`
- `zero_slack_failed = true`

With all four off the result equals M3: private-badge-b 188/255 with 199 vias, and wiccon 1373.553 mm with 37 vias, both
identical to main.

## Guards

- `eprom_ripped_front_only_is_the_m2_result`: **green**, 27/27, 67.034 mm, "+5V" zero-slack. This is identical to
  M3 (route_probe and cargo test).
- eprom two-layer: **byte-identical numbers**, 27/27, 0 vias, 64.628 mm outside, 108.096 mm total, 1 iteration,
  converged. `eprom_ripped_two_layers_routes_all_without_zero_slack` passes, including its own repeat byte-identity.
- wiccon-2023 (harness bench, threads 4), baseline main → A4:
  - routed 62/62 → **62/62**
  - vias 37 → **36**. The bound is ≤ 37. The plan expected ≤ 33, which is not reached.
  - outside 1373.553 → **1365.473 mm**
  - routing_new 0 → 0, unconnected 0 → 0
  - router_s 9.87 → 4.47 (loaded)
  - iterations 20 → 14
  - detour max 1.449 → 1.282, count > 1.5: 0
  - Ablation: own_net alone gives all of the gain; the wall term and the transit pinch change nothing on wiccon.
- Determinism: wiccon route_probe threads 1/2/4 plus a repeat give byte-identical dumps (md5 `f51fe1e9…`). The
  `route_threads` tests are green. The private-badge-b PNG run reproduced the gate bench exactly (190/242, 190 vias).

## private-badge-b, gate options (A-C3: wall 600, partial false, threads 4, rip all), `3e7e66f862a7`

| | main (baseline, this machine) | A4 (harness gate) | A4 (route_probe, same model) |
|---|---|---|---|
| routed / connections_total | 188 / 255 | **190 / 242** | 195 / 245 |
| first-pass connections | 231 | 231 | 231 |
| failed | 67 | **52** | 50 |
| vias | 199 | 190 | 186 |
| routing_new / preexisting | 0 / 4 | 0 / 4 | — |
| unconnected_items | 62 (via_dangling 4) | 51 (track_dangling 1, via_dangling 3) | — |
| iterations / stop_reason | 42 / no gain | 28 / no gain | 25 / no gain |
| router_s / runtime_s | 368.7 / 572.2 | 526.2 / 704.5 (DRC loop 4 rounds, load ~25) | 160.2 wall |
| outside pads | 1725.8 mm | 1522.4 mm | 1551.0 mm |
| detour median / p90 / max / count > 1.5 | 1.003 / 1.523 / 11.62 / 20 | 1.003 / 1.507 / **3.52** / 20 | — |
| 22 locked edge-GND tracks kept | yes | yes | — |

- Harness and probe differ because the harness ran 4 DRC-loop rounds (DRC spots, re-routes). The base run needed 1.
- runtime_s 704 is above 600 on this machine at load ~25. The M3-equivalent probe on the same load took 287 s and
  A4's took 160 s, so on equal load A4 is faster.
- Top 5 detours (A4 gate):
  1. #223 VCC D12.1→D11.1 69.85 / 19.83 = 3.52
  2. #30 VCC U3.1→U3.3 8.07 / 2.67 = 3.03 (was #32, 31.0 mm / 11.6×, the M4-D1 showcase)
  3. #228 GND J1.A12→pour 2.62
  4. #233 Net-(IC1-VCC) C31.1→R17.2 2.42
  5. #102 +3V3 R1.1→R16.2 2.14

Per iteration, pass 1, from `Options.profile` (route_probe):

| iter | metrize_failed main | metrize_failed A4 | ok main | ok A4 | overuse edges main | overuse edges A4 | re-searched main | re-searched A4 |
|---|---|---|---|---|---|---|---|---|
| 1 | 38 | 41 | 170 | 169 | 315 | 42 | 132 | 112 |
| 2 | 41 | 32 | 171 | 176 | 364 | 37 | 132 | 38 |
| 3 | 39 | 34 | 172 | 174 | 435 | 37 | 131 | 31 |
| 4 | 34 | 22 | 175 | 183 | 386 | 29 | 141 | 30 |
| 5 | 49 | 21 | 166 | 186 | 401 | 45 | 141 | 30 |
| 6 | 46 | 20 | 163 | **187** | 403 | 45 | 150 | 24 |
| 7 | 43 | 30 | 165 | 177 | 432 | 51 | 146 | 28 |
| 8 | 53 | 25 | 157 | 184 | 475 | 35 | 150 | 28 |
| 9 | 49 | 25 | 156 | 182 | 444 | 34 | 146 | 23 |
| 10–11 | — | 25, 26 | — | 183, 181 | — | 37, 46 | — | 34, 30 |

- Search and metrization agree much better. metrize_failed is 20–26 from iteration 4 on (main: 34–53). The ok count
  no longer degrades after its best iteration (main fell 175 → 156). Overused edges dropped from 315–475 to 29–51.
- Search cost: 1945 searches at 118 ms (main: 4161 at 66 ms, same load). That is 210 k candidates per search
  against 155 k. Each search is about 1.8× more expensive, but there are far fewer re-searches, so total search and
  commit time went from 222 s to 110 s.

DX classification re-run (`classify.py` on the route_probe dump; mechanism letters as in the diagnosis), DX/main → A4:

| class | DX / main | A4 |
|---|---|---|
| b own-pad / fine-pitch capacity, 0.2 mm | 11 (+ 2 b/g) | 13 |
| b fine pitch, 0.5 mm net (needs neck, NX) | 12 | 8 |
| i taut corridor pinch | 11 | 3 |
| i wall grazed | 7 | 3 |
| i NPTH hole clearance (HC) | 6 | 4 |
| i via-site pair pinch | 4 | 7 |
| i sketch | 4 | 4 |
| i self-check vs item | 3 | 1 |
| i inside obstacle / pass cap | 1 / 1 | 0 / 0 |
| g pour completion | 5 | 1 |
| e dynamic (others present) | 2 | 6 |
| **total** | **67** | **50** |

The remaining (b) 0.2 mm cases are not capacity any more:
- **#77 U4.2→U4.4:** the empty-graph search alone finds a 4-via 4.6 mm path while the ghost finds a direct 1.59 mm
  path with no soft element. That is the A2 state-point / los defect. With the others present, the same-net sibling
  #78 holds the only tip (0.205 + 0.205 > 0.25, a correct refusal). That needs an A5 T-junction.
- **#201 U7.4:** blocked on the empty graph, the A2 class.
- **J3 / U4 0.5 mm nets:** need NX necks.

## Tests

- `cargo test --release`: all green except `anytime_timeout_emits_committed_subset`. That test has a 0.5 s
  wall-clock budget on a 2000-pair synth board and is timing-flaky.
  - Alone it passes 3/3 on A4. It passes in debug too (6.5 s).
  - Run with its whole binary, main (`git archive main` into `target/a4/main-src`, own target dir) failed 1 of 3
    concurrent runs and also failed with `--test-threads=1`. A4 failed 5 of 6 concurrent runs.
  - It is not slower search. The temporary synth timing test (not committed) gave identical first-iteration
    profiles: search 0.16 s, commit 0.10 s, metrize 0.24 s, 552 re-searched. The whole route took 2.4 s on A4 and
    2.7 s on main, with 1990 routed against 1989 (the extra one is from `zero_slack_failed`).
  - I parallelised the `walls_near` build (rayon, edge order kept). It costs 0.03 s per layer on that board. The
    test is still flaky; this is a concern for the coordinator.
- `route_async_cancel_returns_anytime_result` failed once under load 25 and passed on re-run.
- `cargo clippy --all-targets -- -D warnings`: clean. `rustfmt` was applied only to the new test file; I reverted my
  accidental crate-wide `cargo fmt` hunks.
- `pytest -q` (PYTHONPATH=target/pyeng): 470 passed, 7 skipped, 26 deselected, 1 failed.
  - The failure is `test_scaffold.py::test_cargo_workspace_builds`. It runs the debug `cargo test` concurrently,
    which hits the same anytime flake.
  - In the first run, `test_engine_surface.py::test_default_options_lists_every_field` (pins the default dict) also
    failed. I updated it and it passes 15/15.
- Config docs tests: 11/11 pass. Determinism after the change: the wiccon dumps for threads 1/4 are byte-identical
  (md5 `f51fe1e9…`, unchanged).

## PNGs (docs/benchmarks, not committed)

- private-badge-b: `/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-a0f172ccd5405dc85/docs/benchmarks/progress/2026-11-01-m4-A4-private-badge-b.png`
  - 190/242 connections, 40/66 nets, partial 0, failed 52, vias 190, arcs 171, iters 28, pcb `3e7e66f862a7`.
  - routing_new 0, preexisting 4, unconnected 51, outside 1522.4 vs hand 3574.7 mm, router 466.5 s on 4 threads.
  - Detours > 1.5: 20 (max 3.5): VCC 3.5, VCC 3.0, GND 2.6. stop_reason no gain.
- wiccon-2023: `/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-a0f172ccd5405dc85/docs/benchmarks/progress/2026-11-01-m4-A4-wiccon-2023.png`
  - 62/62, 27/27 nets, vias 36, arcs 69, iters 14, pcb `2d3063e70ed5`.
  - routing_new 0, preexisting 0, unconnected 0, outside 1365.5 vs hand 1690.1 mm, router 3.62 s.
  - Detours > 1.5: 0 (max 1.3): RST 1.3, GPIO16 1.2, MOSI 1.2.

## Concerns

1. The private-badge-b gain is 15 fewer failures in the gate run, not the estimated 25–35. The remaining capacity-looking
   failures are A2 (state point / los), A5 (sibling holds the single tip) and NX (0.5 mm necks). One real gap remains:
   the per-edge capacity is a 1D cross-section, so oblique pinches are caught only by the transit check.
2. Two additions are beyond the design note: the transit pinch in `search.rs` `push_state` (refusal) and
   `zero_slack_failed` in `route.rs`. They were needed for the M2 guard, and both are switchable. They overlap A2
   (search.rs) and A7 (route.rs) file-wise, but not function-wise.
3. The private-badge-b gate runtime_s was 704 s at load ~25. That is a DRC loop of 4 rounds at 526 s router time. On equal
   load the route itself is faster than main (160 s vs 287 s probe).
4. Each search costs about 1.8× more (210 k vs 155 k clearance candidates per search). Fewer searches more than
   offset it. A1 or A2 may want to look at it.
5. wiccon vias are 36, not the plan's ≤ 33. That is within the ≤ 37 bound.
6. `anytime_timeout_emits_committed_subset` (0.5 s budget) is flaky on main, but fails more often with A4 when run
   inside its test binary (5/6 against 1/3). The synth board shows no first-iteration slowdown, so the cause is
   not identified.
