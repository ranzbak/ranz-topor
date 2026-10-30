# 23 — Exact distance checks, spatial indexing and integer-nm robustness

**Purpose.** Self-check every generated primitive *before* writing it to KiCad, with exact
analytic formulas (no polygonisation), fast neighbour lookup, and a precision policy that
survives KiCad's integer-nanometre rounding and DRC epsilon.

Conventions: see [20](20-clearance-model.md#conventions-used-in-all-geometry-docs).

---

## 1. Distance primitives (centreline to centreline)

Required test for two tracks: `dist(centreline₁, centreline₂) − w₁/2 − w₂/2 ≥ c₁₂ + ε`.

**Point → segment AB**

    t = clamp(((P − A)·(B − A)) / |B − A|², 0, 1)
    dist = |P − (A + t(B − A))|

**Segment → segment**: `0` if they intersect (orientation tests); else the minimum of the
four point→segment distances.

**Point → arc** `(C, R, span)`: if `angle(P − C)` is inside the span, `| |P − C| − R |`;
otherwise min distance to the two arc endpoints. `P = C` → `R`.

*Span test without atan2* (sweep ≤ π, arc from `S` to `E` with orientation `σ`):
`σ·cross(S − C, P − C) ≥ 0  and  σ·cross(P − C, E − C) ≥ 0`.
For sweep > π: negate the test of the complementary arc (endpoints `S`, `E` handled separately).

**Segment → arc**: minimum over
- `0` if the segment meets the circle at a point inside the span (line–circle quadratic,
  keep `t ∈ [0,1]`);
- point→arc for both segment endpoints;
- point→segment for both arc endpoints;
- foot `F` of `C` on the segment (if inside it) with `|F − C| > R`: `Q = C + R(F − C)/|F − C|`;
  if `Q` is in the span, candidate `|F − C| − R`.

**Arc → arc**: minimum over
- `0` if the circles intersect at a point inside both spans;
- four endpoint→other-arc distances;
- interior critical points on the centre line: with `u = (C₂ − C₁)/|C₂ − C₁|`, each pair
  `(C₁ ± R₁u, C₂ ± R₂u)` where both points lie inside their spans;
- concentric arcs: `|R₁ − R₂|` if spans overlap, else endpoint distances.

These candidate sets are complete: the minimum of the distance function is attained either
at a boundary (endpoint) or at an interior critical point, and interior critical points of
circle–circle distance lie on the line through the centres.

**Pads via skeletons** (exact, no polygonisation):

| Pad | Skeleton | Skeleton radius |
|---|---|---|
| circle | centre point | `r` |
| oval | axis segment | `h` |
| round-rect | rect shrunk by `r_c` (polygon) | `r_c` |
| rect / convex polygon | the polygon | `0` |

    dist(track, pad) = dist(track centreline, skeleton) − w/2 − r_skel
    (= negative/0 if the centreline is inside the skeleton polygon)

## 2. KiCad's own checker (for reference)

- `SHAPE_ARC::Collide(SEG, clearance, …)` tests arcs against segments analytically.
- Arc → polyline conversion uses a max error that defaults to `ARC_HIGH_DEF` (5 µm) or the
  board's `m_MaxError`; used for zones and some custom-pad paths.
- Copper-clearance DRC compares against `max(0, clearance − DRCEpsilon)`;
  `DRCEpsilon` defaults to 0.0005 mm (0.5 µm), configurable via `kicad_advanced`.

**Do not spend KiCad's epsilon.** Target `actual ≥ required + ε` with the margin `ε_build` of §4. Near
zones or polygonised custom pads, also add the polygonisation error (≤ `m_MaxError`,
default 5 µm).

## 3. Spatial indexing

**Arc bounding box**: the two endpoints plus each axis extreme `C ± R·eₓ`, `C ± R·e_y` whose
angle lies in the span; grow by `w/2 + max_clearance/2`.

- **Uniform grid hash** — cell size ≈ `max(w) + max(c)`; insert each primitive into every
  cell its grown box overlaps; query 3×3 neighbourhood. Simple and cheap to update:
  best for *moving* wires during relaxation.
- **R-tree** — static, rebuilt after edits:
  best for the large *static* set (pads, keepouts, locked tracks).

## 4. Quantisation to integer nanometres

KiCad internal unit (IU) = **1 nm**, 32-bit signed integers (≈ ±2.147 m range). A track arc
is stored as `(arc (start X Y) (mid X Y) (end X Y) (width W) (layer L) (net N) (uuid …))`
where `mid` lies **on** the arc. KiCad recomputes the centre with `CalcArcCenter(start, mid,
end)` (exact 64/128-bit intermediates, centre rounded to integer nm) and derives the radius
as `|start − centre|`.

**Drift.** Rounding a point moves it by `δ ≤ 0.71 nm`. For an arc with chord `L` and
sagitta `s ≈ L²/(8R)`, perturbing `mid` changes

    ΔR ≈ (R/s) · δ       (centre moves similarly)

Example: `R = 10 mm`, 5° sweep → `s ≈ 9.5 µm`, `R/s ≈ 1050` → `ΔR ≈ 0.5–0.75 µm`.
The drawn copper inside the span stays within ~1.25δ of intended, but anything derived from
the *centre/radius* drifts: concentric spacing in stacks and diff pairs, tangency with
neighbours, and KiCad's `|P − C| − R` distance computations. KiCad's own tests note rounding
"has a relatively larger effect on small arcs".

**What an implementation must therefore do:**

1. Build geometry in floating point with clearance inflated by `ε_build` (5 µm recommended, range 1–10 µm, plus ≥ 5 µm near polygonised zones/custom pads).
2. **Quantise, then verify.** Round start/mid/end to integer nm; recompute centre exactly
   as KiCad does (or read it back via the API); rerun §1 checks on quantised geometry.
3. Very flat arcs (sagitta < ~5 nm or chord < ~10 nm) → emit a **segment** instead.
4. Shared chain endpoints are computed once, rounded once, and reused — bit-identical
   integers, so KiCad connectivity sees them as connected.
5. `mid` = angular midpoint `C + R·(cos(a_in + σΔθ/2), sin(a_in + σΔθ/2))`, never the
   chord midpoint.
6. Exact integer predicates are possible with arbitrary-precision integers:
   orientation and in-circle tests on integer-nm coordinates are exact. Only predicates
   involving square roots (tangent points) need margins; compare squared quantities where
   possible.

## References

- KiCad `SHAPE_ARC`: https://docs.kicad.org/doxygen/classSHAPE__ARC.html
- `CalcArcCenter` (trigo.cpp): https://docs.kicad.org/doxygen/trigo_8cpp_source.html
- shape_arc.cpp: https://docs.kicad.org/doxygen/shape__arc_8cpp_source.html ; tests: https://docs.kicad.org/doxygen/test__shape__arc_8cpp_source.html
- DRC epsilon: https://docs.kicad.org/doxygen/advanced__config_8cpp_source.html ; https://docs.kicad.org/doxygen/drc__test__provider__copper__clearance_8cpp_source.html
- File format: https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/index.html
