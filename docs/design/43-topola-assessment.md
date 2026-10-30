# 43 — Topola assessment: fork it, mine it, or only read it?

**Purpose.** Hands-on evaluation of Topola (Mikołaj Wielgus et al., Rust, MIT) as a basis for
this project's curved-trace router, with measured results on three of our own 2-layer boards,
a comparison with commercial TopoR, and a recommendation between (A) fork Topola and add a
KiCad bridge, (B) reuse parts with our own engine, (C) own engine, Topola as reference only.
Evaluation date 2026-10-30, Topola `develop` at commit `d832b164` (2026-07-02), built with
Rust 1.96, KiCad 10.0.8 (`kicad-cli`, SWIG `pcbnew`). UNVERIFIED where noted.

---

## 1. Short answer

**Recommendation: C, with a narrow B.** Build our own engine; take from Topola only *ideas*
(thick concentric bends as the band model, per-step "squeeze" instead of rip-up, the
navmesh-with-two-senses-per-obstacle idea) and, at most, the `specctra-core` DSN parser as a
test-fixture loader. Do not fork. Reasons, each backed by a measurement or source reading below:

1. Topola's output cannot reach KiCad as arcs: bends are real arcs internally but the SES
   writer flattens every bend into 100 chords (§4.2, §5). A fork would need a new KiCad bridge
   anyway, and the internal model (f64 coordinates, 1 µm hard-coded epsilon, no integer-nm
   quantisation) is not what [23](../geometry/23-distance-and-drc-checks.md) asks for.
2. The autorouter is single-layer in practice: the ratsnest only connects pads that exist on the
   chosen principal layer, the other copper layer stays empty, the multilayer mode (fan-out vias
   + planar) exited silently on our boards and panicked after 7 min on Topola's own multilayer
   example (§5.3). On the medium board it routed 5 connections on F.Cu and, on B.Cu, looped
   through order permutations for 18 min without producing a result.
3. Keepouts are ignored: KiCad exports NPTH mounting holes as `(keepout …)` and Topola's parser
   reads but never uses them, so traces were drawn straight through the mounting holes (§5.3).
4. One global track width per command, one clearance per netclass, no diff pairs, no length
   rules, planes ignored, pad-outline artefacts leak into the SES.
5. The project is being re-architected: routing work moved to a new `anyangle` crate (GitHub,
   April 2026–), `dearcut` was removed, the exact kernel `cyrk` is a separate WIP repo not used by
   Topola, and the Codeberg `develop` branch has had no commit since July 2026. Forking now
   means forking a codebase its own authors are replacing.
6. Licence is fine (MIT; one file MIT/Apache-2.0; assets MIT/CC-BY-4.0), so nothing legal
   argues against A/B — the arguments are technical and about maturity.

What Topola does well and we should learn from: on a board that fits its model (THT-only,
single layer, uniform width) it routes 26/26 nets in 0.1 s with **zero copper clearance
violations** in KiCad DRC at netclass clearance, shorter total copper than the hand layout and
visibly TopoR-like curved traces (§5.1).

## 2. What Topola is (source reading)

| Aspect | Finding |
|---|---|
| Size, activity | 28.5 kLOC Rust in the workspace (`src/` ≈ 20 k, `planar-incr-embed` ≈ 2.5 k, `specctra-core` ≈ 1.5 k, `topola-egui` ≈ 3.5 k); 1 365 commits, 944 by M. Wielgus, ~240 by E. Zscheile; 36 open issues; no tagged release ever (`release_counter` 0); `Cargo.lock` not committed (issue #57); NLnet NGI0 funded (Entrust 2024–25, Commons 2026 "Topola-autoplacer"). |
| Crates and licences | `topola` (lib + CLI `topola-cli` + GUI `topola-egui`), `planar-incr-embed`, `specctra-core`, `specctra_derive`: all MIT. `crates/planar-incr-embed/src/math.rs` MIT or Apache-2.0. Depends on a Topola fork of `petgraph` (MIT/Apache-2.0) via git URL and on `contracts-try` (own fork, needs nightly only when contracts are enabled). Sister repos on Codeberg: `cyrk` (exact lineocircular kernel, WIP, last update 2025-08), `dearcut` (incremental triangulation, removed from Topola again), `topola-rules`, `planar-brute-embed`. |
| Geometry model | Primitives are *dots* (circles), *segs* (thick straight segments) and *bends* (thick circular arcs stored as inner circle + from/to + width). A routed *band* is an alternating chain seg–bend–seg ("canes"). Bends around the same obstacle are nested concentric arcs with an offset per wrapping band, so a new band can be *squeezed* under or wrapped around existing ones without re-routing them (`wrap_around_bands`, `squeeze_through_under_bends` options). This is the TopoR idea of flexible metrization realised incrementally. |
| Numerics | All coordinates `f64` in DSN units (KiCad exports µm); no integer nanometre representation (issue #47 "should be integers"); collision test inflates shapes by clearance − **1.0 unit (1 µm) epsilon** (`drawing/query.rs`, author comment "epsilon is probably too large"); the exact kernel Cyrk is only referenced in a comment. Delaunay via `spade` ("indeterministic", issue #46; CDT wanted, issue #120). |
| Search | Per connection: build a *navmesh* from a Delaunay triangulation of the dots/bends on the layer, each obstacle giving a clockwise and a counter-clockwise navnode, then Theta*-like search (`router/thetastar.rs`, adapted from petgraph's A*) that **draws the band while searching** — each leap tries to draw a cane; a drawing collision prunes the edge. There is no separate topological sketch phase and no rip-up: a failed connection returns an error. The newer `planar-incr-embed` backend ("TopoAutoroute" command) builds a Voronoi-dual navmesh and keeps a homotopy-class embedding, but is only reachable from the GUI/command file and marked WIP. |
| Net ordering / priority | Connections are the MST ratlines of each net on the principal layer. Order: strongly connected components of the ratsnest sorted by `intersector_count_weight × (ratlines it crosses) + length_weight × length` (`PresortBy::RatlineIntersectionCountAndLength`; `PairwiseDetours` is declared but not implemented). **No user priority, no per-net cost weights, no width-first ordering, no control over which band takes the inner position** — inner/outer is decided by routing order (earlier bands are inside). When a connection fails or the time budget (`initial` + `progress_bonus`) runs out, `permutate: true` tries the *next permutation* of the ratline order and routes everything again: on our easy board a single unroutable connection produced 855 permutations in 5 min without completing (§5.4). A command file may contain several `Autoroute` commands with different pin selections, which is the only way to impose an order or a width per group. |
| Multi-layer, vias | Only *through* vias; `MultilayerAutoroute` = an "anterouter" that places one fan-out via next to each pad whose layer is not the principal layer (bounding-box search in a compass direction, `fanout_clearance`), then runs the planar router on the principal layer. No layer assignment, no via minimisation, no blind/buried/micro vias. Vias cannot be placed mid-route. |
| Clearance model | One `width` and the first `(clearance N)` of each DSN netclass; pairwise clearance = max of the two nets' class clearances, same-layer only; `(clearance N (type smd_smd))` etc. ignored; no board-level edge clearance, hole-to-hole, or custom rules. |
| Pad shapes | DSN `circle` → dot; `rect`/`polygon`/`path` → fixed polygon made of 1 µm-wide fixed segs along the outline plus "fillet circles" at the corners, grouped as a *poly* whose centre is the ratline target. Rotated rectangles and rounded SMD pads have open issues (#249, #293). Those 1 µm outline segs are **written to the SES as wires** (§5.2). |
| Diff pairs, length matching, keepouts, planes | None. `keepout`, `plane`, `place_boundary` are parsed by `specctra-core` and not used by the router (issue #246 for the boundary). Zones therefore do not exist for Topola; KiCad's refill after import makes the pour yield to the new tracks. |
| DRC / self-check | Only the incremental collision test during drawing (above). No final verification pass, no report. |
| I/O | Input: Specctra DSN (KiCad export) — parser is derive-generated and strict; it rejected our boards' `(via … (net GND)(type route))` wiring entries until `(type route)` was stripped (issue #224 family: "many tokens generated by KiCad unsupported"). Output: SES with `(path layer width x y …)` polylines only — no `qarc`, every bend sampled into 100 chords; via padstack hard-coded `__Via` 500 µm with a `TODO: use correct sizes`. The CLI replays a JSON "history" of commands (`Autoroute`, `MultilayerAutoroute`, `TopoAutoroute`, `PlaceVia`, `RemoveBands`, `MeasureLength`); without `-c` it runs `Autoroute` on all pins of layer 0 with a **hard-coded `routed_band_width: 100`** (0.1 mm, ignoring the DSN width) and no squeezing. |
| Robustness | 216 `unwrap/expect/panic` sites in library code; command errors inside `Invoker::replay` are **silently discarded** (the CLI exits 0 with a partial SES); issue #193 "`insert_cane`/`remove_cane` panic quite often" is Priority/Critical and open; #278 CDT constraint-intersection failure; #210 "routing can break existing bends". We observed one panic (`navmesh.rs:440 no entry found for key`) and one silent early exit. |
| Tests | 7 integration tests (`cargo test`) on 17 small example boards (THT adapters, LED matrices, a buck converter), 20 unit tests, 1 proptest, 64 debug contracts. No DRC-based acceptance test. |
| Embeddability | Yes in principle: `topola` is a library crate; `SpecctraDesign::load → make_board → Autorouter → Invoker::execute(Command)` is the whole API, and the `Board`/`Layout`/`Drawing` graph can be walked to extract segs, bends (true arcs: centre, radius, from, to, width) and vias. Everything is keyed to `specctra-core::SpecctraMesadata` (net/layer names, class rules) through the `AccessMesadata`/`AccessRules` traits, so a KiCad-native front end would implement those two traits. No Python bindings, no C API, no stable semver. |

## 3. Topola vs TopoR vs our requirements

Sources for TopoR: [01 §D](../01-bibliography.md), [12](../topology/12-sketch-generation.md),
[13](../topology/13-vias-and-layers.md). Topola column from §2 and §5. Requirements column from
[02](../02-feasibility-study.md) and [30](../kicad/30-kicad-constraints.md).

| Requirement | TopoR (Eremex, commercial) | Topola (this evaluation) | Verdict for reuse |
|---|---|---|---|
| Curved arcs into KiCad | Native arcs, exported to several EDA formats | True arcs internally; SES export flattens to 100 chords; no KiCad writer | A fork needs a new exporter; the arc data is there |
| Topology-first (sketch, then metrization) | Yes: topological sketch on quasi-triangulation, then metrization; rip-up at the topological level | No sketch phase: geometry is drawn during the search; no rip-up, only order permutation | Different architecture from ours ([12 §6](../topology/12-sketch-generation.md#6-recommended-plan-for-this-plugin)) |
| Net ordering / priority (see [12 §3.1](../topology/12-sketch-generation.md#31-net-priority-signal-importance)) | Wide-first then short-first; fluctuating per-conductor cost coefficients; reroute counters; Pareto variants | SCC score = crossings × w₁ + length × w₂, no user tiers, no per-net weights, inner/outer position = routing order; order per group only through separate commands | Must be written by us |
| Multi-layer, vias | Full: layer separation exact for 2 layers, heuristic beyond, via lifting | Through-via fan-out next to pads only; planar routing on one layer; crashed/stalled on our boards | Not reusable |
| KiCad rule compliance | n/a (own DRC) | Netclass width + first clearance only; no edge/hole/custom rules; 1 µm epsilon *reduces* clearance | We need the model in [20](../geometry/20-clearance-model.md) regardless |
| DRC-clean result | Vendor claims yes | 0 copper-copper violations on the board it could route; edge-clearance violations because the rule is unknown to it | Promising evidence that thick-arc stacks can be DRC-clean |
| Pad shapes | All | Circle exact; rect/polygon as outline + corner circles (good idea = our corner anchors); rotated/rounded SMD issues open | Idea reusable, code not |
| Diff pairs, length matching | Yes (TopoR 6+) | No | Ours ([24](../geometry/24-length-diffpairs-tuning.md)) |
| Keepouts, planes/zones | Yes | Parsed, ignored | Ours |
| Robustness | Mature (35 years) | Panics, silent failures, non-deterministic triangulation | Not production grade |
| Speed | Seconds–minutes for whole boards | 0.1–0.7 s for what it routes; minutes to never when permutation kicks in | Fast core, unbounded fallback |
| Embeddability | Closed | Library crate, two traits to implement, f64 µm units | Feasible but the model would need replacing |
| Licence | Proprietary | MIT (+Apache-2.0 for one file, CC-BY-4.0 assets) | Compatible with a GPL-compatible plugin |
| Maintenance / bus factor | Company | Essentially one author (69 % of commits) plus one contributor; funding-driven; mid-rewrite (`anyangle`, 83 commits Apr–Oct 2026, 57 by Zscheile) | High risk for a fork |

## 4. Method of the measurement

### 4.1 Boards

All from `/home/paul/work/kicad/projects` (read-only; copies in the scratch work dir), chosen
2-layer, fully hand-routed, without RF nets, increasing in size:

| Board | Why | Layers | Footprints / pads / nets | Hand routing (all nets) |
|---|---|---|---|---|
| `eprom-to-kernal` (KiCad 5 file) | Smallest routed board: THT only, no vias, no zones — Topola's home turf | 2 | 5 / 56 / 26 | 80 segments, 0 arcs, 0 vias, 189.9 mm |
| `Wiccon/wiccon-main-badge/2023` | Medium: SMD on the back, THT connectors, one GND zone, 36 vias | 2 | 45 / 124 / 38 | 381 segments, 36 vias, 1 832 mm |
| `badge.team/dcsgonebadge-0.8.5` — **robustness data only** | Largest 2-layer board with arcs: 157 footprints, both sides, 203 vias, GND pour but no plane strategy; the user does not consider its hand layout a reference, so it is used only for failure analysis, not for quality comparison | 2 | 157 / 458 / 86 | (not used as comparison) |

A fourth, advanced board (`badge.team/bornhack2026`, 4 layers, RF paths, keepout areas, many
stitching vias) was deliberately **not** tested: the simple boards already fail on
single-layer scope, keepouts and width handling, so the complex one would add nothing.

### 4.2 Pipeline

Per board, on copies: (1) record hand-routing metrics and per-net hand track widths; (2) rip
all tracks, arcs and vias of all nets, keeping free (stitching) vias not touching any track,
locked items and RF nets (none of the three boards had locked or RF copper; 0 / 11 / 7
free GND vias kept); (3) `pcbnew.ExportSpecctraDSN`; (4) strip `(type route)` from the
kept vias' wiring entries so Topola's parser accepts the file; (5) run `topola` release
build with a generated command file selecting all pins (width from the DSN rule, squeeze
on, `permutate` on, Topola's own timeout defaults); (6) `pcbnew.ImportSpecctraSES` into
a copy, save; (7) `kicad-cli pcb drc --format json --refill-zones --all-track-errors
--severity-all`; (8) a second DRC after deleting the 1 µm-wide artefact tracks Topola
emits for pad outlines. Three small diagnostic `eprintln!` patches were applied to the
local clone (CLI: print swallowed command errors; planar stepper: print per-ratline
outcome; reconfigurator: print permutation events); they change no behaviour. Openable
project copies: `…/topola-eval/boards/<board>-topola-routed/` and `<board>-ripped/`.

## 5. Measured results

### 5.1 Summary table

| Board / mode | Connections routed | Topola time | Result copper (routed nets) | KiCad DRC on result (after refill) | Hand routing, same nets |
|---|---|---|---|---|---|
| eprom, default CLI (width hard-coded 0.1 mm) | 26/26 nets, 0 unconnected | 0.1 s | 44 segs + 14 arcs (→ 1 460 KiCad segments), 0 vias, 191.4 mm | 199 `track_width` (0.1 < 0.2 min), 199 `copper_edge_clearance`, 0 `clearance` | 80 segs, 0 vias, 189.9 mm |
| eprom, planar F.Cu, width 0.25 from DSN | 26/26, 0 unconnected | 0.1 s | 44 segs + 14 arcs, 0 vias, 192.2 mm (SES polyline length 165.8 mm + pads) | **0 `clearance`**, 199 `copper_edge_clearance` (board edge rule 0.5 mm; hand layout: 21), 16 `track_width` from 1 µm pad-outline artefacts (0 after removing them); other 71 pre-existing silk/courtyard/library | 189.9 mm, 0 vias; +5V and GND hand-routed at 0.4 mm, Topola 0.25 mm |
| eprom, widths per group (0.4 mm power first, then 0.25 mm) | not completed: 2nd command looped through 4 035 order permutations, connections 18–20 of 24 `NotFound` every time | killed after 300 s | — | — | — |
| Wiccon 2023, multilayer mode | silent early exit (`Invoker::replay` drops the error), 2 + 1 connections | 0.0 s | 6 segs + 1 arc, 0 vias | 54 unconnected | 36 vias, 1 832 mm |
| Wiccon 2023, planar F.Cu | 5 connections (only pads present on F.Cu form ratlines; 36 of 45 parts are on B.Cu) | 0.3 s | 12 segs + 2 arcs | 54 unconnected | — |
| Wiccon 2023, planar B.Cu (where 36 of 45 parts are) | no result: 13 permutations, each failing on one connection (`NotFound`), 48 partial routes undone | killed after 1 105 s | none (no SES written) | — | — |
| dcsgonebadge (robustness only), multilayer mode | silent early exit, 27 connections, B.Cu empty | 0.7 s | 48 segs + 13 arcs, 0 vias, 137 mm; F.Cu traces drawn through both NPTH mounting holes | 198 unconnected, `hole_clearance` 2 (VCC track through NPTH H2 and H4, actual 0 mm), `clearance` 413 → 4 after removing artefacts (all pad-to-pad, present in the original) | — |
| dcsgonebadge (robustness only), planar F.Cu | 27 connections (F.Cu-only pads), identical result | 0.7 s | same | same | — |
| dcsgonebadge (robustness only), planar B.Cu | no result within 10 min (permutation loop), killed | > 600 s | none | — | — |
| Topola's own `tht_de9_to_tht_de9` example, default | 9 nets | 0.1 s | 37 segs + 28 arcs | not run | — |
| Topola's own `3x4_tactswitch_ir_remote` multilayer example with its shipped command file | **panic** `navmesh.rs:440 no entry found for key` | after 7 min 5 s | none | — | — |

Conclusions are drawn from eprom-to-kernal and Wiccon 2023; dcsgonebadge rows are robustness
data only (see §4.1). KiCad DRC counts are totals over all severities; non-copper categories (silkscreen,
courtyards, library mismatches, text size, starved thermals) are identical before and after
and are attributable to the original design.

### 5.2 Observations on the output geometry

- Arcs survive internally (14 bends on the eprom board, 28 on the DE-9 example) but reach KiCad
  as 100-chord polylines: 1 400 of 1 460 imported segments are shorter than 50 µm. KiCad's
  "arcs from polylines" does not re-fit them; a real bridge must read the bends from the
  `Layout`, not the SES.
- Every rectangle/polygon pad contributes its outline as 1 µm-wide wires in the SES (16 on
  eprom, 1 016 on Wiccon, 5 984 on dcsgonebadge). KiCad imports them as 0.001 mm tracks
  → `track_width` violations and, on dcsgonebadge, 409 spurious `clearance` reports; nothing
  functional, but the importer side would have to filter them. On Wiccon the imported board
  therefore shows 1 122 "segments" of which only 6 + one 100-chord arc are routes.
- NPTH mounting holes and rule areas are not obstacles (dcsgonebadge: traces through both
  M3 holes; §5.3) — a disqualifying defect for use on real boards, independent of board size.
- With correct width and clearance the routed copper had **no copper-to-copper violations**
  at KiCad's exact DRC — despite the 1 µm epsilon — on a THT board with 0.2 mm clearance.
  This is the single most encouraging data point for thick-arc metrization.
- Total copper length of the routed nets was 1 % longer than the hand layout on eprom
  (192.2 vs 189.9 mm, including pad-outline artefacts; the SES polylines alone sum to 166 mm).
- Track width per net: Topola uses one width per command. The eprom hand layout has +5V/GND at
  0.4 mm; Topola routed them at the 0.25 mm netclass width (DRC silent, functionally wrong).
  Routing the wide group first in a separate command made the remaining 0.25 mm nets
  unroutable for Topola and triggered the permutation loop. KiCad's DSN export carries only
  netclass widths, so per-net hand widths are not available to any DSN router without a
  temporary netclass; Topola could take them only as separate commands.

### 5.3 Where it fails and why

| Failure | Where | Cause (source reading) |
|---|---|---|
| DSN parse error `ExpectedEndOfList` at the kept GND vias | `specctra-core` `Via` struct (name, x, y, net) | No `type` field; KiCad writes `(type route)` after `(net …)`. Workaround: text strip. |
| Multilayer mode exits in 0.0 s with a partial SES, exit code 0 | `Invoker::replay` ignores `execute()`'s `Result`; `main.rs` does not check | Error swallowed; with the diagnostic patch the planar runs show the stepper simply has 5 (Wiccon F.Cu) or 27 (dcsgone) ratlines because the ratsnest only links pads on the principal layer; the multilayer fan-out did not add anything usable. |
| Multilayer example panic after 7 min | `router/navmesh.rs:440` `no entry found for key` | Navmesh assumes prenavnodes are fixed dots ("ugly shortcut" comment at line 400); a fan-out via/bend node is missing from the map. Matches issues #193/#210. |
| Permutation loop | `PlanarAutorouteReconfigurator::reconfigure` | Any failure or timeout advances to the *next permutation* of the whole ratline order and reroutes from scratch; n! candidates, no memory of which connection failed. |
| Width ignored by default CLI | `topola-cli/main.rs` | `routed_band_width: 100.0` literal. |
| **Traces through NPTH mounting holes** (dcsgonebadge H2, H4; KiCad `hole_clearance` 0 mm) | DSN: KiCad exports each NPTH pad as `(keepout "" (circle F.Cu 3400))` / `(circle B.Cu …)` inside the footprint image and rule areas as `(keepout "" (polygon signal …))` in `structure` — 10 keepouts in that file. `specctra-core` parses `keepout`, `place_keepout`, `via_keepout`, `wire_keepout` into `Keepouts`; `src/specctra/design.rs` never reads them, so no obstacle is created | Disqualifying for any board with mounting holes or rule areas. KiCad DRC does report it (`hole_clearance`); nothing in Topola does. |
| **Other copper layer empty** (B.Cu on dcsgonebadge and Wiccon when F.Cu is principal) | `Autorouter::selected_ratlines` + `Ratsnest::add_layer_to_ratsnest_triangulations`: ratlines are built per *principal layer* between pads that have copper on that layer; the planar command routes only that layer. `MultilayerAutoroute` is meant to add fan-out vias for off-layer pads and then run the same planar pass — it added nothing here (no via in any SES; cause not determined, UNVERIFIED) | "Multilayer" is single-layer plus fan-out, not layer assignment. Choosing B.Cu as principal gives more ratlines but ended in the permutation loop on both boards. |

### 5.4 Runtime

Everything Topola could route took under one second (release build, single thread). The
failure modes are the time sink: permutation (minutes, unbounded) and the multilayer panic
(7 min of search before the crash).

Images and raw metrics of this run: [benchmarks/2026-10-30-topola-d832b164](../benchmarks/2026-10-30-topola-d832b164/README.md).

## 6. Decision: A, B or C

> **Decision (user, 2026-10-30): do not build on Topola.** Option C: own engine; Topola is read as a reference for ideas only — no code dependency, no fork.

| Option | What it would mean | Assessment |
|---|---|---|
| **A — fork + KiCad bridge** | Keep Topola's `Drawing`/`Layout`/navmesh/Theta* core; write a KiCad-native `AccessMesadata`/`AccessRules`, an arc-preserving exporter, fix the multilayer path | Rejected. The bridge is the smaller part; the core lacks a topology phase, rip-up, layer assignment, rule model, integer geometry and determinism, and the authors are replacing it (`anyangle`). We would inherit 216 panic sites and open critical bugs in a codebase with one maintainer. |
| **B — reuse parts** | Depend on `planar-incr-embed` (homotopy-class navmesh), `specctra-core` (DSN), or copy the bend/cane model | Partly. `planar-incr-embed` is 2.5 kLOC WIP with a Voronoi-based navmesh and an "A*-like" search that does not implement the CDT cut-capacity model of [10](../topology/10-triangulation-and-cuts.md); its value to us is conceptual. `specctra-core` is useful only if we want DSN test fixtures (and our pipeline must not go through DSN/SES, [30 §6](../kicad/30-kicad-constraints.md#6-plugin-platform-constraints)). The bend/cane representation is ~1 kLOC worth of ideas better re-expressed in integer-nm form. |
| **C — own engine, Topola as reference** | Follow [31](../kicad/31-rust-engine-and-integration.md); read `drawing/`, `router/navmesh.rs`, `router/thetastar.rs`, `autorouter/anterouter.rs` when designing stacks, squeezing and fan-out | **Recommended.** Keeps the staged plan of [02 §7](../02-feasibility-study.md#7-conclusion-and-recommendation) (taut-arc smoother first) and the KiCad-side design of [30](../kicad/30-kicad-constraints.md) intact. |

Concretely reusable ideas, mapped to our documents:

- Nested thick arcs with per-band offset as the *data structure* of a stack
  ([21](../geometry/21-tangent-arc-paths.md)); squeezing a new band under existing bends
  as the incremental alternative to re-running the whole stack ([25](../geometry/25-relaxation-and-reflow.md)).
- Corner "fillet circles" for polygonal pads = our corner anchor circles
  ([20](../geometry/20-clearance-model.md)); Topola's open issues on rotated/rounded SMD pads
  confirm the risk ranking in [02 §5](../02-feasibility-study.md#5-risk-assessment).
- Draw-while-searching (prune a search edge when the cane cannot be drawn) is a cheap exact
  capacity test for the MVP where [10 §4](../topology/10-triangulation-and-cuts.md) warns that
  edge capacity alone is insufficient; keep it as the inner filter, not the only check.
- Fan-out via placement by compass-direction bounding-box search (`anterouter.rs`) is a
  reasonable first version of the fan-out pre-pass in [13 §4](../topology/13-vias-and-layers.md#4-placing-vias).

## 7. Risks of the recommendation

- Not forking means re-implementing ~10 kLOC of band drawing/collision code that Topola has
  debugged over two years; mitigated by the integer-nm model and exact checks we need anyway.
- Topola's rewrite (`anyangle`: single triangulation annotated per layer, multilayer A* with
  layer-switch penalty, 45° metric, planned via placement) may mature into exactly the engine
  we want in 1–2 years (UNVERIFIED — no timelines published). Re-evaluate at that point; keep
  our front end engine-agnostic so a mature Topola core could be swapped in.
- The measurements cover three boards and one commit; Topola's GUI interactive routing (its
  main product) was not evaluated.

## 8. UNVERIFIED items

- Whether the multilayer example panic reproduces on the authors' nightly/debug build with
  contracts enabled, and whether `anyangle` already fixes the mixed-side ratsnest limitation.
- Whether Topola's `TopoAutoroute` (planar-incr-embed backend) performs better than the
  Theta* backend on our boards — it is not reachable from the default CLI and was not run.
- The exact cause of the `navmesh.rs:440` panic (inferred from code comments, not debugged).
- Why the multilayer fan-out placed no vias on Wiccon/dcsgonebadge, and which connection
  (`NotFound` from Theta*) blocks the B.Cu runs — the per-ratline log only names an index.
- Wiccon hand-routing comparison (1 832 mm, 36 vias) could not be made because no complete
  Topola result exists for that board.
- TopoR behaviour in the comparison table is from the published descriptions cited in
  [01 §D](../01-bibliography.md), not from running TopoR.
- Hand-routing DRC baselines are from the boards as found, which already contain
  violations (eprom: 93, Wiccon: 48, dcsgonebadge: 210 incl. 5 unconnected).

## References

- Topola repository: https://codeberg.org/topola/topola (mirror https://github.com/mikwielgus/topola); website https://topola.dev/
- Technical notes: "planar-incr-embed concepts deep dive" (2025-05-11) https://topola.dev/notes/2025/05/11/planar-incr-embed-concepts-deep-dive/ ; "August 2026 anyangle development update" (2026-08-31) https://topola.dev/notes/2026/08/31/august-2026-anyangle-development-update/
- `anyangle` crate: https://github.com/mikwielgus/anyangle ; `cyrk`: https://codeberg.org/topola/cyrk ; `dearcut`: https://codeberg.org/topola/dearcut
- Open issues cited: #46, #47, #57, #120, #193, #210, #224, #246, #249, #278, #293 at https://codeberg.org/topola/topola/issues
- NLnet project page: https://nlnet.nl/project/Topola/ (UNVERIFIED URL for the 2026 "Topola-autoplacer" grant)
- KiCad Specctra import/export: `pcbnew/specctra_import_export/` (KiCad 10.0 sources); CLI DRC: https://docs.kicad.org/10.0/en/cli/cli.html
- TopoR sources: see [01 — Bibliography §D](../01-bibliography.md)
- Raw evaluation data (scratch, not part of the library): `topola-eval/boards/*/metrics-*.json`, `drc-*.json`, `widths-*.json`, `run-*.log`, `scripts/pipeline.py`, `mkcmd.py`, `mkcmd2.py`, `sesstat.py`, `widths.py`, `cleandrc.py`, `mkproject.py`
