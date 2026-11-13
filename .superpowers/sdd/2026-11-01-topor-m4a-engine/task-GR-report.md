# Task GR report: `[ripup] growth` default by measurement (Q1) + pour_cut_cost sweep

Status: DONE_WITH_CONCERNS. Base: main bde9602 (fast-forwarded the worktree, it was started at 68a77b6 which predates A16).

## Decision
Default `ripup_growth` = **1.25** (max_per_pair 8 unchanged). Rule applied: completion first, then routing_new, then detours > 1.5, then length/vias.

- Completion (sum of failed): identical for every growth on every board (private-badge-b 36, wedding-gift 12, others 0); private-badge-b at 1.5 shows 206/242 (one extra connection counted by the engine, same 36 failed).
- routing_new: identical for all values (private-badge-b 1, all others 0; the private-badge-b 1 is a base effect, not growth-dependent).
- Detours > 1.5 (private-badge-b): 1.0 and 1.25 = 8 (max 4.215); 1.5, 1.75, 2.0 = 9; 3.0 = 8 (max 4.215, 197 vias, rips 92). 1.0 and 1.25 are identical in every column.
- Hard constraints, F.Cu-only eprom guards (both tests use the same fixture and defaults): see Table 2. Growth 2.0 fails the guard (92.29 mm, no zero-slack wire) and `front_only_plus5v_needs_zero_slack_for_failed`; 1.0 to 1.75 and 3.0 pass.
- 1.25 over 1.0: same numbers everywhere, keeps the escalation the user asked for (row 17), and sits in the middle of the 1.0 to 1.75 plateau (not at its edge, not at an isolated value like 3.0).

## Changes
- engine/src/model.rs: `ripup_growth` default 1.25 (+ doc comment); engine/src/negotiate.rs: `NegCfg::default` rip_growth 1.25.
- plugin/topor/schema.py: `[ripup] growth` default 1.25; docs/design/46-configuration.md row 17; docs/user/configuration.md regenerated (`docs_gen --check` clean).
- engine/tests/route_engine.rs: `eprom_ripped_front_only_is_the_m2_result` now runs with the shipped defaults (the m3_ripup pin removed, bounds unchanged: 27/27, +5V zero-slack, <= 67.8 mm). `eprom_ripped_front_only_default_ripup_routes_all_clean` (asserted the 2.0 trajectory, now false) replaced by `eprom_ripped_front_only_default_ripup_is_on_the_plateau`: pins Options::default() growth 1.25 / max_per_pair 8 and 27/27, <= 67.8 mm, no backstop pair at growth 1.0, 1.25, 1.5.
- engine/tests/graph_ownnet.rs untouched: green at 1.25 by root cause (the +5V corridor trajectory of the 1.0 to 1.75 plateau needs the zero-slack wire; with failed candidates off it stays 26/27).
- tests/plugin/test_config.py, tests/test_engine_surface.py: default pins 2.0 -> 1.25; engine/tests/negotiate_ripup.rs comment.
- Judgement calls as options: none new (growth was already an option).

## Table 1: growth sweep (max_per_pair 8, threads 4, rip all, partial_routes False)
Machine load was 14 to 35 throughout, so every run used `wall_clock_s 0` (deterministic, no deadline) per the measuring rule; router_s values are inflated by load (wiccon quiet: 1.5 s, private-badge-b quiet base 120 s) and are NOT valid timings (load in brackets). Per-board detour values of eprom/wiccon/nametag are 0 > 1.5 except where listed.

| board | value | routed/total | failed | routing_new | unconn | detours>1.5 (ref) | ref max | outside mm | vias | router_s (load) | rips (max pair, backstop pairs) | stop | iters |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| private-badge-b | 1 | 205/241 | 36 | 1 | 38 | 8 | 4.215 | 1799.565 | 196 | 247.789 (19.04) | 82 (5, 0) | no gain | 29 |
| private-badge-b | 1.25 | 205/241 | 36 | 1 | 38 | 8 | 4.215 | 1799.565 | 196 | 285.097 (30.04) | 82 (5, 0) | no gain | 29 |
| private-badge-b | 1.5 | 206/242 | 36 | 1 | 39 | 9 | 4.215 | 1807.489 | 196 | 254.072 (32.8) | 85 (5, 0) | no gain | 27 |
| private-badge-b | 1.75 | 205/241 | 36 | 1 | 38 | 9 | 4.624 | 1814.469 | 196 | 406.573 (22.01) | 85 (5, 0) | no gain | 24 |
| private-badge-b | 2 | 205/241 | 36 | 1 | 38 | 9 | 4.624 | 1814.469 | 196 | 273.508 (32.76) | 95 (5, 0) | no gain | 24 |
| private-badge-b | 3 | 205/241 | 36 | 1 | 38 | 8 | 4.215 | 1799.584 | 197 | 183.464 (24.57) | 92 (4, 0) | no gain | 29 |
| eprom-to-kernal | 1 | 27/27 | 0 | 0 | 0 | 0 | 1.147 | 64.628 | 0 | 0.274 (17.74) | 0 (0, 0) | converged | 1 |
| eprom-to-kernal | 1.25 | 27/27 | 0 | 0 | 0 | 0 | 1.147 | 64.628 | 0 | 0.233 (32.18) | 0 (0, 0) | converged | 1 |
| eprom-to-kernal | 1.5 | 27/27 | 0 | 0 | 0 | 0 | 1.147 | 64.628 | 0 | 0.265 (33.04) | 0 (0, 0) | converged | 1 |
| eprom-to-kernal | 1.75 | 27/27 | 0 | 0 | 0 | 0 | 1.147 | 64.628 | 0 | 0.159 (34.96) | 0 (0, 0) | converged | 1 |
| eprom-to-kernal | 2 | 27/27 | 0 | 0 | 0 | 0 | 1.147 | 64.628 | 0 | 0.109 (30.74) | 0 (0, 0) | converged | 1 |
| eprom-to-kernal | 3 | 27/27 | 0 | 0 | 0 | 0 | 1.147 | 64.628 | 0 | 0.202 (30.04) | 0 (0, 0) | converged | 1 |
| nametag-sao | 1 | 34/34 | 0 | 0 | 0 | 0 | 1.159 | 989.164 | 3 | 1.773 (17.5) | 0 (0, 0) | no gain | 9 |
| nametag-sao | 1.25 | 34/34 | 0 | 0 | 0 | 0 | 1.159 | 989.164 | 3 | 1.916 (18.17) | 0 (0, 0) | no gain | 9 |
| nametag-sao | 1.5 | 34/34 | 0 | 0 | 0 | 0 | 1.159 | 989.164 | 3 | 3.908 (19.31) | 0 (0, 0) | no gain | 9 |
| nametag-sao | 1.75 | 34/34 | 0 | 0 | 0 | 0 | 1.159 | 989.164 | 3 | 4.145 (23.0) | 0 (0, 0) | no gain | 9 |
| nametag-sao | 2 | 34/34 | 0 | 0 | 0 | 0 | 1.159 | 989.164 | 3 | 2.592 (28.92) | 0 (0, 0) | no gain | 9 |
| nametag-sao | 3 | 34/34 | 0 | 0 | 0 | 0 | 1.159 | 989.164 | 3 | 1.79 (33.12) | 0 (0, 0) | no gain | 9 |
| wedding-gift | 1 | 77/89 | 12 | 0 | 11 | 1 | 1.502 | 586.56 | 54 | 21.644 (29.05) | 15 (3, 0) | no gain | 16 |
| wedding-gift | 1.25 | 77/89 | 12 | 0 | 11 | 1 | 1.502 | 586.56 | 54 | 17.371 (24.54) | 15 (3, 0) | no gain | 16 |
| wedding-gift | 1.5 | 77/89 | 12 | 0 | 11 | 1 | 1.502 | 586.56 | 54 | 16.005 (22.69) | 15 (3, 0) | no gain | 16 |
| wedding-gift | 1.75 | 77/89 | 12 | 0 | 11 | 1 | 1.502 | 586.56 | 54 | 18.997 (23.51) | 18 (3, 0) | no gain | 16 |
| wedding-gift | 2 | 77/89 | 12 | 0 | 11 | 1 | 1.502 | 586.56 | 54 | 12.379 (21.05) | 18 (3, 0) | no gain | 16 |
| wedding-gift | 3 | 77/89 | 12 | 0 | 11 | 1 | 1.502 | 586.56 | 54 | 14.454 (16.02) | 18 (3, 0) | no gain | 16 |
| wiccon-2023 | 1 | 62/62 | 0 | 0 | 0 | 0 | 1.282 | 1363.375 | 34 | 6.307 (23.08) | 0 (0, 0) | no gain | 16 |
| wiccon-2023 | 1.25 | 62/62 | 0 | 0 | 0 | 0 | 1.282 | 1363.375 | 34 | 7.039 (22.05) | 0 (0, 0) | no gain | 16 |
| wiccon-2023 | 1.5 | 62/62 | 0 | 0 | 0 | 0 | 1.282 | 1363.375 | 34 | 9.612 (30.81) | 0 (0, 0) | no gain | 16 |
| wiccon-2023 | 1.75 | 62/62 | 0 | 0 | 0 | 0 | 1.282 | 1363.375 | 34 | 10.131 (33.57) | 0 (0, 0) | no gain | 16 |
| wiccon-2023 | 2 | 62/62 | 0 | 0 | 0 | 0 | 1.282 | 1363.375 | 34 | 5.021 (29.63) | 0 (0, 0) | no gain | 16 |
| wiccon-2023 | 3 | 62/62 | 0 | 0 | 0 | 0 | 1.282 | 1363.375 | 34 | 6.673 (21.52) | 0 (0, 0) | no gain | 16 |

nametag-sao and wedding-gift show no dependence on growth (wedding-gift 77/89, 12 failed, routing_new 0, unconnected 11, 1 detour > 1.5 max 1.502, 54 vias, 586.56 mm). bornhack2026 not run (the load made a 600 s class run meaningless).

### F.Cu-only eprom guard fixture (Rust probe, max_per_pair 8, threads 1; not committed)
| growth | routed | outside mm | zero-slack wires | rips (max pair) | iters | +5V off-run routed (graph_ownnet expects 26) |
|---|---|---|---|---|---|---|
| 1.0 | 27 | 67.034 | +5V | 13 (4) | 7 | 26 |
| 1.25 | 27 | 67.034 | +5V | 13 (4) | 7 | 26 |
| 1.5 | 27 | 67.034 | +5V | 13 (4) | 7 | 26 |
| 1.75 | 27 | 67.034 | +5V | 13 (4) | 7 | 26 |
| 2.0 | 27 | 92.293 | none | 20 (7) | 12 | 27 (guard FAILS) |
| 3.0 | 27 | 67.095 | +5V | 13 (4) | 7 | 26 |

(A16's 26/27 at 1.75 and 3.0 was with the earlier cap/engine state; with the current engine and cap 8 both are 27/27.) The result is chaotic in growth only at 2.0.

## Table 2 (Part 3, measurement only): `pour_cut_cost_mm` at growth 1.25 (engine key pour_cut_cost_nm)
Baseline 0.0 = the growth 1.25 rows of Table 1. Value column is nm (250000 = 0.25 mm, 500000 = 0.5 mm, 1000000 = 1 mm).

| board | value | routed/total | failed | routing_new | unconn | detours>1.5 (ref) | ref max | outside mm | vias | router_s (load) | rips (max pair, backstop pairs) | stop | iters |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| private-badge-b | 250000 | 211/250 | 39 | 1 | 43 | 13 | 16.793 | 1699.891 | 218 | 245.533 (23.32) | 155 (6, 0) | no gain | 36 |
| private-badge-b | 500000 | 207/241 | 34 | 3 | 37 | 6 | 5.09 | 1761.604 | 205 | 152.935 (14.15) | 94 (5, 0) | no gain | 31 |
| private-badge-b | 1e+06 | 207/247 | 40 | 3 | 38 | 10 | 3.104 | 1678.125 | 214 | 162.223 (14.25) | 115 (5, 0) | no gain | 26 |
| eprom-to-kernal | 250000 | 27/27 | 0 | 0 | 0 | 0 | 1.147 | 64.257 | 0 | 0.152 (17.71) | 0 (0, 0) | converged | 1 |
| eprom-to-kernal | 500000 | 27/27 | 0 | 0 | 0 | 0 | 1.181 | 65.874 | 0 | 0.268 (20.69) | 0 (0, 0) | converged | 1 |
| eprom-to-kernal | 1e+06 | 27/27 | 0 | 0 | 0 | 0 | 1.181 | 65.874 | 0 | 0.198 (32.05) | 0 (0, 0) | no gain | 7 |
| wiccon-2023 | 250000 | 63/63 | 0 | 0 | 0 | 0 | 1.457 | 1369.104 | 41 | 7.751 (34.56) | 0 (0, 0) | converged | 5 |
| wiccon-2023 | 500000 | 60/60 | 0 | 0 | 0 | 0 | 1.154 | 1390.07 | 54 | 6.92 (27.97) | 0 (0, 0) | no gain | 9 |
| wiccon-2023 | 1e+06 | 60/60 | 0 | 0 | 0 | 0 | 1.173 | 1387.758 | 74 | 11.582 (23.68) | 0 (0, 0) | no gain | 13 |

Observations: connection totals change with the cost (wiccon 62 -> 63/60/60, private-badge-b 241 -> 250/241/247: pour-served connections appear/disappear), so rows are not strictly comparable. The cost trades vias and length for pour crossings: wiccon vias 34 -> 41/54/74 and outside +6 to +27 mm; private-badge-b gets routing_new 3 at 0.5 and 1.0 mm and a 16.8 detour max at 0.25. No value improves completion without a cost elsewhere; no default change (A-C15 stays at 0.0).

## Tests
- cargo test --release --no-fail-fast: green except the base-red/flaky ones: route_detour_ref::reference_is_feasible_not_topological, route_steiner (4 of 6), route_control::route_async_cancel_returns_anytime_result and route_engine::anytime_timeout_emits_committed_subset (load flakes), and route_stubs::exit_stubs_off_matches_previous (fails identically with growth 2.0, so not caused by this change; it is not in the coordinator's base-red list, please note). graph_ownnet::front_only_plus5v_needs_zero_slack_for_failed and eprom_ripped_front_only_is_the_m2_result green. Not run: clippy (no non-test Rust change besides two literals).
- pytest default suite: 503 passed, 4 failed (test_keys_without_effect_say_so, test_run_wiccon_refill_gives_harness_connection_count, test_progress_m4_name_and_caption_lines, test_scaffold: all base-red).

## Progress PNG (wiccon-2023, 1.25 default)
/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-a16261d80479c0b38/docs/benchmarks/progress/2026-11-01-m4-GR-wiccon-2023.png
Caption: 62/62 connections, vias 34, DRC routing 0, preexisting 0, unconnected 0, outside 1363.4 vs hand 1690.1 mm, router 1.51 s, detours > 1.5: 0 (max 1.3), pcb sha12 2d3063e70ed5. (Outcome-neutral for the two-layer boards: they never rip.)

## Concerns
1. private-badge-b gate (231/231) is not met at any growth: 205/241, failed 36, routing_new 1 (base reported 0 at wc 600; here wc 0, load 15 to 35), so growth is not the lever for the gate.
2. route_stubs::exit_stubs_off_matches_previous red independent of growth.
3. Timings in the tables are load-inflated; no quiet re-run possible.
