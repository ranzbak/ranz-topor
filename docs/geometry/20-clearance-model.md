# 20 — Clearance model: from pads and rules to anchor circles

**Purpose.** Turn every copper obstacle plus the applicable design rule into a uniform
geometric object the router can reason about. Everything downstream (tangent paths, stacks,
DRC checks) works on the result of this step.

Related: [21 tangent-arc paths](21-tangent-arc-paths.md), [23 distance checks](23-distance-and-drc-checks.md),
[KiCad design rules](../kicad/30-kicad-constraints.md).

---

## Conventions used in all geometry docs

- Points are 2-D vectors. `cross(a,b) = a.x*b.y − a.y*b.x`, `perp(a) = (−a.y, a.x)` (rotate +90°).
- `σ ∈ {+1, −1}` is the wrap orientation of a wire around an obstacle: `σ = +1` means
  counter-clockwise in *math* axes, i.e. the obstacle centre is on the wire's **left**.
- **KiCad's Y axis points down.** CCW in math axes appears clockwise on screen. Rule for this
  project: *convert to math axes (flip Y) once on import and once on export; never mix.*
- `w` = track width, `c(a,b)` = required copper clearance between items `a` and `b`
  (may differ per netclass pair / custom rule — see [KiCad constraints](../kicad/30-kicad-constraints.md)).
- Geometry is computed in floating point; KiCad stores integer nanometres
  (KiCad IU). See [23 §Quantisation](23-distance-and-drc-checks.md#4-quantisation-to-integer-nanometres).

---

## 1. Configuration space

Model a trace as its centreline swept by a disk of radius `w/2`. The copper clearance
`c(O, net)` to obstacle `O` holds exactly when the **centreline** stays outside the
Minkowski sum

    O' = O ⊕ D(ρ_O),   ρ_O = w/2 + c(O, net)

This is the classic configuration-space trick: shrink the moving object (the trace) to a
point and grow the obstacles. The shortest wire in a fixed homotopy class is then a
**taut string** in the free space `F = ℝ² \ ∪ O'`.

- Each obstacle gets its *own* inflation radius, because clearance depends on the pair
  (net class of the wire × net class / type of the obstacle).
- Treat `O'` as open and add a build margin `ε_build` (5 µm recommended, range 1–10 µm, plus ≥ 5 µm near polygonised zones/custom pads; see [23 §4](23-distance-and-drc-checks.md#4-quantisation-to-integer-nanometres)) so that touching the
  boundary is feasible but never violates KiCad's DRC after integer rounding.

## 2. Pads become hulls of circles ("anchor circles")

| Shape | Inflated shape `O ⊕ D(ρ)` | Anchor circles |
|---|---|---|
| Circle pad / via, radius `r` | disk | 1 circle, radius `r + ρ` |
| Rectangle | rounded rectangle, corner radius `ρ` | 4 circles at corners, radius `ρ` |
| Round-rect, corner radius `r_c` | (rect shrunk by `r_c`) ⊕ D(`r_c + ρ`) | 4 circles, radius `r_c + ρ` |
| Oval (stadium), axis segment `S`, half-height `h` | `S ⊕ D(h + ρ)` | 2 circles, radius `h + ρ` |
| Convex polygon (trapezoid, chamfered, custom-pad hull) | hull of vertex circles | 1 circle per convex vertex, radius `ρ` |

Two facts make this uniform:

1. A taut string around an obstacle only touches its **convex** vertices — it never enters
   a pocket. So for tangent computations an obstacle may be replaced by its **convex hull**.
   (Concave custom pads still need the full polygon for DRC checks.)
2. For convex `P`, `P ⊕ D(ρ) = hull{ D(v, ρ) : v vertex of P }`, and any line tangent to the
   hull of circles is tangent to one of the circles.

Consequence: **every obstacle is a small set of anchor circles `(centre, radius)`**, and all
path primitives reduce to circle–circle and point–circle tangents. A wire running along a
rectangular pad's side is just the outer common tangent between two corner circles.

## 3. Which obstacles exist per layer

For routing a net `N` on copper layer `L`, the obstacle set is:

- Pads of other nets that have copper on `L` (SMD pads on their layer, PTH pads on all
  copper layers — respect "remove unused pad layers" settings when read from the board).
- Vias of other nets spanning `L` (through vias: all layers; blind/buried: their span).
- Existing (locked / pre-routed) tracks and arcs of other nets on `L` — as capsules
  (segment ⊕ disk) or arc-capsules.
- Copper zones of other nets on `L` (filled polygons). Preferred policy: treat pours as
  *refilled after routing*, and only respect zones that are locked/no-refill
  (see [42 pitfalls](../design/42-pitfalls-and-open-problems.md)).
- Keepouts / rule areas that forbid tracks on `L` — inflated by `w/2` only.
- Board outline (Edge.Cuts) and board cut-outs: inflated by `w/2 + copper_edge_clearance`.
- Holes (PTH and NPTH): constraint `hole_clearance` to the hole wall.

Same-net pads are **not** obstacles for net `N` (the wire connects into them), but they are
obstacles for via placement if via-in-pad is not allowed.

## 4. Pad entry / attach points

A wire must terminate *inside* its pad, not on the inflated boundary:

- Default attach point = pad centre (or offset pad centre for pads with an offset).
- For SMD pads with a narrow axis, constrain the exit direction (leave along the long
  axis or perpendicular to the component edge): model as a short forced straight stub, and
  start the taut chain at the stub end.
- End the track at the attach point and let KiCad generate teardrops afterwards
  (see [22](22-smoothness-and-biarcs.md#5-teardrops)).

## 5. Practical notes

- Rounded-polygon inflation by common polygon-buffer libraries (e.g. GEOS) **under-approximates** arcs (vertices on the
  circle, polygon inside it) — the unsafe direction. Use the analytic anchor-circle model
  for routing and exact checks; see [23](23-distance-and-drc-checks.md).

## References

- Configuration space: T. Lozano-Pérez, "Spatial Planning: A Configuration Space Approach",
  IEEE Trans. Computers, 1983 (general background).
- Shapely buffer approximation (`quad_segs`): https://shapely.readthedocs.io/en/stable/reference/shapely.LineString.html
- KiCad pad shapes and API access: [KiCad constraints](../kicad/30-kicad-constraints.md).
