# loop-local: price a TopoR-loop re-lay locally, layer the whole board once per cycle

Branch `loop-local` (off main 2f5a61ae). Evidence: scratch/agent-wt/reports/loop-profile-20261118.md
(F1, F1b, F9).

## What
Today `Rr::violations(after)` judges every re-lay (sweep, via-rose relay, pair_min relay) and
`Sp::pair_move` judges every pair move with a **whole-board** layering: `assign_t` = P3 solve +
capacity refinement over every wire, then `measure` = a whole-board CapModel. On qmtech that is 331
layerings per cycle, 80 % of the 37.9 s cycle, and the per-move cost grows with the board (O(n^2)
cycle). The untouched wires keep their layer 99.96 % of the time.

**Step A (exact, F9).** A per-wire cache of `stretches_and_vias` (pieces, via points, slots,
no-site count). Key = everything the function reads for that wire: alive, net, tris, polyline
points (bit-equal), events (bit-equal), the via parameters. Copper must stay bit-identical.

**Step B (method change, F1).** `[optimize] loop_judge = "local"` (default) | `"global"` (today).
A move takes wires O out and lays N. The affected set F = N ∪ T, where T = the different-net
crossing partners of O (before) and of N (after) plus their junction partners (trunk/branch).
1. Local layering (Dayan 1997 pp. 55-57: "only the optimal candidate assignments for the 2-Nets
   affected by this assignment are recomputed"): every wire outside F keeps its events and labels
   exactly. The events of F are built afresh. A crossing of an F wire with a fixed wire becomes a
   fixed-orientation event (2 layers: node 0, `pi = 1 - label`; N layers: mask minus the partner's
   layer, empty mask = unlayerable). The same solvers then run on F alone: the exact domain-graph
   solve with ties for 2 layers, greedy + alpha-beta for N. The tee rule and `tree::fix_layers` run
   on F's junctions. The capacity refinement runs on a CapModel built over W' (the wires with a pass
   on an edge of F, before or after), with every event outside F masked to its present layer.
   The result is a full LayerReport: the partners are remapped by crossing identity, and the
   stretches come from the Step A cache.
2. Local price: pred_after = pred_before + Δ, and vias_after = vias_before + Δ. Each Δ is
   measured the same way on both sides, the before side on the restored before topology:
   - the CapModel over W' (graded over-full edge-layers + vias without a site);
   - the unlayerable crossings of F;
   - the foreign-copper traversals of F (per wire, the same `foreign_copper` rule);
   - the forced set (O(1)).
   The move's price is then the formula it has today: own path price (oscillating coefficients,
   Polubasov 2010 «Перекладка проводников», S = Σ kᵢxᵢ) + extra + viol_mm × graded + via_mm ×
   vias. Only the way the after-state layering is obtained changes.
3. Global layering stays. It is the escape from local minima (Polubasov 2010 «Оптимизация
   расслоения»: it acts globally, never worsens the board, and runs often but takes ≤ 50 % of the
   time). The cycle's stage 2 re-optimisation `assign_t(seed|1)` + `try_layers`, the uncross
   re-layer, the via-rose follow-up and the crossing pass are unchanged. One whole-board **measure**
   (a CapModel build, no layering) at the end of the sweep re-syncs `pred`, so `try_layers`
   compares exact numbers and every archived variant has an exact measure.
   - `cuts` capacity model: falls back to the global judge, with one warning line.
   - `[optimize] loop_judge = "global"`: today's path, bit-identical.

## User rule 2026-11-18 (no worsen-veto)
The local price adds no veto: the keep test is the one main uses (strict descent of the move's
price). As it stands today, main rejects worsening moves. `reroute_set` keeps a move only when
`new + extra < old`. With the oscillating per-wire coefficients and the fluctuating via weight, a
kept move can still worsen the fixed board evaluation (that is TopoR's "раскачивание"), but a move
whose own price rises is undone. `try_layers`, the crossing pass (`p4_better`) and `pair_move`
(`e1 + price < e0`) also undo a worsening. Accepting worse moves, with the bests kept on the side
(overall; fewest errors; shortest + fewest errors; least vias + fewest errors), is left to the
plan. This branch does not touch it.

## Invariants
- Outside F, events, labels, masks and partners are unchanged (local mode).
- Every different-net crossing between F and a fixed wire ends on two layers, or it is counted as
  unlayerable.
- A null move (N = O's path, same labels) has Δ = 0.
- With F = every wire, the local solve equals `assign_with` with the same tie seed.
- `loop_judge = "global"` gives main's copper.

## Tests
- A: `stretches_and_vias` with and without the cache over a sequence of mutations (the path,
  events, polyline and net change). A mutant that drops a key field fails the test (Rule 9b).
  Copper hash = main on eprom, wiccon, esp, qmtech, bornhack.
- B: the invariants above as unit tests on small fixtures (2 and 4 layers, a tee). A counter
  test: global layerings per cycle ≤ 4 on a fixture.
- B, boards: eprom/esp/qmtech counters (global layerings, local W' sizes, work per re-lay) show
  linear cycle work. A ladder at 100 % drawn compares violations by class, forced pieces, vias,
  length and detours per connection against main (`ladder_diff --json`). Seconds come only from
  an `--exclusive` ccx43, else they are marked shared-load.
