# M3 design: radii, spokes, octilinear (shopping list 6.2 + Q1 + Q2)

Branch `radii-m3` from main 69e389d, 2026-11-13. Phase 1 (design). The implementation starts with step a.
Code references are to main 69e389d. Literature pages are PDF pages, with the printed page where it differs. Dayan
1997: PDF = printed + 11. Russian journals: the journal page. HTML sources: the section heading.

## 0. Summary

- **Goal.** The drawing realises the plan's topology. Every stretch (one wire on one layer) is drawn on its own,
  from the plan's radii, as an extended rubber band (ERBS). It is not a sequential search that treats the copper drawn
  so far as obstacles. The draw order then no longer decides who loses (Dayan's "puzzle", Dayan PDF 20, Fig. 1).
- **Input.** Most of the input exists already, as measurement or as the planner's price:
  - per-layer sheets with critical cuts (`sheet.rs`, U0);
  - per-layer stretches pulled taut in the sheet, with their wraps (`cutflow.rs::band`, U1);
  - the items of every cut (`cutflow.rs::flow_items_into`, `cutmodel.rs::CutPrice`).
  The drawing does not read any of it. It packs projection edges instead (`draw.rs:1007`).
- **Three missing pieces.** One per sub-step.
  - **a. Attachment stacks.** The inside-out order of the bands that wrap a vertex, and with it one consistent
    topological order of the items on every cut. Today wraps tie at `t = 0` and are ordered by stretch id
    (`cutflow.rs:21`, `cutmodel.rs:296`). Bands that coincide tie too.
  - **b. Nested radii.** Per cut, the spoke tips of every item from both ends: the prefix sums of widths and
    clearances, including distant features (the cut set has no distance cap). Per wrapped vertex, the stack radius.
  - **c. ERBS + octilinear.**
    - The ERBS: per stretch, the shortest path in its sleeve that keeps out of its own spokes and stack arcs.
      Every stretch is drawn independently.
    - Octilinear: `octi.rs`, fed with the ERBS and run in Staepelaere's precedes order. Staepelaere's own sweep is
      the alternative. Measurement decides.
- **Behind an option.** All of it sits behind `[drawing] realise = "erbs" | "sequential"`. The default stays
  `"sequential"` until the ladder has been measured; the user decides the flip. Nothing is removed (inventory §6
  is for M6).

## 1. Literature

- **ERBS, spokes (Dayan 1997, `dayan1997-rubberband-topological-router-thesis.pdf`).**
  - PDF 26 (pr. 15), Fig. 6: spokes turn the RBS into the ERBS, whose terminals are a superset of the RBS's. "The
    branch paths in the ERBS satisfy the spacing requirements if, and only if, the topological class represented by
    the RBS is routable." The ERBS is "a minimal length member of the topological class". It is "the final
    geometrical embedding" for Euclidean routing; octilinear routing needs the "geometrical transformation [62]"
    (Staepelaere). The spoke test is constructive; the flow/capacity test "merely indicates if such a sketch exists".
  - PDF 32 (pr. 21): a later branch is "inserted between two previous branches without the need for explicit
    shoving". Spoke creation "pushes wires away from fixed objects with open-ended line segments called spokes".
    Rectilinear uses up to 4 spokes per point, octilinear up to 8.
  - PDF 33 (pr. 22), Fig. 11: for any-angle routing, SURF uses the octilinear spokes "as an approximation to the arc".
    The ERBS converts to octilinear "with two plane sweeps".
  - PDF 125 (pr. 114), Fig. 59: per terminal, "the incident local nets are stored in a cyclic ordered list and the
    list of attached local nets are kept in a linear list in an inside-out order, starting from the inner-most arc".
    A terminal with only implicit attachments can have two such lists, one per side. A terminal with an explicit
    attachment has one. This is Q1.
  - PDF 126 (pr. 115), Fig. 60: the attached nets' arcs have radii ε, 2ε, … in stack order. In the ε-sketch the
    stack is the nesting.
  - PDF 152 (pr. 141), Fig. 79: the "Triangle problem". Sequential shortest-path routing cannot reach the optimal
    embedding. ROAR picks a terminal with an attached net and re-routes the inner-most attachment first.
- **Distant pads, radius wave (Popov & Popov 2012, `popov2012-min-length-wire-path.pdf`).**
  - p. 118: in the topological approach "the path (shape) of a wire may be influenced by an element at an arbitrary
    distance from it".
  - p. 118: the radius from pad C for a wire w = C's radius + the widths of the wires between w and C + w's half
    width + the clearances.
  - p. 119: FreeStyle Router and TopoR up to 5.2 "did not account for the influence of pads located at a sufficient
    distance from the wire. Ignoring any factor leads to configurations with design-rule violations."
  - p. 119: non-round objects become several round primitives at the polygon vertices, plus the distance to the edge
    between two wrapped primitives of one object (no "sagging").
  - p. 119, Fig. 3: the path is a funnel of tangents to the ordered left and right circles.
  - pp. 120–121, Figs. 4–6: circles are built in two stages.
    1. Per pad, a wave through the triangles. The sector between the tangents to the neighbouring pads narrows.
       Radii grow by each wire crossed, in topological order. Where the last circle crosses the far edge b, the wave
       continues through the next triangle. It stops when no width or gap can grow the radius any more.
    2. Per wire segment, the circles of stage 1 are taken with the radius cut back so that this segment is the last
       one counted.
  - p. 121: O(B·F²·log(B·F)) time. Used in TopoR.
  - This is Q2.
- **Octilinear transform (Staepelaere 1992, `staepelaere1992-ucsc-crl-92-39.pdf`).**
  - PDF 13 (pr. 5): the input is an ERBS where "all of the spatial constraints have been met". The output conforms to
    the geometry, is topologically equivalent, meets width/spacing and has minimum wire length.
  - PDF 20 (pr. 12): the octilinear cases O1/O2. A segment's transform lies in its "octagonal parallelogram".
  - PDF 62 (pr. 54) §7.3: one spacing per layer. Variable spacing breaks OrderSegments, which needs the resource of a
    segment to be "a function solely of that segment".
  - Further limits are in `docs/dev/inflated-sketch.md` §2.3:
    - room for the transform is "guaranteed by the spoke creation phase" (PDF 18–19);
    - endpoints larger than the wire are future work (PDF 61);
    - there is no local update (PDF 63).
- **SURF post-route (Su & Dai 1997, `su-dai1997-iccad-rubberband-post-route.pdf`), PDF 3.**
  - Spoke creation is "an incremental layer based operation". Each terminal is processed in turn: first the cuts to
    nearby terminals (flow ≤ capacity), then the spokes, which take terminal geometry, spacing and branch width into
    account. "Once the required spokes have been created for each terminal, we have an ERBS", "proven to be routable
    … by Maley and guarantees a legal geometric transformation".
  - Even wire distribution spreads wires across a cut up to a maximal spacing. This is TopoR's nominal clearance; see
    the manual below.
- **TopoR inside (Polubasov 2010, `polubasov2010-topor-inside.html`)**, intro before «Начальная трассировка»: routing
  and metrisation are separate stages. "Only the topological paths … are determined; the exact geometric shape … and
  the exact via positions are computed later, at the metric stage." Its description "is beyond the scope of the
  article". The result is any-angle segments, and arcs as an option.
- **Flexibility (Luzin, Popov & Popov 2013, `luzin2013-flexible-any-direction.pdf`).**
  - p. 99 «Гибкость»: in sequential routers, a laid wire's shape is fixed and later wires go around it. In flexible
    routing "only the relative placement of wires is fixed". Vias are moved to optimal positions, and "the shape of
    the wires is computed automatically after routing".
  - pp. 97–98: the benefits of any-angle routing (Fig. 6).
- **TopoR 6.3 manual (`eremex2017-topor63-user-manual.pdf`).**
  - p. 83: "Calculate wire shapes" (F5) has two submodes: with arcs ("smooth paths around obstacles while observing
    the required clearance … straight lines from bend to bend") or straight lines only. **TopoR has no 45° mode**:
    our octilinear default is our own choice, made for KiCad.
  - p. 105: violations of manual edits "are eliminated when the wire shape is calculated".
  - p. 182 and p. 240: "imprecise detection of the optimal wire shape" on dense boards. The advice is a nominal
    clearance above the minimum.
  - p. 237: "The nominal clearance is used wherever wires can be set apart."
  - p. 239: FreeStyle recalculates shapes but never changes the topological path ("a wire will never jump over a pad
    to go around it on the opposite side").
- **Our own background.** `docs/dev/inflated-sketch.md` §2.2 (spoke creation, SURF sources not obtained), §4.5
  (drawing: spokes then octilinear) and §4.6 (never drop, necks, pad exits). This design implements §4.5–4.6 and
  corrects them in §4.
- **P6 evidence.** Plan `there-is-the-topor-agile-forest.md`. Bast, Brosi & Storandt 2020 §4.3, Fig. 8 p. 7: "keep
  room for lines routed later". Dayan PDF 26–27 (pr. 15–16), Fig. 6: spacing reserved for every wire.

## 2. How the drawing realises the plan today (code survey, read only)

### 2.1 Plan data that reaches the drawing

| Plan datum | Where it enters the drawing |
|---|---|
| Layer runs and via intervals per wire (P3 events), planned via traversal `at` | `draw::Plan` (`draw.rs:39-47`, `plan()` `:51`) |
| Pass order per projection edge (`st.edge_order`) | `Drawer::pack` (`draw.rs:1007-1101`) through `Packer::bounds` (`pack.rs` §bounds, `lmin`/`rmax` prefix sums) |
| Pass widths (plan necks, `CutPrice::necks` → `st.pass_w`) | `pack` (`draw.rs:1020`), `plan_neck` (`draw.rs:3153`) |
| Taut directions per pass (`Packer::taut`) | `Drawer::dirs` (`draw.rs:650`), oblique factor; `neighbour_bound` (`draw.rs:1150-1171`) |
| Planned via sites (5.4 `relocate`), junction moves (`branch_relocate`) | `via_site`, `split_at` (`draw.rs:695-711`), `core.rs:3609-3645` |
| Planned room of undrawn neighbours (P6) | octi lane gates and zones as a price (`octi.rs:242-330`, `planned_room`) |

**What does not reach it.** The cut model, which has been the planner's fit authority since M0 (`[capacity] model =
"cuts"`), never reaches the drawing:

- the per-layer sheets and critical cuts (`sheet.rs`);
- the per-layer bands and their wraps (`cutflow.rs:425-495`);
- the ordered items of every cut (`cutmodel.rs:181-198`, `item_order` `:296`).

There are no per-vertex radii and no attachment stacks anywhere. Several wraps at one vertex are ordered "by stretch
id" (`cutflow.rs:18-21`). In the planner, ties are broken by wire id (`cutmodel.rs:296`).

### 2.2 Where the drawing departs from the plan

1. **Projection edges, not cuts.** Intervals are packed per projection edge and layer (`draw.rs:1007-1101`). An
   edge whose end lies on another layer's pad has no margin there. Non-adjacent cuts (Maley; fit pattern
   `model_gap_cut`) are never packed. This is A8 "approx." in the abstract-reps report.
2. **Over-full edge: "spread evenly over the widest span; the checker will tell"** (`draw.rs:1074-1084`). The plan's
   deficit is not drawn where the plan priced it.
3. **Sequential with copper as obstacles.**
   - The module doc says so (`draw.rs:5-8`), and so does `exact.rs:12-15`: "a wire drawn earlier is an obstacle …
     which is the cumulative-demand radius of the plan".
   - The draw order is width, then length (`core.rs:3647-3653`).
   - The wrap nesting is reached only when the inner wire happens to be drawn first.
4. **`tight_span` is bounded by drawn neighbours only** (`draw.rs:1103-1124`, used by `octi.rs:268`).
   - `octi.rs` walls a run only at triangle edges. The planned room inside a triangle is a price (`octi.rs:242-330`).
   - This is the P6 mechanism (wiccon 3.3v).
5. **An attempt ladder per run** (`draw.rs:2497-2560`):
   - 0: the exact taut path against the drawn copper;
   - 1: with the plan necks;
   - 2: with the drawing's own pad necks;
   - 3–5: funnel + repair.
   An octilinear redraw follows each accepted run (`draw.rs:2572-2620`), then `pull_taut` (`draw.rs:2624`, `:2953`).
6. **Never drop by relaxing the clearance** on the exact path, or with a funnel before its repair (`draw.rs:2687-2800`).
7. **Feedback inside one drawing.** Blockers are ripped and redrawn around a rejected wire (`core.rs:3898-4040`,
   `exact.rs:595` `blockers`). Partners' chords are moved after the vias are placed (`follow_vias`, `draw.rs:1838`).
8. **Vias are placed per wire, in draw order**, each one an obstacle for the next (`core.rs:3727-3800`,
   `draw.rs:1491`). Planned sites are used when they are legal (5.4).
9. **After the drawing:**
   - via reduction (`core.rs:4323`), fuse (`:4355-4442`), octilinear polish (`:4451`), pour stitching and tidy (KiCad
     Cleanup spec, `tidy.rs:1-30`);
   - these are geometric post-steps that M6 keeps (shopping list M6).

## 3. Data structures (new module `topo/radii.rs`, plus `topo/erbs.rs` for step c)

All structures are per signal layer `l`. They are built once per drawing, from the picked plan (M2) and the placed
vias.

```rust
/// The drawing's layer: the sheet (U0) with the placed vias and junctions as features, and the stretches pulled taut.
pub struct DrawLayer {
    pub sheet: LayerSheet,          // sheet.rs, with FeatKind::Via(i) at the drawing's via points
    pub stretches: Vec<usize>,      // indices into DrawSketch::stretches
    pub cuts: Vec<CutItems>,        // per sheet cut that has items (index = cut id via `by_cut`)
    pub stacks: Vec<Stack>,         // attachment stacks (step a)
}

/// The items of one cut in topological order from `p` to `q` (step a), with their spoke tips (step b).
pub struct CutItems {
    pub cut: usize,
    pub items: Vec<CutItem>,        // sorted by `key` (curve order), not by band t
    pub cap: f64,                   // planning norm (sheet.rs Norm), nm
    pub need: f64, pub slack: f64,  // as cutflow::needs (pairwise / conservative by [capacity] mixed_clearance)
    pub deficit: Option<Deficit>,   // slack < 0: how the drawing spreads it (step b, never drop)
}
pub struct CutItem {
    pub stretch: usize,
    pub kind: u8,                   // 0 crosses, 1 wraps p, 2 wraps q (cutflow::FlowItem)
    pub t_band: f64,                // the band's crossing (0 / 1 for wraps)
    pub key: f64,                   // step a: where the stretch's *curve* crosses the cut (0..1 from p)
    pub w: f64, pub s: f64, pub net: i32,
    pub tip_p: f64, pub tip_q: f64, // step b: spoke tips (centre line), nm from p and from q, in the cut's norm
}

/// Attachment stack (Dayan PDF 125): the bands wrapping sheet vertex `v` on one side, inner-most first.
pub struct Stack {
    pub v: usize,
    pub left: bool,                 // band passes right of v (v on the band's left), as cutflow wraps
    pub order: Vec<usize>,          // stretch ids, inside-out
    pub radius: Vec<f64>,           // step b: centre-line radius of each, nm (feature bound + inner widths + gaps)
}

/// The spoke set of one stretch: what its ERBS must keep out of (step c input).
pub struct Spokes {
    pub stretch: usize,
    pub portals: Vec<Portal>,       // per crossed / wrapped cut, in band order: [tip_p, cap - tip_q] on the cut
    pub arcs: Vec<(usize, f64, bool)>, // wrapped vertex, stack radius, side
}
```

- **Radii per obstacle and layer.**
  - `Stack.radius` is the per-vertex radius of each wrapping stretch: the Dayan / Popov circle.
  - `CutItem.tip_p` / `tip_q` are the per-direction radii (spokes) along every critical cut from a feature vertex,
    distant ones included.
  - A polygon pad is a set of vertices. The sheet's feet cuts (vertex to the interior of a side) give the "no sagging
    to the edge between primitives" of Popov p. 119.
- **Spokes.** One per (cut, item, end). Its tip is the nearest point the item's centre line may come to that end
  along the cut. These are SURF's spokes (Dayan PDF 32), but per item, so that every stretch can be drawn alone.

## 4. Algorithm

### 4.1 Step a: attachment stacks and the topological order of cut items (Q1)

1. **Build the drawing's sketch.**
   - As `cutflow::measure` does, but the via features are at the drawing's placed via points, not the static
     triangle sites.
   - Per layer: the sheet (`SheetInput` with `ViaFeat`, `JunctionFeat`), the stretches (`wire_stretches` with the
     placed points), the bands (`cutflow::band`), the cut items (`flow_items_into`).
   - Cost: U0/U1. cutflow already measures this on every run (`Recorder::final_plan`), so the numbers are known per
     ladder board.
2. **Curve key.** For every item (stretch `s`, cut `c = pq`): where `s`'s projection curve crosses segment `pq`.
   - The curve goes through the plan's pass points, which `State::pass_point` (`state.rs:1177`) spreads evenly in
     `edge_order` along every projection edge. So it is a representative of the plan's topology, not of a geometry.
   - Crossing items: the crossing nearest to the band's crossing `t_band`. Wraps at `p` (`q`): the crossing nearest
     to `p` (`q`).
   - Non-crossing curves keep their order along any cut between two features. So `key` orders the items
     topologically. Where the bands do not tie, it agrees with `t_band`.
   - Where bands coincide (nested wraps, parallel bundles), `key` breaks the tie by the topology, not by id.
   - Cost: O(curve segments) per item, with a segment grid per stretch.
3. **Order.** The items of every cut are sorted by `key`.
   - A same-layer planned crossing (two curves that cross) keeps the curve order. The two ERBS then cross: a drawn
     violation where the plan predicted one ("costly, not failed").
   - **Disagreement report:** the number of item pairs whose `t_band` order (outside ties) contradicts `key`. On
     the method path it should be 0; when it is not, the plan is non-planar on that layer.
4. **Stacks.** Per (sheet vertex, side), the wrap items of all cuts at `v`:
   - Each cut ranks its wraps by `key`. The per-cut rankings are merged into one inside-out order by a topological
     sort of "inside of" pairs.
   - A cycle (an inconsistent plan) is broken by the mean key and counted.
   - This is Dayan's list (PDF 125). It is the source of the stack radii in step b, and of a later ROAR-like move
     (PDF 152; not in M3).
5. **Planner (open decision D1).** `cutmodel.rs` orders its items by band `t`, ties by wire id. With the key, its
   need changes only where tied neighbours differ in net or clearance. Using the key there costs one curve query per
   item in the hot path (3.1 perf). Proposal: the drawing first; the planner only after measuring how many tied
   items differ in net.

### 4.2 Step b: nested radii including distant pads (Q2)

1. **Spoke tips per cut** `c = pq`, items `0..n` in key order. With the board's rules (§5):
   - `tip_p(i) = end_p(item 0) + Σ_{j<i} (w_j + g_{j,j+1}) + w_i / 2`;
   - `end_p` = the clearance of the end feature to item 0: the pad's own clearance against the wire's, the
     copper-to-edge rule at the outline, the half width at an own-net junction (`cutflow::half_at`, `cut_ends`);
   - `tip_q(i)` the same from `q`;
   - in the cut's planning norm: the octagonal length for octilinear (`sheet.rs::Norm`, Staepelaere eq. 1.2, PDF
     12), Euclidean for smooth. A tip at norm distance `ρ` lies at `p + ρ · u / ‖u‖_norm`.
2. **Stack radius per wrapped vertex.**
   - Along every cut at `v` in the wedge, the wraps come first, inside-out, so the tip of a wrap item is the same on
     every such cut, up to the feature's bound in that direction.
   - `Stack.radius[k]` is that value. For a polygon vertex it is measured from the vertex.
   - The ERBS uses it as an arc, the circle of Popov p. 119.
3. **Distant features: the wave (Popov pp. 120–121).**
   - The sheet keeps every critical cut from every feature vertex to every visible feature side, with no distance cap
     (`sheet.rs:18-26`; `[capacity] cut_radius_mm` = 0). That is Popov's stage 1: the sector is the visibility
     window, which narrows portal by portal (`sheet.rs` walk), and wires are counted wherever they cross.
   - Popov's stage 2 (per segment, the circles cut back to that segment) is exactly the per-item tip:
     `tip_p(i)` counts the items before `i` only.
   - So the wave needs no new search. It needs the cut set used **as is**, for the drawing: not the projection edges,
     not only the cuts of the stretch's own triangles.
   - **Check.** A stretch near a vertex `v` that it neither wraps nor sits next to in a triangle must still get `v`'s
     cut if one exists through the wires in between. Unit test: the `distant_radius` geometry (§10); `w3` gets `A`'s
     radius 1.625 mm through `w1` and `w2`.
   - Shadow pruning (octagonal, Maley p. 55) leaves out only implied cuts (`pr + rq` = `pq`). Their tips follow from
     the cuts through `r`.
4. **Over-full cuts (never drop; graded, not thresholds).**
   - The plan has priced the deficit `δ = -slack` (`cutmodel.rs` phi). The drawing realises it where the plan put it:
     the tips are computed with every gap scaled by `(Σgaps - δ_g) / Σgaps`.
   - **Necks first.** Inflated-sketch §4.6, TopoR manual p. 56: the minimum width only where needed. Widths go down to
     their floors in the plan's neck order (the plan writes `pass_w`) before any gap shrinks.
   - The shrunk gaps are this cut's predicted violation. The compass reports them as predicted; the drawing does not
     search for something better.
   - Two over-full cuts that share an item use, per item, the smaller tip from each end (the more shrunk spacing).
   - **As built (step b, deviation).** The necks are taken by one equal fraction of every item's neck room
     (`w - floor`, bisected to the cut), not in the plan's `pass_w` neck order; the plan's necked widths are already
     the item widths, so this only acts on cuts the plan left over-full. `neck` is the need it takes off (a same-net
     run counts its widest member once). A cut counts as over-full at `slack < -1 nm`, as `Measure::summary`.
   - **As built (stack radii).** Per member the largest tip from the vertex over the cuts that carry it, the
     smallest over over-full cuts (their squeezed tips hold), then an inside-out pass: every member at least its
     inner neighbour's radius plus the two half widths and their gap (Dayan 1997 Fig. 60, pp. 115-116; Popov 2012
     p. 118). A member the pass pushes past its far tip on a carrying cut is counted (`radius_pushed`, graded
     amount), never cut off.

### 4.3 Step c: ERBS per stretch, then octilinear

1. **ERBS (any angle).**
   - The shortest path from the stretch's start to its end that:
     - stays in its sleeve (the sheet triangles of its reduced crossing sequence, walls elsewhere: the class is kept);
     - crosses every portal `[tip_p, cap - tip_q]` of its items (a spoke is a segment obstacle from the feature to the
       tip, zero width: the tip already holds the half width and the clearance);
     - keeps outside the stack arc of every vertex it wraps.
   - Implementation: the visibility A* of `exact.rs::local_path`, with a new node and obstacle set:
     - nodes: spoke tips, arc samples at the circumscribed radius `ρ / cos(step / 2)` (Dayan PDF 33: octilinear
       spokes as the arc), and the two ends;
     - obstacles: spoke segments, sleeve walls, and the static features grown by the stretch's own end clearance.
   - No drawn copper takes part. Every stretch is independent, so the stretches can run in parallel within the CPU cap.
   - Per stretch: nodes ≈ 2 × cuts crossed + 8 × wraps (about 100–400). Edges are tested against the spokes in a
     segment grid.
2. **Check.** All ERBS are committed. Each is then checked against the static board and against all the others
   (`Checker`).
   - Spacing holds if the model holds. Dayan PDF 26 claims it; the papers that prove SURF's construction were not
     obtained (inflated-sketch §2.5), so for us it is **measured, not assumed**.
   - A violation the plan did not predict is a model gap, reported as such in the compass (`drawing_loss` / new
     class `erbs_gap`) with its place.
   - The old attempt ladder (§2.2 item 5) runs for exactly that stretch, as the fallback, and is counted. It is not
     firefighting the plan: it is the measurement of where the model is still wrong.
3. **Octilinear.** Two candidates. Measurement decides (open decision D2).
   - **c1. `octi.rs` on the ERBS, in Staepelaere's precedes order.**
     - Precedes order: Staepelaere OrderSegments, PDF 29: a DAG of "is below" between overlapping parallelograms,
       topologically sorted.
     - Nodes: the ERBS corners plus the spoke tips as octagon vertices. Spokes on the 8 breakpoint cuts of the
       octagonal norm exist at every feature vertex (`sheet.rs:41-44`), so the tips are the octagon's vertices.
     - Corridor: the stretch's own portals, as walls.
     - Obstacles: the static board and the stretches already transformed, in precedes order. This is the "previous
       boundary" of Staepelaere PDF 18, 33–34.
     - The lane gates / zones of P6 (`octi.rs:242-330`) and `tight_span` are not used on this path.
     - Mixed clearances and pads larger than the wire are fine: the checker judges every leg.
   - **c2. Staepelaere's transform proper** (TransformLayer + StraightenLayer, PDF 16, 29–34).
     - Exact for one spacing per layer (PDF 62). We would use the conservative territory: each segment carries
       `w + s_max`.
     - The "endpoints larger than the wire" extension (PDF 61) is our own work.
     - Its jogs need the chain straightening of PDF 62, Fig. 7.1.
   - Proposal: build c1 first (it reuses `octi.rs` and the checker). Build c2 only if c1 shows bundle-bend conflicts,
     i.e. conflicts between octilinear shapes that the ERBS did not have, on the patterns or the ladder.
   - Any-angle stays the fallback per stretch, priced by `any_angle` (`octi.rs:139`), never an open wire.
4. **Branches (D5 trees).** A branch ends on its trunk's copper (`end_at`). With ERBS, the trunk's ERBS (or its
   octilinear transform) is known before any branch is transformed: trunks come first in the precedes order.
5. **Diff pairs** stay on `realise_pairs` (`core.rs:5028`). They move onto ERBS in M4.

## 5. Widths and clearances (board rules are the authority)

- **Widths.** The item width is the plan's width at that crossing: `Stretch::width_at_idx`, the necked `pass_w` that
  the plan wrote (CutPrice::necks, 2.4). There is no width rule of the drawing's own. Neck floors come from the Board
  Setup minimum or the net's custom rule, as `State::w_min` gives them.
- **Clearance between neighbours.**
  - `[capacity] mixed_clearance`: `pairwise` = KiCad's max of the two nets' clearances; `conservative` = the largest
    clearance on the cut for every gap.
  - The same rule as the plan uses, so the drawing's tips equal the plan's need.
  - Courtyard regimes enter through `need_in`'s `regime` (`cutmodel.rs:828`). Layer-bound regions are CR2, not M3.
- **End clearances.** `Bound` of the end feature:
  - a pad: max(pad clearance, net clearance);
  - the outline: the copper-to-edge rule, or 0 when that check is set to ignore (then a graded `ignored_rule` price,
    as `state.rs:129-132`);
  - an own-net end or junction: the half width.
- **Graded, not thresholds.**
  - A stretch never refuses a portal. An empty portal (`tip_p > cap - tip_q`, over-full) gets the deficit spread of
    §4.2 item 4, and the violation is predicted and priced.
  - Octilinear shapes outside the own corridor pay the `planned_room` price only on the fallback path. On the ERBS
    path the corridor is the stretch's own portals, which follow from the rules, not from a cutoff.
- **Nominal clearance (TopoR manual p. 237).** Not in M3. The tips use the minimum (board) clearance and spread
  nothing. A later even-distribution step (Su & Dai's EWD) can widen gaps where slack allows. This would be a new
  item; it needs permission.

## 6. What it replaces (inventory for M6; nothing is removed in M3)

| Firefighting today | Replaced by | Stays for |
|---|---|---|
| "spread evenly … the checker will tell" (`draw.rs:1074-1084`) | the deficit spread on the over-full cut (§4.2.4) | `realise = sequential` |
| per-edge packing for the drawing (`draw.rs:1007-1101`, `Intervals`, `Lane`) | cut items + spoke tips (§4.2) | sequential; via site test `via_room` |
| `tight_span` (`draw.rs:1103-1124`), `neighbour_bound` (`:1150`), `lane_sides`/`lane_next_drawn` | own portals | sequential |
| octi lane gates / zones / `planned_room` (`octi.rs:242-330`, `:477`) | portals as walls + precedes order (c1) | sequential |
| exact path against drawn copper as the nesting (`exact.rs:12-15`, attempts 0–2 `draw.rs:2497-2530`) | stack radii (§4.2.2) | fallback for an ERBS the checker rejects |
| funnel + repair attempts 3–5 (`draw.rs:2530-2560`, `repair` `:3593`) | ERBS | fallback |
| forced relaxed / funnel never-drop (`draw.rs:2687-2800`) | deficit spread (predicted violation) | fallback |
| `follow_vias` (`draw.rs:1838-1890`) | sheets rebuilt with the placed vias; bands follow | sequential |
| `pull_taut` (`draw.rs:2624`, `:2953`; `draw_pull_taut`) | ERBS is taut by construction | sequential |
| blockers rip + redraw (`core.rs:3898-4040`, `exact.rs:595`) | order-free drawing | `loop = "cycles"` (M2 keeps `drawing_feedback` there) |
| `prov_layer` / `via_side_ok` side tests against nominal chords (`draw.rs:1808-1997`) | via features in the sheet (the via is a terminal; bands are pulled around it) | via placement (unchanged in M3) |

**Kept** (M6 list): via placement and force_via (via sites are a plan matter: 5.4, Q4), reduce_vias, fuse,
octi_polish, stitch, tidy, the Checker.

## 7. Interaction with M2 (draw once)

- M2 draws only the picked variant, once, with no feedback into the plan (`core.rs` DrawGate, branch
  `draw-once-m2`). M3 changes only how that one drawing realises its plan. The new code is in
  `radii.rs` / `erbs.rs` and the `draw.rs` entry point; core.rs only calls it. It avoids the files M2 touches
  (core.rs control, optimise.rs, run.py) except for a call site and one option.
- With `realise = "erbs"`, the within-drawing feedback (blockers, redraw) has nothing to do: an ERBS does not depend
  on another stretch's copper. `drawing_feedback` stays for `sequential` / `cycles`, as M2 left it.
- Under M2.TL (soft time limit), the drawing of the picked variant runs to its end. ERBS per stretch is bounded
  (local A*), and the stretches can run in parallel under the rayon cap.
- Rebase onto main after M2 merges. Expected conflicts: core.rs drawing call site, model.rs options.

## 8. Risks

1. **ERBS correctness is unproven here.** The SURF spoke papers (Dai, Kong & Sato DAC 1991; Kong 1992) are not
   available. Dayan PDF 26 and Su & Dai PDF 3 state the result. We test it on the patterns, and the compass counts
   `erbs_gap` per run.
2. **Curve key on non-planar plans.** Same-layer crossings in the plan give orders that disagree with the band. These
   are reported (§4.1.3) and drawn as the predicted violation.
3. **Sag between cut directions in the Euclidean norm.** In smooth style there are no breakpoint cuts, so the
   wrapped-vertex arcs cover wraps, and a passing (non-wrapping) stretch is held only by the cuts. Mitigation: arcs
   also at every vertex whose cut gives the stretch a tip above its own clearance. That is Popov's stage-2 circles.
4. **Vias placed by the drawing.** Via sites are still chosen in draw order (`place_vias`). The sheets are built
   after the vias, so the bands respect them. A badly placed via is a via problem (5.4/Q4), not M3's.
5. **Runtime.** A per-drawing sheet build with via features, on every layer. U0 measured it per ladder board (cutflow
   `final_plan` runs on every run with the compass). qmtech runs only on Hetzner or in the 2400 s scorecard.
6. **Octilinear conflicts in bundles** (c1). The precedes order should prevent them; if not, c2.
7. **Pads larger than the wire, pad exits (5.10).** The own pad is an own-net end (half width). Neighbour pads are
   features with cuts. Pad-exit violations that the plan does not price (5.10 default off) remain.
8. **Junctions with more than three wires, branch-on-trunk ends.** Trunk-first ordering (c1).

## 9. Measurement plan

- **Unit tests per step** (§11).
- **Approved patterns** (existing suite) after each step, with `realise = "erbs"` and with `"sequential"`.
- **The new M3 patterns** (§10), once the user approves them.
- **Ladder** eprom → wiccon → qmtech → esp-rust-board.
  - Through `scratch/agent-wt/slot.sh`, both realise modes. qmtech: scorecard rules (cpu 2400 s, one board at a time,
    `harness/score_runs.py`), on Hetzner if the local slots are busy.
  - Per board, from the M1 compass (`result.json` compass, `harness/predict_vs_drawn.py`): predicted vs drawn vs
    KiCad DRC per class, `drawing_loss`, the new `erbs_gap`. Detour ratio per connection (flag > 1.5), vias, length,
    drawing seconds.
  - Every changed connection is analysed in context. There is no "routed must not drop" gate.
- **PNGs.** Hand vs topor at every spot where drawn ≠ predicted, and the wiccon 3.3v P6 spot.

## 10. Test patterns (proposed; the user's approval is needed before they are used to judge the algorithm)

Generator `harness/radii_patterns.py`. PNGs are in `scratch/agent-wt/m3-runs/patterns/`.

- **Rules.** The fit-pattern rules: Default 0.25 mm at 0.20 mm, board minimum width = the class width, copper to edge
  0.50. Plus a PWR class at 0.40 mm / 0.20 mm.
- **Single layer.** B.Cu is closed by a rule area (no tracks, no vias), so the patterns test one layer.
- **Reference copper.** Each reference is built first, at exact radii, and KiCad-DRC-checked (0 violations,
  0 unconnected).
- **Validity.** `rules_assumed = board` (`harness/validity.py audit`).

1. **`three_diagonals`.** J1 is three pads at 0.8 mm pitch. U1 is three neighbouring pads of a 0.65 mm pitch module
   row; the rest of the row has no net. The two outer wires are PWR 0.4 mm, so by the route-order rule (thick before
   thin) the middle 0.25 mm wire is drawn last.
   - Expected: three nested octilinear wires (vertical, 45°, vertical). Each one is at its octilinear distance; the
     middle one stays between the outer two in its planned corridor. No via, DRC 0.
2. **`nested_wraps`.** Three wires wrap the top-right corner of a large foreign pad, from a left column of pads to a
   bottom row.
   - The inner-most wire is the thin one (drawn last). The nets are named so that wire-id order is the reverse of the
     inside-out order: this catches "wraps ordered by stretch id".
   - Expected: concentric octagonal wraps at radii 0.325 / 0.85 / 1.45 mm from the corner (stack inside-out C, B, A).
     Shortest lengths, no via, DRC 0.
3. **`distant_radius`** (Popov Figs. 5–6).
   - `w1` and `w2` wrap pad A.
   - `w3` passes above two pads B and C. The triangle edge B–C separates it from A.
   - Its zero-width band does not touch A, but A's radius through `w1` and `w2` (1.625 mm) reaches past edge B–C.
   - `w3` is the shortest wire, so it is drawn first.
   - Expected: `w3` bows over A's stack at 1.625 mm, and `w1`, `w2` nest under it. DRC 0, no via.

## 11. Implementation plan (each step with its own tests)

- **a. Attachment stacks.**
  - `radii.rs`: `DrawSketch` built from the plan and the placed vias (cutflow pieces reused), curve keys, sorted cut
    items, `Stack`s, the disagreement report.
  - Debug output `TOPO_RADII` (stacks per vertex, ties broken).
  - Tests:
    - three nested bands around one sheet vertex, ids reversed → order inside-out;
    - coinciding bands of a parallel bundle → key order = topology;
    - a planned same-layer crossing → reported, order = curve;
    - stack merge over several cuts with a consistent order and with an injected cycle;
    - a determinism check.
  - No behaviour change: nothing reads it yet.
- **b. Radii.**
  - Spoke tips per item, stack radii, deficit spread, necks first.
  - Tests:
    - tips on a 3-wire channel equal the hand numbers of `channel_fit` (fit pattern 1: pitch 0.4625, ends 0.2125);
    - mixed clearances pairwise vs conservative;
    - the `distant_radius` geometry: `w3`'s tip from A = 1.625 mm through a non-adjacent cut;
    - an over-full cut: the deficit spread evenly over the gaps after the necks, Σ tips consistent from both ends;
    - octagonal vs Euclidean tip positions.
- **c. ERBS + octilinear.**
  - `erbs.rs`: per-stretch A* with spokes and arcs, the checker, fallback to the old ladder (counted).
    `[drawing] realise` option (default `sequential`), compass class `erbs_gap`.
  - Then c1 (octi in precedes order).
  - Tests:
    - a unit ERBS on a 3-wire wrap: radii as in the stack;
    - independence: the result does not depend on draw order (permute, bit-identical);
    - the approved existing patterns (fit, untangle, via, pair-off), in both modes;
    - the three new patterns once approved;
    - the ladder (§9).
  - The default flip is proposed to the user with the measurements.

## 12. Open decisions (user)

- **D1.** Curve-key order also in the planner's cut pricing (`cutmodel.rs` item_order), or only in the drawing? The
  proposal is the drawing first, then measure the tied different-net items on the ladder.
- **D2.** Octilinear: `octi.rs` in precedes order (c1, proposed first) or Staepelaere's sweep (c2), by measurement.
- **D3.** The `[drawing] realise` default: stays `sequential` until the ladder shows `erbs` at least as good per
  connection (analysed, not gated). Then the user flips it.
- **D4.** Nominal clearance / even wire distribution (TopoR p. 237; Su & Dai EWD): not in M3. Is it a new item?
- **D5.** The new compass class `erbs_gap` (drawn violation that no cut predicted) next to `drawing_loss`, or folded
  into it?

## 13. Step c follow-up (2026-11-14): order at wrapped vertices, precedes, refinement spokes, pad exits

Measured on the four approved patterns (judge j15, `m3-runs/erbs`); the ladder is §14.

- **Order of two cuts that meet at a wrapped vertex** (`erbs.rs::gate_before`, `Wedge`). The meeting rule (path, `X`
  and the two crossings bound a triangle) holds only while the path, seen from `X`, turns by less than a half turn
  between the crossings. Round a vertex the band wraps it turns by the wrap angle plus a half turn, so two cuts rooted
  on that vertex (stack items, arc spokes) more than a half turn apart came out reversed. nested_wraps B: the
  incoming cut of M1's corner after the outgoing one, a cycle of 8 constraints closed by every flip of a pair near the
  corner, Kahn's break undid the accepted flip, C written by force (0.118 mm model gap). Now such pairs are ordered by
  their rays' share of the wrap from the incoming side: the cyclic order of a terminal's incident nets (Dayan 1997
  PDF p. 125, Fig. 59), the circles taken in their order along the path (Popov 2012 p. 119, Fig. 3).
- **Precedes order** (`erbs.rs::precedes`). The bend test took the neighbouring points of the gate list, and gates
  that share a point (a cut item and the arc spoke along it) gave a zero leg: no precedes pair at all on
  nested_wraps, the outer wire drawn first, its octilinear form took the inner wire's room (Staepelaere 1992 PDF 29:
  the transform needs the "is below" order). Now the nearest distinct points.
- **Feature refinement spokes.** Only the spokes the chord crosses inside their tip; none crossed: the one nearest the
  outward direction. A spoke the chord does not cross made the path cross it, round the vertex the other way (Dayan
  PDF p. 32: a spoke pushes the wire away from the fixed object; it must not move it to the object's other side).
  Radii test `erbs_nested_wraps_radii` un-ignored and passing; three_diagonals had a spoke up into the wire's own
  pad.
- **Cuts from the own terminal pad crossed at the pad** are no gates (the band starts inside the pad, the path ends
  there anyway; crossed at the pad's corner they zigzagged along its edge next to the neighbour).
- **Pad-exit legs: measured, off** (`ErbsOpts::pad_exit`). A straight leg out of an elongated pad (as the drawing's
  `pad_exit`) moves the wire off the tips the radii gave the others: three_diagonals model gaps of 0.25-0.5 mm
  between the bundle's wires, even with the legs as features of the other nets. The leg belongs in the sheet (the
  terminal at the pad's planned exit, `docs/dev/inflated-sketch.md` §4.6), i.e. M7.
- **Left:** three_diagonals model gaps 0.03-0.05 mm next to the pads (pairs that meet inside both portals near a pad,
  ordered by the flip heuristic); the drawing's octilinear form clears them (drawn: 0 violations, length = optimum).

## 14. Runtime of the ERBS on real boards, and the drawing's planned room (2026-11-14)

The pattern solver (§4.3, symmetric Gauss-Seidel over all gates, all gate pairs ordered) did not finish on any
ladder board within the 600 s limit (m3-prof-90def91: all five boards at the limit; eprom stretch 2: 685 gates, cuts
13 mm long with 12 mm portals for a 10 mm path, 97 % of the gate pairs meet). The cost had three sources: the sweeps
(information moves one gate per sweep, O(gates^2) per solve), the pairwise order (O(gates^2) meeting tests per solve)
and the number of solves (order flips, refinement rounds). Each step below keeps the optimum of §4.3 (the objective
is convex for a fixed order) and was checked against the dense form on the patterns (judge j15-j19).

- **Bounded and parallel.** `ErbsOpts::deadline` / `stop` (the run's time limit and Cancel), tested between solves
  through `expired()`: a stretch not finished has no ERBS, its runs are drawn by the sequential ladder and counted in
  `erbs_none`, with a warning. Stretches of a layer are solved in parallel (rayon); each is independent (§4, item 3).
- **Newton interior-point solve** (`erbs.rs::solve_newton`). The smoothed length `sum sqrt(|d_k|^2 + eps^2)` plus the
  log barrier `-mu (ln t + ln (1 - t))` of the gate ends, centring steps for a falling `mu` (Boyd & Vandenberghe
  2004, *Convex Optimization*, §11.2-11.3, pp. 562-569, the barrier method). A leg couples only two neighbouring gates,
  so the Hessian is tridiagonal and one Newton step costs O(gates) (Thomas' algorithm, the LU of a tridiagonal
  matrix; Boyd & Vandenberghe App. C.3, banded systems in O(n k^2)). Neither book is in `docs/literature/`: the
  pages are those of the 2004 Cambridge edition as cited in the code, not checked against a local copy. At most 100 Gauss-Seidel sweeps polish
  after it (exact gate ends, stalled pairs). eprom stretch 2: 617 k sweeps / 39 s before, milliseconds after.
- **Sparse order** (`gate_meets`, `Meets`, `order_with`). Only gate pairs whose boxes overlap can meet; the band
  order of disjoint gates is implicit (Kahn's sort sees only the departures from it). The meetings are computed once
  per `solve_flip` and reused by its re-solves. `sparse_order_matches_dense` checks the same order as the dense form.
- **Lazy gates (row generation)** (`solve_lazy`; Dantzig, Fulkerson & Johnson 1954, *Solution of a large-scale
  traveling-salesman problem*, Oper. Res. 2(4), pp. 393-410: solve with a subset of the constraints, add the ones
  the solution violates). Start with the arc and refinement gates; after each solve add every cut gate whose spoke
  the path crosses (more than 2 nm from its tip); stop when none. A path homotopic to the band that crosses no spoke
  crosses every gate inside its portal, so the result is feasible for all gates; the order flips run once the set
  holds. `with_dropped` then puts the gates left out back where the path crosses them (in path order), so the
  precedes order (§13) and the check see every gate.
- **Swap screen** (`local_gain`). A single order flip is first solved locally, between the chord ends round its
  pin with the ends held; a flip that does not shorten that window is not tried on the whole stretch (eprom: 1689
  solves of one stretch for 3 accepted flips before).
- Debug switches: `TOPO_ERBS_TIME` (per stretch: solves, ordering / solving / screen / lazy seconds, meets),
  `TOPO_ERBS_NOSCREEN`, `TOPO_ERBS_NOLAZY` (all gates from the start, the reference), `TOPO_ERBS_NEAR=<mm>` (also
  start with the cut gates whose spoke root lies that near the band).

ERBS-mode runtime on the plugin dumps (`topo_route`, 1 thread, ccx43; m3-prof*): eprom 600 s (limit) -> 7.6 s,
wiccon 600 s -> 76 s; esp-rust-board, bornhack2026 and qmtech-minimig still reach the 600 s limit, but those
runs also spend most of the limit before the drawing (the sequential runs of the same dumps are long too); their
ERBS stretches past the limit fall back to sequential (`erbs_none`).

**Lazy is right; nested_wraps B 1.016 was the drawing's planned room** (judge j18 -> j19). With lazy gates the B
ERBS is shorter than the all-gates solve (16.083 vs 16.127 mm, both 0 violations): the all-gates solve keeps pins
of wrong free-meeting orders that 18 flips did not all undo. The drawn B was longer because of the octilinear
form: B is drawn after C and before A (precedes order), and the `planned_room` price of A's room
(`draw.rs::neighbour_bound`) took A's crossing from its zero-width taut path. Round the block corner all three
taut paths lie on the corner vertex, so A's crossing less the lane spacing fell behind the vertex and B's whole
span, its own ERBS included, was A's room: 22 mm of price on the leg along B's ERBS. With the all-gates ERBS
the DP had more taut vertices to hide in by chance; with the lazy ERBS (two vertices) it jogged up round A's
pad corner to shorten the priced leg (+0.25 mm). Fix: with `realise = "erbs"` the neighbour's planned crossing is
its ERBS crossing (the ERBS is the plan's realisation, spaced by the radii and the spoke tips, Dayan 1997 PDF
pp. 15-16, 32; room for the lines drawn later, Bast, Brosi & Storandt 2020 §4.3, p. 7). The sequential mode has
the same degenerate bound (nested_wraps seq: 66 mm of room price on B, B 1.115): open item, not changed here
(the sequential path stays as measured on the ladder). Debug: `TOPO_OCTINODES=<wire>` prints the octilinear
DP's nodes, the chosen legs with their room price, the zones and the lane gates.

**The gates left out go back once, at the right meeting** (8bfa82f). `with_dropped` ran inside `solve_lazy`, so
every refinement round's tie-break order (`prio`, the earlier solve's order) held the dropped gates at the first
place the path touched them, a shared spoke tip far before their crossing; the next solve folded the path back
to reach them. eprom stretch 2 (691 gates): 13.41 mm with a fold and a 0.277 mm model gap; now 11.06 mm and
0.107 mm, as the all-gates solve (11.08 mm, `TOPO_ERBS_NOLAZY`). The dropped gates go back once, after the
refinement rounds, each at the meeting whose share of the path length is nearest the share of the band length
at the gate (band and path are homotopic, so they cross the gates in the same order). Left: the same stretch
still bends 0.4 mm off its band (11.06 against 9.15 mm) through a fan of cuts that meet inside both portals;
the all-gates solve shows the same zigzag, so it is the free-meeting order (§13, "Left"), not the lazy rows.
PNG: `scratch/m3-erbs/gaps/eprom-gap-*.png` (lazy before, nolazy, fixed).

**Ladder at 8bfa82f** (seed 0, default config but `[drawing] realise`; score_runs cpu clock, shared ccx43;
`m3-runs/erbs/ladder-m3-lad4.md` against the lad2 sequential runs, `ladder-m3-lad4b.md` with `optimize.rounds = 3`
in both modes so that the time-limited TopoR loop has the same budget):

| board | seq DRC / vias / mm / >1.5 / s | erbs DRC / vias / mm / >1.5 / s | ERBS counters |
|---|---|---|---|
| eprom-to-kernal | 0 / 0 / 111.4 / 0 / 8.7 | 0 / 0 / 111.4 / 0 / 10.7 (identical copper) | ok 27 (octi 1), gap 0.107 mm, 1.4 s |
| wiccon-2023 | 0 / 17 / 1639.8 / 3 / 55.6 | 0 / 17 / 1635.3 / 2 / 35.5 | ok 67 (octi 18), gaps worst 0.032 mm, 12.1 s |
| esp-rust-board (rounds 3) | 313 / 61 / 972.6 / 13 / 293 | 336 / 65 / 987.0 / 14 / 928 | ok 107, none 11, 4716 order cycles, 681.8 s |
| bornhack2026 (rounds 3) | 942 / 224 / 2627.2 / 33 / 1051 | 935 / 228 / 2625.2 / 32 / 1010 | 0 stretches (limit passed), 409 erbs_none |
| qmtech-minimig (rounds 3) | 421 / 164 / 4384.6 / 22 / 820 | 580 / 154 / 4624.1 / 33 / 1107 | 0 stretches, 414 erbs_none |

wiccon: 7 connections change; SCLK detour 1.60 -> 1.50, 4v U2.3-C5.1 dog-leg -> straight (1.20 -> 1.03), RXD
1.04 -> 1.00; GPIO05, 4v J3.1-D4.1, D3-Pad2 and 4v C5.1-TP28 0.3-1.7 mm longer (bundle forms), no DRC change.
On the three big boards the loop picks still differ between modes (the loop is time-limited), so their
per-connection diffs are not the drawing's alone. The plain finding: **the ERBS has no time budget of its own**.
It runs after the loop on whatever the 600 s limit leaves; on esp it takes 682 s and on qmtech it spends 528 s
without one finished stretch, so the sequential fallback that follows is starved (qmtech DRC 421 -> 580). A
share of the limit for the ERBS (an option with a safe default, like `loop_time_share`) and a cheaper
free-meeting order (the 4716 order cycles on esp) are the next steps; until then `realise = "sequential"` stays
the default. PNGs: `scratch/m3-erbs/lad4/`, `lad4b/`, gallery stage "M3 ERBS realise".
