# 45 — MVP and implementation plan (milestones, tasks, agent orchestration)

**Purpose.** Turn the feasibility study into an executable plan: a milestone ladder that first
routes **eprom-to-kernal** DRC-clean with native arcs and then climbs to harder boards, plus
a token-efficient way to execute it with a rotating coordinator and cheaper worker agents.

Decisions this plan builds on (2026-10-30): own engine, option C ([43](43-topola-assessment.md));
Rust engine as PyO3/maturin abi3 wheel + thin Python plugin, Linux first
([31](../kicad/31-rust-engine-and-integration.md)); KiCad DRC as guide rails
([30 §4.1](../kicad/30-kicad-constraints.md#41-kicad-drc-as-guide-rails-not-only-a-final-gate));
benchmarks per run in [benchmarks/](../benchmarks/README.md). Source boards in
`~/work/kicad/projects` are **read-only**; all work happens on copies.

**Configurability rule (user, 2026-10-31):** when a routing/style/rule choice is a judgement call
(bend angles, acute bends, via sizes, margins, priorities, partial routes, …), make it a **config
setting with a sane default** rather than hard-coding it — "if in doubt, make it configurable".
Defaults are the safe/conventional choice; every non-default value used in a run appears in the report.
Settings live in `topor.toml` in the project directory ([46](46-configuration.md)).

**Usable without AI (user, 2026-10-31):** installation, configuration, operation, contributing and
debugging must work from human-written docs alone — self-documenting `topor.toml` (every key:
meaning, unit, default, range; KiCad-controlled behaviours point to the exact Board Setup place),
user guide, config reference, contributor/debug guide, actionable messages ([46 §1.1](46-configuration.md#11-documentation-requirement-user-2026-10-31)).
Part of the M4 gate, not an afterthought.

---

## 1. Milestone ladder

| M | Goal | Acceptance (hard gate) |
|---|---|---|
| **M0 Harness** | Repo, build script, board reader/writer on copies, geometry kernel, DRC + benchmark tooling | Fuzz: 1000 random arcs/segments placed at clearance + margin next to pads → **0 KiCad DRC violations**; benchmark of the Topola baseline reproduces the images in `benchmarks/2026-10-30-topola-d832b164` |
| **M1 Taut smoother** | Take hand routing's topology, pull it taut into segments + arcs | eprom-to-kernal: 26/26 converted, 0 routing DRC, length ≤ 189.9 mm (hand), widths unchanged (0.4 / 0.25 mm), native arcs only |
| **M2 Single-layer router** | Own topological router on F.Cu from the ripped board | eprom-to-kernal: **26/26, 0 unconnected, 0 DRC** (clearance, edge, width, hole), power nets 0.4 mm, length ≤ hand, < 2 s, deterministic |
| **M3 Two layers + vias** | Layer assignment, via placement (JLCPCB profile, drill list) | eprom still clean; **wiccon-2023** ≥ 90 % routed, 0 DRC |
| **M4 In KiCad** (split, user ruling M4-R2: **M4b** = the plugin, version 0.4.0, first; **M4a** = the engine, version 0.5.0, second) | IPC plugin + wheel install, `topor.toml` config ([46](46-configuration.md)), priority tiers, multi-pin (Steiner), pad exit stubs, keepouts, DRC loop, **preview + Accept/Reject before any change to the board**, progress dialog with Cancel | eprom + wiccon routed from inside KiCad 10 in one undo step; pre-flight report; **private-badge-b N/N routed, 0 router DRC, ≤ 10 min** (user 2026-10-31; snapshot (private pin), locked edge GND kept) |
| **M5 Advanced** | 4 layers, fixed RF/stitching vias, diff pairs, matched groups incl. **swap groups** (DDR byte-lane bit order free, crossing-free bundles), multithreading/variants | **bornhack2026** (scope set after M4) |

Every milestone ends with a benchmark run `benchmarks/<date>-topor-<version>/` (same boards,
same procedure) so progress is visible against hand routing and the Topola baseline.

## 2. Repository layout (target)

```
topor/
  engine/          Rust crate (pure library, no KiCad types) + PyO3 bindings (feature "python")
  plugin/          Python IPC plugin: plugin.json, entry script, requirements.txt, wheels/
  harness/         Python test/bench side: board copy, rip, read/write via pcbnew (headless), DRC, render
  tools/           build.sh (wheel → plugin dir), bench.sh, render.sh
  tests/           pytest (integration), fixtures/ (small synthetic boards)
  docs/            this library; docs/progress/ for the living state (§4)
```

Engine-side tests: `cargo test` + property tests for the geometry kernel. Harness tests: pytest
on synthetic boards; gates use the benchmark boards (copies).

## 3. Task breakdown with model tier

Tiers (cheapest that can do the job reliably):
**H** = Haiku — mechanical, fully specified; **S** = Sonnet — standard implementation with
tests; **O** = Opus — coordination, integration, code review; **F** = Fable — hard algorithm
design, hardest implementations, failure diagnosis, gate reviews.

| ID | Task | Tier | Depends | Done when | Docs to read (only these) |
|---|---|---|---|---|---|
| **M0** | | | | | |
| T0.1 | Repo scaffold: Cargo workspace, PyO3/maturin abi3-py39 crate, pytest, fmt/clippy config | H | — | `cargo test`, `pytest` run green on empty skeleton | 31 §3.2 |
| T0.2 | `tools/build.sh`: release abi3 wheel → `plugin/wheels/`, install plugin dir to `~/.local/share/kicad/10.0/plugins/topor/`; dev mode reinstalls into the venv | S | T0.1 | Plugin appears in KiCad, venv imports engine | 31 §3.1–3.4 |
| T0.3 | Harness reader: copy project, rip tracks (keep stitching/RF/locked), export board model (pads incl. per-layer padstacks, net ties, copper graphics/text, outline incl. arcs/Béziers, NPTH, routable layers) to the engine | S | T0.1 | Model of eprom matches pad/net counts | 30 §2, 20 §2–3, benchmarks README |
| T0.4 | Rule resolution: netclasses + board setup (`.kicad_pro`) + basic `.kicad_dru` + width resolution + JLCPCB profile + local pad/footprint clearance overrides + netclass patterns | S | T0.3 | eprom: power nets resolve to 0.4 mm, edge 0.5 mm | 30 §5–5.2, 44 (Must gaps) |
| T0.5 | Geometry kernel: tangents, bitangents, arcs, exact distances, spatial grid, nm quantisation (TDD, property tests) | S, review F | T0.1 | Property tests from the verified formulas pass | 21, 22 §3–4, 23 |
| T0.6 | Writer: segments/arcs/vias into the board copy with shared integer endpoints | S | T0.3, T0.5 | Round-trip arcs, KiCad recomputes same centre ±1 nm | 23 §4 |
| T0.7 | DRC runner (JSON → compact summary), render + benchmark scripts | H | T0.3 | Topola baseline images reproduced | 30 §4, benchmarks README |
| T0.8 | Arc fuzz gate | S | T0.5–T0.7 | Gate M0 | 23 |
| **M1** | | | | | |
| T1.1 | Obstacle model: pads → anchor circles per layer, edge, NPTH | S | M0 | Unit tests on rect/round/oval pads | 20 |
| T1.2 | Topology extraction from existing copper (CDT, crossing sequences, side per anchor, stack order) | **F** | T1.1 | eprom hand routing → sketch, round-trips | 10, 11, 21 §4–5 |
| T1.3 | Metrization: stacks, taut chain/curved funnel, relaxation | **F** design → S impl | T1.2 | eprom traces taut, self-check clean | 11 §5, 21, 25 |
| T1.4 | Self-check + per-trace fallback + gate run | S | T1.3 | Gate M1 | 23 |
| **M2** | | | | | |
| T2.1 | CDT per layer, routing graph, cut capacity | S, review F | M1 | Capacity tests on synthetic channels | 10 |
| T2.2 | Net decomposition (MST), ordering by width / priority tier | S | T2.1 | Order matches 12 §3.1 | 12 §1–3.1 |
| T2.3 | A* on routing graph + PathFinder negotiation, rip-up | **F** design → S impl | T2.1 | Synthetic planar cases routed | 12 §2–4 |
| T2.4 | Integration with metrization, DRC loop, gate | O | T2.2–2.3 | Gate M2 | 30 §4.1 |
| **M3** | | | | | |
| T3.1 | Layer assignment / via minimisation (2 layers) | **F** design → S impl | M2 | Crossings resolved, few vias | 13 |
| T3.2 | Via placement in continuous space, drill list, CDT update | S | T3.1 | Vias DRC-clean, sizes from list | 13 §4–5, 25 §3, 30 §5.2 |
| T3.3 | Gate on eprom + wiccon | O | T3.2 | Gate M3 | — |
| **M4** | IPC adapter, plugin UI/pre-flight, priority tiers, Steiner, exit stubs, keepouts, DRC loop, **routing scope / partial reroute** (selected **pins/pads** — route only the connections between/at the selected pads, e.g. two pins or all pins of a selected footprint — selected nets, netclasses, area, or unrouted-only; scope comes from the current KiCad selection read through the API, name of the selection call UNVERIFIED; everything outside the scope is kept as fixed obstacles; includes batch re-flow of selected nets after moving parts; **partially routed boards in general**: existing copper of a net — complete or partial, hand-made or from an earlier run — is kept and counts as part of that net's tree, and new routes may attach to any point of it (track ends, vias, existing segments), not only to pads; the user chooses per run whether existing tracks are fixed, may be smoothed, or may be ripped up; **finishing hand-started traces**: a dangling end of a hand-drawn track (endpoint not on a pad, via or other same-net copper) is a route start — the router continues from it on the same layer with the same width, leaves it tangent-continuous (G1) in the hand trace's direction, and treats the hand trace's path as a topology hint (which sides of obstacles it already passed) rather than changing it), **partial routes option** (user toggle, default off: unfinishable connections drawn DRC-clean from both ends up to the blocking point, grouped as `topor-partial` and listed in the report; DRC dangling/unconnected counted separately) | S (UI H) | M3 | Gate M4 | 30, 31, 12 §3, 44 |
| **M5** | Scoped after M4 from [44 — feature gap analysis](44-feature-gap-analysis.md) | F plans | M4 | — | 44 |
| **Backlog (after M4)** | **Traditional 45° routing mode** (user request 2026-10-31): optional per-run switch, default curved; reuses topology/search/negotiation, replaces the arc metrization by octilinear (0°/45°/90°) taut paths in the same homotopy class; bends are 45° turns, a 90° turn only where two 45° turns do not fit (EMI, user 2026-10-31; reported per bend), never acute — both configurable (`bend_90`, `bend_acute`), these are the defaults; same DRC/length gates; plugin option + benchmark column | S (design F) | M4 | — | 44 §4 |
| **Backlog (after M4)** | **Missing-width warning** (user request 2026-10-31): pre-flight check — when no default net width is set (no netclass/rule/settings width, [30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths)), show a warning listing every net that still needs a width and stating that those nets default to **0.25 mm** (default value configurable); the user must click **OK** to accept the default before routing starts (Cancel aborts, nothing changed); accepted fallback widths listed in the run report | S (UI H) | M4 | — | 30 §5.1 |
| **Backlog (after M4)** | **Fully free via placement (incremental CDT)** — when bounded relaxation isn't enough, or as an optional routing mode for complex boards (user 2026-10-31) | S (design F) | M4 | — | 25 §3 |
| **Backlog (after M4)** | **Harness migration to the file/`kicad-cli` path (no pcbnew) before KiCad 11** — the dev/test harness still uses pcbnew SWIG as the oracle | S | M4 | — | 31 §6 |
| **Backlog (after M4)** | **BGA-area strategy: dedicated escape-routing mode inside the footprint area of dense BGAs (ring order, dog-bone / via-in-pad policy, layer budget per ring)** (TopoR-inspired, main session request 2026-11-01; source: TopoR on Wikipedia ([01 §D](../01-bibliography.md#d-topor-eremex))) | S (design F) | M4 | — | 44 §4 |
| **Backlog (after M4)** | **Global inter-layer via minimisation: single-layer solution search before layer assignment (Polubasov 2001); the M3 via-reduction pass stays the local post-pass** (TopoR-inspired, main session request 2026-11-01; source: TopoR on Wikipedia ([01 §D](../01-bibliography.md#d-topor-eremex))) | S (design F) | M4 | — | 44 §4 |
| **Backlog (after M4)** | **Re-optimise tracks after the engineer moves a component or via (re-flow with unchanged topology, rip only broken connections); the router reacts to placement, it never places** (TopoR-inspired, main session request 2026-11-01; source: TopoR on Wikipedia ([01 §D](../01-bibliography.md#d-topor-eremex))) | S (design F) | M4 | — | 44 §4 |
| **Backlog (after M4)** | **Lines-only vs lines + arcs output mode; overlaps the 45° mode row above (the same `[output] geometry` switch, octilinear is its strict variant) and is designed together with it** (TopoR-inspired, main session request 2026-11-01; source: TopoR on Wikipedia ([01 §D](../01-bibliography.md#d-topor-eremex))) | S (design F) | M4 | — | 44 §4 |
| **Backlog (after M4)** | **"Route all, repair later": complete every remaining connection allowing violations, then legalise; optional fallback pass, default off, violations always reported** (TopoR-inspired, main session request 2026-11-01; source: TopoR on Wikipedia ([01 §D](../01-bibliography.md#d-topor-eremex))) | S (design F) | M4 | — | 44 §4 |
| **Backlog (after M4)** | **Teardrop-style smoothing of track-to-pad transitions during routing (tapered curved neck, optional, default off)** (TopoR-inspired, main session request 2026-11-01; source: TopoR on Wikipedia ([01 §D](../01-bibliography.md#d-topor-eremex))) | S (design F) | M4 | — | 44 §4 |

## 4. Orchestration: rotating coordinator, narrow workers

**Roles**
- **Coordinator (O).** Owns one milestone. Splits it into the tasks above, writes worker briefs,
  runs gates, decides. Never reads whole source files or whole docs; works from test output,
  `git diff --stat`, compact DRC/benchmark summaries and worker reports.
- **Workers (H/S).** One task each, a fresh context, isolated git worktree when tasks run in
  parallel. Return a short report and leave the code + tests behind.
- **Expert (F).** Called only for: algorithm design notes (T1.2, T1.3, T2.3, T3.1), diagnosing a
  failed gate, and a review at each milestone gate. Writes its design as a short note in
  `docs/progress/notes/` that S workers implement.

**Living state instead of long contexts.** `docs/progress/STATE.md` is the single source of
truth that lets a coordinator be replaced at any time:
current milestone and task status · decisions made · open issues · last gate results (numbers) ·
file map (where things live) · next 3 actions. Every worker report is folded into STATE.md
by the coordinator in ≤ 5 lines.

**Coordinator rotation.** Hand over (update STATE.md, end) when either a milestone gate is
passed, or the coordinator's context passes ~50 % of its window, or after ~15 worker
round-trips. The successor starts with only: this plan (§1–§4), STATE.md, and the docs
sections listed for the current tasks. Nothing else is carried over.

**Worker brief template (keep ≤ 1 page)**
1. Goal and the task ID.
2. Exact files/directories to create or touch; what not to touch.
3. Acceptance: the command(s) that must pass.
4. Docs to read: section anchors only (from the table above).
5. Constraints: Linux only; projects dir read-only (work on copies); no Topola code; integer-nm
   output; no new dependencies without listing them.
6. Report format: ≤ 200 words — files changed, test/gate results, open issues, nothing else.

## 5. Token-efficiency rules

- **Cheapest tier first**; escalate H → S → F only after a failed attempt or for tasks marked F.
- **Tests and gates instead of reviews**: a green property test or a clean DRC summary replaces
  reading code. Reviews (O/F) only at milestone gates and on F-designed algorithms.
- **Compact tool output**: DRC and benchmark scripts print a ≤ 20-line summary
  (counts by type, routed/unconnected, length, vias, arcs); full JSON stays on disk.
- **Images only at gates**, not per iteration.
- **Stable brief template** (prompt-cache friendly), docs referenced by anchor, never pasted.
- **No duplicate work**: one owner per task; parallel workers only on independent tasks
  (e.g. T0.4 ∥ T0.5 ∥ T0.7), each in its own worktree.
- **Budget guide** (rough, per task): H ≤ 30 k tokens, S ≤ 120 k, F ≤ 200 k; exceeding it means
  the brief was too wide → split the task.
- **Failure handling**: after 2 failed S attempts on the same acceptance, stop and call F for a
  diagnosis note instead of a third blind attempt.

## 5a. Complexity budget (Big-O is a review criterion)

No stage may be exponential or factorial in the number of nets, pads or crossings
(Topola's "next permutation of the whole routing order" is n! — explicitly forbidden).
`n` = obstacles/anchors per layer, `m` = connections, `k` = sleeve length of one wire.

| Stage | Target | Rule |
|---|---|---|
| Obstacle model, rule resolution | O(n) | precompute per (layer, netclass) once |
| Triangulation (CDT) | O(n log n) expected | incremental insert/remove local, never full rebuild per via |
| Clearance queries | O(1) average per query | spatial grid / R-tree mandatory; all-pairs O(n²) checks forbidden outside tests |
| Search per connection (A*) | O(V log V), V ≈ triangulation size | admissible heuristic; bounded expansion count |
| Negotiated rip-up (PathFinder) | O(iterations · m · V log V) | iteration cap (default 50) + history costs; no order permutations |
| Metrization / relaxation | O(k) per wire per iteration | iteration cap + ε-band; stacks updated locally |
| Layer assignment | M3 (2 layers): per-connection, by layered search (SURF step 4: shortest path in the layered graph); planar max-cut deferred to M5 (> 2 layers: heuristic) | exact solvers only on local subproblems with a time limit |
| Variants | N × single run | N = thread count |
| KiCad DRC loop | O(iterations) × seconds | iteration cap (default 5) |

Enforcement:
- **Every loop has an iteration cap and the whole run a wall-clock budget** (user setting);
  on timeout the router returns the best result so far (anytime behaviour) — never hangs.
- **Scaling tests** in CI-style test suite: synthetic boards at 10 / 100 / 1000 connections;
  fail when runtime grows faster than ~m log m × constant between sizes.
- **Benchmarks record runtime** per board; regressions are gate failures.
- Runtime targets (4 threads): eprom-to-kernal < 2 s, wiccon-2023 < 10 s; complex boards
  (reference: **Tanmatsu**, 4 layers ~1820 nets, and **Cyber aegg** = bornhack2026, 4 layers): **first complete-or-best-effort result within minutes, full routing
  + optimisation within ~2 hours is acceptable (slack included)** (user, 2026-10-30; TopoR-like). Long runs are
  anytime: improve (rip-up, variants, via/length optimisation) until the user's wall-clock
  budget, show progress, can be stopped with the best result kept. The polynomial-complexity
  rule still applies — the time is for iterations, not for an exploding algorithm.
- Reviews at gates check the complexity of new loops explicitly.
- **Budgets are hypotheses to be proven, not to be relaxed silently.** If a stage cannot meet its
  complexity target without exponential/factorial search, or a runtime target is missed after
  one diagnosis round (F), the coordinator **stops and escalates to the user** with numbers
  (measured scaling, where time goes, options and their quality cost). No coordinator or worker
  changes a budget on its own.

## 6. Risks for the plan

| Risk | Mitigation |
|---|---|
| Metrization (T1.3) harder than estimated | M1 isolates it on a known-good topology; F owns design; gate on eprom only |
| Brief too vague → expensive worker loops | Acceptance command mandatory in every brief; budget guide forces splitting |
| State drift between coordinators | STATE.md is the only handover; gates record numbers, not opinions |
| SWIG harness removed in KiCad 11 | Harness only; engine is KiCad-free; IPC adapter in M4 |
| Benchmarks not comparable over time | Fixed boards + fixed procedure in benchmarks/README |

## 7. Execution

Execution starts at M0 with a first coordinator. The plan file stays stable; progress goes
to `docs/progress/STATE.md`. Multi-agent runs need explicit approval from the user
(given for this plan on 2026-10-30).
