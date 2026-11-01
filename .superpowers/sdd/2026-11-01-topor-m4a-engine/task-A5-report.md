# Task A5 report — same-net Steiner T-junctions and net ordering (M4-D3)

Status: **DONE_WITH_CONCERNS** (see "Concerns"). Branch `worktree-agent-ab734063ffda3814f`, base main 946530c.
Engine used for every Python / harness number: own build via `target/pyeng*` override (Ruling A-C2),
`topor_engine.__file__` = `<worktree>/target/pyeng-v5/topor_engine/__init__.py` (base numbers: `target/pyeng-base`,
a build of 946530c).

## What changed

| file | change |
|---|---|
| `engine/src/steiner.rs` (new) | `SteinerNets` (per live connection: net, from/to cluster ids, pour target, terminal owner labels; per cluster: constraint edges of its kept tracks), `plan()` = the junction goals of one search, `lower_bound()` = commit-phase re-search trigger. Allowed siblings by a union-find over the net's clusters with every *other* connection as an edge (an attached one joins its terminal end to its sibling's): a sibling on the far side → forward goals; only on the start side → reversed search (from `to`). Excluded: crossings on edges touching a vertex owned by the sibling's terminals or the far terminal (a T at a pad is the pad goal), siblings whose terminal shares the start's pad name (sub-pads of one exposed pad, U1.29 ×9), rejected pairs, crossings where the sibling's metrized chain is not within 3 track pitches of the edge. Big-O in the module doc. |
| `engine/src/search.rs` | `OwnStrand`, `JunctionEnd` (+ `SlotPath.junction`), `Goal.{strands, copper, junction_min_angle_deg, junction_cap}`; node kind `K_JUNC`; `Cyc::cross_range` (the slot range; `cross_slot` unchanged in result); `try_junction` / `push_junction`: a state crossing a sibling's edge at a slot adjacent to the sibling's entry (Rule T: nothing between them) or a same-net hand-track constraint edge is a goal, refused below the angle limit or above `junction_cap`; heuristic includes the junction edges (admissible); `finish` closes the leg with a zero-width half crossing; `search()` applies `steiner::plan` (forward, or reversed with the plain forward search deciding when no junction wins); no `find_direct` shortcut when junction goals exist. |
| `engine/src/graph.rs` | a leg with `faces.len() == crossings.len()` (junction leg) is accepted; its last entry gets width 0 / gap 0 (keeps the slot order, takes no capacity). |
| `engine/src/multigraph.rs` | `steiner`, `junction` per connection, `steiner_reject`, cascade: `remove_conn` removes the connections attached to it first (recorded, `take_cascaded`); `chain_cross` per leg (metrized crossing points, the junction estimate of later searches); `take_path` refuses junction paths. |
| `engine/src/negotiate.rs` | active set closed over attachments (cascade, reported `IterOutcome.cascaded`); junction paths never take the O(k) re-validation; a sibling that left makes the path invalid; commit-phase re-search (≤ 1 per connection per iteration) when a sibling committed earlier in the same loop gives a junction lower bound < 0.9 × the path cost, kept only if it is a junction of lower cost (`steiner_researched`). |
| `engine/src/route.rs` | Steiner context per pass (+ hand-copper edges per cluster); junction cap = `steiner_ghost_factor` × the connection's own empty-board ghost cost (`est + via_cost·vias`); metrization: the junction leg ends at `Terminal::Junction { J }`, J refined onto the sibling's chain (base point = the chain point nearest the search's estimate; a straight-piece interior point nearer the approach replaces it only when it shortens the approach and stays within it; ≤ 3 rounds, re-relax the moved legs, deadline checked); a junction leg and its sibling are not nested against each other (same net); validation: end within ¼ of the narrower width of the sibling chain, angle ≥ `steiner_min_angle_deg`, metrized length ≤ `steiner_len_slack` × estimate + 1 mm, an end inside an obstacle → not permanent; a rejected pair goes to `steiner_reject`; a junction whose sibling is not `Ok` fails; the sibling's track is split at the T (seg / arc, pieces ≥ 20 µm); `record_chain_cross` after each metrization; `junction_repair_pass` after the loop (rejected junction connections searched once more in the best graph, kept when all routed connections still metrize); via reduction / zero-slack leave junction-linked connections alone; `ConnReport.junction`, `junction_angle_deg`; `IterProfile.steiner_researched`. |
| `engine/src/decompose.rs` | `Decomposition.conn_clusters`, `kept_cluster`, `pad_cluster`, `via_cluster` (run-unique cluster ids), `net_rank` (M4-D3 net ordering: rank in the net by `est_len`; equal to the existing shorter-first rule of `order`, so the routing order is unchanged — pinned by a test). |
| `engine/src/model.rs` | `Options.steiner` (true), `Expert.steiner_min_angle_deg` (60), `steiner_len_slack` (1.1), `steiner_ghost_factor` (1.0). |
| `plugin/topor/schema.py`, `docs/user/configuration.md` | `[search] steiner` → engine `steiner` (since 0.5.0, live key); new `[expert] steiner_min_angle_deg`, `steiner_len_slack`, `steiner_ghost_factor`; reference regenerated (`docs_gen --check` clean). |
| tests | new `engine/tests/route_steiner.rs` (6 tests); literal fixes (`..Default::default()` on `Goal`) in `negotiate_loop.rs`, `search_layers.rs`, `search_entry.rs`, `examples/gate_diag.rs`; `graph_ownnet.rs::front_only_plus5v_needs_zero_slack_for_failed`: its "M3, by luck" sub-case now also sets `steiner = false` (M3 = every M4 rule off, as Ruling A-C14); `tests/test_engine_surface.py`: the pinned `Expert` defaults gain the three new fields + `steiner is True`. |

No port from the gEDA toporouter, no new crate.

## Options (judgement calls)

| key / field | default | why |
|---|---|---|
| `[search] steiner` / `steiner` | true | the A5 rule; false = byte-identical to main (test) |
| `[expert] steiner_min_angle_deg` | 60 | design §2 (acute slivers / acid traps below 60) |
| `[expert] steiner_len_slack` | 1.1 (+1 mm internal) | a T whose metrized length overshoots the search estimate is dropped (wiccon 3.3v J1.1→U1.3 took a T estimated 54.9 mm that metrized to 68.3 mm: the sibling crossed a 20 mm CDT edge far from the estimate) |
| `[expert] steiner_ghost_factor` | 1.0 | a T is a goal only while it costs no more than the connection's own empty-board pad route: Ts share exits and save vias, they do not dodge congestion (without it: wiccon outside 1385.9 mm > 1380 bound, dcsgone vias +49, a 89 mm T vs 42 mm reference) |
| internal consts | `STEINER_RESEARCH_FACTOR` 0.9, `JUNCTION_ROUNDS` 3, `JUNCTION_MOVE_TOL_NM` 1 µm, `JUNCTION_NEAR_MIN_NM` 0.5 mm, `JUNCTION_LEN_ABS_NM` 1 mm, `JUNCTION_SPLIT_MIN_NM` 20 µm, `JUNCTION_ARC_SAMPLES` 16 | bounds / tolerances, not user trade-offs |

Not done from design §2: Weiszfeld junction relaxation toward the 120° Fermat point (the T stays on the sibling's
chain: J is the foot point of the joining wire, ≈ 90°), and the sibling is not re-relaxed through J (its emitted
track is split at J instead). The search's `-1 re-search without that goal` became: rejected pair → next search
excludes it, plus the post-loop `junction_repair_pass`.

## Tests

- `cargo test --release --test route_steiner`: 6/6 — `t_junction_onto_routed_sibling`, `t_junction_onto_hand_copper`,
  `steiner_off_is_m3_behaviour` (result digests of the USB crop and the ripped eprom fixture equal those of main
  946530c: `0xc58dd349af135e63`, `0x4bbf8e934c970d2f`), `junction_angle_below_60_is_rejected`,
  `steiner_threads_byte_identical` (threads 1/2/4 + repeat), `net_rank_is_shorter_first_within_a_net`. Also pass in a
  debug build (debug assertions).
- `cargo test --release --no-fail-fast`: all suites pass except `route_detour_ref::reference_is_feasible_not_topological`
  (fails identically on main 946530c: ref == ghost 1220656) and `route_engine::anytime_timeout_emits_committed_subset`
  (0.5 s budget, fails under load on main too — known A1/A4 flake; `route_control::route_async_cancel…` likewise
  load-flaky on both). `cargo clippy --all-targets -- -D warnings` clean. New files rustfmt'ed (the repo is not
  globally rustfmt-clean; other files untouched by fmt).
- `pytest -q` (default, PYTHONPATH override): 497 passed, 4 failed, 7 skipped → after the surface-test update: 3
  pre-existing on main: `test_config_docs::test_keys_without_effect_say_so` (configuration.md on main has 0
  "No effect in 0.4.0" markers), `test_router::test_progress_m4_name_and_caption_lines` (fails with the main engine
  too), `test_scaffold::test_cargo_workspace_builds` (runs `cargo test` = the pre-existing `route_detour_ref` failure).

## Numbers (same machine, my base = main 946530c build)

eprom-to-kernal (default bench options): 27/27, vias 0, routing_new 0, unconnected 0, outside 64.63 mm (= main),
router 0.11 s, detour med 1.000 p90 1.006 max 1.147 >1.5: 0 (= main); 0 junctions (two two-connection nets, no T
cheaper than the pad route) — byte-identical outcome. M2 front-only guard green (cargo).

wiccon-2023: 62/62, vias **34** (main 36; bound 37), routing_new 0, unconnected 0, outside **1368.25 mm** (main
1367.28; bound 1380), router 2.3 s, 2 junctions (4v D2.1→S1.C1 39.82 vs 40.04 mm, GPIO04 MODE1.1→U1.19 13.71 vs
13.44 mm). Detour med 1.000 p90 1.053 **max 1.283** (main 1.270) >1.5: 0 (main 0); ghost max 1.295 (main 1.282)
>1.5: 0. The max is RST RESET1.1→U1.1 35.41 mm (main 35.05), pushed 0.36 mm by the GPIO04 junction's copper.
A-C6 revisit, `pour_listed_via_goals = true` with A5: vias 33, outside 1368.51 mm, detours unchanged.

dcsgonebadge, gate options (600 s, partial off, threads 4), both runs on this machine after each other:

| | main 946530c | A5 |
|---|---|---|
| routed / total | 196 / 246 | 198 / 243 |
| failed | 50 | 45 |
| vias | 184 | 169 |
| routing_new / preexisting | 0 / 4 | 0 / 4 |
| unconnected | 51 | 46 |
| router_s / iterations / stop | 174.6 / 31 / no gain | 118.7 / 31 / no gain |
| detour (reference) med / p90 / max / >1.5 | 1.000 / 1.390 / 6.84 / 15 | 1.000 / 1.324 / **1.93** / **7** |
| detour_ghost max / >1.5 | 6.84 / 23 | 1.93 / 17 |
| junctions | 0 | 3 (USB+ J1.A6→J1.B6 1.82 mm vs 2.71 pad-to-pad, +3V3 J3.17→R16.2 newly routed) |

(The coordinator's quiet-machine main numbers 201/243, vias 174, unconnected 41, detour max 22.3 >1.5 8 differ
from my main run: the gate run's DRC rounds depend on the whole-run clock.)

dcsgone, `wall_clock_s 0` (no clock: deterministic, both engines): main 200/242 failed 42 vias 173 routing_new 1
unconnected 42 detour max 5.37 >1.5 10 (ghost 16); A5 210/250 failed 40 vias 196 routing_new 2 unconnected 39
detour max **1.93** >1.5 **6** (ghost 14), 7 junctions (all shorter than or newly routed vs main: matched 58.4 vs
68.5 mm, 1 vs 3 vias; newly routed +3V3 J3.17→R16.2, BT1 TP10.1→U4.14, GND C24.1→pour).

dcsgone 60 s (brief Step 2): 209/250, vias 195, unconnected 39, detour >1.5: **6** (expected ≤ 6), max 1.93, ghost
>1.5 12, stop timeout.

Diagnosis cases (DX §6 item 5, by label): #207/#208 USB+ — the T forms (gate: J1.A6→J1.B6 1.82 mm onto U7.6→J1.A6,
hand B6 branch 1.824 mm); +5V U4.2→U4.4 stays "blocked: no path" (the 0.5 mm track does not fit U4.2's exit at all
— needs NX necking, A6); VCC U3.1→U3.3 3.29 mm unchanged (A2 already fixed it).

The big detours of main (pour completions GND → pour, max 6.8 / 22.3) are gone in the A5 runs; they were not
removed by junctions onto GND copper (no such junction in the final results) but by the different pass-1 copper
the junctions leave; GND hand-copper goals do exist in the completion passes (the pour cluster's kept tracks), they
are few (decomposition puts only tracks touching listed pads/vias into the pour cluster).

PNG (`harness.progress --milestone m4 --task A5 --router topor --board dcsgonebadge`, gate options, rendered twice,
identical): `/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-ab734063ffda3814f/docs/benchmarks/progress/2026-11-01-m4-A5-dcsgonebadge.png`
— caption: 207/251 connections, 45/66 nets, partial 0, failed 44, vias 210, routing_new 0, preexisting 4,
unconnected 42, router 284.1 s (4 threads), detours > 1.5: 15 (max 4.8; ghost > 1.5: 25), stop no gain, pcb
3e7e66f862a7. Main 946530c through the same PNG path on this machine
(`.../docs/benchmarks/progress/2026-11-01-m4-A5base-dcsgonebadge.png`, for comparison only): 196/246, failed 50,
vias 184, routing_new 0, unconnected 51, router 162.6 s, detours > 1.5: 15 (max 6.8). The PNG path and the plain
`run_bench` path differ for A5 only in the harness DRC loop (PNG: 3 rounds / 4 spots, `run_bench`: 2 rounds / 2
spots; each path reproducible), so the PNG numbers differ from the table above.

Scaling (`pytest -m scaling tests/scaling/test_route_scaling.py`, quiet machine): 1 layer t100/t10 16.1, t1000/t100
12.2; 2 layers 18.6 / 21.1 (bounds 40 / 30), all routed, no timeout.

## Concerns

1. Brief "#207 fixture ≤ 1.2 mm T": not reachable. Hand geometry (snapshot): the three USB+ pads meet in one T at
   (90.075, 123.375); branches U7.6 1.389, J1.A6 1.102, J1.B6 1.824 mm; any T on the J1.A6–J1.B6 trace (which clears
   J1.A7's tip by c + w/2) is ≥ 1.375 mm from U7.6. The test bounds are 1.2 × the hand branch of the joining pad and
   the pair ≤ 1.05 × the hand tree (4.315 mm); measured: the T is J1.B6 → onto U7.6→J1.A6, 1.821 mm (= hand branch),
   pair 4.23 mm, 0 vias. Test runs on the crop with `scope.nets = USB±` (all other pads stay obstacles): on the full
   crop the unrelated failing connections make the USB+ pair's outcome trajectory-dependent.
2. wiccon detour max 1.283 vs main 1.270 (count 0 = 0): RST pushed 0.36 mm by a junction's copper; outside +0.97 mm
   (1368.25, bound 1380), vias −2.
3. dcsgone `shorting_items` JP1.1 ↔ Net-(J3-Pin_22) in the wall_clock-0 runs (gate runs: the DRC loop fixes it,
   routing_new 0): a pre-existing engine model issue exposed by a different route — pads of a net-tie group are
   exempt for the other net's *tracks* (`obstacle.rs` / `check.rs` pad `tie_nets`, pinned by
   `obstacle_model::same_net_and_net_tie_are_exempt`), KiCad flags the track touching JP1.1. Not changed here
   (designed M1 behaviour, out of scope); recommend a fix task (tie exemption only for the footprint's own copper).
   Same for `connection_width` (+5V via neck, also on main).
4. Gate-option dcsgone numbers depend on the harness path (DRC-loop rounds differ between `harness.progress` and a
   plain `run_bench` call for the A5 engine; main gives the same in both): PNG path A5 207/251, unconnected 42,
   vias 210, > 1.5: 15 (max 4.8) vs main 196/246, 51, 184, 15 (max 6.8); `run_bench` path see the table. The vias
   rise with the extra routed connections (most newly routed ones take 2-4 vias); matched junction connections use
   fewer vias than main. Neither engine reaches the gate predicate (A12 / A6+NX work).
   `anytime_timeout_emits_committed_subset` passes alone on a quiet machine (A5 and main) and fails under load for
   both.
5. A-C6: with A5, `pour_listed_via_goals = true` is not worse on wiccon beyond +0.26 mm outside (33 vias); default
   kept false (not measured on dcsgone) — recommend the coordinator flips it after a dcsgone check.
