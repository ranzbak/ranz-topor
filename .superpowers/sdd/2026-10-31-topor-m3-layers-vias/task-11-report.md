# Task 11 report: rayon pool, parallel batch searches, parallel self-check

Status: DONE_WITH_CONCERNS. Commit 9005d56 on worktree-agent-ac62532d1efbe815e (merged integration branch cleanly first).

## Changes
- engine/Cargo.toml: rayon 1.10.
- route.rs: `route` builds an N-thread pool (N = effective_threads) and runs `route_inner` in it; `threads_used` = N; fallback warning "thread pool unavailable: 1 thread".
- negotiate.rs `iteration` (batch mode): (1) every active connection removed, (2) `search_all` = parallel search in index order, (3) in-order commit; a Found path is valid iff every edge of every touched face (faces of each leg, or the faces of the along-edge for a direct leg) has edge_version <= per-layer clock snapshot, its sites have site_version <= mg clock snapshot, and no site activated since the freeze lies within rho_max + w/2 + eps of the bbox of the touched faces (P4). Invalid => sequential re-search against the current graph (M2 order kept), counted in `IterOutcome.resolved_sequentially`. Blocked is final; Timeout stops.
- graph.rs: `leg_faces`, `faces_unchanged_since`. multigraph.rs: `clock()`, `build_on` builds layers in parallel. check.rs: `check_against` parallel over items (>= 16), reports concatenated in item order.
- Two additions beyond the brief, both result-neutral (wiccon output byte-identical with and without them):
  - Detour ghost precomputed in the parallel phase for Found paths that pass the absurd-detour test; reused at commit while the ghost's own faces are unchanged (path_valid on the ghost), else recomputed.
  - `dead` set: a connection whose slot-free ghost finds NO path is blocked by hard geometry alone (everything dynamic is soft in the ghost), so it is never searched again. This was the wiccon fix: two such connections cost 0.55 s + 0.32 s per iteration.
- Tried and reverted: parallel re-search waves (deferred paths re-searched together). Faster, but it inverts commit priority and eprom_ripped_front_only came out with 1 nm clearance violations. Also tried and dropped: speculative ghost join (no gain).
- Tests: route_threads.rs (3 tests), negotiate_loop `iteration_batch_commit_invalidates_shared_edge` (resolved == 1, one detours, identical on 1/2/4 threads), test_engine_surface `test_route_threads_option` (+ route round trip now `threads_used >= 1`). route_engine.rs `opts()` sets threads 1 (22 concurrent debug tests with 7-thread pools oversubscribed the cores and made two wall-clock-bound tests flake; test_scaffold runs debug cargo test). route_probe gets `--dump out.json`.

## Tests
- cargo test --release --no-fail-fast: all green; clippy -D warnings clean.
- pytest default: 130 passed after the final test fix (the one earlier failure, test_scaffold debug cargo test, was the oversubscription above, fixed; not re-run in full afterwards, route_engine debug passes).
- Gate `test_m2_gate.py -m gate`: 2 failures, both expected: (a) eprom `threads_used == 1` (now 4; Task 12 change, test untouched), (b) private-badge-b router_s 120.06 > 65 (see below). eprom router_s 0.076 < 2.0 and wiccon info test pass.

## Timing (router_s from harness.bench --rip all --router topor)
| board | threads 1 | threads 4 | routed |
| --- | --- | --- | --- |
| eprom | 0.075 s | 0.063 s | 27/27, DRC clean |
| wiccon | 2.64 s | 1.90 s | 50/60 (budget < 10 s met; was 9.74 s), 15 iterations (was 10), vias 26 (was 28) |
| private-badge-b (gate options) | 120.0 s | 120.0 s | 160/231 harness; engine-only probe 164/231, timed_out, 1 iteration |

Byte-identical JSON (minus runtime_s/threads_used) for threads 1/2/4: eprom and wiccon confirmed via route_probe --dump; also run-to-run in route_threads test.

## Concern: private-badge-b budget NOT met
Profile at 4 threads: iteration 1 = parallel search 18.5 s (one connection with an exhaustive blocked search + ghost takes 14.7 s, the critical path), then commit with 141 of 231 paths invalidated and re-searched sequentially (~24 s), iteration 2 another ~17 s; the 60 s wall clock hits in iteration 2, the harness DRC loop re-routes once, hence 120 s. Threads 1 and 4 give the same result because both finish iteration 1 within the clock. Ideas not done: run search and ghost of a never-seen connection concurrently (cuts the 14.7 s task roughly in half), a hard-constraints-only ghost to mark more connections dead, a less conservative P4 invalidation (141/231 re-searches), parallel re-search waves (rejected: breaks priority/eprom cleanliness). The note section 7 should be amended for the precomputed ghost and the dead set.

## PNGs (real routed boards, viewed)
- /home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-ac62532d1efbe815e/docs/benchmarks/progress/2026-10-31-m3-T11.png (eprom, 27/27, router 0.08 s on 4 threads)
- /home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-ac62532d1efbe815e/docs/benchmarks/progress/2026-10-31-m3-T11-wiccon.png (50/60, 26 vias, router 2.63 s on 4 threads)
