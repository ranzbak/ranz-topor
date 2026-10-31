# M4 design: own-net capacity, Steiner attachment, exit stubs, G1 continuation, via relaxation, rip cost (F, task A3)

Scope: the six engine rules of M4a tasks A4, A5, A6, A13, A14, A16 (engine 0.5.0), fixed here so the S workers only
implement and test. Inputs: `m3-detour-diagnosis.md` (M4-D1/D3, withdrawn F6), `graph.rs:462-500` (`fits_at`, `overuse`),
`search.rs` (`margin_of`, `hard`, `step_cost`, `leave_vertex`), `wires.rs` (terminals, hand T-splits), `relax.rs`,
`negotiate.rs:40-55, 510-528` (`rip`), 46 rows 6/17/17b, 12 §1, 22 §3, 25 §3. Note only; no engine code. A parallel F
worker (DX) classifies the 67 dcsgone failures at the gate options; the numbers below are the committed 0.3.0 run
(dcsgone 60 s, 148/231: 45 timeout, 16 "blocked: no path (wall, capacity or via sites)", 8 "infeasible: wall grazed /
pad-pad / site-site", 5 "self-check: hole gap", 3 "sketch: inconsistent slot path"; wiccon 62/62, vias 37, outside
1373.55 mm; eprom 27/27, 64.628 mm) and the M3 diagnosis cases (#34, #45, #197, #207, #208, wiccon #52). Units: mm in
the text, nm in the code; `ε` = `rules.eps` (5 µm), `c` = the connection's clearance, `w` its width.

## 1. Own-net / same-net capacity, wall-aware (A4, M4-D1)

Today (`fits_at`): `used = Σ_x (w_x + ε + max(c_x, c))`, `fits ⇔ used + w + ε + end_gaps · max(c, c_e) ≤ cap`, with
`cap = |ab| − r_a − r_b` and `end_gaps = 2 − free ends`; net-blind, so a pad's own tip edge (0.6 mm roundrect, corner
anchors r 0.15: `cap = 0.3 − 0.15 − 0.15 = 0`) is unfit for its own 0.5 mm track, and two strands of one net on one
tip edge charge full clearance against each other. The search still crosses such edges as overuse (`present_overuse ·
(1 + overuse/cap)`, factor 4-8 per step), which decides via-vs-direct and feeds `overuse_edges`.

**Rule.** `fits_for(e, n, w, c, ε)` replaces `fits_at` everywhere a net is known (`push_state`, `hard`, `soft_blocked`,
`revalidate_path`, `terminal_open` via `passable_static` + a fit for the connection's width). For an edge `e = (a, b)`
of length `L` and a connection of net `n`:

    own(v)   ⇔ n ∈ nets(v)                       (anchor net or tie net; a via site owned by n counts, a dormant site is free)
    E_v      = 0                                   if free(v) or own(v) and no foreign strand on e
             = r_v + max(c, c_e)                   if foreign(v)                              (unchanged M3 end term)
             = r_v + max_x max(c_x, c_e)           if own(v) but some strand x on e has net(x) ∉ nets(v)  (its gap stands)
    K_v      = r_v                                 if own(v) and E_v = 0   (the strand may run over the own core)
             = 0                                   otherwise
    g(x)     = 0 if net(x) = n, else max(c_x, c)   (gap between an existing strand x and the new one)
    usable   = L − E_a − E_b + K_a + K_b − W_n(e)
    need     = Σ_x (w_x + ε + g(x)) + w + ε
    fits_for ⇔ need ≤ usable

With no own end and no same-net strand this is exactly the M3 formula (`cap − end_gaps·max(c, c_e)` = `L − r_a − r_b −
…`), so boards without the case are byte-identical. `overuse(e)` (net-blind, per edge) becomes the same accounting over
the strands actually present, in slot order: consecutive pairs charge `max(c_i, c_j)` or 0 when same net; the first /
last strand charges `r_v + max(c_s, c_e)` at its end or 0 when its net owns `v` (then `+ r_v` credit);
`overuse = (need − L − credits).max(0)`. `GEdge` keeps `cap`/`c_e` and gains `len` and `walls_near: Vec<usize>`.

**Wall term `W_n(e)`** (why F6 failed: U2.28, THT r 0.8 at the outline; with the radius given back the edge became fit,
the loop took the corridor and taut reported the outline wall "grazed mid-span by 455 µm" — the vertex-only model
cannot see a wall that is not an edge end). Computed only when `K_a + K_b > 0` (an own end), else 0 (M3 behaviour).
At build: `walls_near[e]` = `obs.query_walls(layer, bbox(e).grow(rho_max))`, `rho_max = obs.query_grow()`, minus walls
with an end anchor equal to `a` or `b` (those are the pad's own sides and the neighbour sides already priced by the end
term) — `O(E · q)`, sorted, static. At search start, for every edge incident to a vertex owned by `n` (≤ Σ deg of the
net's vertices; cached per search in `Vec<f64>`): for each `W ∈ walls_near[e]` with `net(W) ∉ {n} ∪ ties(n)`, the
blocked parameter interval `{t : dist(p(t), core(W)) < r_W + max(c, c_W) + w/2 + ε}` (segment-segment distance, one
quadratic in `t`); the usable sub-segment `[E_a − K_a, L − E_b + K_b]` minus the union of blocked intervals; `W_n(e)` =
usable length − the longest remaining free piece (so a wall splitting the span counts the smaller piece as lost: exact
for one wall, conservative for several). Edge-kind walls use `edge_clearance`, holes `hole_clearance` (`wall_clearance`).

**Numbers (margin ε = 0.005).** Entries are `usable / need` before → after:

| case | pad / edge | M3 cap / need (fit?) | new usable / need (fit?) |
|---|---|---|---|
| #34 VCC U3.1 (SOT-23-5 1.325×0.6 rr, r 0.15), w 0.5, c 0.2 | own tip (L 0.3) | 0.0 / 0.905 (no) | 0.6 / 0.505 (yes) |
| same, long side (L 1.025) | own side | 0.725 / 0.905 (no) | 1.325 / 0.505 (yes) |
| #207 U7.6 (0.625×0.25), w 0.2, c 0.2, sibling #206 (w 0.2) on the edge | own side (L 0.5, r 0.0625) | 0.375 / 0.605 + 0.405 (no) | 0.625 / 0.205 + 0.205 (yes) |
| wiccon #52 FLASH1.1 tip (cap 0.8), w 0.2, same-net #55 present | own tip | 0.8 / 1.11 (no) | 0.8 + 2r / 0.41 (yes) |
| #45 SW1.0 tip, cap 0.600 | own tip | 0.600 / 0.605 (no, by ε) | 0.600 + 2r / 0.205 (yes) |
| `search_layers (1)` 0.6×0.3 rect (r 0), w 0.2, c 0.2 | own tip (L 0.3) | 0.3 / 0.605 (soft) | 0.3 / 0.205 (fits, ghost not soft) |
| U2.28 (THT r 0.8) spoke toward the outline, edge clr 0.5 | own end, outline wall mid-span | fit by accident of the old gap | `W_n` removes the span inside `r_W + 0.5 + w/2 + ε` → unfit |
| 0.5 mm GND pour completion from a 0.25 mm pad (R-iii class) | own tip (L + 2r = 0.25) | no | 0.25 / 0.505 (still no: track wider than the pad; needs neck-down, backlog) |

Own tip rule in words: a track fits its own pad's tip iff `w + ε ≤ pad width`; a spoke to a foreign corner iff
`w + ε ≤ d + r_own − r_f − max(c, c_e)`. A fine-pitch gap of 0.25 with c 0.2 stays unfit for every width (correct).

Tests that flip (A4 `graph_ownnet.rs`): `own_tip_edge_fits_own_net` (row 1), `two_same_net_strands_share_one_tip_edge`
(rows 3/4), `own_end_edge_against_outline_wall_stays_unfit` (row 7), `search_layers (1)` → "fits, ghost not soft"
(row 6); `eprom_ripped_front_only_is_the_m2_result` stays 27/27 ≤ 67.8 mm because the U2.28 corridor stays unfit (the
F6 experiment without the wall term was 26/27; with it the corridor is priced as before). Expected effect: dcsgone —
the pad-exit subset of "blocked: no path" and of the overuse-driven 2-via escapes (#34, #197, #207, #45 class; the
"static dead" set shrinks only where `w + ε ≤ pad width`), `overuse_edges` 192 → well under 50 (own tips were most of
them); eprom two-layer byte-identical (F6 measured identical; the wall term only removes fits), front-only guard
green; wiccon vias 37 → ≤ 33 (F6 measured 33 → 30), outside ≤ 1373 mm.

## 2. Same-net Steiner attachment (A5, M4-D3)

**Goal set.** `Goal` gains `strands: Vec<OwnStrand { conn, leg, layer_idx, edge, slot }>`: every crossing of every
committed leg of a routed connection of the same net (`MultiGraph::conn_wires` of the net's earlier connections,
`O(Σ sibling leg length)` per search), plus kept hand copper: a constraint edge with `owner_nets ∋ n` whose owning wall
is a kept track of the net (`Wall.kind == Track`; pads/zones excluded). A state on edge `e` reaches an `OwnStrand` when
its slot is adjacent to the sibling's slot (`k == slot` or `k == slot + 1`: nothing between them, Rule T holds) and the
junction angle estimate ≥ `steiner_min_angle_deg` (60): angle between the chord into the state point and the sibling's
chord through `e` (its previous → next state point). Hand copper: the state point on the constraint edge is the junction.
Cost of the goal = the leg cost so far (no via); the search picks the cheapest of pad goal / junction goal.

**Commit.** Sibling wire `A` (conn `s`, leg `l`) is split at the junction into `A1: start_A → J` and `A2: J → goal_A`
(two leg ids, `wire_of` updated, slots of `A`'s crossings before / after the junction edge re-attributed), the new
wire `B: start_B → J`; the three share `Terminal::Junction { at: J }` — the same terminal `wires.rs` makes when a hand
item endpoint lies on another item's interior, so the smoother, the self-check and the KiCad writer need no new case
(KiCad connectivity accepts a track end on another track's copper; `unconnected 0` is the test). `J` initial position:
the sibling's current chain point on `e` when it has one (routed in an earlier iteration), else the state point.
`MultiGraph.attached_to[conn] = Some(s)`; ripping or removing `s` cascades to its attached connections (depth ≤ p − 1,
bounded by the net's pad count; cascade rips do not count against `(ripper, blocker)` pairs). `ConnReport.junction:
bool`, `junction_angle_deg`.

**Junction relaxation (bounded 120°).** After metrization, per junction, ≤ `junction_relax_steps` (8) Weiszfeld steps
toward the Fermat point of the three branches' last taut anchors (25 §3; the 120° optimum), each step projected onto
the usable sub-segment of `e` between the junction's slot neighbours (the topology — which slot of which edge — never
changes), accepted iff the three chains re-metrize, self-check clean and the total length decreases by ≥
`relax_min_gain_nm` (1 000); rejected steps revert and end the loop. Same shape and acceptance rule as §5.
Post-check: all three pairwise tangent angles at `J` ≥ 60°, else the junction is rejected: the connection is searched
once more without that `OwnStrand` goal (≤ 1 extra search per connection per iteration), reported `junction_rejected`.
Why 60°: below it the branch forms an acute sliver (22 §3 flags acute corners as topology errors; the fillet length
`r·tan(δ/2)` grows without bound, acid trap for the fab); 120° is the length optimum.

**Ordering and batch mode.** `net_rank(conn)` = index in the net's connections sorted by `est_len` ascending
(`O(p log p)` per net, computed in `decompose`) — identical to today's "shorter first" inside a net, so `order` is
unchanged and current boards keep their order; a connection may attach to any committed sibling (the rank only breaks
ties and documents the star/path intent: short spokes first, long trunks attach to them or vice versa). In batch mode
the parallel phase sees only siblings committed in earlier iterations (frozen graph); in the commit phase a connection
whose sibling was committed earlier in this iteration and whose found path is a detour (`is_detour`) or carries a via
is searched once more with the sibling's strands as goals (bounded: ≤ 1 re-search per connection per iteration, the
sequential re-search path that already exists for invalidated paths). `steiner = false` disables goals, split and
cascade (M3 behaviour, `steiner_off_is_m3_behaviour`).

Effect: #207 5.55 mm / 2 vias → ≈ 0.9 mm T onto #208's trace (hand geometry), 0 vias; dcsgone `count > 1.5` ≤ 6
expected; wiccon vias not up (a junction replaces an escape via at worst); eprom: 2 nets have two connections — a T
may shorten them (not worse, identity not claimed).

## 3. Pad exit stubs (A6)

`Terminal::Stub { pad: Vec2, end: Vec2, layer: u32 }`: a straight track of the net width from the pad centre to
`end = centre + d̂ · (h + c + w/2 + ε)`, `h` = half-extent of the pad hull along its long axis `d̂` (roundrect / rect:
`length/2`; oval: the skeleton segment end + r), `d̂` = the long-axis direction pointing away from the nearest foreign
obstacle (`query_anchors` + `query_walls` in a box of `h + c + w` around the pad; square pads: the end with the larger
free distance). The stub end is a terminal vertex of the CDT with `r = w/2` (as a `Copper` junction), the stub body a
kept-copper wall of the net (so other nets keep clearance from it); the connection's `TermRef.at` = `end`,
`ConnReport.stub_nm` = its length. Generation is a pre-pass on the hand board, `O(T · q)`: `Checker` at margin ε on
`[stub]` against pads, kept copper, outline and keepouts; a violation (w wider than the pad, a neighbour inside the
clearance band, outline) → no stub, warning `stub infeasible: <pad>` (`auto`) and the pad terminal as in M3.
Modes: `auto` — a stub when at least one own tip/spoke edge of the pad is unfit for the net after §1 (fine-pitch rows:
the spokes across a 0.25 gap are always unfit; SOT-23 / THT: none, so eprom is byte-identical); `all` — every in-scope
SMD pad with a long axis; `off` — M3. Pads in `ignore_pads` / `do_not_touch` / outside the board get none. Stubs are
emitted as ordinary routed items (first item of the connection's wire, G1 trivially at the pad) and count in
`length_outside_pads` by their part outside the hull (`c + w/2 + ε` ≈ 0.305 mm per stub at 0.2/0.2).

Effect: #201 U1.13 → U7.4 leaves U7.4 from a point outside the row where every incident edge fits (complements A2's
state-point fix); the "0.5 mm track from a 0.25 mm pad" class stays infeasible (reported, not hidden). Risk: wiccon
outside length has 6.4 mm headroom (1373.55 of 1380); `auto` adds ≤ 0.305 mm per stubbed pad and the route it replaces
was leaving the pad anyway, so the net change is expected ≤ 2 mm — A6 reports the number; if it exceeds the bound the
default stays `auto` and the finding is escalated, never the bound relaxed.

## 4. G1 continuation of hand-started traces (A14)

`Terminal::Dangling { at, direction }` (`direction` = `DanglingEnd.direction`, unit vector of travel at the free end,
already in the model). `dangling_ends = "continue"` (default): the continuation must leave `at` along `d̂` with
`g1_angle_deg ≤ 1`.

- **Topology hint (search).** The ray `at + d̂·s`, `s ∈ (0, STUB_LEN]`, `STUB_LEN = max(2w, 0.5 mm)`, is walked through
  the CDT (`O(k)` crossings); the crossed edges, with the sides of their vertices as the ray leaves them, are a forced
  prefix of the slot path (slots by Rule T as usual); an unfit / impassable prefix edge truncates the prefix there
  (≥ 0 edges). The search root is the state at the prefix end, so the first move is committed to the hand direction.
- **Metrization rails.** Two synthetic point anchors (kind `Rail`, clearance 0, visible only to this wire — the
  per-wire mechanism of `ZeroSlack`) at `at ± n̂ · ρ_rail` with `ρ_rail = w/2 + max(c, R_min)`, `n̂ ⊥ d̂`, inflated
  radius = `ρ_rail` (so both discs are tangent to the axis exactly at `at`; margin 0 against the wire there). A chain
  leaving `at` and turning either way wraps the disc on the inside of the turn and starts with an arc tangent to `d̂`:
  G1 by construction, bend radius `≥ c` at the centreline. Both rails enter the sketch as anchors with their sides
  fixed; the one with slack is dropped by relaxation. The plan text's rails "of radius w/2 + ε at ± n̂(w/2 + c)" are
  these discs with `R_min = c − ε` and clearance 0 — written with the net's clearance they would be over-constrained
  by `w/2 + 2ε`, and as parallel segments with clearance 0 they leave `c − w/2 − 2ε` of lateral play (0.09 mm for a
  0.2/0.2 net: up to 10° over 0.5 mm), hence tangent discs.
- **Verification.** `g1_angle_deg` (existing report field) = angle between `d̂` and the chain's tangent at `at`; > 1°
  → the connection fails metrization with `g1: <angle>` and is negotiated again (history on its edges, `penalise_failed`).
- `"fixed"` = M3 (fixed point, kink allowed); `"drop"` = the dangling item is ripped and the connection routes from the
  pad (schema 0.4.0 offers `continue | drop`, plan A14 `continue | fixed` — recommend all three, ruling needed).

Effect on the gates: none (rip = all removes hand copper on every gate board); fixture `make_fixture_board(...,
dangling_trace=True)` carries the tests; plugin keep-mode boards get the behaviour.

## 5. Via position relaxation, bounded Weiszfeld (A13)

Pass after via reduction and before the final self-check, one sweep over the vias in site-index order, deterministic.
For a via joining legs `l_1` (layer `L_1`) and `l_2` (`L_2`) with last taut anchors `a_1`, `a_2` (the last anchor of
each chain before the via; the pad / junction when the chain has none):

    p_{k+1} = (a_1/d_1 + a_2/d_2) / (1/d_1 + 1/d_2),  d_i = |p_k − a_i|      (Weiszfeld; two anchors: a point of a_1a_2)
    p_{k+1} ← clip into R = star(v, L_1) ∩ star(v, L_2)   (union of the faces incident to the site vertex v, each layer)

≤ `via_relax_steps` (8) steps; each accepted iff `checker.via_fits(p, size)` at margin ε (all layers the via spans,
hole-to-hole, keepouts, `via_in_pad` rule), both legs re-metrize against the current chains of all other wires and
self-check clean, and `len(l_1) + len(l_2)` decreases by ≥ `relax_min_gain_nm` (1 000); a rejected step reverts and
ends the via's loop. Confinement to `R` keeps every slot path valid: the via stays in the faces around its site vertex
(crossing a spoke edge of the star changes the final face but not the homotopy w.r.t. any real obstacle — the loop
closes around the old site, not around a neighbour); the outer link edges are never crossed. Pour-completion vias
(one leg, must stay in the pour) and locked vias are skipped. The site vertex keeps its CDT position (fully free
placement is backlog); only the emitted via and the two legs move. `RouteResult.via_relaxation = "moved k of n vias,
−x µm"` / `"off"` / `"skipped: deadline"`. Monotone: wiccon vias 37 unchanged, outside ≤ before; eprom (0 vias)
identical; threads byte-identical (sequential pass over sorted vias).

## 6. Escalating per-pair rip cost (A16, 46 row 17)

`rip()` adds `hist_block · growth^(k−1)` to the blocker's leg edges and sites for the k-th rip of `B` by `A`
(`rip_pairs[(A, B)] = k`), refuses at `k ≥ max_per_pair` (backstop 8; `RIP_PAIR_CAP` removed; `growth = 1,
max_per_pair = 3` reproduces M3). Cumulative history on `B`'s path after `k` rips vs the other scales:

| k | per rip (0.5 · 2^(k−1)) | cumulative 0.5 · (2^k − 1) | M3 constant 0.5 · k (cap 3) | `hist_fail · k` | edge cost factor `1 + ω·h` (ω = 1) |
|---|---|---|---|---|---|
| 1 | 0.5 | 0.5 | 0.5 | 1 | 1.5 |
| 2 | 1 | 1.5 | 1.0 | 2 | 2.5 |
| 3 | 2 | 3.5 | 1.5 (cap) | 3 | 4.5 |
| 4 | 4 | 7.5 | — | 4 | 8.5 |
| 6 | 16 | 31.5 | — | 6 | 32.5 |
| 8 | 64 | 127.5 | — | 8 | 128.5 (backstop) |

Step cost is `Δ · (1 + ω·hist) · (1 + ω·(present − 1))`, so history `h` on an edge multiplies its length by `1 + h`
(`ω` = 0.5 … 1.25 by tier). A connection leaves a contested class as soon as an alternative costs less than
`(1 + h)×` its old path: a ping-pong pair with a 3× alternative settles at `k = 3` (`h = 3.5`), one with a 10×
alternative at `k = 5`; with the constant M3 cost (1.5 at the cap) the 3× case never settles by price and the cap
decides. The three-way contention of the F.Cu-only eprom board (+5V vs two others on one corridor, settled by 3 ×
0.5 = 1.5 = one `hist_fail` in M3) reaches 1.5 after two rips and 3.5 after three: settles at least as fast; the risk
is overshoot (a party priced out at `h = 3.5` takes a long detour) — bounded by `is_detour` (factor 2 vs the straight
line) which rips the blockers of the ghost and by the stop rule (patience 5, `min_gain_pct` 1); the guard
(27/27, ≤ 67.8 mm) is A16's acceptance. Saturation: once `h > L_alt/L_old − 1` (≈ 10-20 on dcsgone for a short
connection vs a board-crossing alternative) further growth changes nothing; `0.5 · 2^(k−1) ≥ 20` at `k = 7`, so
`max_per_pair = 8` sits just past saturation — consistent defaults. Bound: rips per run ≤ `max_per_pair · m(m−1)` =
`O(max_per_pair · m²)`, each `O(path length · layers)`. Report `RouteResult.ripup: RipReport { rips_per_connection:
Vec<u32>, pairs_at_backstop: Vec<[usize; 2]>, total: u32, max_pair: u32 }`; the ping-pong test asserts `max_pair ≤ 4`
and `pairs_at_backstop` empty.

## 7. Complexity, tests, options, expected effects

| component | cost | cap / stop |
|---|---|---|
| `walls_near` per edge (build) | `O(E · q)` | grid query, static |
| `W_n(e)` per search | `O(Σ_{v: own} deg(v) · k)`, `k = |walls_near|` | own vertices of the net only; cached per search |
| `fits_for` / `overuse` | `O(n_max)` per edge (as `fits_at`) | — |
| `OwnStrand` goal set | `O(Σ sibling leg length)` per search; goal test `O(1)` per state | ≤ 1 re-search per conn per iteration |
| split + cascade | `O(leg length)`; cascade ≤ `p − 1` per net | tree depth |
| junction / via relaxation | `O(J · steps · (metrize(3) + q))`, `O(V_via · steps · (metrize(2) + q))` | `steps ≤ 8`, deadline + cancel per step |
| exit stubs | `O(T · q)` pre-pass | one `Checker` call per candidate |
| dangling prefix | `O(k)` ray walk per connection | `k` ≤ crossings within `STUB_LEN` |
| rip cost | `O(max_per_pair · m²)` rips × `O(path · layers)` | backstop |

Tests per rule (names as in the plan): A4 `graph_ownnet.rs` — `own_tip_edge_fits_own_net`,
`two_same_net_strands_share_one_tip_edge`, `own_end_edge_against_outline_wall_stays_unfit`, `search_layers (1)` flip,
`eprom_ripped_front_only_is_the_m2_result` green, eprom two-layer identity; A5 `route_steiner.rs` —
`t_junction_onto_routed_sibling` (#207 fixture ≤ 1.2 mm, 0 vias, sibling split, self-check clean),
`t_junction_onto_hand_copper`, `steiner_off_is_m3_behaviour`, `junction_angle_below_60_is_rejected`; A6
`route_stubs.rs` — `stub_generated_for_unfit_fine_pitch_pad`, `stub_skipped_when_infeasible`,
`exit_stubs_off_matches_previous`, `exit_stubs_all_on_every_smd_pad`; A13 `route_via_relax.rs` —
`via_moves_toward_force_balance_within_face_star`, `via_relaxation_rejects_move_into_keepout`,
`via_relaxation_off_matches_previous`, `via_relaxation_threads_byte_identical`; A14 `route_g1.rs` —
`dangling_end_continues_tangent` (≤ 1°), `dangling_hint_keeps_side`, `dangling_fixed_mode_is_m3`,
`test_bench_dangling_fixture_g1`; A16 `negotiate_ripup.rs` — `ping_pong_pair_converges_before_backstop`,
`backstop_stops_rips`, `growth_one_is_constant_cost`, `ripup_threads_byte_identical`.

Options (every judgement call; `[expert]` keys map 1:1 to `Options.expert` fields):

| key | default | range / values | cost if wrong |
|---|---|---|---|
| `[search] exit_stubs` | `auto` | `auto, all, off` | `all`: extra outside length on every SMD pad; `off`: fine-pitch pads stay unroutable |
| `[search] steiner` | `true` | bool | `false`: sibling connections take escape vias (#207 class) |
| `[search] dangling_ends` | `continue` | `continue, fixed, drop` (ruling: schema has `continue, drop`) | `fixed`: kinks at hand ends; `drop`: hand stubs lost |
| `[vias] via_relaxation` | `true` | bool | `false`: vias stay at CDT sites (length only) |
| `[ripup] growth` | `2.0` | 1.0 – 10 | 1.0 = M3 constant cost (ping-pong until the backstop); > 4: overshoot detours |
| `[ripup] max_per_pair` | `8` | 1 – 1000 | < 4: contention undecided; large: runtime bound only |
| `[expert] own_net_capacity` | `true` | bool | `false` = M3 capacity (identity experiments only) |
| `[expert] wall_aware_capacity` | `true` | bool | `false` = F6 behaviour: outline-wall corridors accepted, taut fails |
| `[expert] steiner_min_angle_deg` | `60` | 30 – 90 | low: slivers / acid traps; high: fewer junctions |
| `[expert] junction_relax_steps` / `via_relax_steps` | `8` / `8` | 0 – 64 | 0 = no move; large: runtime |
| `[expert] relax_min_gain_um` | `1` | 0 – 1000 | 0: oscillation risk; large: no moves |
| `[expert] dangling_stub_len_mm` (`STUB_LEN = max(2w, this)`) | `0.5` | 0.1 – 5 | short: hint too weak; long: prefix through unfit edges truncated |
| `[expert] g1_max_angle_deg` | `1.0` | 0 – 10 | report / failure threshold of §4 |

Expected effect per board (DX measures; not-worse bounds are the acceptance of each S task):

| rule | dcsgone (231) | eprom (27/27, ≤ 64.7 mm, guard ≤ 67.8 mm) | wiccon (62/62, vias ≤ 37, ≤ 1380 mm) |
|---|---|---|---|
| §1 capacity | pad-exit "no path" + overuse escapes (#34/#45/#197/#207 class); `overuse_edges` 192 → < 50 | two-layer identical (F6 measured), guard green via `W_n` | vias → ≈ 30-33, outside −5 mm (F6) |
| §2 Steiner | detours > 1.5: 12 → ≤ 6; escape vias of sibling connections | 2 two-connection nets may shorten | not worse (junction ≤ via) |
| §3 stubs | #201 / U7 row entries | identical (THT) | ≤ +2 mm outside expected; number reported |
| §4 G1 | none (rip = all) | none | none |
| §5 via relax | length only | identical | vias unchanged, outside ≤ before |
| §6 rip cost | contention settles in ≤ 4 exchanges, ≥ 2 iterations in 600 s | guard re-verified by A16 | iterations ≤ 26 expected |

Not addressed here (other tasks): the 45 timeouts (A1 speed + 600 s budget), hole-gap self-checks (A10 via sizes),
state-point / los (A2), `sketch: inconsistent slot path` (A0/A2), tracks wider than their pads (backlog: neck-down).
