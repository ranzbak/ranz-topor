# NEVER-DROP-PERF: why forced draws are slow, and an exact fix (design note, branch nd-perf)

Base main 551add2e. Status: phase 1 done (measurement, root cause, throw-away prototype). No engine code on this branch yet.
Evidence: throw-away branch `nd-perf-exp` (cd7315b3 = `TOPO_NDPROF` hooks: count + seconds per bucket, one line per forced wire;
d931ade9 = prototype behind `TOPO_NDBB`). Runs on the shared ccx43, rounds=1 runs=1, bornhack 4 threads, qmtech 1 thread:
base `hz-runs/ndp-cd7315b`, prototype `hz-runs/ndb-d931ade` (`-bb` = prototype, `-chk` = prototype plus the old pricing for every
pick, compared). Shared VM: the seconds show the size of the effect, they are not benchmark numbers.

## 1. Measurement on main (base run ndp-cd7315b)
| | bornhack (4 t) | qmtech (1 t) |
|---|---:|---:|
| drawing + feedback | 193.5 s | 29.3 s |
| never drop (forced wires / forced runs) | 172.6 s (81 / 105) | 4.25 s (19 / 19) |
| of that: `check_against` calls, seconds | 1.88 M, 155.4 s | 67.7 k, 2.4 s |
| of that: `place_vias` (of it `via_reach`) | 8.4 s (7.0 s) | 1.08 s (1.0 s) |
| exact search (`exact_fix*` + `local_path`), octilinear | 2.8 s, 5.1 s | 0.37 s, 1.17 s |

TW2 did not lower bornhack's forced count (81 wires, as in pdbh-dc90733). Slowest forced wires (s): 519 63.9, 520 30.6, 518 16.6,
481 9.6, 446 7.4, 469 6.8, 456 4.2, 401 4.2, 467 4.0. In each of them 85-96 % of the time is checker calls
(519: 420 k calls, 61.3 s of 63.9 s).

## 2. Root cause: every forced candidate is priced in full, and only the winner's price is used
Search space, retries, the fallback chain and board builds are not the cause (exact search, octilinear, repair and sleeves
together about 11 s on bornhack; never drop builds no board).
The cost is **pricing the candidates** (`draw.rs` `draw_wire_runs`, force mode). For a run with no legal attempt, every candidate
(the ERBS, ladder attempts 0-5, up to 6 relaxed exact paths, 3 funnels) gets `viols_of` before the selection. `viols_of` measures
each raw checker violation's length with 2 probe `check_against` calls (every 0.5 mm, max 48 probes, then every 50 um around the hits,
max 40 per hit, or max 400 when nothing hit). The `min_by` reads only the winner's violations. The losers cost the most:
wire 519, F.Cu run k 1..213: three "attempt" candidates (funnel + repair, about 3000 points, about 630 merged violations, price about
450 k) against funnels of 120-214 points at price 3.4-4.1 k. Over the never-drop pass on bornhack: 1.13 M raw violations, 10.7 M probe
items, `viols_of` 130.8 s of 142.7 s of `draw_wire` (`-chk` run, which prices every candidate the old way).
Second cause: `via_reach` prices its reach segments with `viols_of` at every candidate via site. Its price is only used through
`reach_cost`, and `reach_cost` returns 0 while `[vias] reach_price` is off (the default). The class verdict `ok` reads only the report.

## 3. Exact fix (same copper)
**F1, a lazy price with a lower bound.** A forced candidate keeps its checker report (and its graze violations, which are cheap and
exact). Its lower-bound key is built from the same report without probes. `viols_with` with probing off sets each track violation's
length to its floor `tw.min(len.max(tw))`. The exact length is `max(measured, that floor)`, so the floor is never more than the exact
length. The grouping is `merge_viols` as before, and depth, kind, group count and via lengths are exact. `violation_amount`
= 1 + depth x len / A does not fall when len grows. The merge sums lengths in the same order, and float addition is monotone in each
term, so lower bound <= exact for (short, total, price), also in floating point. Selection: go through the candidates in ascending
lower-bound order (index breaks ties). Price each exactly. Stop at the first bound greater than the best exact key, and skip a bound
that equals it at a larger index. The winner is the least exact key with the first index among ties, which is the `min_by` result
(`Iterator::min_by` returns the first minimum). The winner's violations are exact, as today. The later steps (octilinear form, pull,
necks) are unchanged.
**F2.** `via_reach` computes its price only when `via_reach_price` is on (or when `TOPO_VIADBG` is set, for its VIACAND line).
Invariants: copper hash = main on every board, each forced pick = the eager pick, lower bound <= exact. The never-drop semantics
stay (every candidate is still drawn and judged; only the measuring of losers that cannot win is skipped).
Literature: none needed; the cause was measured directly. F1 is a plain branch and bound over a finite candidate set.

## 4. Prototype result (ndb-d931ade, throw-away, same shared VM)
| | base | prototype |
|---|---:|---:|
| bornhack never drop | 172.6 s | 15.8 s (`draw_wire` 14.1, `place_vias` 1.6) |
| bornhack drawing + feedback | 193.5 s | 35.6 s |
| bornhack wire 519 / 520 / 518 | 63.9 / 30.6 / 16.6 s | 1.5 / 1.0 / 0.8 s |
| qmtech never drop / drawing + feedback | 4.25 / 29.3 s | 1.93 / 19.6 s |
| forced candidates priced (bornhack) | 760 | 238 (522 skipped) |
Copper: `copper_hash --cmp-run` = base for all 4 runs (bornhack, qmtech; `-bb` and `-chk`). Every pick checked: bornhack 105/105
forced runs the same pick as the eager selection, qmtech 19/19, and the bound held for every candidate. Bornhack routing_new reads
942 in `-bb` and 940 in base and `-chk` on the same copper; that 2-count gap is DRC noise past KiCad's caps (judge by tiles).

## 5. Implementation and tests (phase 2, after the lead's reply)
Code: `viols_with_opt(.., probe)` (draw.rs), `Drawer::pick_forced` (lazy + bound) replacing the `min_by`, candidates carry
`Option<CheckReport>`, plus F2 in `via_reach`. The old eager selection stays only as the test reference. `TOPO_FORCEDBG` prints
"lb" for unpriced candidates. No option, because the copper is the same.
Tests (release, fast set):
(1) A property test of `pick_forced` against the eager `min_by` on random candidate sets: equal keys, ties at several indices, short
against no-short, NaN-free.
(2) Lower bound <= exact on random pad fields (`exact.rs` `with_fields`): random tracks across pads and drawn copper, each key
component compared.
(3) F2: the reach verdict is unchanged with the price off, and the price is the same as before with it on.
(4) End to end: copper hash = main on eprom, wiccon, esp, qmtech and bornhack (VM).
Mutants: break on Equal instead of skip (tie order); lower-bound length = segment length (not a bound); no sort (index order with
break); `>=` for `>` in the stop; F2 skipping the price while `reach_price` is on.
Not in scope, priced for later: after F1/F2, bornhack never drop is 15.8 s (checker 4.3, octilinear relaxed 3.5, repair 2.5,
exact search 2.6, pricing 2.2). The via reduction (bornhack 17-25 s) and the feedback also call `viols_of` / `wire_viols`, which
could be the next exact candidate. The 3000-point repaired attempts are hopeless candidates (a quality note for the forced candidate
set, not part of this fix).
