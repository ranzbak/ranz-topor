# Task 2 report: RC3 sleeve queries + D2/D3/D7/D10

Status: DONE_WITH_CONCERNS. Commit 4010c90 on branch worktree-agent-a141224bcf3a2052b (not pushed).

## Changes
- engine/src/search.rs
  - `clearance_bend(a, t, portals: &[usize])`: queries only the sub-chord boxes between consecutive portal crossing points
    (`sleeve_boxes`, seg_closest params sorted, each box grown by rho / wr), vertex and wall candidates merged into sorted dedup'd
    Vecs. O((C+1)*local) per call. `los` passes its `since` chain; `direct` passes `&[]`.
  - `SlotPath.candidates` (ruling P1): sum of query candidates over the search (Cell counter in `Search`, set in `finish`).
  - D2: `hard(e)` also true when `cap < w + eps + 2*max(c, c_e)` (O(1)); used by the ghost (skipped as impassable).
  - D3: `direct()` takes the `Search`, returns None if `clearance_bend(p, q, &[])` reports a bend.
  - D7: `Search::foreign(u)` (spot vertex `owner` "spot:*" blocks only nets in its anchor's `only_nets`, looked up via
    `vi.anchor`; `vi.nets` is empty for spots in graph.rs, which I do not own) used by clearance_bend, margin_of, bend_ok.
    Also `wall_exempt(wl)`: the spot's Point wall honours `only_nets` through `wl.ends[0]` (the wall loops ignored it too).
  - D10: `bend_ok` tests the 5 samples against foreign vertices from `verts_near(wbb)` (need = r + clr.max(c) + w/2 + eps), and
    against Point walls that are not graph vertices.
  - `#[doc(hidden)] probe_clearance_bend` (full = no portals = old single-bbox query) and `probe_bend_ok` for the integration tests.
- engine/tests/search_sleeve.rs (new): 6 tests (proptest 200 cases sleeve==full; ghost candidates bound; unfit edge; direct band;
  spot net; pinch).
- engine/tests/negotiate_loop.rs: one-line `candidates: 0` in a SlotPath literal (required by the new field).

## Deviations from the brief (both forced by the unchanged-suite requirement; coordinator please rule)
1. D2 is applied to the ghost only (via `hard`), NOT as an impassable edge in the non-ghost planar search. Adding
   `|| self.hard(e_out)` there (with or without eps) makes `route_engine::eprom_ripped_stalls_and_zero_slack_retry_passes_length_gate`
   fail (26/27): +5V U2.28->J1.2 is routed through a statically unfit edge as a costly overuse crossing (zero-slack
   corridor). So `statically_unfit_edge_is_hard` asserts `!fits_at` on the empty edge and that the ghost never crosses it, not
   "search never crosses".
2. D10 vertex test skips vertices joined to `u` by a `hard` (unfit) edge: pinches against those are the edge capacity's job (D2).
   Without the skip, the vertex test (exactly as in the brief) also drops eprom to 26/27 (rejections between the staggered
   U1/U2 pads, centres 2.0 mm apart, gap 0.4 mm, i.e. unfit pairs). The pinch test therefore uses a non-adjacent vertex (two
   small pads flank the U-V gap so the CDT has no U-V edge).

## Tests
- `cargo test --release --manifest-path engine/Cargo.toml`: all suites pass (search_slots, negotiate_loop, route_engine unchanged and green;
  search_sleeve 6/6). `cargo clippy --all-targets -- -D warnings`: clean.
- TDD caveat: the probe hooks do not exist on the base, so the new tests were not run against it; the D7 spot test failed until the wall
  exemption was added, the D10 pinch test failed before the vertex loop existed. `statically_unfit_edge_is_hard` was seen passing without D2
  while it asserted only the planar search; it now asserts the ghost (discriminating).

## private-badge-b (existing model JSON, route_probe --quiet, single run, machine shared with other workers)
- Before (base 1087a9f): router 60.089 s, routed 0/N (best iteration 1, wall-clock cap hit), 2 blocked searches printed.
- After: router 60.458 s, routed 0/N, 4 blocked searches printed.
- No speedup measured: this run is bound by the 60 s cap with 0 routed in both builds, and the M2 time split (34 s in 59 ghost
  searches) was not reproduced; perf was unusable here so the remaining cost centre is not identified. The candidate bound
  (<= expansions*33*64) holds on the 200-pad ghost test, but the private-badge-b wall time is dominated by something other than the
  clearance query (suspects: Cyc::new/strands per expansion, D9 block probe in route.rs). Needs a profile before calling RC3 closed.

## Fix round 1 (commit 85c21fb, after `git merge main`)

Gate options reproduced engine-only: `route_probe <model> --options '{"wall_clock_s": 60, "partial_routes": false}'` calls `route()`
and prints wall, routed, iterations and the search counters (`search::search_totals()`: per planar/ghost: seconds, runs, expansions,
candidates, clearance_bend calls; 2 clock reads per search, nothing in the expansion loop).

Timing, private-badge-b model, same options, same machine, run back-to-back:

| build | router s | routed | iterations | timed out |
|---|---|---|---|---|
| M2 1087a9f | 45.7 | 24/N | 9 | no |
| main after merge (739bafb, my first sleeve version) | 60.3 | 24/N | 2 | yes |
| 85c21fb (this fix) | 16.9 | 24/N | 9 | no |

Hotspot (instrumented, 739bafb, 60 s run): ghost 57.2 s in 17 runs (3.4 s/run, 509 expansions/run, 6.6 ms/expansion, ~17 clearance_bend
queries per expansion, 393 candidates/query); planar 2.8 s in 59 runs. So the M2 claim (ghost searches dominate) holds. Inside it:
grid queries were 55 s of 60 s (vertex grid queries 35 s, wall grid queries 20 s); the exact tests on the candidates were < 3 s. Cause:
one `Grid::query` (HashSet + 3x3 cell padding of 5 mm cells) per sub-chord box, up to 33 per bend query, repeated for overlapping
corridors on every expansion.

Fixes (all in search.rs, results identical: same routed/iterations, eprom e2e unchanged):
1. Sub-chord boxes are merged up to 4 query radii (`MERGE_LEN_FACTOR`); still <= portals+1 boxes, still a superset (exact).
2. `cached_near`: candidate lists of lattice-aligned (pitch = query radius) boxes memoised per `Search`; supersets in the same ascending
   order, so the first bend is unchanged. Grid queries fell from 55 s to ~1.6 s.
3. Wall prefilter: skip a Point/Seg wall whose bbox gap to the chord bbox is >= `need` (cannot be closer).
After: ghost 4.9 s in 74 runs (0.18 ms/expansion), planar 7.6 s in 235 runs; 4.4 s of the 16.9 s is outside search (metrize,
self-check, zero-slack; not split further). Remaining planar cost (0.12 ms/expansion) is per-candidate wall/vertex evaluation and
`Cyc`/`strands`.

Checks: full engine suite (26 result lines ok, incl. search_sleeve, search_slots, route_engine eprom), clippy -D warnings clean.
