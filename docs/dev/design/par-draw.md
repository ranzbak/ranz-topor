# PAR-DRAW: parallel drawing, copper identical for any thread count (design note, branch par-draw)

Base main c41e1523. Rebase targets: tw2-metric (joint band commit, `realise = "erbs"` default) and tw3-f67 (F6/F7).
Status: draft for the lead, no engine code yet. Evidence: throw-away branch `par-draw-exp` = main + tw2-metric c5393fe3
+ tw3-f67 ff1c6fce (both merge cleanly) + trace hooks (TOPO_TIMES sub-phases, `TOPO_PDTRACE`: per drawn wire its time,
the cells its spatial queries read, the cells its copper writes, the wires whose drawn state it read). Runs: hz tags
pdexp-5c809f2, pdexp2-5dcdf3c (1 thread, seed 0, rounds=1 runs=1, shared ccx43, so the seconds are indicative only).
Simulations: tools/pardraw_sim.py (in-order windows), tools/pardraw_dag.py (dependency DAG, dataflow bound); traces in hz-runs/pdexp*.

## 1. Where the drawing spends its time (tw2 + f67 merged, qmtech run 2, esp run 2; seconds, 1 thread)

| sub-phase of "drawing + feedback" | qmtech | esp | kind |
|---|---:|---:|---|
| setup, plan metrics, compass; via + branch relocation | 1.6 | 0.5 | sequential |
| phase-1 vias (`place_vias`, 320 / 90 wires) | 0.18 | 0.07 | per wire, 0.1 ms each |
| ERBS `cutflow::measure` (sheets, bands, flows per layer) | 3.65 | 0.67 | per layer, per stretch: order-free |
| ERBS stretches (`erbs_stretch`, 411 / 114) | 3.51 | 0.37 | pure per stretch: order-free |
| ERBS check_layer, precedes | 1.4 | 0.2 | per layer |
| joint: band realise / octilinear form (tw2) | 3.05 | 1.80 | per band against a fixed index: order-free |
| joint: commit + check, selector, losers | 0.27 | 0.04 | small |
| per-wire loop (`draw_wire` in `live` order) | 13.9 | 4.2 | order-dependent; octilinear is 11.2 / 4.5 s of it |
| never drop (19 / 9 forced wires) | 6.1 | 3.3 | order-dependent |
| **total** | **33.7** | **11.1** | |

Plane stubs (qmtech 2.73 s): board build 0.51, pour reach 0.75, select_far 0.88, resite 0.17, the 99 `planes::place`
calls together 0.11 s. After F7 the stub requests are not where the time goes, so the per-request scheme gains nothing
there (esp: 61 requests, 0.02 s). Stubs stay out of this branch unless bornhack shows otherwise.

## 2. Two kinds of units
**A. Order-free units (exact by construction).** An ERBS stretch, the sheet/band/flow work of one layer in `measure`,
and the realise + octilinear form of one band in `reserve_bands`. Each reads only state that no unit of its stage
writes: the plan, the sheets, and an index that holds static copper and vias only. Dayan 1997, PDF p. 17: in
simultaneous routing "all nets are routed at the same time and therefore no net gets higher priority than others";
TopoR 6.3 manual p. 175: FreeStyle computes "optimal wire geometry ... automatically" from the topology; tw2 invariant
I2 (kept set and band copper do not depend on `live`). Execution: rayon `par_iter` over the units, results collected
**by unit index** (`collect::<Vec<_>>()` keeps index order for any thread count), then today's sequential fold (pushes,
counters, `precedes`, the static class, the test hook `fail_first` = index 0) unchanged.
Proof: every unit is the same deterministic function of the same inputs as in the sequential loop, so it returns the
same value; the fold sees the same values in the same order, so its output is bit-identical. Nothing else changes.
Needs: `Drawer: Sync` for the shared `&Drawer` (`only_static: Cell<bool>` -> `AtomicBool`; it is set only in the
TOPO_DIAG_DEEP debug diagnosis), and the f67 test toggle `OCTI_REF` (thread-local) read once on the calling thread and
passed down. Time limit / Cancel: a unit starting after the limit is counted `timed_out` as today; under a passing limit
the cut set depends on timing (it does sequentially too); without one it is identical.

**B. Order-dependent units: one wire of the per-wire loop (and of never drop).** Its drawing reads the copper of the
wires drawn before it and whether its lane neighbours are drawn (reserve planned room). Dayan PDF p. 17: sequential
routing is sensitive to the order. Here a unit may run early only if nothing drawn between its snapshot and its turn
touched what it read.

## 3. Independence test for B and why it is exact
Mutable drawer state M: the item index `idx` (cell vectors per layer key, obstacle slots, free-id heap), the drawer
grid `grid` + `ents` + `handles` + `ent_wire`, `drawn_x`, `drawn_once`, the tw2 `joint` reservations, and append-only
logs/counters (`stats`, `diag`, `fails`, `erbs_long`). `class_ref` is a function of static data only (erbs bands,
`prov_layer`, plan chords): built once before the loop.
- **Read set R_i** (logged while unit i runs): for every `grid` query its cell range grown by the query's one-cell pad
  (exactly `Grid::range(bbox, 1)`); for every `check_against(&idx, items)` the idx cell range per layer key of every item
  (`check_one`'s `bb` for keys {layer, WILD}; one helper in check.rs computes these boxes for both, so they cannot drift);
  a BIG marker when a query returns the side list of oversized items; the wire ids whose state it read: `drawn_once[o]`
  (7 sites), `live_x(q)` (lane neighbours), `via_marks` liveness (`via_walls`, `track_own_pad_ok`), the class-test
  closure, and in core.rs the trunk read by `joint_of`.
- **Write set W_j**: the cell ranges (per grid / idx layer key) of every item it commits or uncommits (an uncommit
  rewrites the same cells: `swap_remove`, the `live` flag), BIG when an item is oversized, and its own wire id
  (`drawn_x` of its passes, `drawn_once[j]`, its `joint` entries; an uncommit of another wire's handle adds that wire).
- **Test:** unit i, speculated on snapshot version s, is valid at its turn iff R_i meets no W_j of the units j committed
  after s (same tag, ranges overlap; BIG meets everything; wire ids equal). Over-approximating R or W only causes redos.

**Exactness (induction over the commit order).** Claim: after committing units 0..i-1, the master equals the sequential
state. Unit i valid: (1) every read of i returns the same value on the snapshot as on the master, because a value can
differ only at a location some committed j wrote, and those locations are in W_j, which R_i misses. Grid queries: the
result set is the cell contents of the range; the order is cell order (x then y, ascending) times the in-cell vector
order; both branches of `Grid::query` (range scan, or sorted occupied keys when the range is larger than the occupied
set, a choice that reads the global `cells.len()`) visit cells in the same lexicographic order, so that global read does
not change the result. (2) Ids: `ents`/`handles` are append-only, so i's own ids are base + t on both, a constant offset
that preserves every order; base ids are equal; the other units' ids never appear (their cells are not read). idx ids
come from a smallest-free heap that other units' uncommits change; they enter i's results only as identity (handles,
skip lists) and as labels `item:<id>` of violations, and every item i's checks can name is a base item (equal id) or
its own (same net, exempt). The replay below re-derives own ids on the master, and a violation label naming an own item
is treated as a conflict (redo), so no proof step relies on the exemption. (3) Same reads, same deterministic code
(no clock, no randomness, no hash-order iteration in the drawing; the f64 timing stats are not results) ⇒ the same op
sequence: `commit(item)`, `uncommit(handle)`, `drawn_x[pid] = (x, h)`, `drawn_once[w] = true`, joint ops, log appends,
counter deltas. **Commit = replay of that op log on the master** (handles remapped by the offset), which leaves the
master exactly as running i there would. Unit i invalid: it runs again directly on the master. Either way the claim
holds for i+1. Lane count, schedule and completion order never enter, so the copper and the whole result are those of
the sequential loop for every thread count.
Completeness guard (tests): `Drawer::state_hash` over every mutable field (cell vectors in order, free heap, ents
live flags, drawn_x, joint, logs), compared with the sequential run after every commit (debug env
`TOPO_PARDRAW_VERIFY=1`, on in the determinism tests). A missed write channel fails there; a missed read channel shows
as a different `WireOut`.

## 4. Commit order
A: unit index order (stretches sorted as `own`, layers as `m.layers`, bands in `order` x runs), i.e. the current loops.
B: the current `live` order (tier, wide, short; trunks before branches; tw2: the precedes order). The core-side work per
wire (`expired()`, test points, `fail`, `settle`, `drawn`, `via_h`, `failed_draw`, progress) runs at commit, in that
order, exactly as today; a unit after the limit fails with `draw_cut_reason` and its speculation is dropped.

## 5. What stays sequential
Via reduction (accept-first relabel sweeps: a par-refine-style candidate stream is a separate item), fuse, polish,
never drop (measured DAG bound 1.43x qmtech / 1.15x esp: one forced wire of 0.77 s dominates; intra-wire candidates are
a follow-up), the `cycles` feedback, junction joins, the selector and the losers' octilinear forms in `reserve_bands`,
via/branch relocation, plane-stub resite/merge and select_far.

## 6. How much B can gain (measured dependency structure)
Units conflict mostly with their neighbours in the draw order: wide-first/short-first puts a bus's lines next to each
other. Conflict with the wire just before: qmtech 148/319, esp 50/89 (random pairs: 9 % / 37 %). The dependency DAG
(edges = traced conflicts) bounds any exact scheme. Critical path of the per-wire loop: qmtech 4.6 of 10.0 s (max
2.2x; run 2: 5.5 of 12.2 s), esp 2.1 of 2.5 s (1.2x), wiccon 0.28 of 0.44 s (1.6x). Dataflow schedule with perfect
dependency knowledge at 8 lanes: qmtech 1.67x, esp 1.19x, wiccon 1.53x. Plain in-order windows (no dependency
knowledge): ~1.0x (qmtech redo rate 57 % at 8 lanes). Never drop: qmtech 3.2 of 4.6 s (1.43x), esp 2.6 of 3.0 s. So B needs a predicted footprint per wire (its sleeve's triangles + clearance + one grid
cell) to schedule (a unit starts only when no uncommitted earlier unit's footprint meets its own); validation stays
the exact test of section 3. Replicas: one `Drawer` clone per lane per start (qmtech 3 613 ents, 2 221 idx items).

## 7. Expected drawing time at 8 threads (Amdahl on section 1; parallel parts at 85 % efficiency)

| board | drawing 1 thread | A only (8 thr) | A + B (8 thr) | router run share saved (A / A+B) |
|---|---:|---:|---:|---|
| qmtech | 33.7 s | ~26.4 s (1.28x) | ~21.8 s (1.55x) | 9 % / 14 % of 83 s |
| esp | 11.1 s | ~9.0 s (1.24x) | ~8.4 s (1.32x) | 8 % / 10 % of 27 s |
| wiccon | 1.5 s | ~1.2 s | ~1.0 s | small |
| bornhack | 567 s base main 27c36203 (seq realise, before F6) | unknown | unknown | needs its split |

`measure` counted at 2x (2 layers; per-layer units), stretches and bands at 7x, loop at 1.5x (between the 1.67x
dataflow bound and a predicted-footprint scheduler). eprom: 0.28 s, nothing to gain. Bornhack: f67q2 (seq realise, 4
threads) drew 94 wires by force; never drop is likely a large serial share there. Without its split I cannot size A or
B on bornhack honestly.

## 8. Tests
- Determinism (in the suite, release, < 60 s): approved m3/erbs fixtures + a 2-layer synthetic bus (8 parallel wires,
  interacting by clearance, from `harness/fixtures.py`, needs user OK as a pattern) routed under rayon pools of 1/2/4/8
  threads (`ThreadPoolBuilder::install`): copper hash and the result JSON (ERBS line, `route.erbs`, warnings) equal;
  `TOPO_PARDRAW_VERIFY=1` state hash equal per commit (B). Test hook `"lanes reverse"`: each unit sleeps (n - i) ms, so
  lanes finish in reverse index order.
- Mutants (must fail the determinism test): fold in completion order (A and B; caught with "lanes reverse"); commit
  without validation (B); a read site not logged (`live_x`, one `drawn_once`, the grid pad 0 instead of 1); idx boxes
  without the WILD key; replay without the handle offset; BIG not a conflict.
- Boards (VM): copper hash at RAYON 1/2/4/8 equal and equal to main (cache) on eprom, wiccon, esp, qmtech, seed 0,
  rounds 1. Timing: exclusive ccx43, RAYON 1 vs 8, TOPO_TIMES sub-phases, qmtech + esp; bornhack one 4-thread run at the end.

## 9. Rebase plan
A touches `erbs::erbs_layer_with` and `cutflow::measure` (tw2 adds 515 lines to erbs.rs) and `joint::reserve_bands`
(exists only on tw2). So A is built on tw2-metric's tip, rebased when it merges. f67: only the `OCTI_REF` toggle and
`Drawer: Sync`; no change to octi.rs/exact.rs logic. B touches draw.rs query sites that f67 rewrote (`exact::Near`,
repair memo): wait for f67 before B. TW3 table: rows are drawn by separate runs, each with its own `Drawer`; A's
`par_iter` and B's lanes run on the one rayon pool (work stealing, no extra threads), and each row's copper is
independent of the lanes, so the table is unchanged.

## 10. Questions for the lead
- Q1: Build A now (on tw2 tip), measure, then decide B? B's measured ceiling is 1.2-1.7x of the per-wire loop on
  esp/qmtech, for the largest and riskiest change (draw.rs read logging at ~30 sites, replicas, op-log replay).
- Q2: One bornhack trace run now (the exp branch, 4 threads, ~10 min, < EUR 0.1) to size A and B there, instead of
  waiting for the milestone run at the end? Bornhack is where the 45 % drawing share is.
- Q3: Octilinear is 80 % of the per-wire loop (qmtech 11.2 of 13.9 s). Parallelism inside one `octilinear()` call (its
  DP over node pairs, which f67 restructured) is exact by construction and has no cross-wire dependency. Should I profile
  it as an alternative or complement to B?
