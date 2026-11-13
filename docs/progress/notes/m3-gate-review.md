# M3 gate review (F, independent) — two layers + vias

Reviewed at `c0d4b5c` (main; engine unchanged since `4bb63a1`, wheel built 14:52 after it). Read-only review; nothing but this
note changed. Re-run here: `cargo test --release` for `route_layers` 14, `negotiate_loop` 11, `route_threads` 3,
`route_engine` 28, `via_sites` 6, `search_layers` 12, `multigraph` 4, lib 9 — all green; `tests/gate/test_m3_gate.py -k
"eprom or wiccon"` 2 passed (wiccon 62/62, signal 57/57, vias 37, drc_new 0, unconnected 0, 3.30 s, 26 it, zs 0);
`tests/test_locked_copper.py` 4 passed incl. the gate-marked full route (private-badge-b 148/N, locked-vs-router violations 0,
preexisting 4 clearance, routing_new 0).

## 1. Complexity vs 45 §5a (every new/changed loop)

| Loop | Where | Cost / cap | Verdict |
|---|---|---|---|
| Via sizes | `vias.rs:32` | `O(|routing_vias| log)` | ok |
| Via sites: union CDT, incentre cull, escape sites, fit, grid dedup | `vias.rs:95-200` | `O(N log N) + O((F+4T)·|sizes|·q)` fits, `O(1)` grid dedup `:178-191`, final sort `:199`; sites only when ≥ 2 routable layers or a pour target (`route.rs:2637`, RC-A1) | ok, no cap needed (`F ≤ 2N`) |
| MultiGraph build | `multigraph.rs:111-198` | layers in parallel, collected in layer order `:123-146`; `pour_membership` `O(S·ΣP)` `:202` | ok |
| Site activate / release | `multigraph.rs:264-335` | `O(layers·deg + neighbours)`, neighbours by one grid box `:285`, sorted `:338-351` | ok |
| Leg-id allocation | `multigraph.rs:373-397` | sorts `free_ids` and `remove(0)` per leg: `O(F log F + n·F)` per insert | ok (bounded), minor |
| Layered A* per expansion | `search.rs:1186-1280` | cap `8·Σ(E+X)+4S+64` (`cap_of` `:79`), deadline every 16 expansions `:1231`, `max_vias` `:1038`, ancestor chain `O(vias)` `:1042-1047`; per expansion as RC3 (sleeve `clearance_bend` `:1514`, cached grid lists `:1480`) | ok |
| Topological ghost (F2) | `search.rs:2444`, `:2636` | 0-1 BFS over `(layer, edge, dir, vias)`, cap `8·ΣE+4S+64` | ok |
| Static dead set (F3) | `search.rs:185-220`, `negotiate.rs:308` | `O(Σdeg)` once, repeated without search `:377-387` | ok |
| Batch search + commit | `negotiate.rs:159-358` | phase 2 `par_iter` in index order `:375`; `path_valid` `O(Σ|faces|·(1+|activated|))` `:409`; `revalidate_path` `O(k)` `search.rs:1862`; re-search ≤ `|active|`; deadline before phase 1 `:195` and before every re-search `:235` | ok |
| Rip damping | `negotiate.rs:477-495` | `MAX_RIP 3`, `RIP_PAIR_CAP 3` ⇒ rips ≤ `3m²` | ok |
| Via-reduction pass | `route.rs:1745-1857` | one pass, order `(−vias, id)`, deadline per candidate `:1767`, `mg.clone()` `O(V)` per candidate, one search + one incremental metrize | ok |
| Pour goals | `route.rs:2718-2735` | `O(S)` per pour connection; `perm` "no via site inside pour" before any search `:2726` | ok |
| Persistent ItemIndex / parallel self-check | `check.rs:504-520` | `O(|items|·q)`, reports concatenated in item order | ok |
| Block probe (D9) | `route.rs:1204-1240` | `PROBE_STEP_CAP 64`, deadline before every step `:1227` | ok |
| Anytime (Q2) | `route.rs:2808-2818`, `negotiate.rs:235-239`, `:327-331` | finished searches committed, timed-out one dropped | ok |
| Pour-reach check (§9) | `pourcheck.rs:396-800` | raster per (pour, layer) in parallel `:459`, cells ≤ `CELL_BUDGET` 4e6 (+3/axis) by `res = max(25 µm, √(area/4e6))` `:135-137`; flood `O(cells)`; DSU `O(T·(P+V+T))`; **knockout stamps loop over each capsule's bbox** `:186-195` ⇒ `O(Σ_stamps bbox/res²)`, worst `O(T·cells)` for long diagonal tracks | bounded; efficiency item (D-9) |
| Completion passes | `route.rs:2323-2522` | ≤ `POUR_REACH_PASSES` 3 (`:2433`), unfreeze radius `hull + 2·(via_d + clearance)` `:2383` (= note §9), unfreeze scan `O(items·lost)` `:2402-2425`, budget `remaining = wall_clock − elapsed` `:2437-2442`, `timed_out` set when exhausted; unfreeze at most once `:2378-2381`; no deadline inside `check_pour_reach` itself | ok (≤ 4 checks + 3 passes) |

No exponential or permutation stage; no all-pairs board loop. Scaling (coordinator run): L1 ratios 16.9/12.8, L2 18.9/18.4.

## 2. Tasks 7–9 vs the design note (P1–P8, §7, §9)

P1 pour goal = the via itself (`search.rs:1223-1225`, `SlotPath.sites.len()==legs.len()`) ✓. P2 leg ids / `wire_of`,
`conn_wires` in `MultiGraph` (`multigraph.rs:46-49`, `:404-447`) ✓. P3 order `ObstacleSet::build → via_sites →
MultiGraph::build_on` with `add_site_anchors` first (`route.rs:2608-2650`, `multigraph.rs:119`), sizes = union over live
nets descending (`route.rs:2622-2630`), fit checker at margin ε (`:2631`) ✓. P4 bump every edge of touched faces +
`along` faces (`graph.rs:596-598`, `set_site` `:363-407`); proximity test per via (F4, `negotiate.rs:409-470`) ✓.
P5 `BlockInfo { edge, site, ghost, last_reached, static_reason }` ✓. P6 `Terminal::Site` (`route.rs:517`) ✓. P7 `Snap
{ mg, metr }`, `sync_anchors` on restore (`route.rs:1667-1688`, `:3023-3026`, `multigraph.rs:517`) ✓. P8 `via_cost`
final per via, `via_weight(0|1)=2.0` (`search.rs:64`, `negotiate.rs:592`) ✓. §7 batch rule as written (phase 1 removes
every active connection, frozen snapshot, in-order commit, Blocked final) ✓. §9 as written (fixed routed copper via
`with_routed_copper` `pourcheck.rs:842`, `site_in_main` restriction `route.rs:2701-2705`, scope pads+vias) ✓.

Deviations, all recorded in the ledger (not silent): via_sizes formula (Ruling P6), reduction overuse `≤ best` instead of
`== 0` (Ruling P8, `route.rs:1821`), listed same-net pour vias not used as goal vertices (`route.rs:2730`, Task 9 report),
pour heuristic on the site bbox (Task 8). **Silent deviations found:** (a) `in_pour` tests `PourFill.outer` only
(`multigraph.rs:210`) although `PourFill.holes` exists since §9 — a site inside a fill hole (foreign-copper knockout wide
enough for a via) counts as "in pour"; the §9 check catches the lost via afterwards, so correctness holds at the price of a
completion pass (D-4). (b) Note §7 amendments do not describe the ghost precompute in phase 2 with the `usable` validity
reuse at commit (`negotiate.rs:393-399`, `:260-280`); behaviour is result-neutral (D-7, doc).

## 3. Review Focus tests 1–5

1. `route_b_cu_pads_route_without_via` (`route_layers.rs:126`: 1 routed, 0 vias, all tracks on 31, `layers == ["B.Cu"]`),
   `route_mixed_layers_uses_one_via` (`:154`: exactly 1 through via 0.6/0.3, `via_sites` = via position, F+B lengths) ✓.
2. `negotiate_site_contention_two_nets` (`:231`: both routed, 1 via each, different sites, `iterations ≤ 50`, run-to-run
   identical) ✓; `site_rip_up_frees_owner_for_blocked_connection` (`negotiate_loop.rs`) covers the rip path ✓.
3. `via_sites_respect_keepout_npth_and_via_holes` (`via_sites.rs:173`: no site inside the `no_vias` keepout, ≥ hole_clearance
   from the NPTH, ≥ hole_to_hole from the via drill; `via_fits` false at each) ✓.
4. `route_pour_completion_without_site_fails_cleanly` (`route_layers.rs:325`: reason `"no via site inside pour"`,
   `iterations ≤ 2`, other net routed) ✓.
5. `iteration_batch_commit_invalidates_shared_edge` (`negotiate_loop.rs:701`: `resolved_sequentially == 1`, one detour, same
   for threads 2/4), `batch_validation_accepts_unaffected_paths` (`:814`, revalidated path equals the sequential reference),
   `route_threads_byte_identical` (`route_threads.rs:63`: threads 1/2/4 + repeat) ✓.

## 4. Determinism

No `HashMap`/`HashSet` in `route.rs`, `negotiate.rs`, `search.rs`, `multigraph.rs`, `vias.rs`, `pourcheck.rs`. The A*
`closed`/`best` are hash sets used for membership only (`search.rs:627-632`); heap key is total `(f, h, layer, edge, slot,
dir, id)` (`:604-613`). `relax.rs:727-746` iterates `HashMap` keys but only into a `BTreeSet` (sorted). All rayon uses collect
in index order: `search_all` `negotiate.rs:375`, `ghost_ref` `route.rs:2774`, `check_against` `check.rs:515`, layer builds
`multigraph.rs:125`, rasters `pourcheck.rs:459` → `BTreeMap`. Commit proof (note §7 (i)–(iii)): phase 2 reads only `&mg`,
`&obs`, `&history` and the `dead` clone; phase 3 is sequential over `todo` order with decisions that are functions of
`results` and the graph built by earlier commits (`negotiate.rs:215-334`); `activated` kept in commit order. Histories are
updated in `BTreeMap`/index order (`route.rs:2900-2917`). Pour-completion passes are pure functions of the model (ids
renumbered by `drop_conns` `route.rs:2196`, merged in order `:2225`). The only non-determinism is the wall clock
(`timed_out`, and the `remaining` budget of completion passes `:2438`), as in M2. Verified: eprom threads 1/2/4 + repeat
(gate), wiccon 1 vs 4 (gate), `route_pour_reach_completes_pad_cut_off_by_foreign_track` 1 vs 4.

## 5. D1–D10 and the ledger's deferred minors

D1 terminal clr/r (`route.rs:2677-2684`) ✓. D2/D10: Ruling P3 stands — the planar search still prices a statically unfit
edge as overuse (`search.rs:1841-1850`), only the ghost treats it as soft (`:1413`, `statically_unfit_edge_is_hard`
`search_sleeve.rs:224`); `hard()` is used by the D10 bend test only (`:1713`). **Strict D2/D10 retest on two-layer eprom
not done**: there is no toggle, it needs a code change (make `!fits(e)` reject in `push_state`), so it is not cheap read-only
— D-1 below. D3 `clearance_bend` in `direct()` ✓ (`:2015`). D4 `tall_channel_board` ✓. **D5: `route_split_vertex_bend_
fails_connection_once` (`route_engine.rs:740`) no longer pins the failure** — front-only fixture, but `:776` accepts
"routed" or "failed"; the split-vertex report path is now covered only at sketch level (`:309`) — D-2. D6 stubs from
`last_reached` (Ruling P4) ✓. D7/D8/D9 ✓ (`PROBE_STEP_CAP`, deadline per step). Rule-T via test still hand-inserts the
strand (`search_layers.rs:636`); no dedicated e2e counter-example, though `negotiate_site_contention_two_nets` and every
`route_layers` test run `clean()` + planarity — D-3. Note §7 amendments for ghost precompute / dead set: dead set written
(F3 amendment), ghost precompute not (D-7).

## 6. Harness

Snapshot store: `board_pcb` verifies MANIFEST sha and the pin (`paths.py:47-72`); only `harness/snapshot.py` reads
`~/work/kicad/projects` ✓; gate tests hash the snapshot dir, so user projects are never touched ✓. Minor: `snapshot_board`
raises for an existing pinned snapshot whose source has since been edited (`snapshot.py:33-37`) — D-8.
DRC attribution: baseline = ripped-unrouted board, key `(type, sorted uuids)`, router uuids never pre-existing
(`drc.py:92-106`, `test_router_item_never_preexisting`) ✓. **`preexisting_unconnected` mislabels router failures**: in the
ripped-unrouted baseline every ripped connection is unconnected, so a pair the router failed to route is "pre-existing"
(`drc.py:118-127`; the docstring says so). The private-badge-b run above prints `preexisting unconnected 49: Pad 0 [GND] of SW5 <->
Pad 1 [GND] of J3 …` — these are the 83 failed connections, not board defects; README column "preexisting_unconnected 53"
reads as board defects. The gate asserts the raw `unconnected_items` (`drc.py:159`, `test_m3_gate.py:30`), so **no gate
number depends on it** — B-3. Locked-copper test asserts what it claims (geometry identical `test_locked_copper.py:105`,
no routing violation pairing a locked uuid with a router uuid `:113-126`) ✓ — **but it is in no verification run** (B-1).
M3-R13 downgrade: `test_m3_router_other_boards_complete` asserts report present + `routing_new == 0`, prints pcb sha,
routed/failed, preexisting, `router_s (<=65: …)`, timed_out, detour stats; `test_m2_gate.py:80-83` prints instead of
asserting `≤ 65` — matches the ruling text ✓.

## 7. Gate tests vs plan numbers and rulings

eprom (`test_m3_gate.py:21-52`): every "M3 gate number" asserted (nets 25/25 + GND `pour_served`, routed = all = measured
hand wires, partial/failed 0, `routing_new 0`, `unconnected_items 0` after refill, width_match 25/25, arcs ≥ 1, no `SS`,
outside ≤ hand, `router_s < 2`, `threads_used == 4`, `timed_out` false, `iterations ≤ 50`, zero_slack printed and
DRC-accepted per M3-R4, threads 1/2/4 + repeat byte-identical, PNG, sha) ✓. wiccon (`:54-73`): `routed ≥ 0.9·connections_total`
(M3-R5, pour completions included, signal-only printed), `routing_new 0`, `router_s < 10`, `timed_out` false, unconnected
printed ✓; **threads: only 1 vs 4, no `2` and no repeat** (plan: "Both: two runs and threads 1/2/4") — B-2. M2 gate:
`threads_used == r["threads"]` (M3-R6) ✓ and all M2 numbers kept ✓. M3-R12 ratchet pinned (`route_engine.rs:611-612`:
≤ 67.8 mm and ≤ hand) ✓. Scaling: layers 1|2, ratios ≤ 40 / ≤ 30, `vias == routed` on L2 ✓. Nothing relaxed beyond rulings.

## 8. Slot-search cost (private-badge-b 162 candidates/query, 94 % of search time)

Correctly listed as M4 carry-over (1) under M3-R13/R14 (STATE.md:92-94); it is not a cap problem (caps hold, F2 removed the
ghost's share) but the per-expansion constant: `los` over the apex chain calls `clearance_bend` (`search.rs:1514`) per
candidate state, each merging `(C+1)` sleeve boxes of vertex + wall candidates (`:1480-1512`) and testing all of them.
Fix shape (M4, one S task + profile): (a) per-layer static "free radius" per face/edge precomputed once (`O(V·q)`), so a chord
whose sleeve lies inside the free radii skips the candidate scan; (b) memoise `clearance_bend` per search on `(portal edge,
side, apex vertex)` — the same chord is rebuilt for every slot of an edge; (c) order candidates by chord parameter and stop at
the first violation instead of scanning all (the `offer` closure already keeps the minimum). Expected ≥ 5× on private-badge-b;
measured target per M3-R14 (N/N, ≤ 10 min).

## Findings

**BLOCKING (M3 gate)**

- **B-1** `tests/test_locked_copper.py:90` — the gate-marked locked-copper test is outside `tests/gate/` and excluded by
  `pytest.ini:7` (`-m "not gate and not scaling"`), so the M3-R13 gate composition ("+ locked-copper test") is never run by
  the verification command (`pytest tests/gate …` collects 13 tests without it). Fix: `git mv` to
  `tests/gate/test_locked_copper.py` (or add the file to the gate command in STATE/plan); 1 line. It passes here (4/4).
- **B-2** `tests/gate/test_m3_gate.py:67-72` — wiccon asserts threads 1 vs 4 only; plan "M3 gate numbers": both boards, two
  runs and threads 1/2/4. Fix: `for t in (1, 2, 4)` + one repeat run, compare all; 3 lines.
- **B-3** `harness/drc.py:118-127`, `harness/bench.py:413-416`, `harness/progress.py:36-38`, benchmark README column —
  `preexisting_unconnected` counts pairs unconnected in the *ripped* baseline, i.e. the router's own unrouted connections,
  and reports them as "pre-existing" (ruling direction: pre-existing never means router errors). Fix: baseline for
  unconnected = the DRC of the original board (`--drc-orig.json`, already produced), or rename the bucket to
  `unconnected_unrouted` and keep `unconnected_new` for pour losses; ~10 lines + `test_preexisting.py:56` + README note.

**DEFERRED (M4)**

- **D-1** Strict D2/D10 retest (Ruling P3) on two-layer eprom: needs a `cfg`/option toggle; measure once, then decide.
- **D-2** D5 e2e pin lost: `route_engine.rs:776` accepts either outcome; add a fixture where the split-vertex bend still fails
  (or assert the reason path via a sketch-level route).
- **D-3** Rule-T via counter-example only with a hand-inserted strand (`search_layers.rs:636`); add an e2e fixture.
- **D-4** `in_pour` ignores `PourFill.holes` (`multigraph.rs:210`); test holes like `pourcheck` does.
- **D-5** Vacuous assertion `route_layers.rs:562` (`"still without pour"` never emitted; the warning is `"without pour after"`).
- **D-6** Listed same-net pour vias as goal vertices (`route.rs:2730`, note §2 "Goals"); ledger Task 9.
- **D-7** Note §7: document the phase-2 ghost precompute and its `usable` reuse (`negotiate.rs:393-399`, `:260-280`).
- **D-8** `snapshot.py:33-37` fails on an existing pinned snapshot after the source changed; verify only on creation.
- **D-9** `pourcheck.rs:186-195` capsule stamps scan the full bbox (long diagonal tracks: `O(L²/res²)`); scanline per row;
  and a per-run cell budget for boards with many pours (no deadline inside `check_pour_reach`).
- **D-10** `multigraph.rs:381-387` `alloc_legs` sorts and `remove(0)` per call; use a `BTreeSet` free list.
- **D-11** Slot-search per-expansion cost (§8), private-badge-b N/N (M3-R14), fine-pitch entry (#201), detour items M4-D1..D4
  (M3-R15) — as STATE.md M4 carry-over.
