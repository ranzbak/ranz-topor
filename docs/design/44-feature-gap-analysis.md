# 44 — Feature gap analysis: KiCad 10 design features vs. the router definition

**Purpose.** Inventory every KiCad 10 PCB-editor design feature that affects or constrains
autorouting or the routed result, check which of them the knowledge library already
defines, state what the router must do with each one, and rank them MoSCoW-style against
the bar *"produce a working, DRC-clean, manufacturable PCB"*. TopoR features not yet in our
definition are ranked the same way. Research date 2026-10-30; KiCad 10.0.8 installed locally.

Related: [30 — KiCad constraints](../kicad/30-kicad-constraints.md),
[12 — Sketch generation](../topology/12-sketch-generation.md),
[20 — Clearance model](../geometry/20-clearance-model.md),
[24 — Length, diff pairs, tuning](../geometry/24-length-diffpairs-tuning.md),
[02 — Feasibility study](../02-feasibility-study.md).

---

## 1. Method

1. **Inventory** from the KiCad 10.0 PCB Editor reference manual (docs.kicad.org/10.0 and the
   identical copy installed under `/usr/share/doc/kicad/help/en/pcbnew.html`, "KiCad 10.0
   Reference Manual"): Board Setup (constraints, net classes, tuning profiles, stackup,
   teardrops, tuning patterns, via protection), custom-rules language (all constraint types,
   object properties, functions), rule areas, zones, vias, pads, groups, locking, length
   tuning, multichannel/design blocks, DRC check list. Release posts for 9.0.0 (2025-02)
   and 10.0.0 (2026-03) on kicad.org were read for what changed; the dev-docs file-format page
   was consulted and is known to lag the real format (it still documents nets by number).
   Anything not confirmed in one of these sources is marked **UNVERIFIED**.
2. **Usage scan** of the user's own projects under `~/work/kicad/projects`, read-only
   (regular-expression counts over `.kicad_pcb`, JSON reads of `.kicad_pro`, token counts
   over `.kicad_dru`; backup directories skipped). Only aggregate counts are reported; no
   design content was copied. Counts are approximate (multi-line tokens, legacy formats).
3. **Gap check** against the library: for each feature, the section that covers it, or "gap".
4. **Priority** with the Must bar above: *Must* = without it the router produces broken or
   unmanufacturable boards on ordinary designs; *Should* = needed for real-board grade or
   present in a noticeable share of the scanned boards; *Could* = niche or cheap later;
   *Meh* = won't do now (ignore or defer entirely to KiCad).

## 2. Usage-scan summary

143 board files (`.kicad_pcb`), 119 project files, 4 rule files. Board-format versions range
from KiCad 4/5 (32 files) to KiCad 9/10 (33 files; 6 in the 10.0 format). 36 boards are in
pre-6 formats whose layer table the scan did not parse; copper-layer counts below cover the
other 107. "Boards" = number of boards in which the feature occurs at least once.

| Feature | Total count | Boards | Reading for the router |
|---|---|---|---|
| Copper layer count | 2 L: 67 · 4 L: 36 · 6 L: 3 · 8 L: 1 | 107 | 2-layer-first scope matches the majority; 4-layer is the second target |
| Nets per board | median 81, max 1 897 | 143 | engine must scale to ~2 000 nets |
| Track segments / **track arcs** | 175 971 / 3 843 | 84 / 22 | arcs already appear in 1 of 4 routed boards (fillets, curved outlines) — existing arcs are obstacles and must round-trip |
| Vias (all) / blind · buried · micro | 41 124 / 184 · 0 · 0 | 84 / 2 · 0 · 0 | through vias only in practice; blind in 2 boards |
| Free (stitching) vias | 5 824 | 37 | stitching/shielding vias are frequent obstacles |
| Vias with `remove_unused_layers` | 939 | 6 | per-layer via copper differs from the drill-based model |
| Via protection tokens (tenting/covering/plugging/filling/capping) | 10 415 | 30 | present in every KiCad 9+ board, almost always at the board default |
| Zones (all) / rule areas (keepouts) | 13 429 / 1 138 | 113 / 58 | keepouts: `copperpour` 1 090 (58 boards), `tracks` 76 (28), `vias` 68 (27), `pads` 84 (24), `footprints` 21 (14) |
| Zone priority ≠ 0 / island removal set / thermal pad connections | 12 089 / 11 639 / 24 756 (pad-level overrides) | 53 / 30 / 114 | pours are the norm; thermal-relief spoke rules apply almost everywhere |
| Teardrop zones | 11 588 (all `padvia`) | 19 | teardrops enabled in 93 of 119 projects' setup |
| Placement rule areas (multichannel) | 343 | 17 | ignore for routing |
| Groups | 225 | 35 | generated items must not break groups |
| Locked segments / arcs / vias | 227 / 0 / 151 | 8 / 0 / 10 | few, but they are deliberate — preserve |
| Locked footprints | 862 | 34 | placement is fixed; irrelevant to routing except as "don't move" |
| Net ties (`net_tie_pad_groups`) | 41 footprints | 15 | 1 board in 10 — needs explicit handling |
| Differential pairs (by `_P/_N`, `+/−`, `P/N` suffix) | 181 pairs | 48 | one board in three has at least one pair |
| Tuning patterns (`generated`) / diff-pair tuning | 219 / 55 | 12 / 7 | length tuning is used, mostly single-ended |
| Pads: SMD / THT / NPTH / aperture / custom-shape / slot drills | 39 649 / 8 016 / 10 264 / 109 / 2 244 / 314 | 87 / 82 / 71 / 20 / 38 / 50 | custom pads in 1 board of 4, slots in 1 of 3; NPTH everywhere |
| Pad fabrication properties: heatsink / BGA / castellated / mechanical | 69 / 192 / 1 / 8 | 18 / 1 / 1 / 2 | fiducial, test-point, press-fit: none |
| Pad / footprint clearance overrides | 407 / 70 | 48 / 10 | local overrides are common — clearance model must read them |
| Pad-to-die length | 0 | 0 | — |
| Copper-layer graphics/text (approx.) | ≈ 5 900 | 86 | copper obstacles without pads; must be in the obstacle model |
| Edge.Cuts arcs / circles / Bézier | 2 812 / 469 / 1 | 30 / 54 / 1 | curved outlines are the norm; footprints also contribute Edge.Cuts shapes (54 boards) |
| Layer types: power / mixed / jumper | 39 / 2 / 3 layers | 17 / 1 / 3 | plane layers marked in 1 board of 6 |
| Jumper pad groups (`duplicate_pad_numbers_are_jumpers`) | 1 420 | 6 | no routing effect |
| Net classes per project | 1: 53 · 2–4: 34 · 5–11: 32 | 119 | 45 % of projects use only `Default` ([30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths)) |
| Netclass with `priority` field / pattern assignments / nets with several classes | 166 / 673 / 0 | 35 / 53 / 0 | multiple-netclass membership is not used yet; pattern assignment is |
| Board-setup minimums set (clearance / track width / via / hole-to-hole / edge) | 56 / 95 / 116 / 116 / 76 | — | most projects set the floors; `use_height_for_length_calcs` on in 112 |
| Pre-defined track widths / via sizes / diff-pair sizes | 666 / 315 / 96 | 98 / 93 / 82 | hand-chosen widths live in these lists, not in netclasses |
| DRC exclusions | 709 | 20 | pre-existing excluded violations must not be re-blamed |
| Custom rules (`.kicad_dru`) | 8 rules in 2 projects | — | constraints used: `clearance` 5, `physical_hole_clearance` 3, `hole_clearance` 3, `silk_clearance` 3, `edge_clearance` 1; conditions all `memberOfFootprint(...)` / `intersectsCourtyard(...)` |

Notable: custom rules are rare in this corpus, but local pad/footprint clearance overrides,
free vias, net ties, pours with thermal reliefs, curved outlines and teardrops are common —
the "ordinary design" the Must bar refers to contains all of them.

## 3. MoSCoW tables

Router-behaviour vocabulary: **obstacle** (keep clearance from it), **rule** (read as a
constraint on what we produce), **produce** (the router creates it), **preserve** (leave
exactly as found), **ignore** (no effect on routing), **DRC** (leave exactness to the
`kicad-cli` guide rails, [30 §4.1](../kicad/30-kicad-constraints.md#41-kicad-drc-as-guide-rails-not-only-a-final-gate)).

### 3.1 Must have

| Feature | KiCad facts | Router behaviour | Covered in docs? | Notes |
|---|---|---|---|---|
| Board-setup constraints (minimum clearance, track width, via diameter/drill, through-hole size, hole clearance, hole-to-hole, copper-to-edge, annular width, connection width, microvia, `max_error`) | Absolute floors; no netclass or rule may go below them. Stored in `.kicad_pro` `design_settings.rules`; not queryable through the IPC API. | rule (floor) | [30 §2](../kicad/30-kicad-constraints.md#2-can-a-plugin-read-everything-needed--mostly), [30 §5](../kicad/30-kicad-constraints.md#5-design-rule-facts-that-shape-the-router), [30 §5.2](../kicad/30-kicad-constraints.md#52-manufacturer-baseline-when-the-design-sets-nothing) | `max_error` (arc approximation) feeds the margin policy in [23 §4](../geometry/23-distance-and-drc-checks.md#4-quantisation-to-integer-nanometres). |
| Net classes: clearance, track width, via size/drill, µvia, diff-pair width/gap; **multiple classes per net with priority aggregation** (9.0+); pattern-based assignment | A net's effective class takes each property from the highest-priority class that sets it; `Default` is the fallback; widths/via sizes are *optimal* values, not limits; `netclass_patterns` live in the project file. | rule | [30 §2](../kicad/30-kicad-constraints.md#2-can-a-plugin-read-everything-needed--mostly), [30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths), [12 §3.1](../topology/12-sketch-generation.md#31-net-priority-signal-importance) | **Gap:** aggregation semantics for several classes per net, and whether the IPC API returns the *effective* class or the plugin must merge pattern assignments itself (UNVERIFIED). |
| Custom rules — clearance family: `clearance`, `hole_clearance`, `edge_clearance`, `hole_to_hole`, `physical_clearance`, `physical_hole_clearance`, `track_width`, `via_diameter`, `hole_size`, `annular_width`, `disallow` | Rules evaluated last-match-wins in file order; `layer`, `severity`, `condition` over `A`/`B` with properties (`NetClass`, `NetName`, `Type`, `Pad_Type`, `Via_Type`, …) and functions (`intersectsArea`, `enclosedByArea`, `memberOfFootprint`, `hasNetclass`, `inDiffPair`, `isMicroVia`, `fromTo`, …). `opt` values are the router's defaults; `min`/`max` are DRC limits. | rule (conservative parse of the common condition shapes; DRC for the rest) | [30 §2](../kicad/30-kicad-constraints.md#2-can-a-plugin-read-everything-needed--mostly), [30 §5](../kicad/30-kicad-constraints.md#5-design-rule-facts-that-shape-the-router) | **Gap:** a written list of which condition shapes are parsed exactly (netclass / net-name / area / footprint / pad-type / layer) and the "unknown condition → take the strictest value for all pairs" fallback. |
| `track_angle`, `track_segment_length` rules | Flag any-angle / arc geometry. | pre-flight detection, refuse or warn | [30 §5](../kicad/30-kicad-constraints.md#5-design-rule-facts-that-shape-the-router), [02 §2](../02-feasibility-study.md#2-can-kicad-support-it--yes) | — |
| Rule areas / keepouts (`tracks`, `vias`, `pads`, `copperpour`, `footprints`), per layer, named, lockable; inner-layer keepouts inside footprints (10.0) | DRC error for any blocked item type inside; names usable in rules (`intersectsArea('name')`). | obstacle (tracks: inflate by `w/2`; vias: via region) | [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer), [13 §4](../topology/13-vias-and-layers.md#4-placing-vias) | **Gap:** footprint-embedded rule areas (new in 10.0) and `disallow` rules scoped by area are the same obstacle class — state it once. |
| Copper zones (pours): net, layers, priority, min width, fill mode, island removal; refilled after routing | Pours are soft obstacles (they re-flow around tracks); zone clearance is max(zone, netclass). | refill after routing; treat as obstacle only when locked / no-refill | [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer), [42 §4](../design/42-pitfalls-and-open-problems.md#4-kicad-specific-pitfalls), [02 §5](../02-feasibility-study.md#5-risk-assessment) | Pour fragmentation / islands after refill is a known, accepted risk. |
| Pad geometry: shapes (circle, rect, oval, roundrect, trapezoid, chamfered, custom with primitives), offsets, slots (oval drills), types (THT, SMD, NPTH, aperture, edge connector) | All readable through the IPC API incl. exact polygons. | obstacle / attach point | [20 §2](../geometry/20-clearance-model.md#2-pads-become-hulls-of-circles-anchor-circles), [20 §4](../geometry/20-clearance-model.md#4-pad-entry--attach-points) | Concave custom pads: convex hull for routing, full polygon for checks (already stated). |
| **Padstacks per layer** (9.0+): pad copper shape/size per layer (Normal / Front-Inner-Back / Custom), vias with per-layer annular ring, `remove_unused_layers` / `keep_end_layers`, `zone_layer_connections` | Obstacle shape differs per copper layer; unused inner annular rings may be absent. | obstacle per layer (read the per-layer shape, never assume the front shape everywhere) | [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer) mentions "remove unused pad layers" only | **Gap:** per-layer padstack reading; "unused layer removed" means the *hole* (plus hole clearance) remains the obstacle on that layer, not the annular ring. |
| Holes: PTH, NPTH, slots; `hole_clearance`, `hole_to_hole`, `physical_hole_clearance` | Hole-wall to copper and hole-to-hole are separate rules; NPTH have no net. | obstacle (hole capsule + clearance on every layer) | [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer), [30 §5.2](../kicad/30-kicad-constraints.md#52-manufacturer-baseline-when-the-design-sets-nothing) | — |
| Pad / footprint **local clearance overrides** (`Clearance_Override`, footprint `clearance`) | Override the netclass clearance for that pad/footprint; resolved by KiCad's clearance resolver. | rule (per-obstacle inflation radius) | [20 §1](../geometry/20-clearance-model.md#1-configuration-space) allows per-obstacle radii; the KiCad source of the value is not named | **Gap:** add overrides to the precedence list (override → rule → netclass → board → profile). Present in 48 of 143 scanned boards. |
| Board outline: Edge.Cuts lines, arcs, circles, Béziers, internal cutouts, footprint-contributed Edge.Cuts shapes; `edge_clearance` | Closed shapes are cut-outs; negative edge clearance allows overlap; castellated / edge-connector pads are exempt. | obstacle (inflate by `w/2 + edge clearance`) | [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer) | **Gap:** Bézier flattening with the same error bound as arcs, footprint-embedded edge shapes, and the exemption for castellated / edge-connector pads. |
| Copper-layer graphics and text (board and footprint shapes on `*.Cu`, with or without a net) | Copper with no net (or an assigned net since KiCad 8, UNVERIFIED version) is checked like any copper. | obstacle | gap | ≈ 86 of 143 scanned boards have copper graphics (logos, text, polygons). Not listed in [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer). |
| Existing tracks, arcs, vias of other nets; **locked** items; user-routed nets | Locked items cannot be moved unless overridden; the API reports the lock. | obstacle; preserve | [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer), [30 §3](../kicad/30-kicad-constraints.md#3-can-a-plugin-write-the-result--yes), [42 §4](../design/42-pitfalls-and-open-problems.md#4-kicad-specific-pitfalls) | Pre-routed same-net copper is a *start point* (connect to it), not an obstacle. |
| Free / stitching vias (`free yes`) | Standalone vias; net follows what they touch. | obstacle (other nets); same-net stitching vias are connection points | [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer) (vias generally) | 37 of 143 boards. |
| Via production: through vias from the netclass or `via_diameter`/`hole_size` `opt`, annular ring, hole-to-hole, via-in-pad policy, discrete drill list; new vias inherit board-default protection ("From rules") | Via properties: size, drill, layers, free, locked, tenting/protection, backdrill, teardrops. | produce | [13 §5](../topology/13-vias-and-layers.md#5-kicad-constraints-for-vias), [13 §4](../topology/13-vias-and-layers.md#4-placing-vias), [30 §5.2](../kicad/30-kicad-constraints.md#52-manufacturer-baseline-when-the-design-sets-nothing) | Write no explicit protection flags so the board default applies. `via_dangling` DRC: every produced via must connect on ≥ 2 layers. |
| Track production: segments + arcs (integer-nm start/mid/end), width resolution, 5–10 µm margin, bit-identical shared endpoints | Arc tracks since 6.0; the interactive router can edit them. | produce | [30 §1](../kicad/30-kicad-constraints.md#1-can-kicad-store-the-result--yes), [30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths), [23 §4](../geometry/23-distance-and-drc-checks.md#4-quantisation-to-integer-nanometres), [22 §1](../geometry/22-smoothness-and-biarcs.md#1-g1-continuity-comes-for-free) | — |
| Copper layer set and **layer types** (signal / power / mixed / jumper) | Types are a "guide for the user only": KiCad routes on any copper layer regardless. | rule (routing-layer selection) | [12 §3.1](../topology/12-sketch-generation.md#31-net-priority-signal-importance) (prefer plane-adjacent layers), [13](../topology/13-vias-and-layers.md) | **Gap:** an explicit setting "routable layers" defaulting to signal + mixed; power and jumper layers off unless enabled. 17 of 107 boards mark plane layers. |
| **Net ties** (`net_tie_pad_groups`) | A footprint may short nets; pads in one group may overlap/touch without DRC error; each net needs its own pad. | Ordinary footprint for KiCad; its pads carry two nets and are routed to normally. Only exception: the tie's internal shorting copper (graphic) is **not** an obstacle for the nets in its pad group (otherwise the pads are buried by gap 1, copper graphics). | gap | 15 of 143 boards. Without handling, the overlapping foreign-net pad sits on top of the attach point and the net is unroutable. Small rule, not a feature (user note 2026-10-30). |
| Unrouted connections / ratsnest | No ratsnest query in the API (UNVERIFIED); `kicad-cli pcb drc` lists `unconnected_items`. | derive from nets + connectivity | [30 §2](../kicad/30-kicad-constraints.md#2-can-a-plugin-read-everything-needed--mostly), [30 §4.1](../kicad/30-kicad-constraints.md#41-kicad-drc-as-guide-rails-not-only-a-final-gate) | — |
| DRC loop: `kicad-cli pcb drc`, `--all-track-errors`, `--refill-zones`, JSON, pre-flight, **DRC exclusions** | Exclusions are stored in the project; existing violations must not be blamed on the router. | DRC | [30 §4](../kicad/30-kicad-constraints.md#4-can-the-result-be-verified--yes-outside-the-api), [30 §4.1](../kicad/30-kicad-constraints.md#41-kicad-drc-as-guide-rails-not-only-a-final-gate) | 20 projects carry exclusions (709). Whether `kicad-cli` honours them by default is UNVERIFIED — the pre-flight diff handles it either way. |
| Routing scope (selected nets / netclasses / area / "unrouted only") and the width/via pre-flight report | — (plugin UI) | — | [30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths) (report), scope: gap | **Gap:** users route incrementally; "route everything" is rarely what is wanted. |

### 3.2 Should have

| Feature | KiCad facts | Router behaviour | Covered in docs? | Notes |
|---|---|---|---|---|
| Differential pairs: `_P/_N`, `+/−` naming, netclass dp width/gap, `diff_pair_gap`, `diff_pair_uncoupled`, dp skew DRC (9.0+), `isCoupledDiffPair`, `inDiffPair` | Router gap from netclass or rule; uncoupled length and skew are DRC checks. | produce (pair as one wide wire, concentric offsets); existing pairs: obstacle | [24 §2](../geometry/24-length-diffpairs-tuning.md#2-differential-pairs-as-offset-curves), [24 §3](../geometry/24-length-diffpairs-tuning.md#3-skew-from-turns) | 48 of 143 boards have pairs → high Should. Pair detection rules (suffix conventions, netclass) are not yet written down. |
| Thermal reliefs and zone connections: `zone_connection`, `thermal_relief_gap`, `thermal_spoke_width`, `min_resolved_spokes` (pad, footprint, zone and rule level) | After refill, DRC demands a minimum number of spokes per pad in a pour. | rule: keep spoke corridors free around THT/SMD pads inside pours (don't wrap tracks tightly around every pad) | gap | Pad-level thermal settings appear in 114 of 143 boards. A minimal-length router hugging pads is exactly what blocks spokes; DRC catches it but rip-up alone may not fix it. |
| Teardrops (board defaults, per-pad/via/track `Enable_Teardrops`, bulk tool; stored as small zones, refilled with zones) | Teardrops avoid other nets when refilled; custom pads may give poor shapes. | leave room at entries; after routing, regenerate through KiCad; existing teardrop zones: refilled, so not hard obstacles | [22 §5](../geometry/22-smoothness-and-biarcs.md#5-teardrops) | 19 boards, 11 588 teardrops. Whether the IPC API can trigger teardrop generation is UNVERIFIED ([42 §6](../design/42-pitfalls-and-open-problems.md#6-open-questions)). |
| Length / skew rules and tuning: `length`, `skew` (incl. `within_diff_pairs`), `via_count`, tuning patterns (`generated`), **time-domain tuning and tuning profiles** (10.0: per-netclass per-layer width and dp gap, via delay, pad-to-die delay), `use_height_for_length_calcs` | Length counts via height through the stackup by default; tuner uses shortest path through pads, Net Inspector sums copper; tuning profiles can raise a DRC "tuning profile track geometries" check (off by default). | rule (report length vs. constraint; keep tuned nets untouched; reserve meander space) ; produce later | [24 §1](../geometry/24-length-diffpairs-tuning.md#1-lengths), [24 §4](../geometry/24-length-diffpairs-tuning.md#4-arc-meanders-serpentines), [24 §5](../geometry/24-length-diffpairs-tuning.md#5-interaction-with-kicads-tuning-tools), [12 §3.2](../topology/12-sketch-generation.md#32-matched-groups-ddr-byte-lanes-buses) | **Gap:** tuning profiles as a *width/gap source per layer* (add to the width precedence in [30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths)); existing `generated` tuning patterns must be preserved as opaque locked objects. |
| Groups (nestable, nameable, lockable, design-block link) | Grouped items move/select together. | preserve (never add generated items to a user group; honour a locked group as locked members) | gap | 35 of 143 boards. |
| Blind / buried / micro vias; `allow_blind_buried_vias`, `allow_microvias`, valid layer pairs from the stackup; `isBlindVia`/`isMicroVia` conditions | Micro and blind/buried vias start and end on any layer. | obstacle on their span (existing); produce: later, only if enabled | [13 §5](../topology/13-vias-and-layers.md#5-kicad-constraints-for-vias), [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer) | 2 boards, 184 blind vias. Producing them is Could. |
| `disallow` with conditions (e.g. no vias under a BGA, no tracks on a layer for a netclass) | Object types: track, via, through_via, micro_via, blind_via, buried_via, pad, zone, hole, footprint, … | rule (area / layer / netclass scoped keepout) | [30 §5](../kicad/30-kicad-constraints.md#5-design-rule-facts-that-shape-the-router), [12 §3.2](../topology/12-sketch-generation.md#32-matched-groups-ddr-byte-lanes-buses) | Same parser as the clearance family; listed separately because it steers layer assignment. |
| Via protection IPC-4761 (tenting, covering, plugging, filling, capping; per side; board default) | No copper-geometry effect; via-in-pad usually needs Type IV/VII. | produce with "From rules"; preserve existing | gap (cosmetic) | 30 boards carry the tokens, nearly all at the default. Plugin setting "allow via-in-pad only if board default protection ≥ Type IV" is a sensible guard. |
| Net priority tiers, matched groups, bus bundles | KiCad has no routing-priority attribute; netclass `priority` only orders class aggregation. | rule (plugin settings + inference) | [12 §3.1](../topology/12-sketch-generation.md#31-net-priority-signal-importance), [12 §3.2](../topology/12-sketch-generation.md#32-matched-groups-ddr-byte-lanes-buses) | Defined; implementation effort is the issue, not definition. |
| Multithreading, reproducible multi-variant search | — | — | [31 §6.1](../kicad/31-rust-engine-and-integration.md#61-multithreading-user-configurable-thread-count) | Defined. |
| Power-net via arrays (several vias per power connection for current) | KiCad has no "via count for current" rule; `via_count` is max/min per net. | produce (n vias from the current/width table in [30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths)) | partial ([30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths) mentions via count for current) | **Gap:** layer-change of a wide trace needs a via array, not one via — otherwise the "working PCB" bar fails on power. |

### 3.3 Could have

| Feature | KiCad facts | Router behaviour | Covered in docs? | Notes |
|---|---|---|---|---|
| Creepage (`creepage` rule, 9.0+) | Minimum creepage path between copper of different nets; path shown in DRC; slots lengthen the path. | rule (approximate as a larger clearance for the matching pairs; exact path left to DRC) | [30 §4.1](../kicad/30-kicad-constraints.md#41-kicad-drc-as-guide-rails-not-only-a-final-gate) (DRC covers it) | Needed only for HV boards; none in the scanned corpus. |
| Component classes (`hasComponentClass`, `memberOfFootprint('${Class:x}')`) | Assigned in the schematic; inherited by pads. | rule (parser support for the condition) | gap | Not used in the corpus; cheap once the rule parser exists. |
| Backdrill (must-cut layer, size; vias and PTH pads) and post-machining (counterbore / countersink) | DRC "track on backdrilled layer"; backdrill larger than the hole. | obstacle (existing: larger hole radius on backdrilled layers, no connection on removed layers); never produce | gap | High-speed multilayer only; none in corpus. |
| Castellated / edge-connector pads, edge plating, bevel | Pads may intersect Edge.Cuts; stackup `edge_connector` / `castellated_pads` only affect Gerber attributes. | obstacle; exempt from edge-clearance reasoning | gap | 1 board. |
| Pad fabrication properties (BGA, fiducial, test point, heatsink, mechanical, press-fit) | Affect Gerber X2 and a few DRC exemptions (heatsink exempt from courtyard check). | ignore (pads route like any other; no-net pads are obstacles) ; fiducials: never route over (keep-out radius) | gap | Heatsink pads in 18 boards; fiducial property unused in corpus (fiducials there are plain pads). |
| Pad-to-die length / delay | Adds to tuner length and time-domain delay. | rule (add to reported length) | [24 §1](../geometry/24-length-diffpairs-tuning.md#1-lengths) mentions it | Only matters with length matching. |
| Stackup: dielectrics, thickness, `epsilon_r`, impedance (`dielectric_constraints`), copper finish | Feeds via-height length and time-domain delays; no direct geometry effect. | ignore except via height for length | [24 §1](../geometry/24-length-diffpairs-tuning.md#1-lengths) | — |
| Jumper layers, jumper pad groups, jumpered pins (10.0) | Jumpered pads share a net and show no ratsnest; jumper copper layer is a hint. | ignore (nets already merged); jumper layer not routable by default | gap (layer policy above) | TopoR-style single-sided routing with jumper wires is a separate feature (§4). |
| Courtyards, `courtyard_clearance`, `physical_clearance` to courtyards (9.0+) | Footprint placement rules; `physical_clearance` can also apply between copper fills and courtyards. | ignore (placement is fixed); DRC | gap | A `physical_clearance` rule between *tracks* and courtyards would be an obstacle — rare, UNVERIFIED whether users do this. |
| Pin / gate swap (10.0 unconstrained swap with back-annotation) | KiCad can now back-annotate swaps. | optional topological move (off by default) | [12 §3](../topology/12-sketch-generation.md#3-removing-order-dependence) | Was "generally off"; KiCad 10 makes it feasible later. |
| Multichannel / placement rule areas / design blocks (repeat layout copies routing) | Repeat Layout copies tracks between channels. | ignore placement areas; optionally route one channel and let KiCad repeat | gap | 17 boards have placement areas; a routing-aware workflow is a later nicety. |
| Solder-mask rules (`solder_mask_expansion`, `bridged_mask`, min web, track mask removal) | Mask-only effects; mask slivers between close vias/pads are a fab concern. | ignore; optional via-to-via spacing ≥ mask web | gap | — |
| Hatched zone fill, zone smoothing / external fillets, island removal modes | Fill-time properties. | ignore (refill handles them) | [42 §4](../design/42-pitfalls-and-open-problems.md#4-kicad-specific-pitfalls) | — |
| `connection_width` DRC (copper neck too narrow) | Mostly pour necks; also pad-to-track partial overlaps. | ensure full-width pad entry (attach at pad centre / inside pad) | [20 §4](../geometry/20-clearance-model.md#4-pad-entry--attach-points) | — |

### 3.4 Meh (won't do now)

| Feature | KiCad facts | Router behaviour | Covered in docs? | Notes |
|---|---|---|---|---|
| Silkscreen, text, dimensions, tables, points, barcodes (non-copper), reference images | No copper. | ignore | — | Barcodes/text placed *on* copper layers fall under copper graphics (Must). |
| Design variants (10.0), DNP / exclude-from-BOM attributes | Pads still exist on the board. | ignore (route as placed) | — | — |
| Net colours, layer presets, appearance | Display only. | ignore | — | — |
| Specctra DSN/SES, Freerouting hybrid | Arcs not carried. | never | [30 §6](../kicad/30-kicad-constraints.md#6-plugin-platform-constraints), [42 §4](../design/42-pitfalls-and-open-problems.md#4-kicad-specific-pitfalls) | Decided. |
| Live "flexible" re-shaping during drag | No hook into KiCad's tools. | batch re-flow instead | [25 §5](../geometry/25-relaxation-and-reflow.md#5-re-flow-when-a-component-moves-interactive--incremental-mode), [30 §6](../kicad/30-kicad-constraints.md#6-plugin-platform-constraints) | Decided. |
| Producing blind/buried/micro vias, backdrills, post-machining | HDI-only. | never (this stage) | [30 §5.2](../kicad/30-kicad-constraints.md#52-manufacturer-baseline-when-the-design-sets-nothing) | — |
| Text-variable DRC assertions (`assertion`, `${DRC_ERROR}`), silk/text/paste constraints | Non-routing checks. | DRC | — | — |
| 3D, plotting, ODB++/IPC-2581 export, output jobs | Downstream of routing. | ignore | — | — |

## 4. TopoR features not yet defined

From [01 §D](../01-bibliography.md#d-topor-eremex), [12](../topology/12-sketch-generation.md),
[13](../topology/13-vias-and-layers.md) and [43 §3](../design/43-topola-assessment.md#3-topola-vs-topor-vs-our-requirements).
Already defined (not repeated here): rubber-band topology, taut arcs, stacks, wider-first
ordering, double-crossing removal, loop/clinch detection, layer separation, segment lifting,
Pareto variants, Steiner 120° points, re-flow after move, parallel length matching of groups.

| TopoR feature | Status in our definition | Router behaviour | Priority |
|---|---|---|---|
| BGA / fine-pitch escape from the centre outwards (dog-bone fan-out pre-pass) | mentioned as "separate pre-pass later" in [13 §4](../topology/13-vias-and-layers.md#4-placing-vias), [25 §3](../geometry/25-relaxation-and-reflow.md#3-vias-and-steiner-points-in-continuous-space) | produce | Should (needed for 4-layer real boards; excluded from the 2-layer MVP) |
| Wire fattening — widen traces where room remains (Hama–Etoh "wire-fattening") | spreading defined in [25 §4](../geometry/25-relaxation-and-reflow.md#4-using-spare-room-dont-leave-everything-at-minimum-clearance); fattening not | produce (optional, power nets first) | Could |
| Single-sided routing with jumper wires (TopoR's jumper feature) | gap | produce jumper footprints / use KiCad jumper layer | Could |
| Automatic via arrays / multiple vias for power | gap (see §3.2) | produce | Should |
| Topology editing by the user (move a wire to the other side of a pad, then re-flow) | batch re-flow defined in [25 §5](../geometry/25-relaxation-and-reflow.md#5-re-flow-when-a-component-moves-interactive--incremental-mode); user-driven topology moves not | plugin command "flip side of obstacle for selected track" → re-flow | Could |
| Route only a region / selected nets, lock the rest ("partial routing") | gap (see §3.1) | — | Must |
| Autoplacement / placement optimisation | gap | — | Meh |
| Output as 45°/polyline instead of arcs (TopoR can export both) | gap | produce (segment-approximated arcs on request, for fabs or tools that dislike arcs) | Could |
| **Traditional 45° routing mode** (user request 2026-10-31; backlog, after M4) — optional per run, default stays curved | gap | produce: same topology (sketch, slot search, negotiation) but octilinear metrization — shortest path within the same homotopy class using only 0°/45°/90° segments, optional small mitre/arc at bends, clearance against octilinear offsets; same DRC gates, length compared to curved mode | Should |
| Pin / gate swapping with back-annotation | "off by default" in [12 §3](../topology/12-sketch-generation.md#3-removing-order-dependence) | optional | Could |
| T-junctions on wires (branch into the middle of an existing trace) | open problem in [42 §6](../design/42-pitfalls-and-open-problems.md#6-open-questions) | produce | Should |

## 5. Gaps to add to the definition (ordered by priority)

**Must**
1. Copper-layer graphics and text as obstacles — extend the obstacle list in [20 §3](../geometry/20-clearance-model.md#3-which-obstacles-exist-per-layer).
2. Net ties: ordinary footprints; only exclude the tie's internal shorting copper from the obstacle set for its own nets (one-line rule in the obstacle model).
3. Local pad / footprint clearance overrides in the clearance precedence ([30 §5.2](../kicad/30-kicad-constraints.md#52-manufacturer-baseline-when-the-design-sets-nothing)).
4. Per-layer padstacks and removed unused annular rings: obstacle shape per copper layer; hole remains.
5. Multiple-netclass aggregation and pattern assignments: define how the plugin computes the effective class (or verify the IPC API does — UNVERIFIED).
6. Rule-parser contract: the exact list of condition shapes parsed, and the strictest-value fallback for everything else.
7. Routable-layer policy (signal + mixed by default; power / jumper layers opt-in).
8. Board-outline details: Bézier flattening, footprint-contributed Edge.Cuts, castellated / edge-connector exemption.
9. Routing scope selection (nets / classes / area / unrouted-only) as part of the plugin definition.

**Should**
10. Thermal-relief spoke preservation around pads inside pours.
11. Tuning profiles (10.0) as a per-layer width / dp-gap source; preserve `generated` tuning patterns as locked.
12. Group handling for generated items; locked groups.
13. Power-net via arrays and the layer-change rule for wide traces.
14. Differential-pair detection rules (naming, netclass) and existing-pair preservation.
15. BGA fan-out pre-pass definition (TopoR: escape from the centre).
16. Via protection: write "From rules"; via-in-pad guard.
17. T-junctions onto existing traces (currently an open problem).

**Could**
18. Creepage as an inflated clearance; component-class conditions; backdrill obstacles; castellated pads; fiducial keep-out; jumper-wire routing; wire fattening; polyline output option; user topology moves; multichannel-aware routing.

## 6. UNVERIFIED points

- Whether the IPC API (kicad-python 0.8.x) returns a net's *effective* aggregated netclass and
  resolves `netclass_patterns`, or only the explicit assignment.
- Whether `kicad-cli pcb drc` applies the project's DRC exclusions by default.
- Whether the IPC API can trigger teardrop generation (carried over from [42 §6](../design/42-pitfalls-and-open-problems.md#6-open-questions)).
- The KiCad version from which copper graphics can carry a net (8.0 believed).
- Whether users write `physical_clearance` rules between tracks and courtyards (would make courtyards obstacles).
- The dev-docs file-format page documents nets by number and lacks the 9/10 via-protection,
  padstack and `zone_layer_connections` tokens; the token names used in §2 come from the
  scanned files, not from documentation.
- Scan counts for multi-line tokens (copper graphics, footprint Edge.Cuts, locked items in
  pre-6 formats) are approximate.

## References

- KiCad 10.0 PCB Editor manual: https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html (Board Setup → Constraints,
  Net classes, Tuning profiles, Teardrops, Via protection (IPC-4761), Back-drilling; Custom design rules → constraint
  types, object properties, functions; Rule areas; Groups; Locking; Length tuning; Net ties and jumper pads;
  Design rule checks list). Local copy: `/usr/share/doc/kicad/help/en/pcbnew.html`.
- KiCad 10.0 "What's new": https://docs.kicad.org/10.0/en/introduction/introduction.html
- KiCad 10.0.0 release post: https://www.kicad.org/blog/2026/03/Version-10.0.0-Released/
- KiCad 9.0.0 release post: https://www.kicad.org/blog/2025/02/Version-9.0.0-Released/ ; 9.0.1: https://www.kicad.org/blog/2025/03/KiCad-9.0.1-Release/
- Board file format (dev-docs, partly outdated): https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/index.html
- JLCPCB capabilities (manufacturer profile in [30 §5.2](../kicad/30-kicad-constraints.md#52-manufacturer-baseline-when-the-design-sets-nothing)): https://jlcpcb.com/capabilities/Capabilities
- TopoR sources: see [01 §D](../01-bibliography.md#d-topor-eremex).
