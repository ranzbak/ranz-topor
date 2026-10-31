# M2 Task 2 — CDT constraint diagnosis (F, 2026-10-31)

Inputs: model JSONs from `python -m harness.bench wiccon-2023 <dir> --rip none` and `... wedding-gift <dir> --rip none --pcb
<scratch copy>`; probe `cargo run --release --example cdt_probe -- <model.json> [--routing]` (`Options::default()`, no exclusions,
no terminals; replays `Cdt::build` with spade and *skips* every rejected constraint so all rejections are listed, runs a pairwise
census of the chords, then replays with `add_constraint_and_split`). Coordinates below are mm in the math frame (Y negated).

## 1. Offending pairs (identical on F.Cu and B.Cu; the outline is on every layer)
**wiccon-2023** — 122 `Edge` walls: 114 segments from two `gr_circle` (r 2.881 mm, lanyard holes at (140.45, −48.80) and
(160.17, −48.85), flattened by `TransformShapeToPolygon`) and 8 `fp_arc` of footprint `WICCON_CAT_MOON_STAR2` (the real
board outline: four arcs r ≈ 57.3 mm, chord ≈ 75 mm, sagitta ≈ 14 mm, four small arcs r ≈ 5.3–5.8 mm). 2 rejections per layer,
census 4 crossing / 0 collinear-overlap / 0 interior-touch pairs — all four are **a big arc's chord crossing a hole polygon**:
- arc wall 214 chord (93.337, −96.734)→(145.937, −43.534) × seg 135 (138.463, −51.024)→(138.725, −51.233), crossing at
  (138.501, −51.055), 48.7 µm from the segment's end; × seg 105 (142.882, −47.076)→(142.673, −46.815) at (142.682, −46.826), 14.8 µm.
- arc wall 219 chord (154.737, −43.734)→(207.837, −97.044) × seg 178 at (157.905, −46.915) (65.5 µm) and × seg 204 at
  (162.095, −51.121) (77.3 µm).
2 zero-length `Edge` walls (the closing segment of each flattened circle, first point == last point) → `ca == cb`, skipped.

**wedding-gift** — 176 `Edge` walls (163 `gr_line`, 13 `gr_arc`, 8 of the lines zero-length). The Edge.Cuts layer holds a second
copy of part of the outline shifted by (+1.023 µm, −1.005 µm) (e.g. `gr_line` (76.2, 191.262)–(86.36, 191.262) and its twin
(76.201023, 191.260995)–(86.361023, 191.260995); `gr_arc` (91.44, 186.182)→(86.36, 191.262) and its twin). 1 rejection per layer,
census 1 crossing / 0 collinear-overlap / 1 interior-touch:
- arc wall 500 chord (91.440, −186.182)→(86.360, −191.262) × twin seg 490 (86.361023, −191.260995)→(76.201023, −191.260995),
  crossing at (86.361005, −191.260995), **18 nm** from the segment's end (a near-T junction of two outlines 1.4 µm apart).
- interior-touch: arc 384 end (23.114, −110.998) vs seg 389 start — same KiCad point, positions differ by < 1 nm (see §2b).

## 2. Geometric causes
a. **Arc chords.** An `Edge` arc wall enters the CDT as its chord (design note m1-sketch §1 "no subdivision"). A 75 mm chord of a
   quarter-circle outline runs 14 mm inside the board and cuts whatever outline items lie there (wiccon: the hole circles; in the
   `Routing` set also pad hulls, e.g. J4:3/J4:4 on B.Cu). This is the expected cause and it is deterministic, not a precision issue.
b. **Sub-nm arc endpoints.** `Frag::prim` anchors an arc at `arc.start()`/`arc.end()` (centre + r·(cos, sin)), not at the integer
   `TrackPrim` endpoints; `Frag::anchor(share)` keys on exact bits, so every arc joint gets two vertices < 1 nm apart (wiccon 5 joints,
   wedding 2). Harmless to spade (exact predicates) and to the split (no split vertex came within 14 µm / 18 nm of them), but the
   outline loop is not closed at arc joints and the census reports them as interior-touch. Fix at the source (obstacle.rs, one line):
   round the arc anchor positions to integer nm (`(x.round(), y.round())`) in the `Prim::Arc` branch of `Frag::prim` — all model
   coordinates are integer nm, the error is < 1e-6 nm. Applies to kept arcs too.
c. **Dirty outlines.** Wedding's duplicated, shifted outline (crossing 18 nm from a vertex) and 8 zero-length lines; both boards'
   zero-length chords are already skipped by `ca == cb`. Not footprint Edge.Cuts overlapping the board outline — refuted on both boards.
d. `Routing` set (pads, kept tracks, keepouts added): wiccon F.Cu 21 crossing pairs, B.Cu 117 crossing + 2 collinear-overlap (kept
   segments of net 7 overlapping each other) + 12 interior-touch (kept track through a pad-hull corner); wedding F.Cu 72 crossing
   (pads D14/D15/U2 placed over Edge.Cuts), B.Cu 29. All are genuine chord intersections that a split handles.

## 3. Step 4 fix rule — confirmed, with amendments
`add_constraint_and_split(fh(ca), fh(cb), |p| V { pos: p })` on both boards, both sets: **no panic, no `Err`**; EdgeOnly: wiccon 4
split vertices per layer (128 constraint edges), wedding 1 (171); Routing: wiccon 21 / 117, wedding 72 / 29. Split vertices lie
≤ 5e-8 nm off their chords; the nearest other vertex is 14.8 µm (wiccon) / 18 nm (wedding). Synthetic checks (spade 2.15.1):
crossing (0,0)-(10,0) × (5,−5)-(5,5) → 5 vertices, vertex 4 exactly (5, 0), 4 constraint edges; collinear (0,0)-(10,0) then
(4,0)-(6,0) (either order) → 4 vertices, edges `[(0,2),(1,3),(2,3)]`; duplicate and reversed duplicate → 1 edge; T-junction with an
existing vertex → routed through the vertex, no split vertex; `from == to` → 0 pieces (keep the `ca == cb` skip).
- **A1 `constraint_owner`: do not walk `LineIntersectionIterator::new_from_handles`** — spade documents that split vertices are not
  exactly on the line, so the walk returns `EdgeIntersection`s instead of `EdgeOverlap` and misses pieces. Also the `Vec` returned by
  `add_constraint_and_split` goes stale when a later constraint splits those pieces. Fill geometrically after the build: for every
  constraint edge `(u, v)` (from `constraint_edges()`), take the first wall of the set (wall order) whose chord has both endpoint
  positions within 1 nm of its line and projection parameters in [−1e-9, 1 + 1e-9]; prune candidates with `query_walls(edge bbox
  grown by 1 nm)`. Deterministic, first writer wins, O(E · candidates).
- **A2 `num_inputs` / `is_split_vertex(v)`:** `n_inputs = tri.num_vertices()` right after the point loop; split vertices are
  `v >= n_inputs` (spade appends; verified: wiccon F.Cu 360 inputs → split ids 360..363). `point_to_anchor` sized `num_vertices()`
  after the constraints, frame ids unchanged (inserted before any constraint).
- **A3 API names (spade 2.15.1):** `tri.undirected_edges()`, `tri.is_constraint_edge(e.fix())`, `e.vertices().map(|v| v.fix().index())`,
  `tri.num_constraints()`; `add_constraint_and_split` returns `Vec<FixedDirectedEdgeHandle>`.
- A4 sketch: `sketch_wire` drops a funnel bend at a vertex with `point_to_anchor == None` (`filter_map`, sketch.rs:263); a split vertex
  on a hole polygon (wiccon) is such a vertex — the bend is lost and the self-check catches a bad chain. Acceptable for M2 (fallback,
  not a crash); note it in the M2 state as open.
- A5 chords stay chords: after the fix a 75 mm arc chord is a constraint edge through the board interior. Constraints only force edges
  to exist (nothing is impassable), so routing is unaffected; arc subdivision remains an M2 router topic, not Task 2.

## 4. Step 2 tests — changes
- `collinear_overlapping_constraints_do_not_fail`: expect `constraint_edges() == [(0,2),(1,3),(2,3)]` (canonical `a < b`, sorted), 4
  vertices, no split vertex.
- `crossing_constraints_are_split`: assert `num_inputs() == 4`, `constraint_edges() == [(0,4),(1,4),(2,4),(3,4)]`, `!is_split_vertex(0..4)`.
- `duplicate_constraint_is_idempotent`: add the reversed pair `(1, 0)`; still 1 edge.
- add `t_junction_uses_existing_vertex`: (0,0),(10,0),(5,0),(5,5), constraints (0,1),(2,3) → 4 vertices, edges `[(0,2),(1,2),(2,3)]`.
- add `zero_length_constraint_is_skipped`: (0,0),(10,0),(0,0) with constraint (0,2) → 0 constraint edges, `Ok`.
- `routing_constraints_include_pad_hulls_and_kept_tracks`: also run the kept segment into the pad hull (e.g. (10.3,10)–(20,10)) and
  assert the hull pieces are owned by `pad:U1:1` and the track pieces by `kept:<uuid>` — this is the wiccon case (14 of its F.Cu
  kept tracks cross a pad hull, e.g. `kept:756a4358…` × `pad:D1:2` at (153.210, −46.040), 0.3 mm from the hull corner).
`sketch_funnel` / `smooth_engine`: unchanged (their fixtures have no crossing chords).
