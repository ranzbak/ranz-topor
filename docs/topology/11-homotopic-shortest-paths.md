# 11 — Shortest homotopic paths: funnel, universal cover, curved funnel

**Purpose.** Given a wire's topology (its crossing sequence through the triangulation),
compute the unique shortest path in that homotopy class — first for zero-width wires among
points, then with clearance (disks), which yields the segment + arc chains KiCad can store.

Related: [10](10-triangulation-and-cuts.md), [21 tangent-arc paths](../geometry/21-tangent-arc-paths.md).

---

## 1. Homotopy in one paragraph

Two wires with the same endpoints are **homotopic** if one can be continuously deformed into
the other without passing through an obstacle. The class is fully described by the
*reduced crossing sequence* of triangulation edges. Every class contains exactly one
shortest path (the "rubber band" / RBE, rubber-band equivalent — Leiserson & Maley). Routing
topologically = choosing classes; metrization = computing the shortest representative with
widths and clearances.

## 2. Funnel algorithm (thin wires, point obstacles)

Discovered independently by Tompa (1980/81), Chazelle (1982), Lee & Preparata (1984) and
Leiserson & Maley (1985); Maley's "Algorithm W" is the same idea.

1. Triangulate free space with obstacle vertices as triangle vertices.
2. Walk the input path; record the crossing sequence of triangulation edges.
3. **Reduce**: cancel adjacent repeats (crossing an edge and immediately back). The reduced
   sequence defines a *sleeve* of triangles — possibly overlapping in the plane, but
   topologically a disk (it lives in the universal cover).
4. Run the funnel: keep an apex and two concave chains (left/right). For each new diagonal
   endpoint, append it to its chain, or — if it crosses the opposite chain — pop vertices
   and advance the apex.
5. Output = apex chain + final chain.

Cost O(n log n + nk) for k crossings; needs only orientation tests (exact with integer
coordinates).

## 3. Universal cover and many paths

- **Hershberger & Snoeyink (1994):** universal-cover framework; O(nk) for Euclidean, link
  and polygonal metrics.
- **Efrat, Kobourov & Lubiw (2002/2006):** many disjoint paths among point obstacles;
  shortcut to x-monotone pieces, route bundles. Note: output complexity can be Ω(2ⁿ) —
  taut paths can stack exponentially many parallel segments. In practice: merge collinear
  and concentric runs.
- Homotopy testing: Cabello, Liu, Mantler, Snoeyink (2004), O(n log n).

## 4. Thick wires: obstacles become disks, paths become tangent + arc chains

Inflate each feature by `w/2 + clearance` (+ widths/clearances of wires already wrapped
inside on that side). The shortest path in a fixed class among disks **alternates bitangent
segments and arcs**, each arc turning to the side the wire passes.

Taut rule: every arc must have non-negative sweep on its own side; an arc with negative sweep
means the wire no longer touches that disk → drop it and recompute (the curved analogue of
the funnel "pop"; Salewski's `nubly` pass).

Formulas: [21 tangent-arc paths](../geometry/21-tangent-arc-paths.md). Bitangent offset angles
in the classic form (d = centre distance): outer tangent `asin((r₁ − r₂)/d)`, inner
("twist") tangent `asin((r₁ + r₂)/d)`; inner exists only if `d > r₁ + r₂`, otherwise the
topology is not realisable at those clearances.

## 5. Curved funnel with forbidden regions (Gao et al.; Hama & Etoh)

**Gao, Jerrum, Kaufmann, Mehlhorn, Rülling, Storb (1988)** — continuous homotopic routing in
the **Euclidean** metric (the curved-trace case): build *fan-shaped forbidden regions* (disk
sectors) at visible features, plane-sweep in the universal cover with the funnel method.
O(n³ log n) bound.

**Hama & Etoh (IBM Tokyo, ASP-DAC 1998)** — practical version, closest published match to
TopoR's geometry:

1. Terminals are circles; outline and keepouts are constraint edges of a CDT.
2. Wire topology = sequence of CDT edge crossings.
3. Generate fan-shaped forbidden regions **lazily**, starting from triangles the wire passes,
   moving outward. Region radius = feature radius + Σ (widths + clearances) of wires
   passing between the feature and this wire (= the stack radius of
   [21 §5](../geometry/21-tangent-arc-paths.md#5-many-wires-around-one-obstacle-stacks-and-spokes)).
4. Use the CDT empty-circumcircle property to stop searching when no further feature can
   push the wire. Measured ≈ 3.8 vertices visited and 0.23 regions per triangle — nearly
   linear in wire length.
5. Run a modified Algorithm W (funnel) over the left and right sequences of circular regions;
   backtrack when a new region on one side intersects the other chain. Worst case O(n²).
6. **Spreading/fattening**: spare space on the critical cut between features i, j:
   `SS_ij = |cut_ij| − R_i − R_j − W_ij − C_ij`, split by ratios `D_c + D_w = 1` into extra
   clearance and extra width.

**Decision:** the metrizer implements this local, per-wire curved funnel (lazy regions, stack
radii), iterated to a fixed point for coupled stacks ([25](../geometry/25-relaxation-and-reflow.md)).
Being local, it also supports incremental re-flow.

## 6. Global shortest paths among disks (when topology is not fixed)

Tangent visibility graph (bitangent endpoints + arcs along disks) + Dijkstra; Chen & Wang
(SODA 2011; SIAM J. Comput. 2013) for convex pseudodisks. Rarely needed in a topological
router; useful for initial guesses or local alternatives.

## References

- Lee & Preparata, Networks 14(3), 1984. https://doi.org/10.1002/net.3230140304
- Hershberger & Snoeyink, CGTA 4(2), 1994. https://doi.org/10.1016/0925-7721(94)90010-8
- Efrat, Kobourov, Lubiw, CGTA 35(3), 2006. https://doi.org/10.1016/j.comgeo.2006.03.003 ; https://arxiv.org/abs/cs/0204050
- Gao et al., "On continuous homotopic one layer routing", SoCG 1988; LNCS 333. https://doi.org/10.1007/3-540-50335-8_24
- Hama & Etoh, ASP-DAC 1998. https://www.cecs.uci.edu/~papers/compendium94-03/papers/1998/aspdac98/pdffiles/06c_3.pdf
- Erickson, notes on shortest homotopic paths. https://jeffe.cs.illinois.edu/teaching/comptop/2023/notes/05-shortest-homotopic.html
- Maley, *Single-Layer Wire Routing and Compaction*, MIT Press 1990. https://archive.org/details/singlelayerwirer00fmil
