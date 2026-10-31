# M2 — eprom `route()` diagnosis against the M2 gate (F, 2026-10-31)

Inputs: Task 9 run (commit 68efda8): 26/27 routed, A13 `infeasible: wall edge grazed mid-span by 298386 nm` for all 50
iterations, outside-pad 82.2 vs hand 76.6 mm (A14 32.10 vs 5.79), 1 overused edge, never converged, 0.31 s. Probe:
`cargo run --release --example route_probe -- <model.json> [--iters N] [--watch A13,A14] [--quiet]` (engine/examples/route_probe.rs;
replays `route()` with the public API and prints per iteration the order, every slot path as `edge(a-b)@slot` with vertex
positions, sketch crossings/anchors, relax chains or `TautError`, self-check hits, overused edges with their capacity arithmetic,
history increments, best bookkeeping). It reproduces the run exactly: best = iteration 2, 26/27, 124.016 mm, same A13 reason.
Coordinates below are KiCad mm; ε = `margin_nm` = 5 µm; all connections have clearance 0.25 (netclass), +5V width 0.4, others 0.25.

## 1. A13 (U2.26 (201.24,72.33) → J1.3 (195.33,63.75)): an infeasible homotopy class the search cannot see

The slot search returns, in every iteration, a variant of one class: between U1.23/U1.24 (gap 0.94 mm, fits), then **around the
right of U1.24 (202.74,71.0) between the pad and the right board edge x = 204**, up the edge, around U2.28 to J1.3. That class is
geometrically impossible: the chain must wrap U1.24 at ρ = 0.8 + 0.25 + 0.125 + ε = 1.18 mm (→ x = 203.92) while the outline band
needs 0.25 + 0.125 + ε = 0.38 mm (→ x ≤ 203.62). The relax stage reports exactly this: depth 0.300 mm = 0.38 − (204 − 203.92); the
298386 nm variant is the same pinch seen from a neighbouring edge sequence. The outline is one 40 mm constraint edge whose only
anchors are the corners, so `penetrations()` finds no end anchor near the penetration ("grazed mid-span") → `Infeasible`. Relax is
right to refuse: no anchor sequence realises that class. A11 failed identically in iteration 1 (around U1.20, depth 0.268 mm) and
recovered in iteration 2 only because a feasible alternative class exists for it.

Why the search admits it (both models are too optimistic in the same spot):
- **Capacity**: `cap(e) = |pq| − r_p − r_q` only measures cuts that *are* CDT edges. The cut "U1.24 ↔ outline" is not an edge: the
  outline has no vertex opposite the pad, and the edges U1.24–corner (9 and 31 mm, `r` = 0 at the corner) fit anything.
- **Line of sight** (`clearance_bend`): the chord is tested against walls, but a bend moves the apex to the *pad centre* ("error O(ρ)
  per bend", note §2). The arc the chain really makes around the pad (radius ρ) is never tested against the wall band, so a pad
  pinched against a wall (gap 1.26 < ρ + 0.38 = 1.56) is invisible.

Why negotiation does not move it: `penalise_failed` puts +1 history on A13's *own* edges only. The fan of thin triangles along the
right edge offers ~10 edge sequences in the same class (iterations 2–10 pick different edges through the GND via (203.35,67.0),
e100, e98, …), so the class is abandoned only twice (iterations 11, 40: the left loop, 37.3 mm est) — and that class is also
infeasible (`infeasible: pad:J1:1 <-> pad:J1:2`): A14 already holds the single-wire corridors it needs (see §2), the stacked radius at
J1.2 (1.23 + 0.25 + 0.25 + 0.125 + ε = 1.74) overlaps the J1.1 hull anchor band (distance 1.89 < 0.38 + 1.74). The connection that
actually blocks A13 (+5V #24, below) is never active again (next active = failed ∪ blocked ∪ overused) and never gets history:
**PathFinder negotiates capacity, not topology** — a wire cutting the only feasible class of another connection is not a cost
the loop can see.

## 2. A14 (U2.27 → J1.4) detours 32 mm: enclosure by the +5V tree, not by A13

A14 is searched *before* A13 in iteration 1 (est 6.9 < 10.4 mm); A13 is not in the graph. The +5V net (tier 2, wider → first) was
routed as its shortest classes: #23 U2.1→J1.2 below J1.1, **#24 U2.28→J1.2 below J1.3/J1.4** (via the GND vias (199.35,66.5),
(196.75,65.85)), #22 U1.24→U2.28. Together with the board edge they enclose J1.3/J1.4: the right opening (U1.24 ↔ edge) is the
pinch of §1, the left opening is the corridor chain U2.1–U2.2 gap (0.94 mm) → U2.1 ↔ left edge (0.44 mm) → J1.2 ↔ top edge
(0.14 mm), each wide enough for exactly **one** 0.25 mm wire. A14's 32.2 mm loop is therefore the *only* metrizable class, the search
found it correctly (110 expansions, no history involved), and A13 is left with nothing. Ordering (+5V first), not history, caused it;
history only re-shuffled A14 later (iterations 12/41, when A13's left-loop attempts made its edges overused).

The hand solution avoids the enclosure by routing the +5V links **above** J1.1 and J1.3/J1.4 along the top edge — the two wires M1
classified as zero-slack fallbacks (STATE.md): 63.75 − 0.85 − 0.25 − 0.2 = 62.45 = 62 + 0.25 + 0.2. With ε = 5 µm that corridor is
10 µm too narrow (ρ_pad 1.305 + band 0.455 > 1.75), between J1 pads a 0.4 mm wire does not fit (0.84 < 0.9), and U1.24 is reachable
only through the U2.28 cut. **Hence no complete F.Cu routing of eprom exists at ε = 5 µm**: the right +5V link must pass below J1.3/J1.4,
which leaves one single-wire opening for A13 and A14. The 26/27 result is the attainable maximum; only its shape is wrong.

## 3. Non-convergence: one overused edge, cheap but pointless

The overused edge is always one of A13's failed path (e100 U1.24–U2.26 cap 0.405 < need 0.755, or e98 cap 0.327): a failed path
"stays in `g` to reserve room" (note §4) and is counted by `penalise_overuse`, so `overuse_edges ≥ 1` as long as A13 fails, and
A13 is re-searched anyway. Cost: each iteration re-runs relax for all 27 wires (3 passes) + Checker ≈ 5 ms → 48 wasted iterations ≈
0.25 of the 0.31 s; length is unaffected (best = iteration 2 by `(routed, Σlen)`). On wiccon-size boards 50 × full relax of a stuck run
would dominate the runtime budget.

## 4. Fixes (smallest first), tests, complexity (45 §5a), expected eprom numbers

**A. Search feasibility: test the bend, not only the chord** (`search.rs::clearance_bend`, ~30 lines). When the apex moves to a
vertex `u` (bend radius `ρ_u = r_u + max(clr_u, c) + w/2 + ε`), also require that the point `at_u + n̂·ρ_u` (n̂ = unit normal from
`u` towards the chord, i.e. the side the string wraps) keeps its band from every wall and foreign vertex returned by the same
queries (`dist ≥ r_core + clearance(kind) + w/2 + ε`, resp. `ρ_v`). If it fails the successor is inadmissible (not pushed); the
slot search then ends `Blocked` → ghost → `penalise_blocked`. Complexity unchanged: O(C² + q) per expansion, same grid queries.
Test `search_rejects_bend_pinched_against_wall` (engine/tests/search_slots.rs): pad r 0.8 at 1.0 mm from the outline, terminals
whose chord bends around the pad on the edge side → outcome is the inner class or `Blocked`, never a path relax reports as
"grazed mid-span". Expected eprom: A11 routes in iteration 1; A13 is `Blocked` with `ghost` = direct path, `blocked_edge = None`
(topological) instead of 50 relax failures; numbers otherwise unchanged (26/27).

**B. Topological rip-up** (`negotiate.rs`, ~60 lines; needs A to be reached). For a `Blocked` connection with `blocked_edge ==
None` walk the ghost path face by face and collect the connections whose strands violate Rule T against the ghost chords
(`Cyc::strands`, same arc test as `cross_slot`); add them to the next active set, `history += HIST_BLOCK` on their crossed edges,
bump the blocked connection (it goes first). Optionally also for a `Found` path with `est_len > 2 × ghost length` (A14's 32 vs 6.9 mm)
so a found-but-absurd detour rips its blockers too. Complexity O(Σk · n_max) per blocked connection, inside the existing
O(iters · m · V log V); monotone history bounds the oscillation, iteration cap unchanged. Test `blocked_by_strand_rips_up_blocker`
(engine/tests/negotiate_loop.rs): A's straight wire cuts B's only short class, A has a longer feasible class → iteration 2 routes both,
converged. Expected eprom at ε = 5 µm: the loop alternates +5V #24 ↔ A13 (both cannot be routed, §2); best by `(routed, Σlen)` keeps
+5V #24 (9.55 mm) over A13 (10.4 mm): **26/27, A14 direct (≈ 5.8 mm outside), outside-pad ≈ 56 mm vs hand 76.6**, A13 failed with
`blocking` at +5V #24. With `margin_nm = 0` (zero-slack allowed) +5V #24 can take the top corridor and 27/27 becomes reachable, at
the price M1 refused (exact tangency; KiCad arc approximation `max_error` 1.5 µm may flag it).

**C. Stuck detection** (`route.rs` loop, ~15 lines): stop when, for 3 consecutive iterations, the routed count and the failed/blocked
set are unchanged and every overused edge carries only failed connections' entries (`converged = false`, warning "stuck after N
iterations"). O(m + E) per iteration. Test `route_stuck_connection_stops_early` (engine/tests/route_engine.rs): unroutable
connection → `iterations ≤ 5`. Expected eprom: iterations 50 → ≈ 5, router 0.31 → ≈ 0.05 s. Also drop the failed path from `g`
before `penalise_overuse` (the reserved room of an infeasible class misleads the overuse count; 3 lines).

Not proposed: a CDT vertex opposite every pad on long outline edges (would make the pinch a capacity cut, but changes the
constraint model, Task 2), and any order permutation (forbidden, 45 §5a).

## 5. Requirement conflict (explicit)

The M2 gate "27/27 connections, 25/25 nets, routing_new 0" is **unattainable on eprom at `margin_nm = 5000` by any single-layer
router**: it requires the two zero-slack +5V corridors the user already ruled as fallbacks in M1 (R5 covers only the *length* gate).
Options for the coordinator, none taken here: (i) extend R5 to the count gate — report A13 (or +5V #24) as the +5V-corridor casualty
and gate on 26/27 + A14 ≤ hand; (ii) run the gate with `margin_nm = 0` for eprom (zero-slack tangency, DRC fragile); (iii) accept
B.Cu for that link (M3). Fixes A–C are worth doing under every option; they do not touch budgets.
