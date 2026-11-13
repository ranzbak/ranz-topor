# Rotation 2 common brief additions (binding for every rotation-2 task; coordinator rotation 2, base main 789e050)

Read in this order: worker-contract.md, global-constraints.md (same directory), this file, your task brief.

## State at base 789e050 (= engine ea1f064), quiet machine, gate options (600 s, threads 4, partial_routes False, rip all)
private-badge-b (private pin): 204/241 routed (first pass N), failed 37, vias 196, routing_new 0, preexisting 4, unconnected 39,
24 it, stop "no gain", router 120 s, runtime 146 s; detour (DRC-feasible reference, A7) med 1.000 p90 1.296 max 4.62, > 1.5: 9
(U1-EN 4.6, VCC 4.2, TACT_B 3.6); ghost max 4.66, > 1.5: 15.
eprom-to-kernal: 27/27, 0 vias, outside 64.628 mm, ref max 1.1, 0.04 s. wiccon-2023: 62/62, 34 vias, outside 1363.375 mm, ref max 1.3,
1.56 s, "no gain".
Red at base: route_steiner 4/6, route_detour_ref::reference_is_feasible_not_topological, graph_ownnet::front_only_plus5v_needs_zero_slack
_for_failed, route_engine::anytime_timeout_emits_committed_subset (load flake), route_control::route_async_cancel_returns_anytime_result
(load flake); pytest test_config_docs::test_keys_without_effect_say_so, test_router::test_progress_m4_name_and_caption_lines,
test_run_flow::test_run_wiccon_refill_gives_harness_connection_count (plugin path 63 vs harness 62), test_scaffold (= the cargo reds);
gate: M2 + M3 private-badge-b 60 s info tests drc_new 1 (routing_new must be 0, M3-R13), M4b headless wiccon total 63 ≠ 62.

## Measuring private-badge-b (machine is shared by several workers)
- Use the gate options: `run_bench("private-badge-b", out, router=route_router, rip="all", threads=4,
  options={"wall_clock_s": 600, "partial_routes": False})` (harness.progress --milestone m4 applies them too).
- Always report `stop_reason`, router_s, runtime_s and the 1-minute load average (`uptime`) next to the numbers.
- If the load average is > 6 or stop_reason is "timeout", the 600 s run is NOT a valid measurement: repeat it with
  `wall_clock_s: 0` (= no deadline; deterministic, equal to the quiet gate run whenever that one stops on "no gain") and say which
  one you report. Never present a timed-out run as a regression or a gain.
- The gate predicate (A-C11): routed == connections_total, failed == partial == 0, unconnected_items == 0, routing_new == 0,
  runtime ≤ 600 s, 22 locked GND items identical with 0 DRC pairs vs router copper.

## Acceptance (every task that can change routing)
- Report for eprom-to-kernal, wiccon-2023 and private-badge-b, at your base and at your head: routed/connections_total, failed, vias,
  routing_new, unconnected, outside-pad length, router_s, detour (reference) median/p90/max/count > 1.5 and detour_ghost max/count.
  Completion, routing_new, count > 1.5 and max must not get worse than base unless a connection newly routes (name it).
- Not-worse numbers (global-constraints.md): eprom 27/27 ≤ 64.7 mm, router < 2 s, the M2 guard eprom_ripped_front_only_is_the_m2_result;
  wiccon 62/62, vias ≤ 37, outside ≤ 1380 mm, router < 10 s; routing_new 0, unconnected 0. Never relax a bound, never weaken/skip a test.
- Determinism: threads 1/2/4 + repeat byte-identical (route_threads / the relevant *_threads_byte_identical tests).
- Every new judgement call = an Options field (+ plugin/topor/schema.py Setting when user-facing, docs regenerated with
  `.venv/bin/python -m plugin.topor.docs_gen` and `--check` clean); name each in the report with its default + why.
- Big-O doc comment on every new component; every new loop capped, checks deadline + cancel (Ctx::stop).
- Progress PNG for an outcome-affecting change: `PYTHONPATH=$PWD/target/pyeng .venv/bin/python -m harness.progress --milestone m4
  --task <ID> --router topor --board private-badge-b` (absolute path + caption numbers in the report; never commit docs/benchmarks/).
- License: topor is GPL-2.0-or-later. Porting from the gEDA PCB toporouter (src/toporouter.c, GPL-2.0-or-later, Anthony Blake) is
  allowed: keep the original copyright notice + attribution at the port site, origin in the commit message, say so in the report
  (coordinator records a ruling). No new crates without asking.
- User rules: "The goal here is to have a robust routing algorithm not to reproduce the hand layout." No per-connection detours where a
  direct path exists. Components are placed by the engineer (placement is a non-goal). Never touch ~/work/kicad/projects (use the
  scratch/boards snapshots). Never `git push`. No subagents.

## Round 2 update (base = main after the round-1 merges: TE, NT, PY, GR, DG + harness DRC-budget fix)
- Merged: NT net-tie exemption (private-badge-b routing_new 0 again), TE steiner/detour-ref/flake fixes, GR `ripup_growth` default 1.25
  (guards run with the default), PY test fixes, DG profile columns (`route_probe --profile`: fail_classes / blocked_classes) and the
  round-2 diagnosis: the private-badge-b completion diagnosis (note removed: private board) "Round 2" (R2.0–R2.4) — READ the section of your fix.
- private-badge-b at the round-2 base (wall_clock 0, deterministic): 205/241, failed 36, vias 196, unconnected 38, routing_new 0,
  detour ref max 4.62, > 1.5: 8–9. `wall_clock_s 0` now also gives the harness DRC loop no budget cut (bench._drc_budget_s), so
  wall_clock-0 runs give valid routing_new / unconnected too.
- **Single private-badge-b runs are chaotic (DG R2.2 item 6: 13 orderings give 29–46 failed).** Judge a fix on private-badge-b over the default order
  AND the net-order variants v3, v5, v7 of DG's prototype (apply the diagnosis prototypes patch (note removed: private board)'s
  `decompose::order_variant` hunk in your worktree for measurement only — `TOPOR_DG_VARIANT=<v>`; never commit it; task VR implements
  variants for real). Report all four rows; a fix is a gain when the sum over the four improves and the default order does not get worse
  by more than noise you can explain.
