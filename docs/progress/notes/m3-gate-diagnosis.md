# M3 gate diagnosis (F, 2026-10-31) — wiccon completion and private-badge-b wall clock

Scope: the two failing M3 gate items after Tasks 1–11 (head 8a03f54). Method: a new dev tool `engine/examples/gate_diag.rs`
(replicates `route_inner`'s setup, then (A) search + ghost of every connection on the **empty** graph, (B) negotiator replay,
(C) blocked-edge / terminal-escape dumps; `--probes` adds removal probes). Numbers below are from this machine under load
(load avg 5.4, another worker rebuilding the venv): wall times are inflated ~2×, counters are exact. Experiment patch (not
committed to the engine, reverted): `docs/progress/notes/m3-gate-diagnosis-outline-arcs.patch`.

## Item 1 — wiccon-2023: 50/60 (83 %), all 10 "blocked: no path"

**Evidence.** All 10 (#2 #11 #15 #21 #25 #26 #28 #30 #42 #53) are blocked **on the empty graph** (no other connection
present) and 8 of them in 0.000 s, i.e. the A* never leaves the start vertex; removing every other connection (vias, direct
paths, everything) changes nothing. So it is not negotiation (rip-up, Rule T at sites, D2/D10, P4, pour goals, the `dead` set):
it is static geometry. The vertex-escape dump shows one terminal per failing connection whose faces are **all `inside ==
false`**: J1.1/J1.5/J1.6 (THT header), S1.B1/S1.C1, TP2.1, J4.5/J4.6, RESET1.2 — the hand board routes all of them (GPIO12/13,
3.3v entirely on B.Cu, 0 vias). wiccon's outline has 114 segments + 8 arcs with chord sagitta up to **14.3 mm**; Python
ray-casting with the arcs tessellated classifies those pads inside, with the arcs collapsed to chords 17 of 122 pads fall
outside. private-badge-b (24 arcs, sagitta 0.59 mm) and eprom (no arcs) have 0 misclassified pads.

**Root cause RC1a (parity).** `graph.rs:157 chord()` maps `Prim::Arc` to its single chord; `RouteGraph::build`
(`graph.rs:305`) and the face-site cull (`vias.rs:152-160`) run `inside_outline` (`graph.rs:165`) on these chords. Every face
in a sagitta pocket is "outside" ⇒ `passable()` (`graph.rs:448`) is false on all its edges ⇒ the terminal cannot escape, no
site is generated there.

**Root cause RC1b (constraint).** With RC1a fixed locally, 7 of the 8 remaining blocks have a ghost with exactly one
soft-blocked edge: the arc's **chord itself**, which enters the CDT as a constraint edge with `owner_nets []` (`obstacle.rs:467-480`
builds one `Core::Arc` wall per outline arc, `Frag::prim` `obstacle.rs:287-297` anchors only its two ends; the sketch turns the
wall's end-to-end chord into the constraint). E.g. B.Cu e2131 (207.8,−97.0)–(162.1,−51.1) walls off J1 (three failures), F.Cu
e997 (154.3,−157.7)–(207.7,−104.0) walls off S1/TP28, B.Cu e2096 walls off TP2. `passable` is false for every net on a
net-less constraint ⇒ the pocket is sealed.

**Experiment (patch above).** Arcs split into sub-arcs of sagitta ≤ 25 µm before they become walls, and the same chords for
the parity test: wiccon **60/60** routed (incl. #42), 33 vias, 10 iterations (stalled, overuse > 0), outside-pads 1357 mm vs
hand 1690 mm, 3.07 s at 1 thread, byte-identical dump at 4 threads; ghost runs 13 → 4. eprom unchanged (27/27, vias 0,
outside 64.628 mm, no zero-slack, converged, 0.21 s).

### Fix task F1 — outline arcs as sub-arcs (gate item 1)
- `geom/prim.rs`: `Arc::split_sagitta(&self, max_sag) -> Vec<Arc>`; `n = ceil(|sweep| / (2·acos(1 − max_sag/r)))` (1 when `r ≤
  max_sag`), sub-arc `i`: same `c, r, sigma`, `a_start + sigma.sign()·sweep·i/n`, `sweep/n`. New `pub fn outline_prims(outline:
  &[TrackPrim]) -> Vec<Prim>` (`geom/quant.rs` next to `dequantise`): dequantise + split arcs with `OUTLINE_SAGITTA_NM =
  25_000` (≫ `max_error` 5 µm, ≪ `edge_clearance` 200 µm; walls keep `Core::Arc`, so clearance stays exact).
- Use it in all three outline consumers: `obstacle.rs:477` (walls; intermediate "edge" anchors ⇒ constraint polyline hugs the
  rim), `multigraph.rs:120` (outline handed to `RouteGraph::build`; `graph.rs:305` chords are then sub-arc chords), `vias.rs:152`
  (site cull). Keep `chord()` for sub-arcs; nothing else changes.
- Tests: `inside_outline_handles_arc_pockets` (D-shaped outline, 180° arc r 20 mm, a point 8 mm inside the pocket ⇒ inside;
  with chords ⇒ outside); `route_pad_in_arc_pocket_routes` (route_layers: pad in the pocket ↔ pad in the body routes, no
  constraint crossing, both layers); `face_sites_exist_in_arc_pocket` (via_sites); gate assertion wiccon ≥ 54/60 (measured 60).
- Expected: wiccon 50 → 60/60; eprom byte-identical (no arcs); private-badge-b: 24 arcs (r ≈ 2 mm) → ~5 sub-arcs each, ~100 more
  anchors, results may move slightly. Cost `O(Σ n_i)` prims, CDT `+O(n log n)`; `n_i ≤ sweep/0.084 rad` ≈ 25–50 per rim arc.
- Deliverable: hand-vs-topor PNG for wiccon and eprom (memory rule), threads 1/2/4 byte-identical check.

## Item 2 — private-badge-b: router_s 120 s (gate ≤ 65 s with wall_clock_s 60)

**Evidence.** (a) Gate options, 4 threads, this machine: iteration 1 does not finish in 60 s (0/231; T11 on a quiet machine:
164/231 in iteration 1). Counters: search 379 runs, 140 ms/run, 268 k clearance candidates/run; **ghost 77 runs, 607 ms/run,
20 M candidates/run (433 per bend query)** = 46.7 s CPU of the 60 s. (b) Empty graph, 1 thread: search total 36.7 s (4 runs
> 0.5 s, max 24.3 s: #201 U1.13→U7.4), **ghost total 759 s** (64 runs > 1 s, max 24.8 s), ghost `None` for **166 of 231**
connections although the search finds 212 of them in ms (e.g. #150 search 9 ms Found, ghost None in 0 ms; #173 search 2 ms,
ghost None after 20.4 s). (c) Replay, 4 threads, no deadline: iteration 1 = 76.6 s, 209/231 routed, **149/231 invalidated
and re-searched sequentially**, 305 overused edges, 164 vias; 22 remain blocked and are static (13 GND pour completions at
0.5 mm width from U4/U5/U7/J3 pads, #191 U4.8→R13.1 at 0.5 mm, net65/net80 at U7.4/U7.9: 0.625×0.25 mm pads, the terminal
cannot be left or entered — same class as item 1's escape, geometry not negotiation). (d) Harness: `bench.py:281-289` gives
every DRC-loop round (and the zero-slack rejection re-run) a fresh `wall_clock_s` and sums `router_s`; `router_s ≤ 65` in
`test_m2_gate.py:80` therefore measures **all rounds**, so any second round makes 2×60 > 65 by construction.

**RC2a — the ghost is wrong and the most expensive thing in the engine.** `Search::hard()` (`search.rs:1377`) forbids the
ghost every statically-unfit edge (`cap < w + ε + end_gaps·max(c, c_e)`) while the real search crosses such edges as overuse
(`leave_vertex` `search.rs:1019`, face loop `:1215`). A pad's own polygon spokes/sides are "unfit" for its own net (same-net
corner vertices still count as clearance gaps in `GEdge::end_gaps` `graph.rs:70`), so the ghost cannot leave most SMD pads ⇒
`None` (166/231), ⇒ `negotiate.rs:240` marks the connection **dead** on a false premise, no blockers, no rip-up, no partial
stubs. When the ghost does explore it is a full geometric A* (`los` + clearance candidates per expansion) whose soft cost
`GHOST_BLOCK_NM = 1e10` makes it visit the **entire reachable component** (f < 1e10) before conceding — 10–25 s per run with
long apex chords (RC3 of the M2 review, `O(V)` candidates per chord). `hard()` also includes dynamic facts (`along`, caps shrunk
by `set_site`), so "dead" is not even static.

**RC2b — exhaustive blocked searches.** #201 spends 24.3 s hitting the cap (209 668 = 8·25 861 + 4·679 + 64) because the goal
U7.4 cannot be entered at all (its own connections #202/#203 block in 11 ms). There is no escape/entry pre-check; the cap formula
is per plan and not the problem — reaching it is.

**RC2c — P4 invalidation is too coarse.** `path_valid` (`negotiate.rs:333`) rejects a path when any edge of any touched face
moved (`faces_unchanged_since`, every `insert_path`/`set_site` bumps whole faces) **or** a site activated since the freeze lies
within `rho_max + w/2 + ε` of the bbox of all faces of the whole path — `rho_max` is the largest `r + clr` of the layer (THT
hole + 0.35 mm), the bbox of a 20 mm path covers much of the board, and iteration 1 activates 164 vias ⇒ 149/231 rejected and
re-searched one by one (`O(V' log V')` each, ~0.1–0.25 s) ≈ 24 s sequential (T11) — the parallel phase is wasted.

**RC2d — harness budget.** See (d). Not a relaxation question: with an anytime engine the budget must be the run's.

### Fix task F2 — topological ghost (`search.rs`)
Replace `ghost_in`'s geometric A* by a 0-1-BFS / Dijkstra over states `(layer, edge, dir, vias ≤ max_vias)` plus `AtSite`
nodes on the same adjacency (`faces[f].e`, `try_via` site vertices, `leave_vertex` on the other layer), **no `los`, no clearance
candidates, no slots/strands**; key = `(soft_count, length)` lexicographic with length = Σ edge-midpoint hops, soft = `!passable
|| !fits` edge, owned/blocked site, `along`; hard = only faces outside the outline. Return the same `GhostOut` (legs with
`slot = slot_count(e)`, sites, first soft element in travel order, `last_reached`); `est_len` = funnel length of the found
sequence (`O(k)`, existing taut code) or the hop sum. `dead` (F3) no longer depends on it. Cost `O(Σ_L E_L · (max_vias+1))` per
run, deterministic (adjacency in index order, `BinaryHeap` with total key). Expected: 607 ms → ≤ 5 ms per run on private-badge-b;
ghost `None` only for truly unreachable goals; rip-up/detour/stubs become effective (wiccon's 15 stalled iterations had 0 rips).
Tests: all `search_layers` ghost tests unchanged; new `ghost_is_never_stricter_than_search` (on every fixture board: `search` Found
⇒ `ghost` Some with 0 soft elements); `ghost_cost_is_linear` (fixture with 1 k faces: < 50 k node pops).

### Fix task F3 — terminal escape pre-check, sound `dead` (`search.rs`, `negotiate.rs`)
Before the A*: start and goal vertices must each have an inside face with a `passable` edge on some layer (goal: `push_goal`
reachable only through such an edge); otherwise return `Blocked{ghost: None}` immediately and let `negotiate.rs:240` mark
`dead` (now a static fact). Report the reason as `"blocked: terminal cannot be left/entered (width w, clearance c)"` so pour
completions at 0.5 mm from 0.25 mm pads are explained. Expected: #201 24 s → ms; the 22 static blocks cost 0 in every
iteration. `O(deg)` per terminal. Test: `search_blocks_unenterable_goal_instantly` (SMD pad narrower than `w + 2c` on a board
where the start is open: `expansions == 0`), `dead_only_for_static_blocks` (negotiate_loop: a connection blocked by another's via
is re-searched after rip-up, not dead). Worker: run `gate_diag private-badge-b --skip-a --iters 1` and read the "vertex escape"
dump for U7.4 to pin the exact failing rule before coding (expected: every incident edge either `passable == false` through a
foreign pad constraint or `los` to the state point fails for `w = 0.5 mm`).

### Fix task F4 — validate instead of re-search in the batch commit (`negotiate.rs`)
For a path that fails `path_valid`, first **re-validate in `O(k)`** against the current graph: per crossing recompute the slot
with `Cyc::cross_slot` against current strands, per edge `fits`, per via `site_free` + `goal_ok`, per `AtSite` the start rule on
the other layer; all pass ⇒ insert with the recomputed slots (`resolved_sequentially` unchanged, count as `revalidated`); else
re-search as now. Also tighten the site rule: test each activated site against the bbox of the **leg's** faces grown by that
via's own `d/2 + clr + w/2 + ε` (not `rho_max` of the layer). Expected: 149 → ~20–40 re-searches on private-badge-b, commit 24 s → ≈ 5 s.
Order of phase 3 unchanged ⇒ determinism and `iteration_batch_commit_invalidates_shared_edge` hold (the cap-1 channel still
fails `fits` ⇒ re-search ⇒ detour). New test `batch_validation_accepts_unaffected_paths` (two connections sharing a face's
bbox but no edge/site: `resolved_sequentially == 0`, both paths identical to a sequential run).

### Fix task F5 — harness honours the remaining budget (`harness/bench.py`)
Round `r` of the DRC loop (and the zero-slack re-run) gets `wall_clock_s = max(0, wall_clock_s − router_s_so_far)`; when the
remainder is < 1 s skip the re-route, keep the previous board, record `drc_loop.budget_exhausted = true`. Then `router_s ≤ wall +
final pass` by construction and the bound keeps its stated meaning. Test: `tests/harness` with a stub router recording the
`wall_clock_s` it receives per round (60, then 60 − t₁). **Needs ruling R-i below** (interpretation, not relaxation).

### Fix task F6 (optional, second-order) — same-net end gaps
`fits_at`/`hard`: an edge end whose vertex `nets` contain the connection's net (own pad corners, own via) needs no clearance gap
(`end_gaps` per net). Removes the false "unfit" on pad escape edges (private-badge-b: 305 overused edges after iteration 1, many are pad
spokes) and the overuse noise that keeps wiccon "stalled" at 60/60. Risk: changes costs ⇒ eprom's path choice may change ⇒ run
only after F1–F5 and keep it out if eprom is not byte-identical (then R-ii).

### Instrumentation F7 — `route_probe` timers for metrize / self-check / commit per iteration
Search + ghost were ≈ 100 s CPU of 240 s CPU available in the gate run; metrization and the self-check are unmeasured. Add
per-iteration `search / commit / metrize / check` seconds to the report (`RouteResult.profile`, behind `--quiet` in the probe).

**Expected private-badge-b after F1–F5 (estimate, quiet machine, 4 threads):** iteration 1 ≈ 3–4 s parallel search (36.7 s CPU minus
#201's 24 s, ÷ 4) + ≈ 5 s commit + metrize; 2–4 further iterations on the active set; ≈ 20–35 s per `route()`; with F5 the gate
sees `router_s ≤ 60 + final pass` even when a DRC round repeats. Routed stays ≈ 209/231 (22 static). Risk flag: if metrize
proves > 20 s (F7), a second round is still needed — report rather than tune.

## Requirement / ruling items for main (coordinator: escalate, do not decide)
- **R-i** `wall_clock_s` semantics in the bench: budget for the **whole** bench run (all DRC-loop rounds and the zero-slack
  re-run, F5) vs per `route()` call. Under the per-call reading `router_s ≤ 65` is unattainable whenever a round repeats
  (2 × 60) — the bound only has meaning under the whole-run reading. Recommendation: whole run; the bound stays 65.
- **R-ii** eprom byte-identity (27/27, vias 0, zero_slack 0, outside 64.628 mm) is preserved by F1–F5 (verified for F1; F2–F4
  change only ghost/dead/commit paths that eprom never exercises: 0 ghost runs, 0 re-searches, converged in 1 iteration — the
  worker must re-verify). F6 may change eprom's cost landscape: if it does, main must rule whether a changed (still clean) eprom
  is acceptable; otherwise F6 is dropped.
- **R-iii** private-badge-b keeps ≈ 22 statically unroutable connections (0.5 mm GND pour completions from 0.25 mm pads, fine-pitch U7
  at 0.2 mm track / 0.2 mm clearance): the informational gate does not assert `routed`, but the final report should say they are
  width/clearance limits, not router defects (F3's reason string).

## Determinism and tooling
All fixes keep the parallel phase pure and phase 3 sequential; no `HashMap` iteration reaches a result (F2's BFS uses index-order
adjacency, F4 only re-validates in commit order). The wiccon experiment was byte-identical at threads 1 and 4. Tool:
`cargo run --release --example gate_diag -- <model.json> [--options JSON] [--iters N] [--skip-a] [--probes]`
(binary under the workspace `target/release/examples/`; `--probes` is slow on private-badge-b — one ghost per probe until F2 lands).
