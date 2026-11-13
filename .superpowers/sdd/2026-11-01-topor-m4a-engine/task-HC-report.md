# Task HC report (NPTH hole clearance, geometry proptest, TopoR backlog rows)

Status: DONE_WITH_CONCERNS (concerns: private-badge-b gate options hit the 600 s wall on a loaded machine; wiccon router_s 11.99 s under load).
Branch worktree-agent-a0f559be8df34d4fb. Commits: fca57b2 (test geom), 86e839b (docs), 2b359ba (fix engine).

## 1. NPTH hole clearance (2b359ba)
Root cause (not the one in the diagnosis note): the NPTH pad is a copper pad (net 0, own clearance 0.2) and the model also has a
coincident `Hole` obstacle (0.35). The CDT keeps one vertex per position and `point_to_anchor` takes the FIRST anchor at it
(sketch.rs:142), the pad's; graph vertex clearance, search, sketch/taut and relax therefore saw 0.2 only, while the Checker
charged both. Fix = one helper, no per-stage patches:
- engine/src/model.rs: `BoardModel::npth_hole_clearance(pad)` (hole_clearance if attr == npth, else 0) and
  `pad_own_clearance(pad, net_clr)` = max(local, net, npth hole clearance); `required_clearance` includes it (vias.rs/fuzz.rs).
- engine/src/obstacle.rs `ObstacleSet::build` and engine/src/check.rs `Checker::new` use `pad_own_clearance` (the only two
  places that computed a pad's own clearance). Big-O O(1) each, unchanged overall.
- Second bug found by the same private-badge-b cases (D17/D18 slots, gap 0.25): the Checker modelled a slot hole as a circle of the long
  side, the obstacle set as the capsule. check.rs `hole_track` now builds the same capsule. No option needed (both are plain bugs).
Tests (engine/tests/npth_hole.rs, written first, failed with `failed self-check: hole:... (gap 205000 < 350000 nm)`, the DX
message, before the fix): `edge_pinch_legal_for_copper_but_not_for_hole_is_not_taken` (0.75 mm pinch hole-to-edge: router keeps
>= 0.35 from the hole, Checker clean, connection routed, no "hole" self-check reason = search and metrization agree),
`edge_pinch_wide_enough_for_the_hole_is_routed_clean`, `checker_models_a_slot_hole_as_its_capsule_like_the_router`.
Fixture gap (0.75) legal for 0.2 (needs 0.65) and illegal for 0.35 (needs 0.80).

Numbers (counts primary; machine heavily shared, all private-badge-b runs hit wall 600 s so completion counts are load-dependent):
| private-badge-b, gate options (wall 600, partial false, threads 4) | before | after |
|---|---|---|
| routed / N | 170 | 178 |
| failed | 61 | 53 |
| connections failing with self-check "hole" | 8 | 0 |
| vias | 148 | 155 |
| routing_new | 1 | 1 (connection_width, same in both) |
| unconnected | 71 | 64 |
| router_s | 600.2 (timed out) | 600.3 (timed out) |
| detour p90 / max / count>1.5 | 1.503 / 9.76 / 17 | 1.51 / 9.76 / 18 |
DX hole cases (final ids differ per run; by endpoints): D5.1-D6.1 (H2), D1.1-C16.1 (H1), H4 case, H3 cases, D17.2-R10.2 and
D17.2-D18.2 (slot), U7.1-J1.A5 (J1 locator): all route or at least no longer fail on the hole. J1.A4-J1.A9 (via-site pair) and
the U4.15/U4.16 cases remain failed for other mechanisms (not HC). The gate itself (N/N) is NOT met; other tasks own that.
eprom: 27/27, 25/25, vias 0, F.Cu 90.323 / B.Cu 17.773, outside 64.628 mm, routing_new 0, unconnected 0, router 0.245 s (loaded),
detour max 1.31, byte-identical geometry to M3 (no NPTH on the board). wiccon: 62/62, vias 37, outside 1373.553 mm (identical),
routing_new 0, unconnected 0, router_s 11.99 s UNDER heavy load (STATE: 3.07 s quiet; no NPTH on wiccon, code path identical;
re-measure on a quiet machine, concern for the coordinator).
PNG: /home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-a0f559be8df34d4fb/docs/benchmarks/progress/2026-11-01-m4-HC-private-badge-b.png
Caption: HC: 90/N connections, 11/M nets, partial 0, failed 141, vias 56, arcs 20, zero slack 0, iters 1, pcb (private pin);
DRC routing 0 preexisting 4, unconnected 139; router 60.34 s on 4 threads (harness.progress uses `wall_clock_s: 60` for
private-badge-b, so this PNG is the 60 s run, stop_reason timeout, not the 600 s gate run); detours > 1.5: 3 (max 2.7) USB- 2.7, GND 1.8, GND 1.8.

## 2. Geometry proptest (fca57b2)
Evidence: the analytic `arc_arc` is right. `arc_arc(a,b) = arc_arc(b,a) = 1524.79789 nm`; a 4M-point scan along b against an
independent exact point-to-arc distance, golden-refined, gives 1524.79789 nm; the old oracle gave 1531.462 from a's side but
1524.79789 from b's side. Cause: for nearly concentric arcs the 2000x2000 nearest-partner profile has 762 / 409 tied local
minima, the 12 best seeds are all in the wrong basin. Fix in the oracle only (`sampled`): also take the min of an exact 1-D
profile scan (20 000 points + golden refinement of the 12 best local minima) along each curve (`profile_min`, independent
`point_to_prim`). Tolerance unchanged (1e-3 nm). Regression `regression_arc_arc_near_concentric`. Debug-profile
`cargo test --test dist_props`: 23 passed (257 s), release 23 passed.

## 3. Docs (86e839b)
44 §4: six rows (BGA-area, global via minimisation (Polubasov 2001), re-optimise after the engineer moves a component/via,
lines-only vs lines+arcs (overlaps the 45° row and the existing "Output as 45°/polyline" row, same `[output] geometry` switch),
route-all-repair-later, teardrop smoothing), source TopoR Wikipedia via 01 §D. The "Autoplacement" row is now **non-goal**
with the user's reason; no placement row added. 45 §3: six matching "Backlog (after M4)" rows. Note: docs/01-bibliography.md has
no Polubasov 2001 entry (only 2010), the row says so (UNVERIFIED year as given in the request). tests/test_docs_links.py 15 passed.

## Tests
- `cargo test --release` all green; `route_control::route_async_cancel_returns_anytime_result` (2 s cancel timing test, no holes) failed once under load and passed (5/5) on rerun.
- `cargo clippy --all-targets -- -D warnings` clean.
- `.venv/bin/pytest -q` (PYTHONPATH engine override): 451 passed, 7 skipped, 26 deselected.
- No Options/schema changes (nothing configurable added; both fixes are bug fixes).
