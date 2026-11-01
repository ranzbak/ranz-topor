# Task A7 report: DRC-feasible detour reference (M4-D4, M3-R15)

Status: DONE_WITH_CONCERNS. Commit f7983f0 on branch worktree-agent-a8d6a6f956ec48585 (base 68a77b6; cba0e2a is an ancestor).

## What changed
- `engine/src/model.rs`: `Options.detour_reference: bool = false`, `detour_reference_budget_pct: f64 = 10.0`.
- `engine/src/route.rs`: after the ghost block, with the option on, one `search()` per live connection on the same
  empty `MultiGraph` (fixed copper only; same layers, via rules, `via_cost`, `max_vias`, expansion cap), `par_iter`
  collected in index order, under its own deadline = now + pct % of the time left to the run deadline (`wall_clock_s
  0` = run deadline). Cancel checked per connection. `ConnReport.ref_len_nm: Option<i64>` (search `est_len`; ghost
  length on fallback) and `ref_kind: "feasible" | "ghost" | "none"`. O(m * search); each search is capped by the
  expansion cap and the deadline.
- `toporlib/detour.py`: `detour_stats(connections, field="ghost_len_nm")`; with `field="ref_len_nm"` it adds `field`,
  `ghost_fallbacks`, `short_ref` (reference < 2 mm) and per-row `ref_kind`, `short_ref`; `detour_line` appends
  "(ghost fallbacks N)" for the reference. Default field stays the ghost (old callers/tests unchanged).
- `harness/bench.py`: sets `detour_reference: true` by default (offline), `result["detour"]` = reference stats,
  `result["detour_ghost"]` = ghost stats; summary prints both. `harness/progress.py`: caption says "vs DRC-feasible
  reference (ghost fallbacks N, ghost < 2 mm M)"; PNG rings use the reference ratio.
- Plugin: `[run] detour_reference` is now `engine="detour_reference"` (No-effect tail removed; configuration.md
  regenerated, `--check` clean), `run.build_options` passes `detour_reference_budget_pct = 10.0` when on
  (`DETOUR_REFERENCE_BUDGET_PCT`, ENGINE_INTERNAL reason updated), the report shows the reference line and
  "Detours vs ghost line". operate.md paragraph rewritten; test_config_docs updated (pour_served keeps the marker).
- Tests: `engine/tests/route_detour_ref.rs` (4 tests), `test_detour_stats_uses_reference_and_reports_ghost`.

## Judgement calls (Options / settings)
`detour_reference` (setting, default true in plugin, false in engine), `detour_reference_budget_pct` (engine-internal 10).
Default field of `detour_stats` kept at the ghost for back-compat. Bench "columns" (A0) are not in this base: no column wiring done.

## Tests
- cargo `--test route_detour_ref`: 4/4 (reference_is_feasible_not_topological, off_leaves_fields_none,
  budget_exhausted_falls_back_to_ghost, threads_byte_identical incl. on-vs-off minus ref fields and threads 1/2/4/1).
- Full `cargo test --release --no-fail-fast`: all green except timing tests under load average ~21
  (`route_control::route_async_cancel...`, `route_engine::anytime_timeout...`), both pass when run alone.
  clippy `-D warnings` clean. Full `pytest -q tests`: 451 passed, 7 skipped, 1 failed
  (`test_scaffold::test_cargo_workspace_builds`, which shells out to cargo test and hit the same anytime timing flake).
- Identity (bench route JSON minus ref fields/runtime_s): eprom and wiccon, ref on vs off identical; threads 1/2/4 identical with ref on.

## Concern: the #208 test is synthetic and does not show ref 2.9 / ghost 1.0
The slot search accepts overuse (unfit edges are crossed at a cost), so its `est_len` is not always >= the ghost (my
synthetic USB row: ghost 1.70 mm, ref 1.46 mm, committed 2.21 mm) and it inherits the state_point / clearance_bend
runaway of M4-D2 (#208 est 24.9 mm on the empty graph; A2 is the fix). `reference_is_feasible_not_topological`
therefore asserts: kind feasible, ref != ghost (independent search), ref >= pad distance; the 2.9 / 1.0 figures on the
real board need A2 and a dcsgone run. The reference is the search's estimated length (funnel), not a metrized path.

## Numbers (threads 4, machine load average ~20, so timings are rough)
| board | routed | detour (ref) median / p90 / max / >1.5 | ghost median / p90 / max / >1.5 | fallbacks | ref < 2 mm |
|---|---|---|---|---|---|
| eprom bfaf705c0f25 | 27/27 | 1.0 / 1.006 / 1.147 / 0 | 1.0 / 1.03 / 1.312 / 0 | 0 | 0 |
| wiccon 2d3063e70ed5 | 62/62 | 1.0 / 1.079 / 1.434 / 0 | 1.0 / 1.088 / 1.449 / 0 | 0 | 2 |
| dcsgone 3e7e66f862a7 (60 s, 86/231 routed, timeout) | 86/231 | 1.0 / 1.0 / 1.83 / 2 (n 86) | 1.0 / 1.024 / 2.711 / 3 | 36 of the 86 routed (120 of 231) | 34 |

Top 5 by reference / by ghost: eprom ref A11 1.147, +5V 1.024, +5V 1.007, A12 1.006, A7 1.0; ghost A11 1.312, +5V 1.072, A14 1.038, A12 1.024, A13 1.017.
wiccon ref RST 1.434, MOSI 1.256, GPIO0 1.244, GPIO16 1.241, 3.3v 1.145; ghost RST 1.449, GPIO0 1.323, MOSI 1.243, GPIO16 1.233, 3.3v 1.145.
dcsgone ref GND#63 1.83 (ghost fb), GND#71 1.767 (ghost fb), Net-(D24-K)#143 1.155, VCC#6 1.041, Net-(D20-K)#73 1.01;
ghost USB-#204 2.711, GND#63 1.83, GND#71 1.767, Net-(D24-K) 1.155, Net-(Q1-G) 1.113. (The gate-option 600 s dcsgone run was not made: A12 owns completion; only the 60 s default run.)

Reference time share (wiccon, 2 runs each): router_s 8.53 / 8.52 off vs 9.44 / 9.64 on, about +1.0 s (11 %), within the 10 % share plus noise;
eprom 0.24 -> 0.37 s. dcsgone: the 10 % share (6 s) is not enough, 120/231 fall back to the ghost; with the share at 100 % the reference ate the
whole 60 s budget (217 feasible, 14 ghost, nothing routed). The search speed (A1) decides this; the budget cap and fallback behave as designed.
Note for the plugin: wiccon `router_s < 10.0` is tight with the reference on (about +1 s); the plugin default is on per the coordinator ruling.

## PNG (not committed)
/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-a8d6a6f956ec48585/docs/benchmarks/progress/2026-11-01-m4-A7-dcsgonebadge.png
Caption: A7: 86/231 connections, 10/66 nets, partial 0, failed 145, vias 54, arcs 16, zero slack 0, iters 1, pcb 3e7e66f862a7;
DRC routing 0 preexisting 4, unconnected 143, outside pads 181.0 vs hand 3574.7 mm, router 60.38 s on 4 threads;
detours > 1.5: 1 (max 2.7) — USB- 2.7, Net-(D24-K) 1.2, Net-(Q1-G) 1.1 vs DRC-feasible reference (ghost fallbacks 36, ghost < 2 mm 33); stop_reason: timeout.
(The caption's top-3 are drawn from the stats of the run; 86/231 is the loaded-machine 60 s result, not an A7 effect.)
