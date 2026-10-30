# 10 — Triangulation, routing graph and cut capacity

**Purpose.** Build the discrete "topological working field" on which wires are routed
without coordinates, and the capacity model that says whether a topology can be realised
with the required widths and clearances.

Related: [11 homotopic paths](11-homotopic-shortest-paths.md), [12 sketch generation](12-sketch-generation.md),
[20 clearance model](../geometry/20-clearance-model.md).

---

## 1. Why a triangulation

Every topological router in the literature — Bazilevich's "discrete topological working
field", SURF, TopoR/FreeStyle Router, gEDA toporouter, Salewski, Topola, Woody — partitions
the board into convex cells (triangles) whose corners are the features (pads, vias,
corners of obstacles). A wire's topology is then the **ordered sequence of triangle edges it
crosses**; each edge records which wires cross it and in what order. Nothing else (no
coordinates) is needed until metrization.

Luzin & Polubasov (2002) define it this way: a method is topological if wire paths are fixed
"only up to topological equivalence"; the model uses only relations — *lies between*,
clockwise order, layer, crossings.

## 2. Which triangulation

**Constrained Delaunay triangulation (CDT)** per copper layer:

- **Vertices:** pad centres for round pads; anchor-circle centres for non-round pads
  (rect/roundrect corners, oval end centres — see [20](../geometry/20-clearance-model.md));
  vias (when present); Steiner/branch points; board-outline and keepout polygon vertices.
- **Constraint edges:** board outline (Edge.Cuts), keepout/rule-area boundaries, the
  internal edges of each pad's anchor hull (so no wire is routed *through* a pad), locked
  tracks of other nets.
- **Long constraint edges** (board edge) are subdivided (gEDA toporouter samples every
  ~100 mil) so triangles near the edge stay well-shaped.

Why Delaunay: maximises minimum angles (no slivers) and has the **empty-circumcircle**
property, which Hama & Etoh use to bound which distant features can influence a wire
(only features within the circumcircle chain matter).

**Per-layer vs shared.** FreeStyle Router used one triangulation shared by all layers;
its authors later concluded layer-specific objects (SMD pads on one side, blind vias)
fit badly and recommended separate models. **Decision: one CDT per copper layer**, with
through-hole pads and through vias present in all of them.

**Numerical robustness.** Use exact (or adaptive) orientation/in-circle predicates
(Shewchuk-style). Integer-nm input coordinates make this easy in Python (exact ints) — see
[23 §4](../geometry/23-distance-and-drc-checks.md#4-quantisation-to-integer-nanometres).

## 3. Routing graph

Two equivalent formulations are used in practice:

| Formulation | State | Used by |
|---|---|---|
| **Edge crossings** — nodes are positions *on* triangulation edges (slots between already-crossing wires) | (edge, slot) | gEDA toporouter, Woody, Hama–Etoh |
| **Vertex regions** — wires incident to a vertex `v` split its neighbourhood into angular *regions*; the route goes from region to region | (vertex, region) or (node, previous node, lane) | Dayan / SURF, Salewski |

In the region formulation, after a net is routed, the regions along its path are **split**
into left/right copies, so later nets cannot cross it on that layer. Pruning rule (SURF):
regions with angle `< π` can never lie on a taut path; regions with angle exactly `π` merge.

Search: Dijkstra / A* / Theta* (Topola uses Theta* on a navmesh). Cost terms: estimated
length, capacity usage, crossings (→ vias), turn penalties, congestion history.

## 4. Cut capacity — the routability condition

A **cut** is a segment between two features `p`, `q`. Wires crossing it need room:

    flow(pq)     = Σ_i (w_i + s_i)        over wires crossing pq (s_i = clearance to next; conservative by one clearance if R already includes the feature's own clearance)
    capacity(pq) = |pq| − R_p − R_q       (R = feature radius incl. its own clearance)
    feasible  ⇔  flow ≤ capacity  for every cut

**Theorem (Cole & Siegel 1984; Leiserson & Maley 1985):** a single-layer sketch is
routable **iff every cut is safe** (flow ≤ capacity). Maley (1996) tests this in
O(N log N) time for polygonal wiring rules; Gao et al. (1988) handle the Euclidean case.

**Practical caveat:** checking only triangulation edges is **necessary but not sufficient**.
A narrow passage between two non-adjacent features (a "disturbance") can be missed.
Mitigations, in order of effort:

1. gEDA toporouter: also check **triangle interior capacity** (vertex to the perpendicular
   foot on the opposite edge).
2. Check all *short* pairwise cuts within a radius `r_max` (spatial index).
3. Kallmann's **Local Clearance Triangulation** (LCT): refine the CDT until local edge checks
   become exact; O(n) triangles in practice.
4. Fallback that always works: post-metrization exact collision check + rip-up
   ([23](../geometry/23-distance-and-drc-checks.md)). **MVP uses this.**

With mixed widths, capacity depends on widths *plus* the pairwise clearances of adjacent
wires, not on wire counts — keep `flow` as a sum of actual values.

## 5. Data structure requirements

- Incremental insertion **and removal** of vertices (vias appear/disappear during rip-up).
  Not all GPL-compatible CDT libraries support removal; otherwise rebuild locally.
- Per edge: ordered list of crossing wires; per vertex: cyclic order of attached wires per side.
- Map back from every vertex to its anchor circle and originating board item.

## References

- Luzin & Polubasov, «Топологическая трассировка: реальность или миф?», Chip News 2002 №5. http://chipnews.ru/html.cgi/arhiv/02_05/9.htm
- Cole & Siegel, "River routing every which way, but loose", FOCS 1984. https://doi.org/10.1109/SFCS.1984.715902
- Leiserson & Maley, "Algorithms for routing and testing routability of planar VLSI layouts", STOC 1985.
- Maley, "Testing homotopic routability under polygonal wiring rules", Algorithmica 15, 1996. https://doi.org/10.1007/BF01942604
- Dai, Kong, Jue, Sato, "Rubber band routing and dynamic data representation", ICCAD 1990.
- Kallmann, "Dynamic and robust local clearance triangulations", ACM TOG 2014. https://mkallmann.github.io/papers/14-tog-lct.pdf
- Hama & Etoh, "Curvilinear detailed routing algorithm and its extension to wire-spreading and wire-fattening", ASP-DAC 1998. https://www.cecs.uci.edu/~papers/compendium94-03/papers/1998/aspdac98/pdffiles/06c_3.pdf
