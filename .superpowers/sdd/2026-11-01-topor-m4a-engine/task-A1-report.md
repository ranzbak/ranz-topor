# Task A1 report: slot-search speed

Status: DONE_WITH_CONCERNS (speed target far exceeded; private-badge-b 231/231 not reached, that is A2..A12)

## Key finding (premise correction)
Instrumenting the planar search on private-badge-b (1 thread, 30 s): `clearance_bend` was only ~13 % of search time
(2.1 s of 15.8 s); **`bend_ok` (called from `los`) was ~86 %** (13.6 s). The "94 % in clearance_bend/los" in the
carry-over lumped them. So A1 also memoises and cache-queries `bend_ok` (exact; on the `los` call path).

## Changes
- `engine/src/graph.rs`: per face `incentre`, `clear_r` (vertex part), `clear_w` (wall part), `face_walls`
  (own walls: both end anchors are face corners; excluded from `clear_w`, tested exactly when a scan is skipped).
  `clear_w` = min over non-own walls of `dist(incentre, core) - r_core - static_clearance`; `clear_r` = nearest
  non-corner vertex distance. Both capped at `longest + 2*rho_max + 2 mm` (a cap only lowers: still a lower bound).
  Cheap reject: faces whose 1-ring nearest vertex < `inradius + rho_max` get `-1` (never skip) without grid queries
  (without this the build cost 212 ms / 10k faces and broke `anytime_timeout_emits_committed_subset`).
  Vertex extent is charged at query time through the monotone `rho_max()` (set_site can raise it), so dynamic via
  sites stay conservative.
- `engine/src/search.rs`: `clearance_bend` (a) skips a sub-chord piece when both end points lie in
  `D(incentre(f), clear - reach)` for a face next to a portal crossed inside the piece (vertex scan uses
  `clear_r - rho`, wall scan `clear_w - (c + w/2 + eps)`, 1 nm margin); the face's own corners / own walls are
  added to the candidate list instead; (b) memo per search on the bit patterns of `(a, t)` (the result does not
  depend on the portals); (c) candidates tested in ascending lower-bound chord parameter, stop when
  `lb > best + 1e-9`, tie-break `(kind, index)` reproduces the old order. `bend_ok`: memo on `(a,u,t,side)`,
  lattice-cached candidate lists (`cached_near`), memoised point-wall-is-vertex. `BendMode{skip,memo,early}`
  (FAST default, EXACT = reference). `search_totals()` is now a 7-tuple: ..., `skipped_faces`, `memo_hits`.
  `Core::bbox` made pub.
- `route_probe` prints the new counters; `search_terminal.rs` pattern adapted to the 7-tuple.
- Tests `engine/tests/search_speed.rs` (5): `clear_radius_skip_is_conservative` (proptest 200: disk free of
  non-face vertices/walls by the charged reach + skip == exact), `memoised_bend_equals_recomputed`,
  `early_stop_bend_equals_full_scan`, `memoised_bend_ok_equals_recomputed`, `skip_fires_on_sparse_board`.
  Mutation check: loosening the skip radius makes two tests fail. (Tests were written together with the code; the
  red phase was verified by that mutation, not by a pre-implementation run.)
- Big-O: build `O(F * deg)` + bounded grid queries for faces passing the reject; `clearance_bend` per call
  `O((C+1)*hit)`, memo hit `O(1)`; `bend_ok` hit `O(1)`.
- No new Options/settings (all exact-preserving, no judgement call).

## Numbers (private-badge-b, `--options '{"threads":4,"wall_clock_s":60}' --profile`)
| | before | after |
|---|---|---|
| candidates/query | 109 (brief said 162) | 11 (9.9x) |
| search ms/run | 775 | 28 (27x) |
| bends / skipped_faces / memo_hits | 1.44M / - / - | 30.8M / 142k / 28.2M (91 %) |
| routed in 60 s | 86/231 (1 iteration) | 175/231 (6 iterations) |
| stop_reason | timeout | timeout |
After A1 the remaining cost in 60 s is ghost (28 s CPU, 35 ms/run) and commit (26 s in the profile), not slot search.
eprom 0.275 s -> 0.10 s, wiccon 17.7 s -> 4.1 s (1 thread), 9.7 -> 2.9 s (4 threads), nametag 3.6 -> 1.0 s.

## Exactness
Route JSON minus runtime_s/threads_used byte-identical to the main engine (68a77b6 line) on eprom and wiccon for
threads 1, 2, 4; nametag-sao identical (threads 2). wedding-gift not comparable (old build times out at 60 s).

## Gate-options run (info for A12), private-badge-b 3e7e66f862a7, wall_clock_s 600, partial_routes false, threads 4
Harness run: 188/255 connections reported (DRC-loop rounds included), failed 67, vias 199, routing_new 0,
preexisting 4, unconnected 62, router_s 144.1, runtime_s 190.0, iterations 42, detour median 1.003 / p90 1.52 /
max 11.6 (VCC U3.1->U3.3) / count>1.5: 20 of 188 (67 without reference). Hard gate NOT met (231/231 pending
A2..A12), but 190 s whole run is well inside 600 s.
PNG (not committed): /home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-a8a5912ff53400ed1/docs/benchmarks/progress/2026-10-31-m4-A1-private-badge-b.png
Caption: "A1: 188/255 connections, 40/66 nets, partial 0, failed 67, vias 199, arcs 216, zero slack 0, iters 42,
pcb 3e7e66f862a7; DRC routing 0 preexisting 4, unconnected 62, outside pads 1725.8 vs hand 3574.7 mm, router 144.08 s
on 4 threads".

## Tests
`cargo clippy -D warnings` clean; `cargo test --release`: all pass. `anytime_timeout_emits_committed_subset`
(0.5 s budget) is load-sensitive: it failed a few times while other workers loaded the machine (load avg 10-20)
with "nothing emitted", passes unloaded (repeatedly); test unchanged. pytest default suite: 450 passed, 1 failed
(`test_scaffold.py::test_cargo_workspace_builds`, passes on rerun; concurrent cargo runs).
Other workers' files untouched (cargo fmt noise reverted).

## Concerns
- The skip rarely fires on dense boards (142k of ~31M queries); the win is memo + `bend_ok`. Kept: exact and cheap.
- Face clear radius assumes the search's `obs` is the one the graph was built with (true in `Env::of`).
