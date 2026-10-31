# M2 gate review (F, 2026-10-31, HEAD ac31995)

Mandate: plan Task 11 Step 7 (complexity of every new loop vs 45 §5a, Tasks 6–7 vs `m2-search-design.md`, Review
Focus tests, determinism, STATE "Deferred to the F gate review" list) plus the one F diagnosis round of 45 §5a for the
failing scaling test. Measurements were taken with a scratch copy of the engine instrumented with stage timers
(`scratch/review/engine-instr`, harness `scratch/review/scale`, models `scratch/review/models/synth-*.json`, logs
`scratch/review/run-*.log`, renders `scratch/review/*.png`); nothing in the repo tree was changed. Confidence tags:
**[M]** confirmed by measurement/test, **[R]** suspected from reading.

## 1. Scaling diagnosis (`tests/scaling/test_route_scaling.py`, synth seed 1)

### 1.1 Where the time goes [M]

| n | wall | routed | PF iters | searches (expansions, cap hits) | search s | relax s | self-check s | probe s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 10 | 0.003 s | 10/10 | 1 | 10 (—, 0) | 0.001 | 0.001 | 0.000 | 0 |
| 30 | 22.9 s | 23/30 | 13 | 165 (5 710, 0) | 0.045 | **20.3** (iters 11/12: 200 relax iterations = 10.1 s + 9.8 s) | 0.014 | 0.42 |
| 100 | 26.8 s | 75/100 | 9 | 375 (61 798, 0) | 0.35 | **25.4** (iters 7/8: 12.4 s + 8.2 s) | 0.021 | 0.93 |
| 1000 | 120 s timeout | 0/1000 | **0** | 1000 (14 608, 0) | 0.13 | **119.9** (first call, never completes) | — | — |

Graph sizes: n=100 V 208 / E 617 / F 410; n=1000 V 2008 / E 6017 / F 4010. Decompose, obstacles, CDT and graph build are
≤ 4 ms even at n=1000. The slot search is cheap (6 µs per expansion, 15 expansions per search at n=1000; the cap
`8(E+X)+64` is never reached). **All of the time is `relax`** (`route.rs:289-375` → `relax.rs:350-445`), and the
PathFinder loop does not converge: at n=100 iteration 1 already fails 25/100 metrizations with **0 blocked searches**,
and the loop stalls (R6 `STALL_ITERS`) at 75/100.

### 1.2 Why connections fail [M] — the sketch→relax hand-off is not topology-faithful

Trace of synth n=30, iteration 1 (`PROF_PEN` trace, `scratch/review/emit-30.png`):

1. N4 (P4.1 (3,3) → P4.2 (9,3), straight through P11.1 (6,3)): the slot search passes P11.1 between P5.1 and P11.1
   (edge e49 @0, 1 crossing). `sketch_from_crossings` reduces it to **zero anchors** — the exactly collinear start /
   P11.1 / goal makes the funnel side test a tie (`sketch.rs:235-262`, `cross3(..) >= 0.0` / `<= 0.0`: a portal
   endpoint exactly on the string is no bend), so the touched vertex is not recorded (`sketch.rs:347-358,386-394`).
   `tauten` starts with the chain through the pad, `penetrations()` re-adds P11.1 with a
   geometrically chosen σ (`taut.rs:383-390`, `sigma_for`) — on a tie this is the side *opposite* to the slot the
   search assigned: the topology the negotiator believes in is silently violated.
2. The first relax pass gives N4 the arc radius **r = 1.740** at P11.1 (= 0.83 + 2 × 0.455): `Stacks::new`
   (`relax.rs:77-92`) makes *every* wire listed at an anchor a member, and `stack_order` (`sketch.rs:420-443`) lists a
   wire at both ends of every crossed edge — so wires that pass P11.1 on other edges count as "nested inside" until
   their own tauten runs. The bulged 71° arc sweeps across the direction in which N11 leaves its own start pad P11.1.
3. N11 (P11.1 → P11.2) is tautened next: its start segment intersects N4's arc (`dist 0`), and `penetrations()`
   (`taut.rs:418-430`) inserts the arc's anchor — **N11's own start pad** — into N11's sequence; N1 then hits N11's
   segments and inherits their *adjacent* anchors P6.1 / P11.1, 3–6 mm away in the opposite direction. The final
   chains loop around foreign pads (N1: 21 mm for a 6 mm connection, `SASASAS`, anchors P6.1/P11.1/P11.2).
4. Gauss–Seidel never settles (each pass moves the bulges and re-adds anchors) → `relax_iter_cap` 200 is hit (10 s per
   call at n=30, 25 s at n=100, > 120 s at n=1000); chains cross other chains (self-check `gap -250000` = centrelines
   intersect) or pass through pads (`gap -348333`), or `tauten` returns `infeasible: pass cap` / `pad <-> pad`.
5. PathFinder feedback cannot repair this: `penalise_failed` puts history on the failed path's edges, the re-search
   finds an equivalent class, metrization fails again → stall stop after 8 iterations without improvement.

The same mechanism, not the fixture, limits the gate boards: eprom converges only because its hand-like geometry has
slack (relax 2–4 iterations); dcsgone relax converges (2–4 iterations, 36 ms total) but see §1.4.

### 1.3 Is the fixture valid? [M] Partly

`synth.py` matches the plan's Interfaces spec except: the leftover-cell fallback (`synth.py:31-33`) produces connections far
over the 9 mm reach (n=100: max 33.1 mm, 2 > 9 mm; n=1000: max 90.3 mm, 28 > 9 mm). The grid is exactly regular: 42 % of
straight airwires pass within ρ of a foreign pad centre (mostly *through* the centre, n=100: 42/100; n=1000: 432/1000),
which is the collinearity tie of §1.2 step 1, and 53 % (n=100) / 69 % (n=1000) of airwires cross at least one other
airwire, so single-layer routability of 100 % at n=100 and ≥ 95 % at n=1000 is **asserted, not guaranteed** by the
fixture (random matching on a grid; 3 wires fit per 2 mm gap). Verdict: the engine is at fault for the measured failures
(the slot search finds planar paths; metrization breaks them), the fixture is adversarial (collinear ties) and its
routability assumption is unproven; the > 9 mm fallback is a fixture bug.

### 1.4 Real boards [M]

wiccon-2023: 8 live connections (52 "terminal not on layer F.Cu"), 0.028 s, trivial. dcsgonebadge (V 4552 / E 13 649,
34 live): 46.6 s = **41.0 s in searches** (225 slot searches; 59 ghost searches = 34.0 s, 0.58 s each; iterations 2 and 7
= 15 s each) + 5.3 s block probe; relax 0.04 s. The ghost search of an unroutable connection explores the whole graph
(cap `8E+64` ≈ 109 k) and every `push_state` runs `los` → `clearance_bend` (`search.rs:557-628`) with an axis-aligned
bbox of the apex→target chord grown by ρ_max: on a large board that query returns O(V) vertices and walls, so the search
is O(V·E) per connection, not O(V log V) (45 §5a "Search per connection"). This does not affect the eprom gate.

### 1.5 Root causes, fixes, expected effect

- **RC1 (BLOCKING for the scaling test) metrization is not faithful to the slot topology.** (a) `Stacks::new` initial
  membership = all listed wires → bulged first-pass radii; (b) `penetrations()` inserts anchors that contradict the
  slot order (a wire's own terminal pad; the far adjacent anchors of a penetrated segment); (c) the funnel drops bends
  at vertices the string touches exactly. Fix: derive the stack *from the slots* — the slot index of a crossing on an
  edge incident to anchor `a` is the exact number of wires nested inside at `(a, σ)` (`graph.rs` occupancy), so
  `Stacks` can start with the true membership instead of "everyone"; in `bends()` treat a touched vertex as a bend on
  the side the crossing's `left/right` dictates; in `penetrations()` never add the wire's own terminals and only add a
  dyn-segment's adjacent anchor when the penetration point lies within that anchor's ρ (else report the conflict to the
  *other* wire via its stack radius — the M1 F5 "fan" rule). Expected: relax converges in 2–5 iterations as on eprom /
  dcsgone (~0.1–0.15 ms per wire-pass), n=100 iteration-1 metrization failures 25 → ~0, n=1000 first relax ≈ 0.5–1 s.
- **RC2 (BLOCKING for the scaling test) metrization is global per PathFinder iteration.** `route.rs:1420-1423` relaxes
  *all* routed wires every iteration (`O(iters_PF · relax_iter_cap · m · k · q)` — polynomial, but 200·m tautens per
  iteration), and at n=1000 the deadline strikes inside the first call so `best == None` and every connection reports
  `timeout` (the anytime promise of 45 §5a is met only in letter). Fix: metrize incrementally — re-tauten only the
  connections searched this iteration plus their dynamic neighbours (grid query), keep the others' chains; and complete
  a first result before any global relax (one Gauss–Seidel pass in routing order with the deadline). Expected n=1000:
  per-iteration cost ≈ Σ changed wires, a completed iteration 1 within seconds.
- **RC3 (DEFERRED M2.5, hurts wiccon/dcsgone budgets) search per-expansion cost O(V).** Restrict `clearance_bend` to the
  sleeve: the chord from the apex must cross every portal pushed since the apex (≤ `LOS_PORTAL_CAP`), so the candidate
  vertices/walls are those of the ≤ C+1 sleeve faces and their ρ-neighbourhood (or query ≤ C small boxes along the
  chord). Expected dcsgone: ghost searches 0.58 s → ms; 46 s → a few s.
- **Fixture:** drop the > 9 mm fallback (pair leftovers within reach or skip the cell) and add a deterministic 0.1-pitch
  jitter so no three centres are exactly collinear, *or* keep the exact grid as a deliberate robustness test (then RC1(c)
  must be fixed first). Budget verdict: the m log m test is attainable in principle (search 6 µs/expansion, 15
  expansions/connection at n=1000, graph build ms), but **not demonstrable before RC1+RC2**; `relax_iter_cap` 20 / 5
  alone gives 35.7 s 76/100 / 11.5 s 78/100 at n=100 (quality unchanged) — do not tune caps.

## 2. Complexity of the new loops vs 45 §5a (plan Caps bullet)

| Loop | Bound / cap | Wall clock | Verdict |
| --- | --- | --- | --- |
| Graph build `graph.rs:173-290` | O(V+E) + O(F·outline) inside test | n/a (ms) | ok [M] |
| Slot search `search.rs:968-1079` | expansions ≤ `min(param, 8(E+X)+64)` (`:1028,1181`); per expansion O(n_max + C²) **+ O(V) worst case in `clearance_bend` (`:579,597`)** | every 16 expansions (`:1032`) and before every search (`:1175`) | bounded; RC3 |
| Ghost `search.rs:1208-1240` | cap `8E+64` (`:1229`); soft block 1e10 | same | bounded; RC3 |
| Negotiation `negotiate.rs:93-201`, `route.rs:1408-1552` | `pathfinder_iter_cap` 50; stuck stop 3 (`route.rs:592`), stall stop 8 (`:595`); extra ghost per absurd detour (`negotiate.rs:145`); rip ≤ `MAX_RIP` 3 (`:208`), bounded history/bumps | before every search (`negotiate.rs:124`), per iteration (`route.rs:1409`) | ok [M] |
| Fix-R6 zero-slack `route.rs:1010-1233` | ≤ m searches + ≤ 4 candidates × (≤ m searches + 3 metrize) | `:1048,1072,1130,1155,1167,1180,1196` | ok [R]; each metrize = full relax (RC2) |
| Decomposition `decompose.rs:222-481,498-533` | per net O(np·nk + nk² + ports²·clusters²) | n/a | ok [R] (per-net, as allowed) |
| Clinch detection `negotiate.rs:281-348` | O(Σk·n_max + E·n_max + m), only on a better best | n/a | ok [R] |
| Metrize / relax `route.rs:296-485`, `relax.rs:385-438` | `relax_iter_cap` 200 × m tautens, `TAUT_PASS_CAP` 64, `MAX_SLEEVE`; self-check rounds ≤ n+1 (`route.rs:417`) | before relax (`:351`), per wire inside relax (`relax.rs:388`), per self-check round (`:433`) | bounded but RC1/RC2 |
| Partial stubs `route.rs:782-955` | 2 × 24 bisection `check_items` per block; self-check rounds ≤ n+1 (`:888`) | `:849` once; `bisect_stub` has none | ok [R] |
| Block probe `route.rs:639-779` | per failed conn O(k · (|base| + 784) · q) + 33-dot pinch | **none** (only skipped when already timed out, `:1565`) | DEFERRED D9: 5.3 s on dcsgone [M] |
| KiCad DRC loop `harness/bench.py:228-262` | `drc_loop_cap` 5 (`rounds > loop_cap`) | n/a | ok [R] |

No exponential/factorial stage, no order permutations, no all-pairs board loops found. Minor: `route.rs:1602` and
`:1254` do linear `find`s per connection (O(m·nets), O(m·kept)) — harmless.

## 3. Correctness: Tasks 6–7 vs the design note, Review Focus tests, gate test

Tasks 6–7 implement the note: state `(edge, slot, dir)` and Rule T via `Cyc` (`search.rs:84-328`), start/corner/goal
rules incl. P1 spokes, direct path (`:1151`), string-pulled `g` with apex and `LOS_PORTAL_CAP`, penalised cost
(`:834-851`), ghost (`:1208`), negotiation constants/feedback/next-active/converged (`negotiate.rs`, `route.rs:1454-1551`),
clinches, stubs. Documented deviations: Fix-ABC bend admissibility (`bend_ok`) and topological rip-up (`strand_blockers`,
`rip`), Fix C removes a failed path before the overuse count (`route.rs:1482`), stubs start at the terminals (D6),
`Negotiator::record_best` is unused by `route()` (dead weight, harmless). Review Focus tests 1–5 exist and assert the
specified facts: 1 `route_engine.rs:173-178` (reason, others routed, B.Cu warning; no search: `route.rs:1310`);
2 `search_slots.rs:189-198`; 3 `route_engine.rs:196-201` (`iterations ≤ 2`); 4 `:209-221`; 5 `negotiate_loop.rs:192-226`
(both found, one channel user, no overuse, deterministic). `tests/gate/test_m2_gate.py` asserts every Global-Constraint
criterion with the plan's thresholds (27/27 = `facts["wires"]`, 25/25, pour_served, partial = failed = 0, routing_new 0,
unconnected 0, width_match 25, arcs ≥ 1, no `SS`, outside ≤ hand measured in-run, router_s < 2.0, threads 1, no timeout,
iterations ≤ 50, byte-identical runs without `runtime_s`, R6 zero-slack only when DRC-kept, sources unchanged). Faithful.

## 4. Determinism [R, gate test M]

No `HashMap`/`HashSet` iteration reaches an output or an order: `stack_order` and `Stacks.order` are consumed by lookup;
`Stacks::new` iterates `order` only to fill sets (`by_wire` Vec order is used for set removal only, `relax.rs:81-85,146`);
`edge_of`, `term_at`, `constraint_owner`, `State.best/closed`, `DynLayer.store`, `seen_grids` are lookups; grid queries
are sorted (`verts_near`, `query_walls`, `dyn_near`); `corridor_obstacles` sorts. A* heap key `(f, h, edge, slot, dir, id)`
(`search.rs:417-426`); connection order `(−bump, tier, −width, est_len, net, id)` is total (`decompose.rs:570-584`);
clusters/MST/ports by lowest index and first minimum; zero-slack candidates `(gain desc, id)`. Deadline-dependent outcomes
are the only run-to-run variance (by design).

## 5. STATE "Deferred to the F gate review" items

| # | Item | Still an issue? | Severity / class |
| --- | --- | --- | --- |
| D1 | terminal `clr = min_clearance` (`graph.rs:213`) | yes: a terminal's own net clearance (and a junction's track width, `r = 0`) are ignored in `cap`/`fits`; caught by relax/self-check → churn | low, DEFERRED M2.5 |
| D2 | non-fitting edge only 4× penalised (`search.rs:844-850`) | yes; an edge that cannot fit the wire even empty (`cap < w+ε+2·max(c,c_e)`) should be hard, it only produces `infeasible` metrizations | low–med, DEFERRED M2.5 (O(1) check) |
| D3 | direct path has no third-vertex clearance test (`search.rs:1151-1163`) | yes; a thin Delaunay triangle lets a foreign vertex sit within ρ of the edge; relax then bends the "along" wire across fan edges the graph thinks are free | med, DEFERRED M2.5: call `clearance_bend(p,q)` in `direct()` |
| D4 | contended-channel test never exercises the history switch (`negotiate_loop.rs:192-226`) | yes: `run_channel` asserts one channel user, not that a wire ever yielded through `hist`; fixture detours in iteration 1 | test gap, DEFERRED |
| D5 | A4 split-vertex bend fails the connection, no e2e test (`route.rs:315`) | yes; bounded by `penalise_failed` + stuck stop | low, DEFERRED (test) |
| D6 | partial stubs start at the terminal, not `last_reached` (`route.rs:794`) | yes, deviation from note §6; functional | low, DEFERRED M3 |
| D7 | spot vertex blocks every net in search (`search.rs:579-588` ignores `only_nets`) | yes; over-conservative in DRC-loop rounds ≥ 2 only | low, DEFERRED |
| D8 | clinches summary prints `len(u)` | cosmetic | DEFERRED |
| D9 | block probe cost (`route.rs:639-708`) | **measured 5.3 s on dcsgone, 0.9 s at synth n=100**; no deadline check; `probe_path` builds 784 stubs per face step against the full `base` | med, DEFERRED M2.5: deadline per step, cap steps, probe only the blocked step |
| D10 | Fix-ABC bend test skips point walls / foreign vertices (`search.rs:727`) | yes; a bend pinched against a non-adjacent *vertex* is invisible (same argument as the wall case) | low–med, DEFERRED M2.5 |

## 6. Findings

**BLOCKING** (the M2 gate test passes; these block Task 11 Step 2, the scaling test, which the plan makes a gate item):
- **B1** RC1 — `relax.rs:77-92` (initial membership), `taut.rs:418-430` (anchor insertion from dyn penetrations),
  `sketch.rs:235-262` (funnel tie at a touched vertex): metrization violates the slot topology → wires cross, 25 % failures at
  n=100 with 0 blocked searches, non-convergence. Fix as §1.5 RC1 (stack from slots; no own-terminal / far-anchor
  insertion; tie = bend on the portal side). [M]
- **B2** RC2 — `route.rs:1420-1423` + `relax.rs:385`: global 200-iteration relax of all wires per PathFinder iteration;
  n=1000 returns nothing after 120 s. Fix: incremental metrization of the searched connections + their neighbours; first
  completed pass before any global relax. [M]
- **B3** Fixture — `tests/scaling/synth.py:31-33` violates its own ≤ 9 mm spec for leftover cells (n=1000: 28 connections
  up to 90 mm). Fix: pair leftovers within reach or drop them; decide (user) whether exact collinearity stays. [M]

**DEFERRED** (M2.5 unless noted): RC3 search per-expansion O(V) (`search.rs:579,597`; dcsgone 34 s in 59 ghost searches);
D1–D10 above; `record_best` dead code; stall stop may end before the cap while progress is still possible (by R6, note only).

Escalation (45 §5a): the scaling budget cannot be met at ac31995; one diagnosis round is spent. The eprom gate criteria
themselves are met by the measured numbers in the task brief. Recommended path: B1 + B2 as S fix tasks with
`route_scaling` re-run (expected t100 ≪ 1 s, t1000 a few s), B3 as an H task; the user decides whether M2 is declared
DONE on the eprom gate alone with the scaling test carried into M2.5, or held until the scaling test is green.
