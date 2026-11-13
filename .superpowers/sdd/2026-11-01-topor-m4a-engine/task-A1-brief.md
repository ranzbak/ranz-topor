### Task A1 (S, F review at the gate): slot-search speed — face clear radius, memoised `clearance_bend`, early stop — `engine/src/search.rs`, `engine/src/graph.rs`

**Interfaces:** `RouteGraph.clear_r[f]` = distance from face `f`'s incentre to the nearest vertex not of `f` and nearest wall (one grid query per face at build, `O(F·q)`); `clearance_bend` (a) skips the candidate scan for a sub-chord between consecutive portals whose endpoints both lie inside `D(incentre(f), clear_r[f] − ρ)` (convex disk ⇒ every point of the sub-chord is ≥ ρ from every non-face obstacle; the face's own 3 vertices are still tested exactly), (b) memoises per search on `(portal edge, side, apex vertex, target vertex)` (`RefCell<HashMap>`, membership only), (c) tests candidates in ascending chord parameter and stops at the first violation. Per call `O((C+1)·hit)`. Counters in `search_totals()`: `bends`, `candidates`, `skipped_faces`, `memo_hits`.

- [ ] **Step 1:** `clear_radius_skip_is_conservative` (proptest 200 cases), `memoised_bend_equals_recomputed`, `early_stop_bend_equals_full_scan` (`engine/tests/search_speed.rs`). FAIL → implement → PASS.
- [ ] **Step 2:** measure on private-badge-b (`route_probe … --profile`): candidates/query (was 162) and search share (was 94 %) in the ledger; target `≥ 5×`; `< 2×` → stop and report. eprom/wiccon byte-identical; private-badge-b 60 s ≥ 148. PNG `--task A1 --board private-badge-b`. Commit `perf(search): face clear radius, memoised clearance_bend, early stop`.


## Coordinator additions (binding)
- Context: docs/progress/notes/m3-gate-review.md §8 (fix shape), STATE.md M4 carry-over (1): private-badge-b 162 candidates/query, 94 % of
  search time in clearance_bend/los. The M4a HARD gate is private-badge-b N/N in ≤ 600 s whole run — your speed-up is its basis.
- Measure BEFORE and AFTER with the same command and report both: `cargo run --release --example route_probe -- <private-badge-b model.json>
  --options '{"threads":4,"wall_clock_s":60}' --profile --quiet` (export the model once via `PYTHONPATH=$PWD/target/pyeng
  .venv/bin/python -m harness.bench private-badge-b /tmp/<you>/private-badge-b --router topor` or reuse the `--model.json` that run writes).
  Report: candidates/query, search share %, bends, skipped_faces, memo_hits, routed/total, iterations, stop_reason, router_s.
- Then ONE gate-options run (Ruling A-C3): wall_clock_s 600, partial_routes false, threads 4 (harness run_bench, or
  `harness.progress --milestone m4 --task A1 --board private-badge-b` once A0's flag is in — if not merged yet, call
  `harness.progress.render_progress("A1", board="private-badge-b", milestone="m4", ...)` with the 600 s options in a small script
  outside the repo, or route_probe with `"wall_clock_s":600` + harness bench for DRC). Report routed/total, vias, routing_new,
  preexisting, unconnected, runtime_s, detour stats, iterations, stop_reason. This is info for the coordinator (gate is A12).
- Speed target ≥ 5× on candidates/query·search-time; < 2× → stop and report with the profile. Results must stay exact: eprom and
  wiccon byte-identical (route JSON minus runtime_s/threads_used, threads 1/2/4); if a change alters results it is not a pure
  speed-up → explain why or revert that part.
- Commit `perf(search): face clear radius, memoised clearance_bend, early stop`.
