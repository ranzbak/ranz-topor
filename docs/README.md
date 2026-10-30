# Knowledge library: topological routing with smoothly curved, minimal-length traces

Feasibility study and background knowledge for a TopoR-style router as a KiCad plugin.
No implementation here — algorithms, constraints, risks and sources only.

## Start here

1. [00 — Overview](00-overview.md): what the method is, why it works, the pipeline.
2. [02 — Feasibility study](02-feasibility-study.md): verdict, risks, effort, open questions.
3. [KiCad constraints](kicad/30-kicad-constraints.md): what KiCad can store, read, write and verify.
4. [Rust engine and KiCad integration](kicad/31-rust-engine-and-integration.md): plugin shapes, Rust wheel in the plugin dir (Linux first), crate fit.
5. [Pitfalls and prior attempts](design/42-pitfalls-and-open-problems.md).
6. [43 — Topola assessment](design/43-topola-assessment.md): hands-on test of the closest open-source project (fork / reuse parts / reference only).
7. [44 — Feature gap analysis](design/44-feature-gap-analysis.md): KiCad 10 design features vs. our definition, usage scan of real projects, MoSCoW priorities, gaps to add.
8. [45 — MVP and implementation plan](design/45-mvp-plan.md): milestones M0–M5, tasks per model tier, rotating coordinator.

## Background knowledge

**Topology** (deciding which side of each obstacle a wire passes)
- [10 — Triangulation, routing graph, cut capacity](topology/10-triangulation-and-cuts.md)
- [11 — Shortest homotopic paths, curved funnel](topology/11-homotopic-shortest-paths.md)
- [12 — Sketch generation, ordering, rip-up](topology/12-sketch-generation.md)
- [13 — Layer assignment, via minimisation](topology/13-vias-and-layers.md)

**Geometry** (turning topology into exact segments and arcs)
- [20 — Clearance model, anchor circles](geometry/20-clearance-model.md)
- [21 — Tangent-arc paths, stacks, spokes](geometry/21-tangent-arc-paths.md)
- [22 — Smoothness, bend radius, biarcs, teardrops](geometry/22-smoothness-and-biarcs.md)
- [23 — Exact distance checks, integer-nm robustness](geometry/23-distance-and-drc-checks.md)
- [24 — Length, differential pairs, meanders](geometry/24-length-diffpairs-tuning.md)
- [25 — Relaxation, via placement, re-flow](geometry/25-relaxation-and-reflow.md)

**Benchmarks**
- [Routing benchmarks over time](benchmarks/README.md) (images + metrics per run)

**Reference**
- [01 — Annotated bibliography](01-bibliography.md) (with reading order)
- [Glossary](glossary.md)

Claims not confirmed by a primary source are marked **UNVERIFIED**. Research date 2026-10-30.
