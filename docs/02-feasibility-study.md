# 02 — Feasibility study: a TopoR-style curved router as a KiCad plugin

**Question.** Can a KiCad plugin perform topological routing with smoothly curved,
minimal-length traces, respect KiCad's constraints, and produce a working (DRC-clean,
manufacturable) PCB?

**Short answer.** **Yes, with limits.** The algorithms are published and well understood,
and KiCad can store, edit and verify the result. The open risk is engineering, not theory:
turning topology into exact geometry (*metrization*) robustly on dense real boards — the
step on which every open-source attempt so far has stalled. Expect a credible result for
2-layer boards of moderate density; parity with TopoR is not realistic.

Research date 2026-10-30. Effort figures are rough estimates (UNVERIFIED).

---

## 1. Is the method known? — Yes

| Ingredient | Status | Where |
|---|---|---|
| Topology on a triangulation, cut-capacity routability | Proven theory since 1984–85 (Cole–Siegel, Leiserson–Maley, Maley) | [10](topology/10-triangulation-and-cuts.md) |
| Shortest wire in a homotopy class | Classic, funnel algorithm; universal cover (Hershberger–Snoeyink) | [11](topology/11-homotopic-shortest-paths.md) |
| Curved geometry with clearances | Published: Gao et al. 1988; **Hama & Etoh 1998** (practical, free PDF) | [11 §5](topology/11-homotopic-shortest-paths.md#5-curved-funnel-with-forbidden-regions-gao-et-al-hama--etoh), [21](geometry/21-tangent-arc-paths.md) |
| Complete router system | SURF (UCSC 1990–97), Dayan thesis 1997 | [01](01-bibliography.md) |
| TopoR's own strategy | Partly published (Polubasov 2010; Luzin et al.) — search, rip-up, layer separation, Pareto variants. **Metrization details not public.** | [12](topology/12-sketch-generation.md), [13](topology/13-vias-and-layers.md) |
| Via minimisation | Exact for 2 layers (planar max-cut), NP-hard beyond → heuristics | [13](topology/13-vias-and-layers.md) |

No missing theory. No patents on TopoR's method were found (Google Patents / web search; Rospatent not searched — UNVERIFIED).

## 2. Can KiCad support it? — Yes

Details: [KiCad constraints](kicad/30-kicad-constraints.md).

- **Storage:** segments + circular arcs (start/mid/end, integer nm) are exactly what the
  method produces. ✔
- **Read/write:** IPC API reads all geometry and netclasses, writes tracks/arcs/vias in one
  undoable commit, refills zones. ✔
- **Verification:** `kicad-cli pcb drc` with JSON output maps violations to items. ✔
- **Limits:**
  - ✘ No access to KiCad's rule engine (KiCad 10) → clearances approximated conservatively
    from netclasses / parsed rule files; KiCad DRC is the final judge.
  - ✘ No live interaction → TopoR's "flexible" real-time re-shaping becomes a batch re-flow.
  - ⚠ Arc DRC needs a 5–10 µm margin (verified tolerances + recent arc-DRC bugs).
  - ⚠ A user `track_angle` rule rejects arcs — detect and warn.
  - ⚠ Freerouting/Specctra cannot carry arcs — no hybrid via DSN/SES.
  - ⚠ Dependencies only from wheels → a native accelerator must ship prebuilt wheels.

## 3. Can the output be a working PCB? — Yes, if verified

"DRC-clean by construction" is only partly achievable (rules can't be evaluated exactly), but
"DRC-clean after verify-and-repair" is: route at clearance + margin, check exactly against
own geometry ([23](geometry/23-distance-and-drc-checks.md)), then KiCad DRC, rip up offending
connections. Manufacturability concerns specific to curved routing:

- tangent contact at pad corners and grazing pad exits (acid traps) → exit stubs;
- minimum bend radius for flex/RF → virtual enlarged anchors ([22 §2](geometry/22-smoothness-and-biarcs.md#2-minimum-bend-radius));
- some fabs prefer segmented arcs — KiCad's Gerber export handles native arcs.

## 4. What has been tried — and how far it got

| Attempt | Outcome |
|---|---|
| gEDA PCB toporouter (2008) | Good with many free layers; **poor with few layers and dense constraints**; never production-ready. |
| Salewski Ruby/Nim router (2013–) | Crashes, touching traces, DRC violations; author estimates ~3000 h more for usefulness. |
| Woody `pcb_topo` (2026) | 1–2 layers + vias + multi-pin working; dense fan-in still fails. |
| Topola (Rust, 2024–) | Active, interactive + autorouter, thick-arc bends; closest living project; exchanges via DSN/SES (no arcs into KiCad that way). **Tested hands-on in [43](design/43-topola-assessment.md):** routes a small single-layer THT board DRC-clean in 0.1 s, but is single-layer in practice, has no rule model, and is mid-rewrite — verdict: own engine, Topola as reference. |
| TopoR (commercial, ~35 years) | The reference; full method, metrization unpublished. |

Pattern: graph-search half works; **metrization robustness** and **dense, few-layer escape**
decide success. Details: [42](design/42-pitfalls-and-open-problems.md).

## 5. Risk assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Metrization fails on non-circular pads / mixed widths (stack coupling, order inversions) | High | High | Corner anchor circles; iterate stacks to fixed point; exact post-check; per-connection fallback. |
| Cut capacity on triangle edges misses narrow passages | Medium | Medium | Post-check + rip-up; later Local Clearance Triangulation. |
| Dense fine-pitch (BGA/QFN) escape | High | High | Exclude initially; separate fan-out pre-pass. |
| KiCad DRC disagrees (rule semantics, arc tolerances) | Medium | Medium | 5–10 µm margin; DRC loop; conservative rule approximation. |
| Performance in pure Python on 300+ nets | High | Medium | Engine in Rust from the start, shipped as a PyO3 wheel inside the plugin dir ([31](kicad/31-rust-engine-and-integration.md)); removes the risk at ~1–2 weeks of build infrastructure. |
| KiCad API churn (SWIG removal, IPC growth) | Certain | Low | Keep router independent of KiCad; thin adapters. |
| Exact TopoR behaviour unknowable | Certain | Low | Not required; Hama–Etoh + SURF suffice. |
| Multi-pin nets: Steiner points / T-junctions in the stack model | High | Medium | MST pad-to-pad first; T-junctions later (open problem). |
| Deliberately lengthened/spaced traces get "tightened" (length-matched buses) | Medium | High | Per-net/netclass opt-out; respect `length`/`skew` rules. |
| Hand-editing curved boards afterwards: KiCad's router re-lays touched traces in its own style (UNVERIFIED) | Medium | Medium | Batch re-flow command; document workflow. |
| Incremental triangulation with via insert/remove under GPL-compatible, wheel-only constraints | Medium | Medium | Local rebuilds; affects router phase, not smoother. |
| Zones refilled after routing: pour fragmentation / pour-to-trace effects not modelled | Medium | Low | Review pours after refill; optional pour-aware cost later. |
| Unrouted-connection derivation (no ratsnest query, UNVERIFIED) | Low | Medium | Derive from nets + copper connectivity. |

## 6. Effort and expected quality (estimates, UNVERIFIED)

- **"Taut-arc smoother"** — re-shape an *already routed* board (manual / Freerouting /
  other router) into curved minimal-length copper, keeping its topology: the hardest stage
  alone, useful by itself, ~3–6 person-months.
- **2-layer topological router**, ~100 nets, DRC-clean: ~8–15 person-months total.
- **Real-board grade** (4+ layers, BGA escape, diff pairs, length matching): 1–2+ person-years.
- Basis: feasibility estimates, widened against prior art (gEDA toporouter: one GSoC, "not ready"; Salewski: years of work, ~3000 h more estimated). UNVERIFIED.
- Expected quality vs Freerouting: lower completion (≈85–95% vs ≈99%; UNVERIFIED estimate) but shorter traces and
  fewer vias where it completes. vs TopoR: "TopoR-looking 2-layer boards", not parity.

### Implementation language

The engine (obstacle model, CDT, topological search, metrization, exact checks) is best
written in **Rust** and delivered as a PyO3/maturin abi3 wheel that a build script places in
the KiCad plugin directory; a thin Python plugin (kicad-python) handles the IPC conversation,
commits and the `kicad-cli` DRC loop. KiCad's wheel-only installer picks the local wheel up
through a `--find-links` line in `requirements.txt`. Rust buys memory safety, GIL-free
parallelism and ~10× or more on geometry loops, but not floating-point robustness or
algorithmic completeness, and panics must be mapped to errors. Linux first; other platforms
are a wheel-building matter. Details, alternatives (Rust `exec` plugin, file CLI) and crate
survey: [31 — Rust engine and KiCad integration](kicad/31-rust-engine-and-integration.md).

## 7. Conclusion and recommendation

Feasible. Recommended to proceed only in stages, starting with the **taut-arc smoother**,
because it de-risks the critical metrization step on real boards before any investment in
the topological search. Go/no-go criterion for continuing: on several real 2-layer boards the
smoother converts ≥ 90% of traces with zero KiCad DRC violations **on boards with mixed track
widths, non-circular (rect/roundrect/oval) pads and netclass-specific clearances** — the
conditions the risk table ranks highest. (Shorter length is nearly automatic and not a useful gate.)

## 8. Open questions for the decision

1. Target boards: is fine-pitch BGA/QFN escape required? (Largest risk driver.)
2. Is a 2-layer-first scope acceptable?
3. Acceptable runtime — resolved for Linux by the Rust engine ([31](kicad/31-rust-engine-and-integration.md)); other platforms need wheel builds.
4. Obtain Dayan's thesis (ProQuest) and the TopoR books — the only detailed sources on
   metrization.
5. Study Topola before starting: possibly contribute an arc-preserving KiCad bridge instead of
   building from scratch. — **Resolved** in [43 — Topola assessment](design/43-topola-assessment.md):
   own engine (option C), Topola's ideas reused, no fork.
6. Which KiCad 10 design features the router must respect, read, produce or preserve, and
   what the definition still lacks: [44 — Feature gap analysis](design/44-feature-gap-analysis.md).

**Next step:** implementation per [45 — MVP and implementation plan](design/45-mvp-plan.md).
