# M3 Task 5 — via sites, layered slot search, site negotiation, via reduction, parallel batch (F, 2026-10-31)

Design for Tasks 7 (`vias.rs`, `multigraph.rs`), 8 (`search.rs`), 9 (`negotiate.rs`, `route.rs`), 10 (reduction pass) and 11
(rayon). Builds on `m2-search-design.md` (state `(edge, slot, dir)`, Rule T, start/goal rules, apex `g_len`, `los`) and
`m2-metrize-design.md` §1–2 (`SlotNesting`, `Relaxer`); binding rulings M3-R1…R7. Symbols (45 §5a): `L` layer index into the
sorted routable layers (`layers[0]` = lowest id = F.Cu), `E_L`/`X_L` edges / slot entries of layer `L`, `S` sites, `F` faces of
the union CDT, `T` SMD terminal pads, `q` grid candidates per clearance query, `m` connections, `k` sleeve length, `C =
LOS_PORTAL_CAP = 32`, `ε = Options.margin_nm = 5 000`. All constants: `MIN_SITE_PITCH_NM = 100_000`, `Options.via_cost_nm =
2_000_000`, `via_weight(tier) = {0|1: 2.0, 2: 1.0, 3: 1.0, 4: 0.75}`, `Options.max_vias_per_connection = 4`, `HIST_SITE =
HIST_BLOCK = 0.5`, `HIST_FAIL = 1.0`, `HIST_OVERUSE = 0.5`, `PRESENT_OVERUSE = 4.0`, `TIER_WEIGHT ω = {0|1: 0.5, 2: 0.75, 3:
1.0, 4: 1.25}`, `MAX_RIP = 3`, `DETOUR_FACTOR = 2.0`, `GHOST_BLOCK_NM = 1e10`, `REDUCTION_VIA_FACTOR = 4.0`,
`REDUCTION_LEN_SLACK = 1.10`, `pathfinder_iter_cap = 50`, `STALL_ITERS = 8`, `STUCK_REPEATS = 3`, `PROBE_STEP_CAP = 64`.

**Decision: discrete via sites (M3-R3), not continuous placement (25 §3).** A continuous via is a new CDT vertex on every
layer (13 §4 step 3): spade 2.15.1 can `remove`/insert, but `RouteGraph` indexes vertices, edges (`edge_of`), faces,
`faces_of_vertex`, the vertex grid and every live path's `(edge, slot)`/`faces` by position — all of it would need remapping
per via insert/remove, inside the PathFinder loop, for every candidate position. Sites fixed before the first search keep the
M2 per-layer graph immutable in structure; only vertex radii/clearances toggle (§3). Coverage: one site per face of the union
CDT (the incentre is the deepest point of a triangle) plus four escape sites per SMD terminal (fan-out, 13 §4); the price is a
via offset of at most one face from the length-optimal point, recovered by Weiszfeld refinement in M4.

## 1. Site generation (`vias.rs`, Task 7)

`via_sizes(rules, net)`: `[(nr.via_d, nr.via_drill)]` (`nr = rules.net_rule(net)`) followed by every `drill ∈
rules.routing_vias` with `rules.min_via_drill ≤ drill < nr.via_drill`, `d = max(drill + 2·min_annular, min_via_diameter)`,
sorted by `(d, drill)` descending, dedup; never empty, never a drill outside `routing_vias`. **Sizes for the fit** = union of
`via_sizes(rules, net)` over the nets of the live connections, sorted descending (P3; a netclass via larger than the default
is then fittable). **Candidates** (deterministic order: escape sites first, then face sites, each by `(x, y)`):

- *Union CDT:* `Cdt::build(points, &[])` with `points` = anchor centres of every routable layer's `LayerObstacles.anchors` ∪
  outline chord endpoints (dedup by exact position); no constraints. For every face whose centroid is inside the outline
  (`inside_outline` as `graph.rs`) with inradius `2·area/perimeter ≥ d_default/2 + c_max + ε` (`d_default =
  rules.default.via_d`, `c_max = max(default.clearance, min_clearance, hole_clearance, edge_clearance, per_net clearances,
  pad local clearances)`): one candidate at the incentre, `SiteOrigin::Face`. The inradius test is a cull only.
- *Escape sites:* for every pad index in `escape_pads` (SMD terminal pads of live connections): `hull_r` = radius of the
  smallest disc centred on the pad centre containing the pad copper (`model.skeleton(pad, layer)`: max over its primitives of
  far distance + half width); `c_pad = model.required_clearance(0, pad)`; `R = hull_r + c_pad + d_default/2 + ε`; candidates
  `at + R·u` for `u ∈ {+x̂, −x̂, +ŷ, −ŷ}` rotated by the pad rotation, `SiteOrigin::Escape(pad)`.
- *Fit:* `Checker::via_fits(at, size)` = `check_items(&[Item::Via { kind "via", net 0, at: kicad(at), d, drill, layers [],
  group None, wire None }])` has 0 violations — the checker is built with `margin_nm = ε` (not the margin-0 self-check
  instance). Net 0 is foreign to everything (`is_exempt` false), so pads of every net, kept tracks, copper shapes, NPTH
  (`hole_clearance`), existing via drills (`viahole:*`, `hole_to_hole`), edge and `no_vias` keepouts all reject. `max_d/max_drill`
  = the first size (largest→smallest) that fits; none ⇒ the candidate is dropped. `O(q)` per size.
- *Dedup:* walk the candidates in the order above; keep one iff no kept site lies within `MIN_SITE_PITCH_NM` (grid at that
  cell, `O(1)`). Fit guarantees `≥ d/2 + c` from every copper, hence no site coincides with a terminal or anchor vertex.
- *Output:* `Vec<ViaSite>` sorted by `(x, y)` (`total_cmp`); index = site id for the whole run. `in_pour[s]` (MultiGraph) =
  sorted pour indices `p` with a `PourFill` on a routable layer whose `outer` contains `at` **and** whose boundary is ≥
  `max_d/2` away (the via annulus lies in the fill; `O(S · ΣP)` once).

Complexity `O(N log N)` union CDT (`N` anchors) + `O((F + 4T) · |sizes| · q)` fits + `O(F + 4T)` dedup. Pinned:
`via_sizes_default_then_smaller_routing_vias`, `sites_fill_open_board_and_avoid_pads`, `escape_sites_next_to_smd_terminal`,
`via_sites_respect_keepout_npth_and_via_holes` (Review Focus 3), `site_pitch_dedup_and_determinism`.

**Build order (P3):** `ObstacleSet::build` → `via_sites(model, &obs, &checker_ε, layers, escape_pads, sizes, ε)` →
`MultiGraph::build(model, &mut obs, sites, …)` which first calls `obs.add_site_anchors(&sites, layers)` (`Anchor { c: at, r_core:
max_d/2, net 0, kind Via, own_clr 0, enabled: false, label "site:<s>" }` per layer; the anchor grid box uses `max_d/2`, never
grown later) and then `build_layer_cdt(obs.layer(l), terminals_on_l, Routing)` per layer — sites enter every CDT as anchor
points, so `ls.point_to_anchor` resolves them and `vertex_of(L, at)` is a lookup. `RouteGraph::build` maps a disabled anchor to
`VertexInfo { r 0, clr 0, nets [], owner None, anchor Some(ai), site Some(s) }`; `ObstacleSet::exempt_anchor` is true while
`!enabled` (the taut drops such `seq` entries, `taut.rs:161`). A site that is not a vertex of some layer ⇒ `Err`.

## 2. Layered slot search (`search.rs`, Task 8)

**State** `Edge(L, e, slot, dir)` as M2 on `mg.layers[L].g`, plus `AtSite(s, L)` ("standing on the via at site `s`, about to
leave on `L`"). Every node carries `vias_used ≤ max_vias` and the chosen `ViaSize` per site. Closed-set / best-cost key `(L, e,
slot, dir, vias_used)`; `AtSite` nodes use `e = usize::MAX, slot = s, dir = 0`; terminal goal nodes `e = usize::MAX, slot = q, dir
= 1`. Heap key `(f, h, layer_idx, edge, slot, dir, id)` by `total_cmp` then integers (Global Constraints). **Roots:** one root node
per start `(L, v)` (`starts[i]` = the terminal vertex on every layer of `TermRef.layers`, in layer order); `finish` walks parents
until a root. Ties between layers resolve to the lower `layer_idx` through the heap key (and `direct()` runs per layer in index
order, D3 included) → `route_tht_board_prefers_f_cu_on_ties`.

**Unowned site vertex** = a split-like vertex: `r = 0`, `clr = 0`, `nets = []`, no Rule T strand of its own; wires may cut its
corner and cross its incident edges freely. **Via move** from a state entering face `F` of layer `L` with a site vertex `v =
site_vertex[s][L] ∈ F.v` (≤ 3 per expansion, `verts[v].site`): admissible iff (i) `vias_used < max_vias`, (ii) `s` is not in the
node's ancestor site chain, (iii) `size` = the largest of `via_sizes(rules, net)` with `mg.site_free(s, net, size)` exists
(owner `None`, `blocked == 0`, `max_d ≥ size.d`, `max_drill ≥ size.drill`), (iv) the **M2 goal rule** `Cyc::goal_ok(strands, p0,
v)` holds (no strand of `F` separates the entry port from `v`), (v) `los` to `site.at` passes (as `push_goal`). Then push
`AtSite(s, L)` with `g_len` = apex rule to `site.at`, `g_cost += Δ + via_cost · (1 + ω · site_history[s])` where `via_cost =
SearchParams.via_cost` (= `opts.via_cost_nm · via_weight(tier)`, tier-weighted by the caller, P8), `h` as below. On pop: if `s ∈
goal.pour_sites` ⇒ **Found** (pour via, P1); else for every `L′ ≠ L` in index order apply the **M2 start rules** from vertex
`site_vertex[s][L′]` (fan walk over `faces_of_vertex`; incident edge slot = `|v-spokes|`, opposite edge blocked by a corner-`v`
cutter) with `vias_used + 1`, apex `site.at`, `since = 0`, `face_in` on `L′`. Legs are cut at `AtSite` nodes: `Leg { layer_idx,
start, goal, crossings, faces, est_len }`, `SlotPath { legs, sites /* (s, size) per via */, est_len = Σ legs, cost = g_cost at
the goal, expansions, candidates = AtSite nodes pushed }`, `vias() = sites.len()`.

**Why the goal rule is needed (Step 2 counter-example).** Layer L, terminal `p` of connection A adjacent to site `s` (edge
`(p, s)` exists), third vertex `x`. A leaves `p` through edge `(s, x)`: strand `Vertex(p) ↔ Cross((s,x), i)` in face `(p, s, x)`.
B (foreign) enters that face through `(p, x)` and wants the via at `s`. Cyclic ports: `p, [(p,x) entries], x, [(x,s) entries], s,
[(s,p) entries]`; B's chord `Cross((p,x)) → Vertex(s)` cuts the cycle into an arc containing `x` and A's `(x,s)` entry and an arc
containing `p`: A has one port on each ⇒ `goal_ok` false ⇒ the move is rejected (B must enter through `(s,x)` beyond A's entry or
reach `s` from another face). Without the rule B's via and its leg would sit inside A's spoke and A's wire would be cut — a
crossing on L. On leaving `s` the start rule places B at index `|s-spokes| = 0`, i.e. inside any strand wrapping `s` on `L′`
(planar; the wrapping wire's capacity shrinks on activation, §3). → `via_move_respects_rule_t_at_site`.

**Goals.** `Goal { vertices: [(L, v)] /* terminal on every layer of to.layers; for a Pour target also every same-net via listed
in pour.vias on every routable layer */, pour_sites: sorted sites with in_pour ∋ pour, pour_layer }`. A terminal goal is the M2
goal rule at `v` on its layer; a pour goal is satisfied by the via itself (P1): the path ends at `AtSite(s, ·)` with `sites.len()
== legs.len()`, no zero-length leg on the pour layer; a track from a pad on the pour's layer ending at a via inside the fill is
equally served. **`h`** = `min(min_{(L,v)} |P − at_v|, dist(P, bbox of the pour fills))` (0 inside): Euclidean, ignores via cost,
admissible; `O(|vertices|)` per node. `f = g_cost + h`. **Direct path** per layer as M2 + D3 (`clearance_bend(p, q, &[])`), first
layer in index order wins. **Caps:** `default_expansion_cap(mg) = 8·Σ_L(E_L + X_L) + 4·S + 64`; deadline every 16 expansions.
**Ghost:** same machinery with no strands, no slots, no ownership (sites and layers free, `vias_used ≤ max_vias` still), cap
`8·Σ_L E_L + 4·S + 64`; soft cost `GHOST_BLOCK_NM` on non-passable/unfit edges **and** on owned or blocked sites; returns `(path,
blocked: Option<(L, e)>, blocked_site: Option<s>, last_reached: Option<(leg, idx)>)` = the first soft-blocked element in travel
order (P5). `search()` runs the ghost on `Blocked` as M2. Per expansion `O(n_max + C² + (C+1)·local)` (Task 2); `O(V′ log V′)`, `V′
= Σ_L 2(E_L + X_L) + 2S`. Pinned: `tht_pads_route_on_either_layer_without_via`, `smd_f_to_smd_b_needs_exactly_one_via` (cost =
est_len + 2 mm at tier 3), `via_cost_prefers_detour_when_cheaper`, `owned_and_blocked_sites_are_not_used`, `max_vias_cap_blocks`,
`pour_goal_reaches_nearest_site_inside_pour` (legs 1, sites 1 after P1), `layered_search_is_deterministic_and_capped`.

## 3. Site occupancy (`multigraph.rs`, `graph.rs`, `obstacle.rs`, Task 7)

`owner[s]: Option<(conn, ViaSize)>`, `blocked[s]: u32`, `blocks_of[s]: Vec<usize>` (sorted; sites `s` blocks while active),
`blocked_by[t]` (reverse, sorted), `site_version[s]: u64`, `clock: u64`. **activate(s, conn, net, size, clr)** (exclusive; a
second activation is a bug, `debug_assert`): (a) on every layer `L`: `g.set_site(v, Some((net, d/2, clr, "via:<conn>:<s>")))` →
`VertexInfo { r = d/2, clr = clr, nets = [net], owner = Some(label) }`, `cap`/`c_e` of the incident edges recomputed (`cap = |ab|
− r_a − r_b`, `c_e = max(clr_a, clr_b)`), `rho_max = max(rho_max, r + clr)`, `edge_version` bumped on every edge of every face
around `v`; (b) obstacle anchors on every layer: `enabled = true, r_core = d/2, net, own_clr = clr, label` (anchor grid
untouched: `max_d/2 ≥ d/2`); (c) neighbours: `t ∈ sites_near(bbox(at ± (d_s/2 + max_d_all/2 + max(c_s, hole_to_hole))))`, `t ≠ s`,
with `|st| < d_s/2 + max_d_t/2 + max(c_s, hole_to_hole)` ⇒ `blocked[t] += 1`, `blocks_of[s].push(t)`, `site_version[t] = ++clock`;
`site_version[s] = ++clock`. **release(s)** undoes (a)–(c) from the stored `blocks_of[s]` (restores `r 0, clr 0, nets [], owner
None`, anchors disabled, net 0). Overuse created by (a) on edges crossed by other legs is ordinary edge overuse (negotiated in
§4). Via–via hole-to-hole among new vias is guaranteed by (c), via–existing-drill by the fit (§1); the self-check re-checks
tracks vs vias and vias vs the board, not new-via pairs (KiCad DRC is the final judge: `routing_new == 0`). Same-net wires may
pass over an active via (`nets = [net]` ⇒ no clearance in `los`/taut, as a same-net pad). `O(layers · deg + neighbours)`. Pinned:
`sites_are_vertices_of_every_layer`, `activate_shrinks_capacity_and_blocks_neighbours`, `in_pour_membership_from_fills`,
`graph_set_site_recomputes_only_incident_edges` (non-incident edge versions unchanged — face-edge bumps touch only faces around
`v`).

**Leg ids (P2).** A path may have two legs on one layer (F→B→F), so the per-layer `RouteGraph` cannot index paths by
connection: `MultiGraph` owns `wire_of: Vec<(conn, leg)>` and `conn_wires: Vec<Vec<usize>>`; `insert_path(conn, &path)` allocates
one id `k` per leg from the sorted free list (then push), calls `layers[leg.layer_idx].g.insert_path(k, start, goal, crossings,
faces, w, c)` per leg, `activate` per site, and returns the ids; `remove_conn(conn)` reverses (sites released, ids freed). `k` is
also the `Wire.id` of §5. `strand_blockers`/`overused_conns` return leg ids, mapped to connections by `wire_of` (sorted, dedup).
`edge_version` **semantics (P4):** `insert_path`/`remove_conn` bump (`++clock`) every edge of every face in `faces` and, for a
direct path, the `along` edge and the edges of its two faces; `set_site` bumps the edges of every face around the vertex. The
*touched set* `T(path)` = those same edges; §7 validates a path by `T(path)`.

## 4. Negotiation over layers and sites (`negotiate.rs`, Task 9)

`Negotiator { history: Vec<Vec<f64>> /* [L][e] */, site_history: Vec<f64>, bumps, iteration, pending_rip, cur }`. Iteration =
M2 §4 over `order(conns, bumps)` (key `(−bump, tier, −width, est_len, net, id)`), in batch form (§7). Search params per
connection: `history`, `site_history`, `weight = ω(tier)`, `present_overuse`, `expansion_cap`, `deadline`, `eps`, `via_cost =
opts.via_cost_nm · via_weight(tier)`, `max_vias = opts.max_vias_per_connection`. Edge step cost unchanged (`Δ · (1 + ω·hist[L][e])
· (1 + ω·(present − 1))`); via cost as §2. **Found** ⇒ absurd-detour test as M2 (`est_len > 2 × straight` and `> 2 × ghost
est_len`, lengths only) then `mg.insert_path`. **Blocked** ⇒ `BlockInfo { edge: Option<(L, e)>, site: Option<usize>, ghost,
last_reached }` (P5); when `edge == None && site == None` with a ghost (purely topological) **or** the ghost uses an owned or
blocked site: blockers = `strand_blockers` per ghost leg on its layer ∪ `owner[s].conn` for every owned ghost site ∪
`owner[u].conn` for `u ∈ blocked_by[t]` of every blocked ghost site; `rip(i, blockers)` as M2 (`≤ MAX_RIP = 3`, else nothing):
`history[L][e] += HIST_BLOCK` on the blockers' leg edges, `site_history[s] += HIST_SITE` on their sites, blockers → `pending_rip`
(removed — sites released — before the next iteration's searches, and in `active`). `penalise_blocked(i, b)`: `HIST_BLOCK` on
the ghost's edges per layer and `HIST_SITE` on its sites, `bumps[i] += 1`. `penalise_failed` (`HIST_FAIL` on all legs' edges,
bump) and `penalise_overuse` (per layer) unchanged; sites never carry overuse (ownership is hard in the slot search). Next
`active`, convergence, stuck and stall rules as M2; `timed_out` as M2. **Best iteration** key `(routed ↓, vias ↑, Σ est_len ↑)`
(`Snap` adds `vias`). Review Focus 2 (`negotiate_site_contention_two_nets`): A takes site `S1`, `S2` is blocked by (c); B finds
`S1` owned, `S2` blocked ⇒ pays the detour to another site (Found, 1 via, different site) — or, with no other site, Blocked with a
ghost through `S1` ⇒ A ripped, B bumped ⇒ next iteration B (first) takes `S1`, A detours; monotone `site_history` and the bump
order make the switch happen once; cap 50. Review Focus 4: a pour connection with `pour_sites == []` and no listed via on a
layer of `to.layers` fails permanently before any search with `"no via site inside pour"` (`perm`, removed from `active`,
`iterations ≤ 2`). `detect_clinches` per layer, summed. Complexity `O(iters · m · V′ log V′)` + `O(Σ_L E_L + S)` per iteration.
Pinned: `site_rip_up_frees_owner_for_blocked_connection`, `route_pour_completion_without_site_fails_cleanly`,
`route_two_layers_deterministic`, M2 `negotiate_loop.rs` through `SlotPath::single_layer`.

## 5. Metrization per leg (`route.rs`, `relax.rs`, Task 9)

One `Wire` per leg, `id = k` (P2), `layer = layers[leg.layer_idx]`, `net`, `width`; terminals: the real terminal (`terminal_of`)
at the path ends, `Terminal::Site { site: s, at: site.at }` at sites (P6: new variant, `at()` = `at`, `is_fixed_point() = false`;
`Terminal::Via { via }` keeps indexing `model.vias`), `hand = [start, goal]`, `hand_len = leg.est_len`. `MetrState` keeps `wires`
by `k` (`Relaxer::replace/remove` on path change: all old legs of the connection dropped, new ids allocated — `O(legs)`);
`SlotNesting` = `slot_nesting(&mg.layers[l].g, conn_of = identity)` per layer, merged into one map (`(k, AnchorRef)` is
unambiguous because `k` lives on one layer); one `Relaxer` over both layers (`stacks`/`dyns` are keyed by layer already).
Own via anchors are exempt for every leg of the owner: `Members.terms` holds the leg's site centre and the connection's other
vias carry its net (`exempt_anchor`); disabled site anchors are exempt for everyone. Items: tracks of every leg + one `ViaItem
{ kind "via", net, at: kicad(site.at), d, drill, layers [], group None, wire: Some(conn) }` per site, attached to the wire of
the leg that ends at the site (the trailing pour via to the last leg) so `ItemIndex` replacement follows the leg; `check_against`
with `skip = own ids`, same-net exemption covers the connection's other legs/vias. A connection is `Ok` iff every leg and via
passes; `length_outside_pads_nm = Σ_legs outside_len` with the pad skeletons of the real terminals only (a `Site` end subtracts
nothing). Partial stubs from the ghost's first leg (D6) only when `BlockInfo.edge` is on that leg; `blocking` = midpoint of the
edge or the blocked site point. Zero-slack retry (R6) per leg on its layer. `Snap` (P7) stores `mg: MultiGraph` (graphs, site
tables, `wire_of`) + `MetrState`; restoring a snapshot re-syncs the obstacle anchors from `mg.owner` (`sync_anchors(&self, obs:
&mut ObstacleSet)`, `O(S)`); `ObstacleSet` is never cloned and `Ctx` no longer holds it (passed `&`/`&mut` explicitly). Reports:
`ConnReport { vias, layers: [names of the legs' layers, dedup in order], via_sites: [kicad nm] }`, `RouteResult { vias,
length_by_layer_nm, sites, via_reduction }`. Complexity as M2 per layer; `O(vias · (layers·deg + neighbours))` per `insert_path`.
Pinned: `route_b_cu_pads_route_without_via`, `route_mixed_layers_uses_one_via` (Review Focus 1), `route_wall_on_f_cu_goes_under_on_b_cu`,
`route_pour_completion_lands_via_in_pour`, `route_terminal_on_no_routable_layer_fails`, `test_route_surface_round_trip`.

## 6. Via-reduction pass (`route.rs`, Task 10; M3-R2, TopoR lifting 13 §2)

After the loop ends by convergence, stall or stuck (not by timeout; `"skipped: timeout"`), before `zero_slack_pass`, from
`best`: candidates = connections with `vias ≥ 1` in order `(−vias, id)`. Per candidate: deadline check (`"skipped: timeout"`
with the counts so far); `snap_c = Snap` of the current state; `mg.remove_conn(c)`; `search` with `via_cost · REDUCTION_VIA_FACTOR`
(= 4.0), the **current** `history`/`site_history`, `present` as in the loop; accept iff `Found` ∧ `vias < old` ∧ after
`insert_path` + incremental `metrize` + self-check: every previously `Ok` connection is still `Ok` ∧ `c` is `Ok` ∧ `overuse_edges
== 0` ∧ `out_c ≤ old_out_c · REDUCTION_LEN_SLACK` (1.10, outside-pad length of `c`); else restore `snap_c` (graphs, metr state,
anchors re-synced). One pass, no second visit; `(removed, candidates)` → `RouteResult.via_reduction = "removed N of V vias in C
candidates" | "off" (`Options.via_reduction = false`; default `true`) | "skipped: timeout"`. Accepted results update `best` (fewer vias ⇒ better
by the §4 key). `O(m_v · (V′ log V′ + metrize))`. Pinned: `via_reduction_removes_detourable_via_pair`,
`via_reduction_keeps_necessary_vias_and_length_bound`, `via_reduction_off_option`.

## 7. Parallel batch with deterministic commit (`negotiate.rs`, `route.rs`, Task 11)

`route()` = `ThreadPoolBuilder::new().num_threads(opts.effective_threads()).build()` → `pool.install(|| route_inner(..))`,
`threads_used` = pool size (fallback 1 + warning). **Iteration (batch mode, also with 1 thread):** (1) remove `pending_rip` and
*every* active connection from `mg` (sites released), `t0 = mg.clock` — the snapshot is the graph as it stands now and is not
mutated until (3); (2) `results = active_in_order.par_iter().map(|i| search(&mg, &obs, conn_i, starts, goals, params)).collect::<Vec<_>>()`
(index order; every search reads only `&MultiGraph`, `&ObstacleSet`, `&history`); (3) commit sequentially in the same order:
`Found(p)` is **valid** iff every edge of `T(p)` (§3: all edges of all faces of every leg, `along` edges) has `edge_version ≤ t0`
∧ every site in `p.sites` has `site_version ≤ t0` ∧ no site activated or released since `t0` lies inside `bbox(vertices of p's
faces) ⊕ ρ` with `ρ = g.rho_max() + w/2 + ε` (the `los` clearance test may read vertices just outside the sleeve; the list of
activated sites is kept in commit order) — valid ⇒ the M2 Found handling (`ghost` for the detour test and `insert_path` run
against the current graph); invalid ⇒ `search` again sequentially against the current graph (`IterOutcome.resolved_sequentially
+= 1`) and handle that outcome; `Blocked` from the snapshot is final (commits only add strands, capacity use and ownership — a
monotone restriction — so the current graph blocks too); its ghost is used for `strand_blockers`/site blockers against the
current graph; `Timeout` ⇒ `timed_out`, stop (the iteration is discarded, as M2). `check_against` maps items in parallel and
concatenates reports in item order; `MultiGraph::build` builds layers in parallel into a `Vec` in layer order; metrization stays
sequential Gauss–Seidel. **Thread-count independence:** (i) during (2) the inputs are immutable and each `search` is a pure
deterministic function of them (sorted candidate lists, total heap order, no `HashMap` iteration reaches a result), so
`results[i]` is the same for any pool size and `collect` preserves index order; (ii) phase (3) is sequential and every decision
(validity, re-search, rip, insert) is a function of `results` and of the graph produced by the earlier commits, which is the
same by induction; (iii) the only non-determinism is the wall clock, reported as `timed_out` exactly as in M2. Hence `threads ∈
{1, 2, 4}` give byte-identical JSON minus `runtime_s`/`threads_used`. Difference to M2 sequential: a later-ordered active
connection no longer sees the *old* paths of earlier-ordered active ones (all removed in (1)); every conflict it would have had
with their *new* paths is caught by `T(p)` and re-searched, so `iteration_batch_commit_invalidates_shared_edge` (two
connections, one cap-1 channel) ends as M2: the second is invalidated, re-searched against A's path, and detours. Cost per
iteration `O(|active| · V′ log V′ / threads + Σ |T(p)| + re-searches)`, re-searches ≤ `|active|`. Pinned:
`route_threads_byte_identical`, `threads_zero_uses_available_parallelism_minus_one`, `parallel_self_check_matches_sequential`,
`iteration_batch_commit_invalidates_shared_edge` (Review Focus 5).

## 8. Complexity table (45 §5a) and caps

| Component | Cost | Cap / bound | Pinned by |
| --- | --- | --- | --- |
| Union CDT + candidates | `O(N log N + F + 4T)` | `F ≤ 2N` by Euler | `sites_fill_open_board_and_avoid_pads` |
| Fit + dedup + `in_pour` | `O((F+4T)·n_sizes·q + S·ΣP)` | once per run | `via_sites_respect_keepout_npth_and_via_holes`, `site_pitch_dedup_and_determinism` |
| `MultiGraph::build` | `O(Σ_L N_L log N_L)` | layers in parallel | `sites_are_vertices_of_every_layer` |
| Layered search | `O(V′ log V′)`, per expansion `O(n_max + C² + (C+1)·local)` | `8·Σ_L(E_L+X_L) + 4S + 64`; ghost `8·Σ_L E_L + 4S + 64`; `max_vias 4`; deadline /16 | `layered_search_is_deterministic_and_capped`, `max_vias_cap_blocks` |
| Site activation / release | `O(layers·deg + neighbours)` | neighbours within one box query | `activate_shrinks_capacity_and_blocks_neighbours` |
| Negotiation | `O(iters·m·V′ log V′ + iters·(Σ_L E_L + S))` | `pathfinder_iter_cap 50`, `STALL_ITERS 8`, `STUCK_REPEATS 3`, `MAX_RIP 3`, `PROBE_STEP_CAP 64` | `negotiate_site_contention_two_nets`, `site_rip_up_frees_owner_for_blocked_connection` |
| Metrization | `O(k·q)` per wire per pass, incremental; `SlotNesting O(Σocc·n_max)` per layer | relax caps as M2; self-check `O(changed·q)` | `route_mixed_layers_uses_one_via` |
| Via-reduction pass | `O(m_v·(V′ log V′ + metrize))` | one pass, deadline per candidate | `via_reduction_keeps_necessary_vias_and_length_bound` |
| Batch commit | `O(Σ #T(p) + activated·faces)` + re-searches | ≤ `n_active` re-searches; deadline before every batch | `iteration_batch_commit_invalidates_shared_edge` |

No stage is exponential; no order permutation; no all-pairs loop (neighbour tests, fits and `in_pour` are grid/polygon
queries). Determinism checklist: `sites` sorted `(x, y)`; `in_pour`, `blocks_of`, `blocked_by`, `sites_near`, blockers sorted;
free-list ids sorted; heap key total; `collect` in index order; no `HashMap` iteration reaches an output.

## Plan amendments (Tasks 7–11 text vs. this note)

- **P1** Task 8: a pour goal is reached by the via itself — `SlotPath.sites.len() == legs.len()` for a pour path, no zero-length
  leg on the pour layer; `vias() = sites.len()`; `pour_goal_reaches_nearest_site_inside_pour` expects `legs 1, sites 1, sites[0].0 ∈
  in_pour`; `Goal.pour_layer` is informational; listed same-net pour vias are ordinary `(L, v)` goals on every routable layer.
- **P2** Tasks 7/9: per-layer graph paths are indexed by **leg id**, not connection id (a path may have two legs on one layer);
  `wire_of`/`conn_wires` move from `MetrState` to `MultiGraph`; `MultiGraph::insert_path` allocates and returns leg ids;
  `strand_blockers`/`overused_conns` results are mapped through `wire_of`.
- **P3** Task 9 orchestration order: `ObstacleSet::build` → `via_sites` → `MultiGraph::build` (which calls `add_site_anchors`
  before `build_layer_cdt`, sites as anchor points); `via_sites` receives `sizes` = union of `via_sizes(rules, net)` over the live
  nets (descending), not `via_sizes(rules, 0)`; the fit checker is a margin-`ε` instance.
- **P4** Task 7/11: `edge_version` is bumped on every edge of every face a path's `faces` list touches (plus `along` edges and
  the faces around a `set_site` vertex); batch validity additionally rejects a path when a site activated/released since the
  freeze lies within `ρ = rho_max + w/2 + ε` of the path's face bbox.
- **P5** Task 8/9: ghost/`BlockInfo` gain `site: Option<usize>` (first soft-blocked element may be a site); `blocked_edge:
  Option<(layer_idx, edge)>`, `last_reached: Option<(leg, crossing idx)>`.
- **P6** Task 9: new `Terminal::Site { site, at }` for leg ends at sites (`Terminal::Via { via }` keeps indexing `model.vias`).
- **P7** Task 9/10: `Snap` holds `mg: MultiGraph` + `MetrState`; obstacle anchors are re-synced from `mg.owner` on restore;
  `Ctx` drops `obs` (passed explicitly, `&mut` only for activation).
- **P8** Task 8: `SearchParams.via_cost` is the final per-via cost (caller multiplies `via_cost_nm · via_weight(tier)`; the
  reduction pass additionally `× REDUCTION_VIA_FACTOR`); `via_weight(0) = via_weight(1) = 2.0`.

## Amendments (F2, 2026-10-31)

- **§2 / §4 ghost (M3-R9):** the ghost is topological: a Dijkstra over `(layer, edge, dir, vias ≤ max_vias)` plus
  `AtSite` nodes on the same adjacency, key `(soft, length)` lexicographic (`soft` = edge not passable or not fitting,
  owned / blocked site; `length` = hops between edge points, the point of the edge nearest the straight line to the goal),
  no `los`, no clearance candidates, no slots; only faces outside the outline are hard. `est_len` = funnel length of the
  found crossing sequence (`sketch::funnel`, `O(k)`). Ghost crossings carry the slot `Cyc::cross_slot` would choose from
  the previous crossing's gap (the append position where none is feasible), and `leg_strand_blockers` walks those slots:
  the M2 routine assumed append slots everywhere and missed every strand that ends on the ghost's own exit edge (the
  plus-board fixture `rip_up_ping_pong_is_damped`, wiccon's 0 rips). `GHOST_BLOCK_NM` is gone. Cost
  `O(Σ_L E_L (max_vias + 1))` per run: dcsgone 976 → 33 ms/run, 0 candidates.
- **§7 damping (RC-A2):** `Negotiator::rip` keeps `rip_pairs[(ripper, blocker)]`; a connection rips a given blocker at
  most `RIP_PAIR_CAP = 3` times per run (both the absurd-detour rip and the blocked-ghost rip), so total rips are bounded
  by `3·m²` whatever the length comparisons do. Why 3 and not 1: one exchange leaves `HIST_BLOCK = 0.5` on the blocker's
  class, which does not settle a three-way contention (F.Cu-only eprom: with a cap of 1, +5V is left failing on its
  zero-slack corridor, 26/27); three leave 1.5, the scale of one `HIST_FAIL`. The topological ghost itself already
  removes the RC-A2 cycle (synth layers=1 n=1000: 17 → 9 iterations, converged, with no damping at all; the cap keeps 9).
- **§5 report (M3-R11):** `ConnReport.ghost_len_nm` = `est_len` of the ghost on the empty graph (every live connection,
  computed once in parallel, index order), `null` when never searched; the harness reports `length_nm / ghost_len_nm`.
  It is not reused by the absurd-detour rule: that rule needs the ghost against the current graph for its blockers, and
  a funnel length of a hop-sum-optimal sequence is not monotone in the occupancy, so a static reference would not keep
  the rule's semantics.
- **§2 / §4 terminal pre-check and `dead` (F3):** before any search, `search::terminal_open(g, v, net)` must hold for
  some start vertex and (terminal goals) some goal vertex: an inside face around the vertex with a statically passable
  edge (`RouteGraph::passable_static`: both faces inside, constraint owned by the net; `along` excluded) or a via-site
  vertex. Otherwise `Blocked { ghost: None, static_reason: Some("blocked: terminal cannot be left/entered (width w,
  clearance c)") }` at once, `O(Σ deg)`, no expansion, no ghost (`ghost_len_nm` null). `Negotiator.dead` holds exactly
  these connections (with the reason, repeated every iteration without a search); a ghost `None` no longer makes a
  connection dead. Finding on dcsgone U7.4 (#201/#202/#203, net65 at 0.2 mm / 0.2 mm, 0.5 mm pitch): the rule does
  **not** fire — all 4 faces around v3284 are inside and all 12 edges are passable; every edge is statically unfit
  (spokes cap 195 194 nm, long sides 375 000, short sides 0 against a need of 605 000) and the block is geometric: a
  0.2 mm wire with 0.2 mm clearance must keep its centreline within ±75 µm of the pad axis between the 0.5 mm-pitch
  neighbours and no state point of a first / last move lies there, so `los` refuses every entry and the search
  exhausts the component (#201: 209 668 expansions, 24 s). A sound geometric entry probe is deferred (F4/F8 candidate);
  "all edges unfit" is not a sound static block (the rect-pad fixture of `ghost_is_never_stricter_than_search` is
  all-unfit and routes).
- **§7 batch re-validation (F4):** a snapshot path whose touched faces changed is re-validated in `O(k)` by
  `search::revalidate_path` (slots recomputed with `Cyc::cross_slot` against the current strands, every crossed edge
  passable and fitting, `goal_ok` at the goal / site vertex, `site_free` per via, a direct leg's edge free) and inserted
  with those slots (`revalidated`); it is searched again only when that fails or the geometry moved (a site of the path
  changed, or a via activated since the freeze within its own `d/2 + clr + w/2 + ε` of a leg's faces bbox — per leg and
  per via, no longer `rho_max`). Conservative: the accepted path is never looser than a fresh search.
- **Anytime (coordinator ruling Q2):** when an iteration times out before any `Snap` exists, the paths it committed are
  metrized (first pass + one self-check round run regardless of the deadline) and emitted; unrouted connections report
  `timeout`.
- **F3b (geometric entry probe) — skipped, not sound.** The forward entry into a goal is `push_goal`'s `los` over the
  whole apex chain of the arriving state; the only local necessary condition provable from it is "some point of an
  inside face around the goal has a clear straight segment into the goal", which the pad centre itself satisfies at
  U7.4 (the centre is 0.375 mm from the neighbour copper > 0.3 mm needed), so it never fires there. What actually blocks
  U7.4 is the discretisation of the slot search: state points sit at the vertex margins (62.5 µm off the pad axis at a
  roundrect corner) outside the ±75 µm corridor the 0.2 mm wire has between 0.5 mm-pitch neighbours; a reverse
  "can the goal be left" probe reproduces that, but it is not a necessary condition of the forward entry (different
  state points, different apex), so it could block a connection the search would route. Left to the gate review.

## 9. Pour reach after routing (`pourcheck.rs`, `route.rs` completion passes; R3-pourreach, 2026-10-31)

**Bug.** wiccon-2023 (2d3063e70ed5) routed 60/60 with routing_new 0, but the routed DRC reported 2 unconnected items, both
GND: `U1.15` (SMD 2.5 × 1.0 mm on B.Cu) ↔ zone and `J2.2` (PTH ⌀1.7) ↔ zone. Measured on the refilled routed board (pcbnew
SWIG, zone: thermal relief, gap 0.508, spoke 0.508, min thickness 0.254, clearance 0.508): KiCad's four spoke test points per
pad (pad bbox edge + gap + ε, `buildThermalSpokes`) were all outside the refilled copper — U1.15: nearest fill 1.4–1.8 mm away
on every side, GPIO0 passing 0.3–0.5 mm from the pad's top/left edge and RST below-right on B.Cu; J2.2: MISO (B.Cu, 1.18 mm from
the centre), MOSI (F.Cu, 1.30 mm) and 4v in the thermal band on both pour layers. In the original both pads were served by
spokes alone (no GND via or track within 4 mm; U1.15 spokes at 180°/270°, J2.2 at 180°/270° on both layers); the ripped
board's fill reaches them, so `decompose` (Task 6) correctly made no connection for them — and nothing kept that true while
the foreign nets routed. Root cause: *pour-reached* was decided on the pre-route fill and never re-verified.

**Fix (engine, preferred (a)): verify the reach after routing, complete what was lost, negotiate if the completion is boxed in.**

*Fill approximation* (`pourcheck::check_pour_reach`, per (pour, layer), parallel, deterministic): raster of the pre-route fill
polygons at `cell = max(25 µm, √(bbox area / 4·10⁶))` — the exported fill is KiCad's own (fractured outlines; holes exported
too), so it already carries every knockout of the pre-existing copper and the thermal holes + spokes of the pour's pads —
minus the router's foreign-net tracks/vias dilated by `max(zone clearance, net clearances) + min_thickness/2 + cell/√2`
(clearance knockout, then the min-width opening, conservative by half a cell); 4-connected islands by flood fill.
*Connectivity* of the pour net: union-find over islands, pads, vias (model + routed) and tracks (kept + routed): a thermal pad
joins the island a spoke test point lands in (a point is in the opened fill when a fill cell lies within `min_thickness/2`
of it), a solid pad the islands under its copper (`ZONE_CONNECTION` per zone, pad override exported), a via the island under
its disc, a track the islands it runs over; tracks join the pads / vias / tracks they touch (copper gap ≤ 0). *Main component*
= the one with the most pour-listed items (ties: largest fill area). *Lost* = listed pads/vias and pads of connections routed
in this run that are not in it. `O(cells + Σ stamps + T·(P+V+T))`.

*Completion passes* (`route_inner` → `route_pass` × (1 + ≤ `POUR_REACH_PASSES` = 3)): pass 1 routes a pour-completion
connection per lost cluster against the routed copper as fixed obstacles (`with_routed_copper`: items → kept/vias with uuids
`topor:<i>`, pours relisted with the main component's items only, `Scope { pads: lost, vias: lost }`; pour goals restricted
to via sites inside the main component, `site_in_main`, so the completion via cannot land in a cut-off island). When a
completion is blocked (wiccon U1.15: no via of any allowed size fits around the pad, GPIO0/RST seal it), the next pass
unfreezes the routed foreign connections whose copper lies within `hull + 2·(default via + clearance)` of the lost item: their
reports/items are dropped (`drop_conns`, ids renumbered), their terminals join the scope, and they re-route together with the
completion in one negotiation. A pass that still fails after unfreezing, a pad blocked by fixed copper only, the pass cap or
the budget end the loop with a `pour reach: … without pour` warning. A model without fills, or a clean check, returns pass 1
untouched (eprom byte-identical). Results merge by `merge_pass` (ids continue, per-net reports and totals recomputed).

wiccon: 60/60 → 62/62 (J2.2 + U1.15 completion, 1 via each; pass 2 re-routed GPIO15, RST, SCLK, GPIO0 ×2 around U1.15),
unconnected 2 → 0, routing_new 0, vias 33 → 37, router 1.5 → 3.4 s (4 threads), threads 1/4 byte-identical.

Pinned: `pourcheck::tests::{untouched_fill_keeps_every_listed_pad, foreign_track_cutting_the_fill_loses_the_pad_on_the_small_side,
same_net_track_and_via_bridge_the_cut_pad, routed_pad_whose_via_lands_in_a_cut_island_is_lost,
spokes_blocked_on_all_four_sides_lose_the_pad}`, `route_pour_reach_completes_pad_cut_off_by_foreign_track` (route_layers:
one routed connection and a cut-off pad before; completion via inside the main fill, byte-identical across threads after).
Harness: `preexisting_unconnected` (count + items; an unconnected pair is pre-existing iff the same uuid pair is unconnected
in the ripped-unrouted DRC) in `classify` / bench metrics / summary / caption, `test_unconnected_preexisting_iff_same_pair_in_ripped_baseline`.
Model: `Pour { clearance, min_thickness, thermal_gap, spoke_width, pad_connection }`, `PourFill.holes`,
`Pad { zone_connection?, thermal_gap?, spoke_angle_deg? }`, `Scope.vias`.
