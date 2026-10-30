# 42 — Pitfalls, lessons from prior attempts, open problems

---

## 1. Lessons from earlier implementations

| Project | What it did | What went wrong / lesson |
|---|---|---|
| **gEDA PCB Toporouter** (A. Blake, GSoC 2008, C) | Dayan-style CDT topological router; output lines + arcs meeting DRC. | Good on contrived problems / many free layers; **poor on few layers with dense constraints** (= typical real boards). Vias disabled in benchmarks. Author: "isn't really ready for users". Source code (`toporouter.c`) is still the most complete open implementation — worth reading. |
| **Salewski Ruby router** (2013–15) | CGAL CDT, Dijkstra on regions, region splitting, tangents/convex hulls. | Self-reported: crashes/invalid routes on some inputs, traces touching or not shortest, DRC violations, blocked paths. Layer assignment never done. |
| **Woody `pcb_topo`** (2026, in progress) | CDT per layer, edge capacities, routes as crossed-edge sequences, priced crossings → negotiated layer assignment, funnel with nested clearances last, KiCad DRC as gate. | Dense fan-in case still fails; main board still routed with Freerouting. |
| **Topola** (M. Wielgus, Rust, MIT/Apache-2.0, 2024–, NLnet) | Interactive rubber-band router + Specctra DSN→SES autorouter; Delaunay/Voronoi navmesh + Theta*; bends are thick concentric arcs, enabling cheap local "squeeze" instead of rip-up; exact line/circle kernel *Cyrk* (quadratic surds). | Active; closest living project — **study first** (https://codeberg.org/mikolaj/topola). Uses DSN/SES, so arcs do not reach KiCad that way. |
| **KiCadRoutingTools** (A. Haas, Python + Rust A*, MIT) | Grid-based octilinear multi-layer router in PCM; diff pairs, length matching, fan-out; `kicad-cli` DRC cross-check. | No arcs. Good source of *topology* for the Phase 1 smoother. |
| **Corner-rounding tools** (KiCad "Fillet Tracks", kicad-round-tracks) | Round corners after routing. | Users report clearance errors near pads — smoothing without a clearance model. Not a substitute for metrization. |

Takeaways: (1) metrization robustness and (2) dense, few-layer pad escape decide success.
Hence metrization-first ([feasibility study](../02-feasibility-study.md)) and a separate fan-out pre-pass later.

## 2. Geometry and metrization pitfalls

1. **Mixed widths/rules make stacks coupled.** A wire's radius at anchor A depends on which
   wires are inside it at A; that set can differ at B. Metrization is a coupled system, not
   independent funnels → iterate ([25](../geometry/25-relaxation-and-reflow.md)).
2. **Order inversions** between consecutive anchors: the tangent model has no solution;
   needs a local reorder or an extra bend. Detect and report rather than emitting overlaps.
3. **Arcs sweeping through regions the sleeve never crossed** (wrap around a pad corner
   reaching into a neighbouring triangle) → always run the post-metrization collision check.
4. **Non-circular pads**: approximating a rect pad by its circumscribed disc wastes 30–50%
   of the space in fine-pitch areas (estimate) — use corner anchor circles ([20](../geometry/20-clearance-model.md)).
5. **Grazing pad exits**: taut wires leave SMD pads at shallow angles → acid traps / near
   misses; KiCad DRC does not flag acute angles. Use exit stubs.
6. **Degenerate arcs**: negligible sagitta (≲ 5 nm) → emit a segment; nearly collinear
   integer start/mid/end make KiCad's centre reconstruction ill-conditioned.
7. **Shared endpoints** must be bit-identical integers, or connectivity breaks.

## 3. Topology pitfalls

1. **CDT edge capacity is necessary but not sufficient.** Narrow passages caused by nearby
   constraints ("disturbances") can be missed. Fixes: Kallmann's Local Clearance
   Triangulation, or checking all short pairwise cuts (Leiserson–Maley cut condition).
   MVP: rely on the post-metrization check + rip-up.
2. **Net ordering dependence** in sequential routing → negotiated congestion (PathFinder).
3. **Rip-up oscillation** → history costs + per-net reroute counters (TopoR uses a counter).
4. **Vias change the triangulation** on every spanned layer → need incremental insert/remove
   (not every GPL-compatible CDT library supports removal; else rebuild locally).
5. **Multi-layer via minimisation** is NP-hard beyond the 2-layer/degree-≤3 case → heuristics.

## 4. KiCad-specific pitfalls

1. **Arc DRC history**: segment-approximated arc collisions caused false errors at exact
   min clearance (issue #8234); arc-to-arc angle-overlap rounding fixed Oct 2024; an arc
   diff-pair distance report issue in 9.0.1 (#20636). → margin of 5–10 µm, DRC as gate.
2. **Zones**: if routed copper changes, zone fills are stale — refill before DRC.
3. **Specctra DSN/SES** (Freerouting interchange) does not carry KiCad arcs faithfully —
   do not round-trip curved boards through it (details: [KiCad constraints](../kicad/30-kicad-constraints.md)).
4. **API transition**: SWIG deprecated (removal planned for 11); IPC API in 9/10 requires a
   running PCB editor and cannot export/plot → DRC via `kicad-cli` on a saved file.
5. **Y axis down** in KiCad; keep one conversion point.
6. **Locked items** and user-routed nets must be preserved.

## 5. Numerical tolerances (orders of magnitude)

- Floating-point geometry, integer-nm storage; exact predicates possible on nm coordinates.
- Clearance margin ≈ 5 µm (range 1–10 µm, [23 §4](../geometry/23-distance-and-drc-checks.md#4-quantisation-to-integer-nanometres)). Extra ≥ 5 µm near zones/custom pads
  (polygonisation error).
- Tolerances: slack/penetration ε-band 0.1 µm in relaxation; degenerate-arc thresholds above.

## 6. Open questions

- Exact TopoR metrization method — not public (Dayan's thesis and the Luzin/Lyachek/
  Polubasov TopoR book are the best leads).
- Proven-complete capacity test that stays cheap on PCB-scale inputs.
- Best way to handle T-junctions inside the stack model.
- Whether KiCad exposes teardrop generation and effective-clearance queries through the
  IPC API in 10.x/11 (see [KiCad constraints](../kicad/30-kicad-constraints.md)).
