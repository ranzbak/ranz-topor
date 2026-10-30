# 21 — Tangent-arc paths, wire stacks and spokes

**Purpose.** Given a wire's homotopy class (which side it passes each obstacle, see
[11](../topology/11-homotopic-shortest-paths.md)) compute its minimum-length geometry:
an alternating chain of straight tangent segments and circular arcs. Then extend to many
wires sharing obstacles (nested arcs).

Conventions: see [20 §Conventions](20-clearance-model.md#conventions-used-in-all-geometry-docs).

---

## 1. Point-to-circle tangent

Point `P`, circle `(C, R)`, `d = |P − C| > R`, `û = (P − C)/d`, `α = arccos(R/d)`:

    T± = C + R · rot(û, ±α)
    rot(v, φ) = (v.x cos φ − v.y sin φ,  v.x sin φ + v.y cos φ)
    tangent length = sqrt(d² − R²)

To leave `P` and wrap the circle with orientation `σ`, pick the `T` with
`σ · cross(T − P, C − T) > 0` (centre lies on the σ-side of the travel direction).

If `d ≤ R` the point is inside the inflated obstacle: the topology or the start point is
invalid (e.g. a pad attach point too close to a foreign pad) — report, don't route.

## 2. The four bitangents of two circles (signed-radius form)

Circles `(C₁, R₁)`, `(C₂, R₂)`, `d = |C₂ − C₁|`, `v = (C₂ − C₁)/d`.
For sign choices `s₁, s₂ ∈ {+1, −1}` let `r₁ = s₁R₁`, `r₂ = s₂R₂`.
Seek a unit normal `n` and offset `c` with `n·C₁ + c = r₁`, `n·C₂ + c = r₂`:

    k = (r₂ − r₁)/d                    # requires |k| ≤ 1
    n = k·v + h·sqrt(1 − k²)·perp(v),   h ∈ {+1, −1}
    T₁ = C₁ − r₁·n,   T₂ = C₂ − r₂·n   # tangent points

- **Outer** tangents: `s₁ = s₂`, exist if `d ≥ |R₁ − R₂|`.
- **Inner** (crossing) tangents: `s₁ = −s₂`, exist if `d ≥ R₁ + R₂`.
- `(s₁,s₂,h)` and `(−s₁,−s₂,−h)` give the same line → at most 4 distinct lines.

**Choosing the one you need.** A wire leaving circle 1 with orientation `σ₁` and arriving at
circle 2 with `σ₂` uses the candidate whose direction `t = (T₂ − T₁)/|T₂ − T₁|` satisfies

    σ₁ · cross(t, C₁ − T₁) > 0   and   σ₂ · cross(t, C₂ − T₂) > 0

Equivalently: outer tangent when `σ₁ = σ₂`, inner tangent when `σ₁ ≠ σ₂`.
If the needed inner tangent does not exist (`d < R₁ + R₂`) the two inflated obstacles
overlap: the wire cannot pass between them — the topology is infeasible (capacity
violation, see [10](../topology/10-triangulation-and-cuts.md)).

A point is a circle with `R = 0`, so §1 is a special case of §2.

## 3. Arc between consecutive tangent points

On circle `(C, R, σ)`, from entry `T_in` to exit `T_out`:

    a_in  = atan2(T_in  − C)
    a_out = atan2(T_out − C)
    Δθ    = (σ · (a_out − a_in)) mod 2π        # in [0, 2π)
    length = R · Δθ
    mid   = C + R · (cos(a_in + σΔθ/2), sin(a_in + σΔθ/2))   # for KiCad start/mid/end

`Δθ` near `0` means the anchor is barely touched — candidate for removal (slack).
`Δθ > π` is legitimate (a wire doubling back around a pin) — do **not** use `Δθ` alone as
a slack test.

## 4. Pulling a wire taut along its sleeve

The topology gives a sequence of candidate anchors `(circle, σ)`: for each triangulation
edge `(u, v)` crossed with `u` on the left and `v` on the right, candidates are
`(u, σ=+1)` and `(v, σ=−1)`. The exact method is the **funnel algorithm** generalised to
circles ([11](../topology/11-homotopic-shortest-paths.md)). A simple, robust fallback:

1. Form the chain *start → candidate anchors (in sleeve order) → goal*; start and goal are
   circles of radius 0 (pad attach points).
2. For each interior anchor `a` between neighbours `p` and `q`, compute the tangent `p → q`
   directly (§2, using the orientations of `p` and `q`).
3. Anchor `a` is **slack** — and is removed — if that shortcut keeps `a`'s centre on side
   `σ_a`, stays at least `R_a` from it, and the region swept between the old path and the
   shortcut contains no other anchor core (otherwise removing `a` would jump over an
   obstacle, i.e. change the homotopy class).
4. Repeat until no anchor is removed; the remaining chain is the taut wire.

Complexity: O(k²) worst case for `k` candidates (fine for PCB-scale sleeves); the funnel is
O(k). Use the fallback first, the funnel when profiling demands it.

## 5. Many wires around one obstacle: stacks and spokes

**Key fact.** In a taut configuration every arc of every wire is centred on an *anchor*
(pad centre / hull vertex, via, Steiner point). A wire can only curve where its obstacle
boundary curves; another wire's boundary is that wire's offset curve, whose arcs are
concentric with the same anchors. By induction all curvature centres are anchors. This is
the rubber-band sketch model of SURF / Dayan (see [01](../01-bibliography.md)).

On each side of an anchor `A` the topology gives an **ordered stack** of wires (k = 0
innermost). The centreline radius of wire `k`:

    R_k = max( r_A + c(A,k) + w_k/2 ,
               max_{j<k} [ R_j + w_j/2 + c(j,k) + w_k/2 ] )

With uniform rules this is `R_k = R_{k−1} + w_{k−1}/2 + c + w_k/2`. The `max` form matters
when clearances differ (e.g. a high-voltage net in the stack). Same-net pair → `c = 0`.
For polygonal pads, the stack is offsets of the rounded hull: each corner arc is
concentric with its hull vertex.

SURF calls the radial lines from an anchor along which wires are spaced **spokes**; the
stack radii above are exactly the spoke positions.

**Order comes from topology, not geometry.** The sketch records the crossing order of wires
on every triangulation edge; the stack order at `A` is the cyclic order around `A` on that
side. Freezing this order is what makes relaxation converge (see
[25](25-relaxation-and-reflow.md)).

### Straight runs between stacks are exactly parallel

- Wires `k, k'` both from stack `A` to stack `B`, **same sides** (outer tangents): both radii
  grow by the same `Δ`, so `k = (r₂ − r₁)/d` is unchanged → parallel lines `Δ` apart.
- **Opposite sides** (inner tangents): in a channel crossed by `m` wires, the `i`-th from `A`
  is `(m−1−i)`-th from `B`, so `r₂ − r₁` is constant → again parallel, spacing `Δ`.

So clearance along straight runs is automatic once stacks are built consistently.

### Wire wrapping wire

When wire `k` bends around wire `j`, it bends around `j`'s arc centred at anchor `A`, so
`k`'s arc is concentric at `A` with the stack radius. A wire pressed against another wire's
straight part simply runs parallel and does not bend there.

**Stack membership.** Conservative rule: count every wire the sketch places between `A`
and `k`. After relaxation, recompute counting only wires actually taut at `A` (or within
reach), iterate to a fixed point.

## 6. Global (non-homotopic) shortest paths — when needed

For an initial route or to evaluate alternatives, a **tangent visibility graph** over all
anchor circles works: nodes = tangent points, edges = unblocked common tangents plus arcs
between consecutive tangent points on the same circle; then Dijkstra. Size O(n²) edges,
naive blocking test O(n³). Chen & Wang give O(n log n + k) for convex pseudodisks.
In a topological router this is mostly used *locally* (e.g. inside one region); the main
loop works in the fixed-homotopy setting above.

## References

- Hershberger & Snoeyink, "Computing minimum length paths of a given homotopy class", 1994. https://link.springer.com/chapter/10.1007/BFb0028273
- J. Erickson, lecture notes on shortest homotopic paths: https://jeffe.cs.illinois.edu/teaching/comptop/2023/notes/05-shortest-homotopic.html
- D. Z. Chen, H. Wang, "Computing Shortest Paths amid Convex Pseudodisks", SODA 2011 / SIAM J. Comput. 2013.
- Dai, Dayan, Staepelaere, "Topological routing in SURF: generating a rubber-band sketch", DAC 1991. https://dl.acm.org/doi/10.1145/127601.127622
- T. Dayan, "Rubber-band based topological router", PhD thesis, UC Santa Cruz, 1997.
