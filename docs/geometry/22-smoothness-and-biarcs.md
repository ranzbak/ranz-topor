# 22 — Smoothness: G1 chains, bend radius, fillets, biarcs, teardrops

**Purpose.** Guarantee traces are smoothly curved (no corners), respect a minimum bend radius,
and express any curve using only the primitives KiCad tracks support: straight `segment`
and circular `arc` (start/mid/end).

Conventions: see [20](20-clearance-model.md#conventions-used-in-all-geometry-docs).

---

## 1. G1 continuity comes for free

Taut chains from [21](21-tangent-arc-paths.md) consist of tangent segments joined to arcs
*at their tangent points*, so direction is continuous everywhere (G1). Curvature jumps
between `0` and `1/R` at joins (not G2) — electrically irrelevant at PCB scale.

**Invariant to enforce at export:** consecutive primitives share a bit-identical integer
endpoint, and the arc's tangent at that endpoint equals the segment direction within
`~δ/L` (δ ≈ 0.7 nm rounding, L primitive length). See
[23 §4](23-distance-and-drc-checks.md#4-quantisation-to-integer-nanometres).

## 2. Minimum bend radius

Tightest arc in a taut chain is around a bare small anchor: `R = ρ = w/2 + c` (plus the
anchor's own radius). If a rule demands `R ≥ R_min > ρ` (manufacturing, RF, flex):

Replace anchor `(C, ρ)` with an enclosing *virtual circle* `(C', R_min)`:

    C' = C + (R_min − ρ) · b

where `b` is the unit bisector of the turn pointing to the *inside* of the bend (toward
where the original centre lies relative to the wire, i.e. away from the wire).
`disk(C, ρ) ⊂ disk(C', R_min)` so clearance still holds; the path becomes slightly longer.
Since `b` depends on the tangents, iterate 2–3 times. Heuristic, not optimal (the exact
problem is Dubins-like with obstacles).

## 3. Fillets for unavoidable corners

Corners may still arise at forced pad-entry stubs, T-junctions (Steiner points), or
user-locked segments. Two lines meeting at vertex `V` with deflection `δ` (heading change),
fillet radius `r`:

    distance V → each tangent point = r · tan(δ/2)
    |V − centre| = r / cos(δ/2)        (along the interior bisector)
    arc length    = r · δ
    length saved  = 2r·tan(δ/2) − r·δ

The fillet needs `r·tan(δ/2)` of straight length available on both legs; otherwise reduce
`r`. Acute corners (deflection `δ > 90°`) should never come out of a taut chain — flag them
as topology errors rather than filleting.

**T-junctions** (Steiner points where 3 wires meet): KiCad connectivity is by coincident
endpoints; fillet each pair of branches is not possible for a 3-way junction, so leave the
junction as a point where straight stubs meet (typical branch angles ~120° at Steiner-
optimal points). DRC treats same-net overlap as fine.

## 4. Biarcs: converting arbitrary smooth curves to KiCad arcs

KiCad has no Bezier/spline *tracks* (only `segment` and `arc`). Any other smooth curve
(meander template, user-drawn spline, smoothing around a junction) must become an
**arc spline**. A *biarc* is two arcs sharing a tangent at their junction, matching G1
Hermite data `(P₀, T₀) → (P₁, T₁)` with `T₀, T₁` unit vectors.

**Equal-parameter biarc:**

With `v = P₁ − P₀` and `t = T₀ + T₁`, the common distance `d` from each end point to the
corresponding control point is

    d = ( sqrt((v·t)² + 2(1 − T₀·T₁)(v·v)) − v·t ) / ( 2(1 − T₀·T₁) )      (T₀ ≠ T₁)
    d = (v·v) / (2 · v·t)                                               (T₀ = T₁, parallel)

and the junction point is `J = ((P₀ + dT₀) + (P₁ − dT₁)) / 2`. The parallel case fails when
`v ⟂ t` (an S-curve): split the curve first.

Derivation: require `|(P₀ + dT₀) − (P₁ − dT₁)| = 2d`, giving
`2(T₀·T₁ − 1)d² − 2(v·t)d + v·v = 0`, which has exactly one positive root.

**Arc from start point, start tangent, end point** — with `N = perp(T)`:

    r_signed = |Q − P|² / (2 · N·(Q − P))
    C = P + r_signed · N

If `N·(Q − P) ≈ 0` the arc degenerates — emit a straight segment.

**Error control.** Subdivide recursively until sampled radial deviation from the target
curve is below a tolerance (e.g. 1 µm). Split at inflection points first (a single biarc
fits inflections poorly). For spiral segments (monotone curvature) Walton & Meek give
rigorous bounds. For a smooth curve, error decreases roughly cubically in segment length.

Where biarcs matter in this plugin: meanders, smoothing Steiner junctions, user-defined
curves. The taut routing itself needs none.

## 5. Teardrops

KiCad (7.0 and later) generates teardrops natively for pads, vias and track-width
changes; they are stored as special zone-like filled shapes and handled by DRC and zone
filling. Policy for this plugin:

- End traces exactly at the pad attach point; let the user (or the plugin, through the
  API if exposed) run KiCad's teardrop generation afterwards.
- Leave room for them: the teardrop widens the trace near the pad, so clearance to
  neighbouring pads at the entry is checked on the teardrop shape. A conservative router
  adds a little extra clearance around pad entries, or checks the final board with
  `kicad-cli pcb drc` after teardrops have been generated ([KiCad constraints](../kicad/30-kicad-constraints.md)).

## References

- K. M. Bolton, "Biarc curves", Computer-Aided Design 7(2), 1975.
- D. J. Walton, D. S. Meek, "Approximation of a planar cubic Bézier spiral by circular arcs", J. Comput. Appl. Math. 1996.
- Biarc tutorial with derivations: https://raw.org/math/computer-graphics/approximating-bezier-curves-with-circular-arcs/
- Robust biarc computation: https://arxiv.org/pdf/1711.00935
- KiCad board file format (track primitives): https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/index.html
