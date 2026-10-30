# M1 Task 4 — funnel + sketch design (F, 2026-10-30)

Scope: `engine/src/sketch.rs` per task-4-brief Interfaces (unchanged except Deviations below). All coordinates nm `f64`,
math frame (Y already flipped). Formulas: 11 §2 (funnel), 21 §5 (stack order from topology), 10 §2 (CDT content).

## 1. `build_layer_cdt(obs, terminals)` — what enters the CDT
- Points, in this order (index order = insertion order, deterministic): `obs.anchors[i].c` for all i (pads, via copper +
  hole, hole/slot anchors, outline vertices, shape corners, kept-track endpoints, `Terminal` obstacles), then `terminals`
  (every wire's `start.at()` / `goal.at()` on this layer, bit-identical to `Wire.hand[0]` / `hand[last]`), then 4 frame
  corners: bbox(all points) grown by `FRAME_MARGIN = max(10 mm, max(width, height))`. Empty point set → `Err`.
- Constraints: `(w.ends[0], w.ends[1])` for every wall with `kind == Edge` (arc cores use the chord; no subdivision in M1).
  Pad-hull edges, kept tracks and shapes are NOT constraints (crossing them is a DRC-dirty hand; Task 5 / self-check
  catch it). `Cdt::build` dedups coincident points and `Err`s on crossing constraints → whole layer fails → every wire
  of that layer falls back with reason `"cdt: ..."`.
- `point_to_anchor` is indexed by canonical CDT vertex id (`Crossing.left/right` space), length `num_vertices`; first
  writer wins (via copper beats via hole), terminals and frame corners are `None`. `LayerSketch` also keeps a private
  `vertex_of: HashMap<(u64,u64) bits of Vec2, usize>` (exact position → canonical vertex) for the checks in §2.
- Why the frame: outside the convex hull a hand path crosses nothing and sketches as a straight line (tests 6/7 need it).

## 2. `sketch_wire(wire, ls)` — hand polyline → crossings → reduce → funnel → `AnchorRef`s
1. `hand` with consecutive duplicates removed; `hand[0]`, `hand[last]` must be CDT vertices (`vertex_of`), distinct,
   else `Err`. Any interior vertex equal to a CDT vertex (`vertex_of` hit) → `Err("hand vertex on CDT vertex")` (Task 3
   rule; it is either DRC-dirty or passes through its own pad corner — fallback is correct).
2. Two-point hand whose endpoints are CDT-adjacent (`is_edge`) → crossings `[]` (the hand IS the edge; Task 3 would
   return `EdgeOverlap`). This is the common straight track between neighbouring same-net pads.
3. `cdt.crossings(&hand)`; on `Err` whose message is "lies on a triangulation edge" / "overlaps" retry at most twice with
   all interior vertices translated by `(+1, +2)` nm, then `(-2, +1)` nm (a line cannot be parallel to both; after step 1
   a 2 nm move cannot cross a CDT vertex, so the class is unchanged). Any other `Err` or a third failure → wire fallback.
4. Any reduced crossing whose two vertices are both frame corners → `Err("hand leaves frame")`.
5. `crossings = Cdt::reduce(raw)`; portals = `(vertex_pos(c.left), vertex_pos(c.right))`; `funnel(start, goal, &portals)`.
6. Map each `(i, is_left, σ)` to vertex `is_left ? crossings[i].left : crossings[i].right`, then `point_to_anchor` →
   `AnchorRef{anchor, sigma}`; `None` (frame/terminal vertices) are dropped. Exempt anchors (same net, net tie) are NOT
   filtered here (`LayerSketch` has no nets); Task 5 drops them (`exempt_anchor_is_never_kept`). `Sketch.crossings` =
   reduced crossings (used by `stack_order`, kept for diagnostics).

## 3. `funnel` (Lee–Preparata, 11 §2 step 4), O(k): each portal vertex is pushed once and popped at most once
Portal convention: `(left, right)` as seen in the direction of travel (= `Crossing.left/right`). `cross(o,a,b)` =
`(a-o)×(b-o)`. σ rule: apex advances along the LEFT chain ⇒ the wire turns left ⇒ obstacle on its left ⇒ `Ccw`;
right chain ⇒ `Cw` (matches `point_circle_tangent`: `sign(σ)·cross(t−p, c−t) > 0`).
```text
apex = start; L = R = []            // chains of (pt, portal i, is_left) excluding apex; prev(X) = X[-2] or apex
for i in 0..=k:  (l, r) = portals[i] or (goal, goal) when i == k         // goal is processed as both sides
  add_right(r, i): if r == R.last.pt: skip
     while R≠[] && cross(prev(R), R.last, r) >= 0: R.pop()                  // tighten (r is left of / on last edge)
     if R == []: while L≠[] && L.first is not goal && cross(apex, L.first, r) >= 0:
                     apex = L.first; out.push((L.first.i, true, Ccw)); L.pop_front()     // funnel collapsed
     R.push(r)
  add_left(l, i): mirror image: pop while cross(prev(L), L.last, l) <= 0; if L == []: pop R.front while
     R.first is not goal && cross(apex, R.first, l) <= 0 emitting (i, false, Cw); L.push(l)
guard: a step counter ≤ 8·(k+1) + 8 → Err/`debug_assert` (cannot trigger; keeps "every loop capped")
```
Non-strict comparisons keep a vertex that lies exactly on the taut line in `seq` with a definite σ (Task 5's add rule
would otherwise see `cross = 0`). Brief test values check out for tests 1–3; test 4 is wrong as written (Deviations).

## 4. `stack_order(sketches, ls)` — order from crossings on shared edges (21 §5), never geometry
- Candidate: wire W is a candidate of `(A, σ)` iff some reduced crossing of W has `A == left` (σ = `Ccw`) or `A == right`
  (σ = `Cw`). Candidates (not only `seq` members) are listed: Task 5's add rule may add `(A, σ)` to a wire whose
  funnel missed it (inflation), and Task 6 then needs its position. Key = position of the FIRST such crossing measured
  from A along that edge: `t` for σ = `Ccw` (`t` is the fraction left→right), `1 − t` for `Cw`; sort ascending (inner
  first), ties by `wire` id. O(Σk + Σ m log m).
- Known weakness: two wires entering A's fan on different edges have keys on different edges, which are not comparable;
  a wrong order yields crossing chains → self-check fails → that wire falls back (bounded by the wire count). If the
  gate shows such fallbacks, refine to: key edge = fan edge crossed by the most candidates (ties: lowest neighbour id).
- The `HashMap` is only ever indexed, never iterated for anything order-dependent (determinism).

## 5. What Task 5 (tauten, swept-region/centre-triangle test) relies on from the sketch
`seq` is in path order with the correct σ per anchor; between consecutive `seq` anchors the funnel line is the shortest
path in the class among CDT vertices, so any penetration Task 5 finds is due to inflation (ρ > 0), never a missed
vertex → its `σ = sign(cross(t, C − P))` is well defined and its centre-triangle check only has to look at anchors
within ρ of the chain. `stack_order` gives the relative order; membership is Task 6's (arc with sweep > 0).

## 6. Determinism, complexity, caps, errors
Insertion in index order, no randomness, no HashMap iteration in outputs → two runs bit-identical. CDT O(n log n)
expected; per wire O(k) for crossings/reduce/funnel (k ≤ `MAX_SLEEVE` = 2000), ≤ 3 crossing attempts. Every `Err` of
`sketch_wire` is a per-wire fallback (hand kept, reason string); `build_layer_cdt` `Err` is a per-layer fallback.

## Deviations from the brief (tests only; interfaces unchanged)
- D1 `funnel_two_bends_zigzag`: with portals `((4,1),(4,−5)), ((8,5),(8,−1))` the line y = 0 lies inside both portals →
  `[]`. Use `((4,−1.0),(4,−5)), ((8,5),(8,1.0))` → `[(0,true,Ccw),(1,false,Cw)]` (verified by hand with §3).
- D2 `sketch_wire_around_one_pad` and `stack_order_inner_first`: pad at (5, 0.5) with the hand below y = 0 is in the same
  point-obstacle class as the straight segment (pad on the left of both) → pure funnel gives `seq == []`; the inflated
  pad is only found by Task 5's add rule (`penetrating_obstacle_is_added`). Put the pad at **(5, −0.5) mm** in these two
  tests (hand y = −1 / −2 passes below it → `Ccw`; wire 0 nearer → `[0, 1]`). `sketch_wire_hand_on_other_side_gives_cw`
  keeps (5, 0.5) with hand y = 2 → `Cw`.
- Clarification: `stack_order` doc says "distance from the anchor"; implement as `t` / `1 − t` (fraction), see §4.
