# M2 Task 5 — slot search, ghost search, negotiation, clinches, partial stubs (F, 2026-10-31)

Design for Tasks 6 (`search.rs`), 7 (`negotiate.rs`) and 10 (partial stubs) on the Task 3 `RouteGraph`. Conventions:
`GEdge.cross` is ordered `a → b` (`a < b`); "index from `v`" of an entry on edge `e ∋ v` is its position counted from
vertex `v` (= index when `v == a`, `n − 1 − index` when `v == b`, `n = slot_count(e)`). A *slot* `s ∈ 0..=n` is an
insertion position: the new wire lies between entries `s−1` and `s`. Vertex positions come from `VertexInfo.at`
(API addition A1). Complexity symbols as 45 §5a: `E` edges, `X = Σ_e slot_count(e)`, `V = 2·(E + X)` search states,
`n_max` = max entries on one edge (bounded by capacity), `k` = path length, `C = LOS_PORTAL_CAP = 32`.

## 1. Slot state and transitions — the strand model

**State** `(edge e, slot s, dir)`, `dir ∈ {0, 1}` = index in `GEdge.faces` of the face being *entered* (A* closed set and
heap tie-break both include `dir`; a sleeve may cross one edge in both directions — never twice in the same direction, a full
loop costs strictly more). Each routed connection contributes *strands* to the faces it passes: in face `F`, a strand is a
chord between two *ports* of `F`'s boundary cycle — a crossing `Cross(e, i)` on an edge of `F` or a terminal `Vertex(v)`
with `v ∈ F.v`. The partner of entry `i` on `e` in `F` is `RouteGraph::strand(e, i, F)` (A2; O(1) from the stored travel
order and face sequence). Walking the cycle of `F` with the entries of each edge in geometric order gives a cyclic port order.

**Rule T (admissibility and slot, all transitions).** A new strand from port `P` (entry port or start vertex) to a port on
edge `e_out` or to a terminal vertex is admissible iff no existing strand of `F` has one port on each of the two boundary arcs
cut by the new chord (planar: wires never cross). When `e_out` is crossed, the new port is placed at index from `v`
= `|{entries on e_out whose strand partner lies on the arc containing v}|`, where `v` is the vertex of `F` on the
*inner* side of the chord — the three instances, with `v` the vertex shared by `e_in` and `e_out`:
- **Corner cut** `e_in@s → e_out` (shared vertex `v`): `G` = entries on `e_in` with index from `v` `< s_v` (`s_v` = `s`
  counted from `v`). *Blocked* if some `c ∈ G` continues through the third edge `e_3`, or some entry on `e_3` has partner
  `Vertex(v)` (a spoke `v → e_3` separates `e_in` from `e_out`). Else `slot_out` from `v` = `|{entries on e_out: partner
  ∈ G}| + |{entries on e_out: partner = Vertex(v)}|` (the brief's "corner-`v` group" plus `v`'s own spokes, which nest
  innermost; see amendment P1). Invariant **Inv-1** (maintained by `insert_path`, asserted by the regular-clinch detector):
  on every edge the entries ordered from `v` are `[v-spokes and corner-v cutters of either face] ++ rest`.
- **Start through an incident edge** `(p, x)` of a face `F ∋ p` (fan walk): slot from `p` = `|{entries on (p,x) with
  partner Vertex(p)}|` (same-net spokes; 0 when none — "slot 0 nearest `p`"); never blocked (the chord `p → (p,x)` hugs
  vertex `p`; the only strands on its inner arc are `p`-spokes on `(p,x)`, which have both ports on that arc).
- **Start through the opposite edge** `(a, b)` of face `(p, a, b)`: blocked iff some strand `(p,a) ↔ (p,b)` (corner-`p`
  cutter), `Vertex(a) ↔ (p,b)` or `Vertex(b) ↔ (p,a)` exists; slot from `a` = `|corner-a group|` = entries on `(a,b)` with
  partner on `(p,a)`, `Vertex(a)` or `Vertex(p)` (`p`-spokes are counted on the `a` side — any side is consistent, this one
  is deterministic).
- **Goal** `q`: a state entering a face `F ∋ q` may terminate; admissible by Rule T with the new chord `P → Vertex(q)`
  (a foreign corner-`q` cutter between `P` and `q` blocks: that is exactly the regular clinch, §5). Goal nodes are pushed
  with `h = 0` and the search ends when one is popped.
- **Direct path** (Review Focus 2): if `edge(p, q)` exists, `passable(e, net)`, `slot_count(e) == 0` and `along(e)
  == None`, return `Found` at once with `crossings = []`, `faces = []`, `est_len = |pq|` — it is the global lower bound
  `h(start)`, no crossing path can beat it. `insert_path` of an empty path sets `GEdge.along = Some(conn)` (A4) so the edge is
  impassable for everyone afterwards (a later crossing would cut the track). → `search_adjacent_terminals_zero_crossings`.
Every crossed edge must satisfy `passable(e, net)` (hard: outside faces, foreign constraint edges). Each expansion produces
≤ 2 edge successors + 1 goal candidate; each costs O(`n_max`) (scan the entries of the three edges once). Complexity:
O(`V log V`) per search with constant work per expansion; expansion cap `8·(E + X) + 64` (Blocked when hit, Timeout only
on the deadline). Pinned by `slot_transition_nests_corner_groups` (corner cut and Inv-1), `search_respects_capacity`,
`search_expansion_cap_and_deadline`, `search_is_deterministic`.

## 2. Cost: string-pulled `g` (Lazy-Theta\*-style apex), Euclidean `h`

**Decision: Lazy Theta\*-style apex shortcut, Polyanya deferred.** Polyanya is exact only for a *point* agent in the
*class-free* navmesh: it cannot give our `g` (class, slots, inflation) and would add a second interval search per
connection (≈ the cost of the A* itself, ~400 lines of delicate interval geometry) merely to tighten `h`. The length gate is
measured after metrization (`length_outside_pads_nm`), never from the estimate, so exactness is not needed there; Euclidean
`h` is admissible and on ≤ 10⁴-state graphs the expansion cap bounds the damage of a loose `h`. Revisit (M3) only if the
Task 11 scaling test shows expansions growing faster than `V`.

**Apex rule (`g_len`).** Each state carries an *apex* `A` (a point: the start terminal, a vertex, or — after a cap — an edge
point) and `g_len = g_len(A) + |A P|`, where the *state point* `P` of `(e, s, dir)` is the point of segment `e` nearest to the
segment `A → goal` (slots do not move `P`; the sketch `t = (s+1)/(n+1)` is route.rs's business). Generating successor `Q`
from `P` with apex `A`: the chord `A → P_Q` must (i) cross the interior of every portal pushed since `A` (≤ `C` of them;
portals incident to a vertex apex are skipped) and (ii) keep clearance: for every vertex `u ∈ g.verts_near(bbox(A P_Q)
.grow(ρ_max))` (A3) with `net ∉ verts[u].nets`, `point_seg(at_u, chord) ≥ ρ_u = r_u + max(clr_u, c) + w/2 + ε`, and for every
wall `obs.query_walls(...)` not exempt, `dist(chord, core) ≥ r_core + clearance(kind) + w/2 + ε` (kind rules from
`g.rules`, A5). On the first failure the apex moves: to the endpoint of the missed portal on the side the chord passes
(i), or to `at_u` (ii) (bend approximated at the centre; error O(ρ) per bend), `g_len(A') = g_len(A) + |A A'|`, and the
chord is re-tested from `A'` (≤ `C` rounds; after `C` portals since the apex, `A := P` unconditionally). This is eager, not
lazy: with ≤ 2 successors per expansion the lazy deferral saves nothing and costs a re-push; the estimate is the polyline
through bend vertices (thin-string taut length, exact to O(ρ) per bend). Per expansion O(`C² + q`) = constant.
→ `search_goes_around_a_foreign_pad` (`est_len ∈ [30.0, 30.6]`: the string bends at the pad centre, 30.0; metrized ≈ 30.05).

**Penalised cost.** `Δ = max(g_len(Q) − g_len(P), 0)`, `cost(step) = Δ × (1 + ω·hist(e_out)) × (1 + ω·(present(e_out) −
1))`, `present(e) = 1` if `g.fits(e, w, c)` else `PRESENT_OVERUSE × (1 + overuse(e)/cap(e))` with `PRESENT_OVERUSE =
4.0`; `ω = TIER_WEIGHT[tier] = {1: 0.5, 2: 0.75, 3: 1.0, 4: 1.25}` (tier-1 feels congestion least, tier 4 yields). `f = g_cost +
h`, `h = |P goal|`; the final leg `P → goal` costs `Δ` only. Heap key `(f, h, edge, slot, dir)` by `total_cmp`. `f` is not
monotone (apex shortcuts), so closed-set A* is not optimal — accepted, as for Theta\*. `SlotPath.est_len = g_len` at the
goal. `hist` is `SearchParams.history[e]` (never decays; bounded by `pathfinder_iter_cap × HIST_FAIL` = 50). Pinned by
`search_respects_capacity` (a non-fitting edge is never crossed when any fitting class exists: `present ≥ 4`).

## 3. Ghost search (blocked connections)

Same A* with state `(e, dir)` (no slots, no strands), cap `8·E + 64`, `g_len` by §2, no `hist`/`present`. Hard: both faces
inside the outline (`Face.inside`), the `along` edges of other connections. Soft: `!passable(e, net)` or `!fits(e, w, c)` adds
`GHOST_BLOCK_NM = 1e10` (10 m ≫ any board: the ghost minimises the number of blocked edges first, then length).
`ghost()` returns `(path, blocked_edge, last_reached)`: `blocked_edge` = first crossing whose edge was soft-blocked,
`last_reached` = the crossing before it (`None` when it is the first); ghost crossings carry `slot = slot_count(e)` at search
time (append position, so route.rs's `t = (slot+1)/(n+1)` is the outermost point from `a`); `est_len` = pure length.
`search()` calls `ghost()` itself when the slot search ends `Blocked` (open list empty or cap) under the same deadline;
`blocked_edge == None` with `ghost = Some` means a purely topological block (strands), handled in §4/§6. O(`E log E`).
→ `search_blocked_by_wall_reports_ghost` (edge owned by `kept:<uuid>`, `last_reached` the crossing before).

## 4. Negotiation (PathFinder; `Negotiator`)

Constants: `HIST_FAIL = 1.0`, `HIST_BLOCK = 0.5`, `HIST_OVERUSE = 0.5`, `PRESENT_OVERUSE = 4.0`, `pathfinder_iter_cap`
default 50, `ε = Options.margin_nm` (5 000).
**Iteration** over `active` in `order(conns, bumps)` (key `(−bump, tier, −width, est_len, net, id)`): check the deadline
(→ `timed_out`, stop; untouched connections keep their paths in `g`); `g.remove_conn(c)`; `search`; `Found` ⇒
`g.insert_path(c, start, goal, &path)`; `Blocked` ⇒ `blocked[c] = BlockInfo` (nothing inserted). Then Task 8 metrizes and
self-checks and calls:
- `penalise_failed(c)` (relax / self-check failure): `hist[e] += HIST_FAIL` on `g.occupancy(c)` edges, `bumps[c] += 1`; the
  path stays in `g` until its next search (it reserves room). A class-independent failure (`InsideObstacle`, Review Focus 3)
  is *not* penalised: route() marks it `failed`, removes it from `g` and from `active` (hence `iterations ≤ 2`).
- `penalise_blocked(c, b)`: `hist[e] += HIST_BLOCK` on the ghost path's edges (none when `ghost == None`), `bumps[c] += 1`.
- `penalise_overuse()` after every iteration: `hist[e] += HIST_OVERUSE` on every `e` with `overuse(e) > 0`; returns the count.
Next `active` = failed ∪ blocked ∪ {connections with a crossing on an overused edge}. **Converged** ⇔ no failure, no
blocked, `overuse_edges == 0`. `record_best` after every completed iteration; best = max routed (metrized ∧ checked), then
min Σ `est_len`; snapshot = clone of the paths (O(Σk)). On cap/deadline route() reports the best; a deadline inside iteration
1 leaves `best() == None` ⇒ every connection `failed: "timeout"`, no items (Review Focus 4, `iteration_respects_deadline`).
Review Focus 5 (`negotiate_contended_channel_routes_both`): iteration 1 routes A through the channel; B sees `!fits` ⇒
`present = 4(1 + overuse/cap)`; if the detour is still dearer B takes the channel ⇒ overuse ⇒ `hist` +0.5 on the channel edge;
both active next iteration, A first (order is total), A now sees B inside and a growing `hist`, detours as soon as
`len_detour < len_channel × (1 + ω·hist) × present`, B then finds the channel free — monotone `hist` makes the switch
happen within a bounded number of iterations, never oscillates (the first-ordered connection is the one that yields).
Complexity O(`iters · m · V log V`) + O(`E`) per iteration. Also pinned by `blocked_connection_is_bumped_first_next_iteration`,
`overuse_raises_history_on_overused_edges_only`, `best_iteration_keeps_max_routed`.

## 5. Clinch detection (`detect_clinches`, diagnostic only)

- **Regular clinch** (Luzin): connection `c` with terminal `p` crossing an edge `e ∋ p` at a slot with a *foreign* entry
  nearer `p`. Structurally impossible: the start rule places `c` at index `|p-spokes|` and every later corner cut keeps `p`'s
  side = spokes ∪ corner-`p` cutters, and a corner-`p` cutter between `c` and `p` is rejected by Rule T at the goal. The
  detector stays as the Inv-1 invariant: O(`Σk · n_max`), expected empty (`u_clinch_detected_and_regular_absent`).
- **U-clinch**: edge `(p, q)`, `!constraint`, with a net `N ≠ 0` in both `verts[p].nets` and `verts[q].nets`, ≥ 1 entry
  whose connection's net ≠ `N`, while a connection of net `N` with terminal set `{p, q}` has ≥ 1 crossing (p, q in one
  cluster ⇒ no connection ⇒ no clinch). Report `(conn, edge)`; O(`E · n_max + m`).
- No feedback. Counter-example where a U-clinch is necessary: board 10 × 8 mm, N pads at (0.6, 4) and (9.4, 4) touching
  the edge-clearance band, F pads at (5, 6) and (5, 2). F cannot pass outside either N pad (no room between pad and outline),
  so every routing has F crossing the segment pq; N goes around an F pad. Penalising the U-clinch would only make N fail.
  Mutual evasion (12 §5 c) is not detected in M2.

## 6. Partial stubs (`partial_routes`, Task 10)

For a `failed` connection with `BlockInfo { ghost: Some(gp), edge: Some(e_b), last_reached }` (with `edge == None`: no stub,
`blocking = None`, reason `"blocked topologically"`): `k` = index of `e_b` in `gp.crossings`; forward crossings =
`gp.crossings[..k]`, reverse = `gp.crossings[k+1..]` reversed. Ghost crossing points use route.rs's `t` rule. `Q_f` = point
of `last_reached` (or `from.at` when `k == 0`), `P0` = ghost point of `e_b`. `P_f = Q_f + λ·(P0 − Q_f)` with `λ` the largest
value for which the straight `TrackItem` `Q_f → P` (width `w`, net) has 0 violations under `Checker` at margin `ε` against the
model + the routed items: bisection on `λ ∈ [0, 1]`, `STUB_BISECT_STEPS = 24` (resolution < 3 nm on a board; exact because a
sub-segment of a passing segment passes), `λ = 0` ⇒ no forward stub. Reverse stub symmetric from `Q_r` (ghost crossing after
`e_b`, or `to.at`) toward `P0`. Wires `from → Junction{P_f}` / `to → Junction{P_r}` with sketches from the forward / reverse
ghost crossings, metrized in one extra `relax` + self-check pass with the routed wires; a failing stub is dropped; status
`partial` iff ≥ 1 stub survives (else `failed`, reason `"partial stub failed self-check"`); `blocking` = midpoint of `e_b`
(KiCad frame). `partial_routes == false`: nothing emitted, `blocking` still reported. Per connection 2 × 24 Checker calls (O(`q`)
each) + O(`k`) relax work: O(`m`) overall. Wall fixture: `0.2 + 0.125 + 0.125 + ε = 0.455 mm` to the wall centreline
→ `partial_on_emits_two_checked_stubs` (`[455_000, 460_000]` nm), `partial_off_emits_nothing_for_blocked_connection`.

## Required Task 3 API additions (ruling R4)

- **A1** `VertexInfo.at: Vec2`; `Face.e[i]` is the edge opposite `Face.v[i]`; `GEdge.faces[0]` is the face on the left of
  `a → b` (defines `dir`).
- **A2** `pub enum Port { Vertex(usize), Cross(usize, usize), Detached }`; `pub fn insert_path(&mut self, conn, start_v,
  goal_v, crossings: &[(usize, usize)], faces: &[usize], width, clr)` (travel order; `faces.len() == crossings.len() + 1`,
  empty path ⇒ `along`); `pub fn strand(&self, e, i, face) -> Port`; `pub fn path_of(&self, conn) -> Option<(&[(usize,
  usize)], &[usize], (usize, usize))>`. `CrossEntry` gains `k: Option<usize>` (index in its path); the plain `insert` keeps
  working for the capacity tests with `k = None` ⇒ `Port::Detached`, which the search treats as capacity-only (never blocks,
  never counted). `insert`/`remove_conn` keep every other connection's `occupancy` slots current (O(`n_max`) per edge).
- **A3** `pub fn verts_near(&self, bb: Aabb) -> Vec<usize>` (grid over vertices built in `build`, O(`V`)) and `pub fn
  rho_max(&self) -> f64` (max `r + clr`); the search adds `w/2 + ε`.
- **A4** `GEdge.along: Option<usize>`; `passable(e, _)` is false while set; `remove_conn` clears it.
- **A5** `pub rules: GraphRules` on `RouteGraph` (wall clearance by kind in the LoS test).

## Plan amendments (Tasks 6/7/10 text vs. this note)

- **P1** Brief §1 slot formula: add `v`'s own spokes on `e_out` and the spoke-through-`e_3` block (otherwise a wire cutting the
  corner of a pad with a routed spoke lands *inside* that spoke). Start slot on an incident edge is `|p-spokes|`, not 0.
- **P2** Task 6 Step 3 "closed set per `(edge, slot)`" → per `(edge, slot, dir)`; heap key `(f, h, edge, slot, dir)`.
- **P3** Task 6 `SearchOutcome::Blocked` is produced by `search()` after it ran `ghost()`; Task 6 tests that pre-place a wire
  (`search_respects_capacity`, `slot_transition_nests_corner_groups`) must use `insert_path`, not `insert`, when the pre-placed
  wire is expected to block or nest (a `Detached` entry only consumes capacity).
- **P4** Task 7 `iteration` does not insert a Blocked connection; `penalise_blocked` with `ghost == None` only bumps.
- **P5** Task 10: a connection is `partial` iff ≥ 1 stub survives self-check; `BlockInfo.edge == None` ⇒ no stub.
- **P6** Brief §2 "line-of-sight through `LayerObstacles::query_anchors`" → vertex query on the graph (A3; anchors are CDT
  vertices and `RouteGraph` already resolved their clearances) plus `query_walls` for wall bands.
