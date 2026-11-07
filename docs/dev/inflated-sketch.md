# Inflated sketch: the exact cut test and spoke inflation per layer

Design document, written 2026-11-07 on branch `inflate-design` (from `topor-topo` @ ccd4a3a). Nothing here is
implemented. It works out the user's request to "inflate" the traces during the topological phase, while changes are
still cheap, so the drawing only realises a plan that already fits.

Conventions:

- `file:line` refers to `engine/src/topo/` at ccd4a3a.
- Literature pages are given as **PDF page** (the page of the file) and, where it differs, the printed page ("pr.").
  - Dayan 1997: PDF = pr. + 11.
  - Staepelaere 1992: PDF = pr. + 8.
  - Maley 1987: PDF = pr. up to pr. 150. The scan skips printed 151–303, and from there PDF = pr. − 104.
- "Unverified" marks a claim I could not check against a source or the code. "Inferred" marks my own reasoning from
  a cited statement.
- Downloaded papers are in the scratchpad (`.../scratchpad/inflate/papers/`), not in `docs/literature/`.

## 0. Summary

1. **Theorem.** The Leiserson–Maley theorem says a single-layer sketch is routable iff no nonempty cut is unsafe
   (congestion > capacity). It suffices to check the **critical cuts**: from every feature endpoint to the closest
   point of every other feature (Maley 1987 pp. 32–33, 40).
   - It holds per layer with polygon pads, mixed widths and any norm (Maley pp. 29, 32, 314).
   - It needs five conditions: spacing folded into per-element widths, a planar sketch per layer, point terminals,
     fixed vias and junctions, and polygons that enclose the copper (§1.2).
2. **Nearby pairs alone are not provable.** Maley says "none of these principles can be justified formally" (p. 53).
   - The provable reduction is **shadowing**: a cut pq is implied when ‖p−q‖ = ‖p−r‖ + ‖r−q‖ for a feature point r
     (p. 55).
   - In the Euclidean norm this removes almost nothing.
   - In the **octagonal norm**, which the 45° drawing needs anyway (Staepelaere eq. 1.2, PDF p. 12), shadows are
     wide cones. That makes the octagonal norm the norm to plan in for the default style.
3. **topor already has half of it.**
   - `cuts.rs` inserts nearly Maley's critical cuts into the projection CDT (LCT feet).
   - `pack.rs` computes the per-cut need and the lmin/rmax prefix sums, which are spoke lengths.
   - Planner and drawing agree per edge: the qmtech run had 0 edge-layers that were planned to fit and over-full when
     drawn.
4. **Four things are wrong for an exact test.**
   - The cuts are taken from one projection over all layers. An edge whose end is another layer's pad is no cut on
     this layer, and the true cut behind it is never checked (`state.rs:247-263`, `pack.rs:579`).
   - Critical cuts that cross a kept cut, lie beyond 3 mm or are not the nearest per owner are dropped
     (`cuts.rs:151-184`, `:279-293`, `board.rs:49`).
   - Flows count projection passes, not per-layer rubber-band crossings.
   - The drawing realises the plan sequentially: each wire is an obstacle for the next (`exact.rs:11-14`). This is
     the "puzzle" failure of Dayan's Fig. 1 (PDF p. 20).
5. **Design.**
   - Per signal layer, a *layer sheet*: a CDT over that layer's features with that layer's critical cuts.
   - Every stretch on the layer gets its rubber-band (taut) path in the sheet.
   - Every critical cut gets its ordered crossing list and its slack.
   - A smooth overflow price per cut drives P3 relabels and P4 rip-up and moves.
   - The drawing gets the **spoke radii** (prefix needs along each cut from each vertex) for all wires at once. Each
     stretch is drawn independently through its spoke tips, which is the extended rubber-band sketch (ERBS).
   - In the octilinear style, the ERBS is then made octilinear (Staepelaere's plane sweep, or `octi.rs` fed with the
     ERBS).
6. **Staging.** Measure first: U1 is the M-A classification of qmtech's 89 violations with the cut test. Then
   price, then draw. All units start after D7 lands.

## 1. Short answers

### 1.1 Which cuts

The theorem is stated over *all* cuts: segments that touch features only at their ends (Maley p. 32). Three
reductions are proven and one is not:

| Reduction | Status | Source |
|---|---|---|
| Only **nonempty** cuts: congestion > 0 and the ends on different islands | proven | Maley p. 33 |
| Only **critical** cuts: feature endpoint → closest point (in the wiring norm) on another feature; O(\|F\|²) of them | proven ("decisive") | Maley p. 40 |
| Drop **shadowed** cuts: pq is implied if some feature point r has ‖p−q‖ = ‖p−r‖ + ‖r−q‖. Keep the diagonal (norm-breakpoint) cuts, O(\|F\|) of them. Typically O(\|F\| log \|F\|) cuts remain in the rectilinear norm | proven implication; the count is average-case only | Maley pp. 55–56 |
| Only **local** cuts (bins, nearby terminals) | **not provable**. An unsafe cut found proves unroutability; none found proves nothing | Maley p. 53 ("None of these principles can be justified formally") |

Why a shadowed cut is implied (inferred from eq. 1-3 on p. 55):

- With r on a feature and the norm additive along p→r→q, capacity(pq) = capacity(pr) + capacity(rq) + width(r).
- Every trace that must cross pq must cross pr or rq, or wrap r.
- So pq overflows only if pr or rq does.

In the Euclidean norm, eq. 1-3 holds only for r exactly on segment pq, and then pq is no cut at all (Maley p. 55:
"this shadow would be nothing more than the ray"). In a piecewise-linear norm the shadow is a cone.

- **Octagonal norm** ‖(x, y)‖ = max(|x|, |y|, (|x| + |y|)/√2): this is Staepelaere's octilinear capacity, eq. 1.2,
  PDF p. 12, for zero-width wires at unit spacing.
- Its shadow of r, seen from p, is the 45° cone at r that holds the two octilinear directions bracketing p→r
  (inferred: the norm is linear inside each 45° sector bounded by the 22.5° + k·45° breakpoints).

SURF itself checks only "the cuts with respect to other nearby terminals" before it creates spokes (Su & Dai,
ICCAD 1997, PDF p. 4). This is a practical heuristic, not a theorem.

**Recommendation.** Use the critical cuts of each layer, pruned by shadowing in the planning norm. Add a measured
locality radius as an *option*, never as the default authority.

- The radius is unsafe in exactly the case the user named: a squeeze between two non-neighbouring obstacles
  (boardlet B2, §6.2).
- U0 measures how many unshadowed critical cuts the real boards have (§5).
- If that count is too large, the fallback is Maley's dual-walk evaluation: O(log n) plus O(1) per cable crossed
  (p. 54).

### 1.2 Is the theorem valid per layer with our pads and mixed widths?

Yes, under these conditions. Each is either met by topor already or has a stated workaround.

| Condition (Maley's model) | Source | topor | Workaround |
|---|---|---|---|
| One layer, traces do not intersect (a planar sketch) | p. 29 | P3 forbids same-net-different crossings per layer; an *unlayerable* crossing (M-B2) breaks planarity | price unlayerable crossings separately, as today; leave that layer region out of the theorem's claim |
| Features are **points or line segments**; curved features are not handled (pp. 28, 310, per the search agent) | p. 29 | pads are polygons (`Obstacle.poly`); round parts are polygonised *inside* the copper (`sag`, `state.rs:82-84`) | grow each polygon by `sag` (or polygonise circumscribed) so the polygon contains the copper; the drawing already grows by `sag` (`draw.rs:785`) |
| **Mixed widths**: every element has its own width; territory = points closer than width/2 | p. 29; congestion = Σ width · entanglement, p. 32 | per-pass widths, necked or not (`state.rs:544`, `pass_w`) | none needed |
| **Spacing** is folded into the widths: two elements interact iff their territories overlap | p. 29 | KiCad uses max(s_i, s_j) between nets and 0 within one net (`demand.rs:79-83`) | **exact** when all nets on a layer share one clearance (the usual case); with per-class clearances use territory w_i/2 + s_max/2 of the region (conservative). Same-net zero gap is outside the model; treat it as allowed merging (inferred safe) |
| **Terminals are isolated points** | p. 29 (4) | wires end at pad centres *inside* the pad polygon (`state.rs:478-483`) | model the pad as an obstacle island plus a terminal point at its planned exit. The terminal-to-own-pad cut is degenerate and skipped, as `demand.rs:55-60` does with "own-net end = half width" (inferred; Maley's "terminal merging", p. 310, is the related case of a trace's two terminals) |
| One **wiring norm** for all elements, any norm | p. 29; any norm via a surrogate polygonal norm, pp. 314–315 | KiCad checks Euclidean clearance | plan in Euclid for `style = smooth`; plan in the octagonal norm for `octilinear`. The octagonal norm ≤ Euclidean, so it is conservative: up to 1/cos 22.5° = 8.2 % more length needed (inferred from eq. 1.2) |
| **Fixed** terminals | p. 29 | vias are intervals until P6 (`state.rs:11-13`, `ViaSlot`); junctions can slide (D5) | fix every via at its planned site before the test (`CapModel::via_k` + a site point, `pack.rs:37-43`). A through via is a terminal on its two layers and a feature on all layers (`pack.rs:49-52`) |
| **Two-terminal traces**; multi-terminal wires are not representable (p. 28, per agent) | p. 28 | net trees are already split into 2-terminal wires with node terminals (`state.rs:483-485`) | junction points are fixed point terminals of the net, as in SURF's 2-Nets (Dayan PDF p. 32) |

So the per-layer test is **exact** for single-clearance layers and **conservative** otherwise.

- It is *necessary* in every case: a legal layout must satisfy the need along every straight cut (inferred, since
  along a straight segment the copper of consecutive crossings must be at least w/2 + w'/2 + s apart).
- It is *sufficient* only under the conditions above.
- The sufficiency proofs are in Maley's chapters 6–8, which are missing from the available scan (§2.5).

### 1.3 Where the agreed proposal is wrong or needs a correction

1. **"Capacity = distance; flow = widths + terminal radii + spacing"** (Dayan PDF p. 25) is right. But:
   - The *metric* must be the drawing's norm (octagonal for 45°).
   - The oblique-chord factor of `pack.rs` (1/sin, `pack.rs:216-226`) must **not** be applied on critical cuts: Maley's
     congestion is a plain width sum (p. 32).
   - The oblique factor stays only as long as the cut set is incomplete, as a proxy for the missing shorter cuts.
2. **"Corner stacks are only an estimate."** They are more than that. The traces that wrap a corner p contribute
   exactly to every cut from p that lies inside their wrapped wedge (inferred from entanglement, p. 32: a band that
   wraps p on the cut's side must cross the cut). The corner stack is the *wrap part* of the exact flow; a cut adds
   the traces that cross its open segment.
3. **"It suffices to test the rubber-band sketch."** True, and important: the test must run on the **per-layer**
   rubber band (taut path in that layer's sheet), not on projection passes.
   - A stretch can wind around another layer's pad in the projection, which over-counts crossings.
   - The true per-layer cut can run *through* another layer's pad region, so it is not a projection edge.
4. **"Spokes; in Euclidean metric the ERBS is the final embedding"** (Dayan PDF p. 26). But SURF uses *octilinear*
   spokes as an approximation of the arc even for any-angle wiring (Dayan PDF p. 33, Fig. 11).
   - A Euclidean drawing through spoke tips 45° apart loses 1 − cos 22.5° = 7.6 % of the radius between the tips.
   - So either flatten the arcs with circumscribed tips (as `exact.rs:8-10` already does at 22.5° steps, costing
     1.9 %), or plan in the norm that the spokes realise.
5. **"Staepelaere's octilinear capacity, scaled by real widths."** Mostly right. Two limits:
   - Staepelaere assumes one spacing per layer and widths that are "a function solely of that segment"
     (PDF p. 62). Our max(s_i, s_j) rule violates that; use the conservative per-element territory.
   - His spacing guarantee is not proven in the thesis. It rests on Kong 1992's S-route argument (PDF p. 18–19),
     which I could not obtain.
6. **PathFinder.** The plan file lists "Passage prices as the coordination engine (PathFinder, overflow price …)"
   under *Removed* with "Cut/LCT/layer-combination checks only measure" (`wise-hatching-bunny.md` §"Removed"), and
   its "Mathematical basis" says the cut theorem is a "measuring tool, not the engine".
   - The code already runs a negotiated rip-up with history (`core.rs:1077-1116`, `state.history`).
   - The user's agreement now overrides that; the plan text should be updated (§8).

## 2. Literature

### 2.1 The Leiserson–Maley theorem (Maley 1987 thesis; L&M 1985 not obtained)

Read from Maley's MIT PhD thesis "Single-Layer Wire Routing" (1987, MIT DSpace 1721.1/14657, scanned, no OCR). I
checked pp. 29, 32, 33, 40, 53–56 and 310, 314 myself from the page images.

- **Model (p. 29).**
  - A *feature* is a point or line segment.
  - A *trace* is a piecewise-linear path that touches features only at its endpoints, which are its terminals.
  - Each terminal is an isolated point.
  - Every element has a *width*; its territory is the set of points at distance < width/2.
  - Distance is a single "wiring norm" for all elements.
  - Islands are connected groups of features.
- **Cut, capacity, congestion (p. 32).**
  - A cut is a segment that touches features only at its ends.
  - capacity(pq) = length(pq) − width(P)/2 − width(Q)/2, with length in the wiring norm.
  - entanglement(trace, pq) = the minimum number of crossings of pq over all routes of the trace in its class
    (crossings at p or q excluded).
  - congestion(pq) = Σ width · entanglement.
- **Theorem (p. 33).** "If a sketch is safe, then it is routable. Conversely, every routable sketch is safe": the
  *sketch routability theorem*.
  - A sketch is safe iff all its nonempty cuts are safe.
  - The *sketch routing theorem* gives the minimum-length realisation (the "ideal realization" of every trace); its
    construction is deferred to the missing chapters.
- **Critical cuts (p. 40).** "A critical cut is a cut that begins at a feature endpoint and travels to the closest
  point on another feature … there are O(|F|²) of them."
  - Testing takes O(|F|² log |F|) given the condensed rubber-band equivalent (RBE).
- **RBE (pp. 33–40).** The shortest homotopic band of every trace, built in O(|F||T| log |S|) (p. 34, per the search
  agent).
- **Congestion by the dual walk (p. 54).** Every cut is a path in the dual of the (triangulated) condensed RBE; its
  congestion costs O(log n) plus O(1) per cable crossed.
  - The expected total is Θ(n log n); the worst case is Θ(n³).
- **Locality (p. 53).** Quoted: "it is highly unlikely that a long nonempty critical cut is unsafe without some
  shorter nonempty critical cut being unsafe also. None of these principles can be justified formally".
  - A bin-local scan "has the drawback of relying on the locality principle for correctness".
- **Shadowing (pp. 55–56).** Eq. 1-3, with Cole & Siegel as an independent source.
  - Euclidean shadows are only a ray.
  - Piecewise-linear shadows are substantial.
  - Typical O(|F|^{3/2} log |F|) time for a "typical" sketch.
- **Norms (ch. 10B, pp. 310–315).**
  - The rectilinear norm keeps the routability theorem (p. 311, per the search agent).
  - Any norm, curvilinear too, via a surrogate polygonal norm that circumscribes the unit circle (pp. 314–315).
  - No octagon-specific section; applying it to 45° is inferred from the general statement.
- **Not in the scan:** chapters 6–8, printed pp. 151–303. They hold the formal statements and proofs, including §6D
  "Cuts That Decide Routability" (pr. p. 196 per the table of contents).

Secondary statements about L&M 1985 (not read):

- They work in a grid / L∞ model (Duncan, Efrat, Kobourov, Wenk, "fat edges", printed p. 2).
- They introduced the RBE, the rubber-band sketch and spokes (Staepelaere PDF p. 65; Hama & Etoh 1998, PDF p. 1).

### 2.2 How spokes are created (SURF)

- **What.** Spokes are open-ended line segments radiating from the endpoints (terminals, vias, junctions) of the RBS.
  They "prop up" the bands so the spacing holds (Staepelaere PDF p. 10, pr. 2).
  - Rectilinear wiring uses up to 4 spokes per point, octilinear up to 8 (Dayan PDF p. 32, pr. 21).
  - For any-angle wiring SURF still uses the 8 octilinear spokes "as an approximation to the arc" (Dayan PDF p. 33,
    Fig. 11).
- **Result.** The bands with spokes form the ERBS. "The branch paths in the ERBS satisfy the spacing requirements if,
  and only if, the topological class represented by the RBS is routable" (Dayan PDF p. 26).
  - The ERBS is a minimal-length member of the class.
  - In the Euclidean metric it is "the final geometrical embedding".
  - The test is constructive, unlike the flow/capacity criterion.
  - On failure, spoke creation "will locate all of the endpoints that participate in overflowing cuts"
    (Staepelaere PDF p. 13).
- **How (SURF).**
  - Spoke creation is "an incremental layer based operation"; each terminal is processed sequentially.
  - It first checks cuts to nearby terminals (flow ≤ capacity), then enforces spacing with spokes that take "the
    terminal geometry, spacing rules, and branch width" into account (Su & Dai 1997, PDF p. 4).
  - L&M generate spokes "from all the features". Dai's algorithm "traces each wire and generates spokes only where
    they are necessary" (Hama & Etoh 1998, `docs/literature/hama-etoh1998-curvilinear.pdf`, PDF p. 1).
  - The SURF spoke algorithm proper is in Dai, Kong & Sato, DAC 1991 pp. 45–48, and Kong's MS thesis 1992. **I could
    not obtain either.**
- **Complexity.** Not stated in any source I could read. Maley's testing is O(|F|² log |F|) (p. 40). Spoke cost in
  SURF is **unverified**.
- **Rubber-band data structure.** Each layer's RBS is built on an incrementally maintained constrained Delaunay
  triangulation (Dayan PDF p. 29, pr. 18). A later branch can be inserted between two earlier ones "without the need
  for explicit shoving" (PDF p. 32).

### 2.3 Staepelaere 1992: geometric transformation to octilinear

`docs/literature/staepelaere1992-ucsc-crl-92-39.pdf`.

- **Input.** An ERBS where "all of the width and spacing constraints have been met" (PDF p. 13).
- **Method.** Each layer is transformed separately (PDF p. 16, Fig. 2.1):
  - OrderSegments: a "precedes" DAG plus a topological sort, O(n log n) (PDF p. 29).
  - TransformLayer: a bottom-up enhanced plane sweep with a "previous boundary". Each band segment is pushed down
    into the lower triangle of its routing parallelogram (PDF pp. 18, 33–34).
  - StraightenLayer: a top-down sweep that removes jogs (PDF p. 16).
- **Octilinear cases (PDF p. 20).** Steep segments (O1) use vertical + 45°; shallow ones (O2) use horizontal + 45°.
  The length equals the octilinear distance, which is the minimum.
- **Does it preserve spacing?**
  - Each pushed segment "always maintains minimum design rules from the segments below it".
  - That room exists is "guaranteed by the spoke creation phase", via Kong's S-route argument (PDF pp. 18–19).
  - The ordering accounts for width and spacing by sliding one-sided endpoints and adding overlap segments at
    two-sided endpoints (PDF pp. 29–31).
  - Same-net segments that meet at a point need a "double-width previous boundary" (PDF pp. 44–46).
  - So the transform keeps spacing **only if the ERBS was built with spokes for the target geometry**. The
    rectilinear ERBS of the results was also used for the octilinear output (PDF p. 55), which is sound because
    rectilinear capacity ≤ octilinear capacity.
- **Cost.** O(n log n + m) for n band segments and m output segments; m is Θ(n²) in the worst case (PDF pp. 7, 43).
  - Measured jogs on an MCM layer: octilinear 357 initial → 322 after straightening (PDF p. 54).
- **Limits (stated by the author).**
  - Uniform width per segment.
  - **One spacing per layer**: variable spacing needs the resource "be a function solely of that segment"
    (PDF p. 62).
  - Endpoints larger than the wire (pads, vias) are future work, with "no conceptual difficulty" (PDF p. 61).
  - Recompute from scratch on any change: no localized update (PDF p. 63).

### 2.4 SURF: vias, layers, polygonal terminals

- **Layers.** Layer assignment first, then the problem splits into single-layer sub-problems ("bin-layers") that are
  routed independently (Dayan PDF p. 3 abstract; p. 31, pr. 20).
  - Conflict between nets is a continuous cost (estimated detour), not go/no-go (PDF p. 32).
- **Vias.**
  - Assignment chooses among 1–5 candidate via points spaced evenly along a 2-Net (PDF p. 59).
  - A force-driven post-processor then moves vias and Steiner points by the "tension" of the bands until no single
    relocation shortens the wiring (PDF p. 60, Fig. 31; Dayan & Dai 1991, `dayan-dai1991-ucsc-crl-91-40`).
  - After assignment, vias are ordinary **terminals** of each bin-layer, as are the bin-boundary crossings
    (PDF p. 100).
  - Our through vias are additionally *features* on every other layer (`pack.rs:49-52`).
- **Polygonal terminals.** Dayan's model has zero-width point terminals and branches. Obstacles and keep-outs "can be
  handled in a similar way" (PDF p. 21); the flow adds "the radius of the end-terminals" (PDF p. 25). Maley allows
  segment features, so polygons are modelled as segment sets (p. 29). Larger endpoints in the transformation are
  future work (Staepelaere PDF p. 61). **No source describes non-convex or rotated pad polygons in detail.**

### 2.5 Missing papers (please add to `docs/literature/` if you can get them)

| Citation | Why | Tried |
|---|---|---|
| C. E. Leiserson, F. M. Maley, "Algorithms for routing and testing routability of planar VLSI layouts", STOC 1985, pp. 69–78, doi 10.1145/22145.22153 | the original theorem and the spoke construction | ACM DL: 403 (Cloudflare); no free copy |
| F. M. Maley, *Single-Layer Wire Routing and Compaction*, MIT Press 1990 (book of the thesis) | ch. 6–8 (proofs, §6D "Cuts That Decide Routability") missing from the scan | archive.org `singlelayerwirer00fmil`: borrow only |
| F. M. Maley, "Testing homotopic routability under polygonal wiring rules", Algorithmica 15 (1996) 1–16, doi 10.1007/BF01942604 | **octilinear/polygonal norms explicitly** | Springer: login |
| W. W.-M. Dai, R. Kong, M. Sato, "Routability of a rubber-band sketch", DAC 1991, pp. 45–48 | **SURF spoke creation** | ACM: 403 |
| W. W.-M. Dai, T. Dayan, D. Staepelaere, "Topological routing in SURF: generating a rubber-band sketch", DAC 1991, pp. 39–44 (doi 10.1145/127601.127622) | RBS generation | ACM: 403 |
| R. Kong, "Incremental routability test for planar topological routing", MS thesis, UCSC, Dec. 1992 | **incremental cut test**; S-route proof the transform relies on | not searched online (UCSC tech report archive likely) |
| W. Dai, R. Kong, J. Jue, M. Sato, "Rubber band routing and dynamic data representation", ICCAD 1990, pp. 52–55 | dynamic RBS updates | not fetched |
| H. Chen, D. T. Lee, "A faster algorithm for rubber-band equivalents of planar graphs" | faster batch DRC than Maley (Su & Dai PDF p. 2) | not located |
| Gao, Jerrum, Kaufmann, Mehlhorn, Rülling, Storb, "On continuous homotopic one layer routing", SoCG 1988, pp. 392–402 | Euclidean (curvilinear) version, O(n³ log n) per Hama & Etoh | blocked |
| McMurchie & Ebeling 1995 (PathFinder) | the negotiated-congestion formula | already on the INDEX "no free URL" list |

Read from the scratchpad (not added to `docs/literature/`): the Maley 1987 thesis scan, Su & Dai ICCAD 1997, and
Duncan et al. "fat edges".

## 3. Mapping onto topor today

### 3.1 What already approximates the cut test and the inflation

| Concept | Where | How close to Maley / SURF |
|---|---|---|
| Critical cuts | `cuts.rs:1-22, 129-306` (LCT feet), inserted as CDT constraints in `board.rs:358-398` | the same definition (vertex → nearest point of another obstacle side). Kept: nearest per (vertex, owner) (`cuts.rs:151-184`), within `lct_mm = 3.0` (`board.rs:49`), only if it crosses no kept cut (`cuts.rs:279-293`) and the owners share a layer (`board.rs:380-381`) |
| Islands / empty cuts | `state.rs:250-251` (two vertices of one obstacle: no cut), `room_for` own-net ends (`state.rs:348-356`) | Maley's "ends on the same island" (p. 33) |
| Congestion (need) | `demand.rs:53-88` `need`: widths + max(s) between different nets + end clearances; own-net run at an end = half width | Maley's congestion with spacing folded in; see §1.2 for max vs additive |
| Capacity | `state.rs:247-263` `room[e][l]` = edge length; `pack.rs:570-614` margins by bisection against the real end polygon | Euclidean length; the margins use the true polygon distance, better than a point end |
| Prefix sums = spoke lengths | `pack.rs:617-651` `bounds`: `lmin[i]` from end 0, `rmax[i]` from end 1 | `lmin` is exactly the spoke length of pass i from end 0 along that edge |
| Per-layer evaluation | `pack.rs:113-136` `layer_items` (passes on layer l plus the via discs of other layers) | per layer, but on **projection** edges |
| Overflow price | `refine.rs:369-381`: `price · (1 + over/pitch)` when over > 0 | graded above 0, but a **step** of `price` at 0 |
| Negotiation | `core.rs:1077-1116` P4 rounds; `state.rs:529-531` `history` per projection edge; `init.rs:442` prices it | PathFinder-like |
| Octilinear room | `demand.rs:117-146` `octi_stretch`: factor ≤ 1/cos 22.5° when wrapped, elbow weighting when straight | a per-pass factor instead of the octagonal norm per cut |
| Wrap detection (corner stacks) | `pack.rs:515-536`: the taut path wraps an end vertex when the edge lies in the wrapped wedge | the geometric test the exact flow needs, done on the projection |
| Nested radii in drawing | `exact.rs:11-14`: "a wire drawn earlier is an obstacle … which is the cumulative-demand radius of the plan" | the ERBS radius, but reached **sequentially** |
| Octilinear drawing | `octi.rs:1-30`: per drawn run, node search over taut vertices + octagon vertices, greedy in line order (Bast, Brosi & Storandt 2020) | not Staepelaere; sequential |

### 3.2 What is missing or inconsistent

1. **Per-layer cuts are not projection edges.** The projection is one CDT over the obstacles of all layers
   (`state.rs:1-3`, `mod.rs`).
   - On layer l, an edge with an end on an obstacle absent from l has `room = INFINITY` (`state.rs:255-258`), and the
     packer's margin at that end is 0 (`pack.rs:579-580`).
   - The real layer-l cut, from the layer-l feature on one side to the next layer-l feature *behind* the other-layer
     pad, is never formed or checked.
   - qmtech has 814 pads touching F.Cu but only 243 touching B.Cu (§5), so B.Cu has many such false ends.
2. **The cut set is incomplete.** Dropped are:
   - the critical cuts that cross a kept cut (`cuts.rs:279-293`);
   - those longer than 3 mm;
   - all but the nearest per owner (Maley keeps one per *feature*, i.e. per side);
   - the cuts of features that share no layer, which is right for a projection but leaves no per-layer cut through
     the other layer's region.
   - The 1/sin factor (`pack.rs:216-226`, clamped to 2.5) partly compensates; Maley's theorem has no such factor.
3. **Flow is projection passes, not rubber-band crossings.** `edge_order` counts every pass of the projection path
   (`state.rs:524-525`).
   - A stretch on layer l that winds around another layer's vertex (or a free Steiner vertex, `board.rs:425-430`,
     `steiner_mm = 2.0`) crosses cuts it would not cross as a per-layer rubber band.
   - Maley's entanglement is the *minimum* crossing number (p. 32). The rubber band achieves it (inferred: geodesics
     in the universal cover of the plane minus the features meet a straight cut at most once per lift; Maley
     computes congestion from the RBE for this reason, p. 40).
4. **The taut path is per wire, not per layer.** `Packer::taut` takes the least margin over all layers
   (`pack.rs:402-413, 473-474`), so its crossing directions, wraps and oblique factors are a mix of layers.
5. **Vias are intervals.** The cut test needs fixed points. The planner chooses a triangle (`refine.rs` `via_k`); the
   drawing chooses the point (`draw.rs:1088-1110`), which can differ from the planner's.
6. **Step costs and hard gates against graded factors** (memories graded-factors-not-thresholds, no-monotone-gates):
   - `refine.rs:376-381`: a jump of `price` at over = 0.
   - `init.rs:478`: a flat `overflow_mm` when over.
   - `moves.rs:17-22, 155-166`: "refused whatever its price" when unsafe passes increase.
   - `moves.rs:26-29`: label moves need "no edge-layer they touch gets more over-full".
   - `core.rs:1087-1116`: P4 scoring is lexicographic on the *count* of over-full edge-layers. A measured reason is
     recorded at `core.rs:1088-1090`: one weighted sum routed fewer on bornhack.
7. **The drawing is sequential.**
   - Intervals are packed per edge independently (`draw.rs:924-1012`).
   - On an over-full edge the passes are spread evenly with zero-width intervals: "the checker will tell"
     (`draw.rs:991-998`).
   - The exact repair treats earlier wires as obstacles (`exact.rs:11-14`).
   - Never-drop then relaxes clearance on the exact path (`draw.rs:2181-2215`).
   - The order of wires decides who loses: Dayan's puzzle (PDF p. 20, Fig. 1).

**Evidence from the qmtech run** (`~/.cache/kicad/10.0/topor/qmtech-minimig/20261107-125211/result.json`,
`route.warnings`):

- 309 wires; the plan handed **18 over-full edge-layers and 7 vias without a site** to the drawing.
- "0 edge-layers planned to fit but over-full when drawn": planner and packer agree per edge.
- Yet 61 runs were rejected (packing 8, **repair 51**), with "infeasible against the static board alone 0".
- 28 wires were drawn by force: 14 on the exact path with relaxed clearance, 15 on a funnel, 3 on a normal attempt.

So the drawing failed where the per-edge model saw room. That is the signature of item 1–3 (missing cuts) or item 7
(sequential drawing); the U1 classification decides which.

- The SDRAM_A0 violations all carry "clearance gap −0.200 mm … against item:1431" (an overlap, i.e. a forced
  relaxed drawing), against A5, A4, A1 and a via of A2. The A0×A5 and A0×A1 crossings come from that forced run.
- Classes in `route.violations`: pad_exit 54, foreign_copper 30, static 3, via_site 2.

## 4. The algorithm

### 4.1 Data structures (per signal layer l)

```text
LayerSheet {                       // built once per run, patched per via
  l: usize,
  cdt: Cdt,                        // over F_l: pad polygons on l (grown by sag), drilled holes,
                                   // other nets' TH pads, keepouts on l, board edge,
                                   // planned via discs (all layers), terminal points
  vfeat: Vec<FeatureId>,           // vertex -> feature (island), or Terminal
  cuts: Vec<Cut>,                  // critical cuts of F_l, shadow-pruned (4.2)
  cut_grid: Grid,                  // spatial index over cut segments (reuse cuts.rs Grid)
  vcuts: Vec<Vec<CutId>>,          // per feature vertex: cuts that start there, by angle
}
Cut { p: VertId, q: CutEnd /*vertex or foot on a side*/, dir: Vec2,
      cap: f64,                    // ‖q−p‖ in the planning norm (Euclid | octagonal)
      bp: Bound, bq: Bound,        // demand.rs bounds (pad/edge/hole/open, clearance, net)
      regime: Option<f64> }        // courtyard rule (state.rs:368-391)
Stretch { wire, k0, k1,            // projection traversals of the run on l (refine::pass_layers)
          ta, tb: Term,            // pad exit point | via point | junction point
          w: Vec<f64>,             // per projection pass (necks), mapped to cut crossings
          taut: Vec<Vec2>,         // rubber band in the sheet (zero width)
          wraps: Vec<(VertId, Side, u8 /*depth*/)> }
CutFlow { items: Vec<(StretchId, f64 /*pos along cut*/, Kind /*open | wrap_p | wrap_q*/)>,
          need: f64, slack: f64 }  // per (cut): need via demand::need_in, slack = cap − need
```

Diff pairs (D7) are one `Stretch` with width 2w + gap and the pair's clearance; their pad ends are two terminals
merged into one envelope terminal (inferred; follows `docs/dev/kicad-diff-pairs.md`: KiCad checks no pair rule by
default).

### 4.2 The cut set

For each layer l:

1. **Critical cuts.** For every feature vertex p, and every other feature segment s visible from p, take the closest
   point q of s to p in the planning norm. Ties break by Euclid (Maley p. 40).
   - Reuse `cuts::lct_feet` with per-layer `shares` and **per side** (not per owner).
   - Remove the 3 mm cap and the "drop crossing cuts" rule (`cuts.rs:279-293`). Crossing cuts become extra cut
     records instead of CDT constraints; only the non-crossing ones are inserted as sheet edges.
2. **Vertex–vertex cuts** between feature vertices that see each other. With a polygonal norm the closest point is
   often a vertex.
3. **Shadow pruning** (Maley eq. 1-3, p. 55): drop pq when a feature point r satisfies
   ‖p−q‖ = ‖p−r‖ + ‖r−q‖ (± ε).
   - In the octagonal norm, r must lie in the octilinear routing parallelogram of p and q (Staepelaere PDF p. 20
     defines that parallelogram).
   - The query is "any feature vertex or side inside the parallelogram", answered by the grid.
   - In the Euclidean style there is almost no pruning; the U0 count decides whether a locality option is needed.
4. **Empty cuts** (same island, or no flow) are kept in the index but cost nothing.
5. **Optional locality** (`[capacity] cut_radius_mm`, default off). With it on, the report says "routability not
   proven" (Maley p. 53).

### 4.3 Flow bookkeeping and incremental update

Build, per layer and after P3 labels (and again after every P4 round):

1. **Stretches.** From the labels: `refine::pass_layers` (`refine.rs:66-91`) plus the via sites in `CapModel::via_k`
   with a concrete site point (the first feasible of `tri_site_points`, `pack.rs:37-43`, the same order as the
   drawing's `via_candidates`).
2. **Homotopy in the sheet.**
   - Map the stretch's projection traversals to a curve (the pass points, `state.rs:877`).
   - Walk it through the sheet to get the sequence of sheet edges crossed, and cancel back-and-forth crossings (the
     reduced / normal form).
   - Funnel it (`pack.rs:241-293`, with zero-width portals from vertex to vertex) to get the rubber band and its wraps.
   - O(length in sheet triangles).
3. **Order of stretches.**
   - Along a cut: by crossing position.
   - Coincident positions (several bands wrap the same vertex): by the projection's `edge_order` on the radial edges
     at that vertex, filtered to layer l. Planarity makes it consistent; no geometry is needed.
4. **Flow of cut c = (p, q):**
   - the stretches whose band crosses the open segment;
   - plus the stretches that wrap p with c's direction inside their wrapped wedge (the `pack.rs:527-535` test, on the
     sheet), innermost at p;
   - plus the same at q;
   - `need = demand::need_in(regime, items, bp, bq)` (`demand.rs:92`), with the per-pass widths (necks) and no oblique
     factor; `slack = cap − need`.

Incremental operations (U3):

| Operation | Work |
|---|---|
| insert stretch s on l | walk + funnel (O(m_s)); for each sheet edge it crosses that is a cut, and each extra cut it crosses (grid query along the band's segments), insert s into `items` at its position (O(load)); for each wrap at vertex v, add s to the cuts in `vcuts[v]` inside the wedge (O(deg_cut(v))); recompute `need` of the touched cuts |
| remove stretch s | the inverse; the touched cuts are remembered per stretch |
| relabel (flip, lift, via shift) | remove from the old layer, insert on the new layer(s); a via that moves is a feature change (next row) |
| via placed / moved / removed | insert/remove a sheet vertex in **all** layers (a through via is a feature everywhere). Recompute the bands of the stretches whose sheet triangles contain the point (local), and the cuts from the via to features within reach of the largest possible flow (grid). **Open point:** the CDT wrapper (`crate::cdt`, spade) may not support vertex removal with constraints; fallback is a local re-triangulation of the affected cavity, or batch rebuild per round |
| topological move (reroute, attach, merge) | the wire's projection path changes; recompute its stretches on every layer they use |

Other stretches' bands do not change when s is inserted: zero-width bands of a planar sketch depend only on their own
class and the features (inferred from the RBS definition, Dayan PDF p. 24; SURF inserts a branch "without the need
for explicit shoving", PDF p. 32). Only features (vias, junctions) change others.

### 4.4 The overflow price (graded) and where it enters

Let u = −slack / pitch, the overflow in pitches (u < 0 is slack), where pitch = mean (w + s) of the cut's items as
in `refine.rs:380`. Let τ = `[capacity] overflow_softness` (default 0.1 pitch). Then:

```text
φ(u) = overflow_mm · ( logistic(u/τ) + τ · ln(1 + e^{u/τ}) )
```

- For u ≫ τ this is `overflow_mm · (1 + u)`: today's `refine.rs` price, so the measured weights carry over.
- At u = 0 it is about 0.57 · `overflow_mm`.
- For u ≪ 0 it goes to 0 smoothly. A nearly full cut already carries a small social cost.
- No jump, no cutoff.
- Negotiation (PathFinder style; formula from memory, page unverified): cost_c = (1 + h_c) · φ(u_c). The history
  h_c is per (cut, layer), raised by the existing `history_fade` schedule when u_c > 0.

Where it enters:

| Phase | Today | New |
|---|---|---|
| P2 (all layers in one plane) | per-edge room sum over the allowed layers, flat `overflow_mm` (`init.rs:419-478`) | **unchanged** (cheap first guess, per the agreement). Optional later: smooth the step |
| P3 capacity refinement | `CapModel` per projection edge-layer (`refine.rs`, `layers.rs:22`) | `CutModel` with the same interface (`total`, `overfull`, move deltas) over per-layer cuts; φ instead of the step |
| P4 negotiated rounds | lexicographic (count over-full, objective) (`core.rs:1110-1116`) | Σφ in the objective, the count still reported. The earlier bornhack measurement (`core.rs:1088-1090`) is re-run and analysed per connection, not used as a gate (no-monotone-gates) |
| P4 label moves | kept only if price drops **and** no touched edge gets more over-full (`moves.rs:26-29`) | price only (Δφ + via price) |
| P4 topo moves | refused when unsafe passes increase (`moves.rs:155-166`) | Δφ of the cuts of the new stretches on their current layers, plus the existing P2 terms |
| reroute search cost in P4 | per projection edge (`init.rs`) | per projection edge, plus the current φ′ of the per-layer cuts that cover the edge on the stretch's layers (a lookup table refreshed per round) |

### 4.5 Drawing: spokes, then octilinear

Per layer, after the plan is final:

1. **Spoke radii.** For every cut c = (p, q) and every stretch s crossing it at order index i:
   - ρ_p(s, c) = c_p + Σ_{j<i} (w_j + g_{j,j+1}) + w_s/2;
   - ρ_q(s, c) the same from q;
   - these are exactly `lmin` / `len − rmax` of `pack.rs:644-649`, but on critical cuts.
   - No overflow ⇒ ρ_p + ρ_q ≤ cap for every s on c.
   - Spokes also go in the 8 octilinear directions of every vertex with a non-empty stack, as SURF does (Dayan
     PDF p. 32), with lengths from the ray to the nearest feature.
2. **ERBS per stretch.**
   - The shortest path in the stretch's class that passes every crossed cut beyond both spoke tips, and goes around
     every wrapped vertex outside its spoke tips.
   - Concretely: the funnel of `pack.rs:241` with the portal of cut c = [ρ_p(s, c), cap − ρ_q(s, c)], in the order
     the band crosses the cuts. The wrapped vertices become arc samples at radius ρ / cos(step/2), the circumscribed
     flattening of `exact.rs:8-10`.
   - All radii come from the plan, so **every stretch is drawn independently of the others** (no order, no puzzle).
   - It can run in parallel, within the CPU cap.
3. **Checker.** Every drawn run is still checked (`draw.rs` `Checker`). The ERBS construction above is my reading of
   the spoke idea; its equivalence to Maley's ideal realisation is **unproven here**, since the papers that define
   SURF's spokes were not obtained (§2.5). The boardlets (§6.2) test it.
4. **45° (default style).** Two options, decided by U5's measurement:
   - (a) Staepelaere's transform (§2.3) on the ERBS. It needs a uniform per-layer spacing (use the conservative
     per-element territory) and the octilinear capacities in the plan (the octagonal norm, §1.1).
   - (b) Keep `octi.rs` but feed it the ERBS and the spoke radii as its octagon nodes, instead of the sequential
     "copper drawn so far" (`octi.rs:21-23`).
   - With either, plan in the octagonal norm so that whatever fits in the plan fits at 45°.
5. **The existing exact path** (`exact.rs`) stays as the fallback for runs the checker rejects. These are logged as
   "drawing loss" (§6.1).

### 4.6 Never drop, necking, pad exits, vias

- **Never drop (M-B1).** An overflowed cut has deficit δ = −slack > 0, known in the plan.
  - The least-violating realisation spreads δ over the cut's gaps: each different-net gap shrinks by δ / n_gaps,
    after necks (next item).
  - The ERBS uses the shrunk radii, so the violation is drawn where the plan said and priced by the `violation`
    factor before drawing.
  - Instead of "spread evenly over the widest span; the checker will tell" (`draw.rs:991-998`).
- **Necking** (memories neck-from-pad-rule, route-order-and-neck-rule). A cut that overflows at nominal widths but not
  at floor widths is resolved by necking first:
  - the stretches crossing it, with the graded neck price;
  - widest slack per neck first;
  - from the nearest pad end back to the T split, never narrow-wide-narrow;
  - width ladder: pad width, then default, then widest that fits plus a warning (route-order-and-neck-rule).
  - `State::refit_wire` (`state.rs:629-715`) does this per projection edge today. The per-layer cut flow replaces its
    "bottleneck" test.
- **Pad exits.**
  - The terminal point at the pad's planned exit is a feature-terminal of the sheet (§1.2).
  - The exit cuts (own pad vertex → neighbour pad side) are critical cuts. Their flow is the half-width own run
    (`demand.rs:55-60`) plus anything passing between.
  - So the 54 qmtech pad-exit violations become planned, priced cuts. U1 tells how many were planned over-full and how
    many were drawing losses.
  - Exit direction (the `fx_fine_pitch_exit` 45° case) follows from the octagonal norm.
- **Vias.** Through vias are features on all layers and terminals on their two layers. The via disc plus clearance is
  the feature's width (Maley p. 29 allows a terminal width). "Via squeeze" (`pack.rs:204`) becomes the slack of the
  via's cuts.

## 5. Complexity with real sizes

Sizes from the input dumps (`scratchpad/mb1/in-{dcs,qmtech}/model-1.json`) and the qmtech/dcs run logs.

| | dcs | qmtech |
|---|---|---|
| signal layers routed | 2 (F, B) | 2 (F, B); In1/In2 are planes |
| pads (TH) | 458 (25) | 875 (191) |
| features touching F.Cu / B.Cu | 145 / 328 | 814 / 243 |
| polygon vertices F / B (estimate: rect 4, roundrect 20, circle 16, oval 18) | ~2.8 k / ~4.8 k | ~12.5 k / ~4.1 k |
| projection triangles | 39,230 (dcs-r8) | 39,719 |
| projection edges carrying passes | 6,788 | 8,766 |
| wires in P2 | 185 | 309 |
| P3 vias | 144 | 126 |
| P4 negotiated rounds / wires ripped | 6 / 209 | 6 / 143 |

Per-layer estimates (unverified, to be measured in U0):

- **Sheet size.** V_l ≈ the vertices above plus terminals and vias. Build O(V log V): at most about 15 k vertices, a
  few ms.
- **Critical cuts before pruning.** About V_l × (feature sides within sight). Assuming 5–10 per vertex: qmtech F
  ~60–120 k, B ~20–40 k.
- **After octagonal shadow pruning:** Maley reports O(|F| log |F|) in typical rectilinear sketches (p. 55). That is
  ~10^5 on qmtech F at most; nonempty ones are a fraction.
- **Nonempty cuts.** Each wire crosses a cut about every 0.3–0.5 mm in pin fields. With 2.8 m of F.Cu copper in
  qmtech (`length_by_layer_nm`), that is ~10^4 cut crossings, the same order as today's 8,766 loaded projection
  edges.

Cost per operation:

| Operation | Today | New (estimate) |
|---|---|---|
| batch model build per P4 round | `CapModel::new`: O(passes · margin lookups) | sheets (static, once) + bands O(Σ stretch length) + flows O(Σ crossings): ~10^5 ops per layer, about 10–50 ms (unverified) |
| P3/P4 label move | O(passes on the touched edges), margins cached (`refine.rs` header) | walk + funnel O(m_s ≈ 30–60 sheet triangles) + grid queries + need of ~30–60 cuts at load ~3: **about 2–5× today** (unverified) |
| topo move | one capped A* (`max_expansions`) + pricing | unchanged search; +1 band per stretch, as above |
| drawing | exact A* per run window (≤ 1200 nodes, `exact.rs:31`); 479 exact runs + 61 rejected on qmtech | one funnel per stretch through spoke tips, O(m_s): expected much cheaper; the exact path only for rejects |
| octilinear transform (option a) | `octi.rs` node DP per run | O(n log n + m) per layer (Staepelaere PDF p. 43) |

The incremental mode (U3) is needed for moves; the batch mode suffices for U1 and for P4 rounds. CPU cap: the sheets
of different layers and the per-stretch ERBS are independent and can run in parallel within `RAYON_NUM_THREADS`
(memory cpu-cap-10-threads).

## 6. Verification plan

### 6.1 First: M-A per-violation classification on the qmtech run

Input: the run above (89 violations; `result.json` `route.violations` and `drc.json`). Steps:

1. Rebuild the plan as handed to the drawing: P4 final labels and planned via sites. This needs an engine dump: a
   debug env or `[debug] cut_report`, writing per layer the cuts with cap, need, slack and items.
2. For every violation (layer, `at`, wire, against):
   - **planner: over-full cut.** A cut on that layer within 2 pitches of `at`, crossed or wrapped by the wire, has
     slack < 0.
     - sub-class *static*: it overflows with the wire alone;
     - sub-class *neck not planned*: it fits at floor widths.
   - **planner: via site.** The violation is a via without a site (already classed `via_site`).
   - **drawing loss.** No over-full cut nearby. Sub-classes by the forced kind (relaxed exact / funnel / attempt) and
     by `class` = pad_exit.
   - **model gap.** The per-edge packer said "fits" while the cut test says over-full (counts the effect of §3.2
     items 1–3).
3. Output: a table per class for qmtech, then dcs, compared with today's `class` field.

Expectation, a hypothesis to falsify: the SDRAM_A0 group, which was forced relaxed with −0.2 mm, is either a model
gap (a per-layer cut missing near IC1) or a drawing loss.

### 6.2 Synthetic boardlets

`harness/mechfix.py` style (memory synthetic-test-layouts), 2 layers, routed end to end with KiCad DRC.

| ID | Board | Expect |
|---|---|---|
| B1 corner bundle | k = 4 wires wrap one pad corner, then pass a second pad. Gap at exactly the need (variant a) and 1 µm less (variant b) | a: no over-full cut; ERBS drawn DRC-clean at pitch. b: exactly one over-full cut, δ = 1 µm, drawn as one priced violation where the plan says. Today's `fx_corner_wrap` is the start |
| B2 non-neighbour squeeze | pads A, C on one line, L = 2 mm apart; small pads B, D at (L/2, ±0.7 mm), so BD (1.4 mm) is shorter than AC, and LCT keeps BD and drops AC as crossing (`cuts.rs:279-293`); vertical wires: some between A and the B–D column, some between the column and C, sized so AB, AD, BC, CD fit but AC overflows. Inferred condition: \|AB\| + \|BC\| − L > 2·c_B, i.e. h ≳ √(c_B·L) | per-edge packer: fits; cut test: AC overflows; P4 relabels or reroutes one wire; 0 violations. Missed today by construction |
| B3 must reroute | a gap of capacity 2 wires on the only allowed layer (SMD both ends), 3 wires planned through, a 5 mm longer alternative | P4 moves one wire around; 0 violations; detour ratio of the moved wire reported |
| B4 45° bundle bend | 4 wires bending 45° around a pad corner (extends `fx_octi_bundle`) | octagonal capacity used; pitch kept through the bend; no any-angle fallback |
| B5 other-layer pad | two F.Cu pads 1.0 mm apart with a B.Cu-only pad between them, 3 F wires through | today's projection sees two cuts each ending at the B pad (room INF); the sheet sees one cut F–F and reports the true slack |
| B6 SDRAM_A0 crop | qmtech region x 158–173, y 66–84 mm (IC1, U4 pins, SDRAM_A0..A5), extracted as a fixture | no same-layer crossing; the classification of §6.1 reproduced; 0 forced relaxed runs |
| B7 fine-pitch exit | `fx_fine_pitch_exit` (0.5 mm pitch) | the exit cuts are critical cuts; graded neck or planned violation, no drawing loss |

### 6.3 What the scorecard should show

Only via `harness/score_runs.py` (memory time-limit-fixed). Per board:

- **Violation classes** (§6.1): planner over-full / static / neck not planned / via site / drawing loss / model gap.
  Target: drawing loss → near 0; model gap → 0.
- **Planned over-full cuts** before drawing vs **over-full when drawn**; per-edge vs per-layer-cut counts side by side
  during the transition.
- **Forced runs** (relaxed exact / funnel / attempt), from the existing never-drop line.
- KiCad new DRC, routed, violating connections, **detour ratio** (memory no-detours), vias, runtime (cpu clock).
- **Per-connection loss analysis** for any connection that routes worse, not a monotone gate (memory no-monotone-gates).

## 7. Staging

The implementation starts **after D7 lands**. D7 changes `demand.rs` and `draw.rs` on `d7-diffpairs`, which has no
commits yet relative to `topor-topo`. Each unit is one agent, with an Opus check of its gate.

| Unit | Content | Gate |
|---|---|---|
| U0 sheets | `LayerSheet` per signal layer (per-layer features, `sag` growth, via discs, terminals); per-layer critical cuts (per side, no 3 mm cap, crossing cuts as records); octagonal norm + shadow pruning; unit tests on tiny geometries | tests; counts of cuts (raw / pruned / nonempty) on the ladder boards logged; build time < 5 % of P2 on qmtech; routing output byte-identical |
| U1 measure (M-A) | per-layer bands of the final plan, cut flows, `cut_report` dump, Python classification joined with `result.json` | classification table for qmtech's 89 and dcs; B1, B2, B5 detect what §6.2 says; routing output byte-identical |
| U2 price in P3 | `CutModel` with φ replacing `CapModel` in P3 (batch per call), octagonal capacity for the octilinear style, oblique factor dropped on critical cuts | ladder eprom → wiccon → dcs → qmtech A/B (score_runs); per-connection analysis; planned over-full cuts down |
| U3 incremental + P4 | incremental insert/remove/relabel/via; P4 rounds, label moves and topo moves priced by Δφ; hard gates (`moves.rs:17-29`) replaced by prices; history per (cut, layer) | per-move cost measured (target ≤ 5× today); ladder A/B; B3 passes |
| U4 ERBS drawing | spoke radii from the plan; independent funnel per stretch; exact path only as fallback; never-drop deficit spreading | B1, B2, B6 DRC-clean (or the planned violation only); qmtech drawing-loss class → near 0; forced runs down |
| U5 octilinear | Staepelaere transform (option a) or `octi.rs` on the ERBS (option b), chosen by a measured comparison on B4 and qmtech | `fx_octi_*` pass; `any_angle` count not worse per connection; DRC-clean octilinear bundles |
| U6 necks and exits | per-layer cut flow drives `refit_wire` (neck to T split, width ladder); pad-exit terminal points | B7; qmtech pad_exit class |

Re-run the lower ladder rungs per fix (memory board-ladder-testing).

## 8. Risks and open points

1. **The plan says the opposite in two places** (`/home/paul/.claude/plans/wise-hatching-bunny.md`):
   - "Mathematical basis (measuring tool, not the engine): buildable ⇔ every cut safe (Leiserson–Maley, Cole–Siegel)".
   - §"Removed": "Passage prices as the coordination engine (PathFinder, overflow price …) … Cut/LCT/layer-combination
     checks only measure."
   - This design makes the cut test the engine's authority with a negotiated overflow price, as the user agreed on
     2026-11-07. The coordinator should update those lines.
   - The code already negotiates (`core.rs:1077`), so the plan was out of date before this design.
2. **The plan calls the per-edge check "exact per layer"** (§"Capacity for N layers": "After P3, exact per layer:
   `Σ (w+s) of the wires labelled ℓ through e ≤ room[ℓ](e)`").
   - It is not exact, for the reasons in §3.2 items 1–3.
   - The plan's "room[ℓ] … only obstacles present on ℓ count" is also not what `state.rs:247-263` does: it gives INF,
     not the free length to the next layer-ℓ obstacle.
3. **Plan, user decisions, octilinear:** "clearance metric octagon when drawing (the fit check uses the circle)".
   - With Staepelaere and Maley, the fit check for the octilinear style should use the **octagonal norm**, or 45°
     drawings will not fit where the plan says (up to 8.2 %).
   - M-45 already moved this way with `octi_stretch` ("decide early").
   - TopoR itself draws any-angle in automatic mode (plan, M-45 note, TopoR 6.3 manual p. 238). So 45° is our choice,
     and its capacity cost belongs in the plan.
4. **Lexicographic P4 scoring.** `core.rs:1088-1090` records a measurement that a weighted sum routed fewer on
   bornhack. U3 must re-measure with φ and analyse per connection. If the lexicographic order stays, it should at
   least use Σφ rather than a count.
5. **Proof gap.** The sufficiency proofs (Maley ch. 6–8), the SURF spoke algorithm (Dai, Kong & Sato 1991) and Kong's
   S-route proof are not available (§2.5). Necessity is safe; sufficiency for our construction rests on the checker
   and the boardlets until the papers are read.
6. **Mixed clearances.** KiCad's max(s_i, s_j) and same-net zero gap are outside Maley's additive model and
   Staepelaere's "one spacing per layer".
   - Exact for single-clearance layers.
   - Conservative per-element territories elsewhere, which costs capacity near high-clearance nets (e.g. the 0.455 mm
     3V3 / U9 rule in qmtech).
   - An option `[capacity] mixed_clearance = conservative | pairwise` with conservative as the default (memory
     configurable-sane-defaults).
7. **Cut count.** If unshadowed critical cuts per layer reach ~10^5 and moves get slow, the locality option is the
   escape. It must stay an option and be reported (Maley p. 53).
8. **Vias move.** The test needs fixed via points, but P3/P4 move via intervals. Batch rebuild per round is simple;
   incremental via moves need sheet vertex insert/remove (spade support unverified).
9. **Free Steiner vertices** (`board.rs:425-430`) exist only for the projection. The sheets must not include them,
   or bands would wrap non-features.
10. **D7 interaction.** Diff pairs as one wide stretch need a merged envelope terminal at the pad ends where the pair
    splits. That is decided by D7's coupling model (`docs/dev/kicad-diff-pairs.md` notes local uncoupling is allowed).
11. **Unlayerable crossings** (M-B2) make a layer non-planar. The theorem says nothing there; keep pricing them as
    today and leave their neighbourhood out of the "proven routable" claim.
12. **Diagonal / breakpoint cuts.** In the octagonal norm, the cuts at the norm's breakpoint directions
    (22.5° + k·45°) cannot be shadowed and must always be kept. This is the analogue of Maley's diagonal cuts for L∞
    (p. 55), inferred, not stated by him for octagons.
