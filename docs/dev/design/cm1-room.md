## cm1-room (2026-11-17): planned vias are features of the capacity test; pad_exit only at the terminal's own pins

### Cause (wiccon GUI input, reports/wiccon-gui27-20261117-2215.md, cluster A)
TXD's via sits in the 1.0 mm gap between U1 pads 22 and 21 (CDT triangles A-C-D / A-B-C of that gap). RST runs
through the same gap. The per-edge packer (default `[capacity] model = "edges"`) packs a via disc only on the two
edges its own wire crosses (`pack::wire_discs`): there it shares the 2.693 mm diagonal with RST and fits; RST's
other two edges of the gap carry no disc. The cut model (`cutmodel`) counts a via only on a static cut that passes
within `r + s` of it, and every static cut starts at a feature vertex (pad corner); a via in the middle of a 2.5 mm
pad side is near none of them. Both models predict 0; the drawing needs 0.8 + 0.2 + 0.25 + 0.2 = 1.45 mm in 1.0 mm.
topo_reroute52 (cut model): SDA's band through a planned VCC via disc, no cut inside the via.

### Rule (one function, both capacity models: `topo::viacut`, priced inside `refine::CapModel`)
Maley 1987: a planned via is a feature (an island of width d, p. 32: capacity = length - width(P)/2 - width(Q)/2);
a critical cut runs from a feature to the closest point of another feature (p. 40); routable iff no critical cut is
over-full (pp. 32-33). For every planned via of the labelling (wire w, traversal k, point V = the site the drawing
tests first: `cutflow::via_point` / the 5.2 end site; reach diameter d, net n, clearance s), on every copper layer l:
- **Gap cuts through V.** Obstacles near the via = the obstacles of the triangles that share a vertex with V's
  triangle (the CDT neighbourhood: scale-free, no distance constant), those on l (pads on l, drilled holes, shapes,
  keepouts for tracks on l) plus the board outline. For each, q = its closest point to V; the cut runs from q through V
  to the first such obstacle the ray from V away from q meets (none: no cut). It is the union of the two collinear
  Maley cuts V-q and V-hit: an over-full union means no via position on that line fits, so the via point's exact
  place on the line cannot raise a false alarm. Near-parallel duplicates: the shorter kept.
- **Items** (ordered along the cut): the via (width d, net n), and every traversal of another net's wire in that
  neighbourhood on l whose taut segment (`Packer::taut`, pass crossing points) crosses the cut, anywhere, through the
  disc included (a foreign band through the via disc counts). Width = its pass width (necks included). Ends = the
  obstacles' bounds (`demand::Bound`; own-net pad: the via may touch it). Need = `demand::need` (the packer's
  pairwise clearances).
- **Price**: over = need - length (planning norm); a via-layer whose worst cut is over pays `overflow x (1 + over /
  pitch)` like an over-full edge-layer, and counts 1 in `CapModel::overfull` / `violations` (so in the loop's predicted
  errors, the P3/P4 prices, `optimise`'s via placement and the compass `planner_overfull`). Costly, not failed.
- **Incremental**: the geometry (V, cuts, crossing traversals) is fixed per (triangle, net) for the model's life (the
  State does not change inside a CapModel) and cached; only pass layers and via traversals change. A slot per
  (via triangle, layer) with its own undo entries; a changed pass reprices the slots whose cuts it crosses (reverse
  index). P4: an over-full via cut rips its via wire and its crossers (routed last first until it fits), history on
  the edges of the via's triangle.
- Option `[capacity] via_cuts = true` (default; false = the model before CM1), schema + docs.

### violation_class (route.rs:128)
`pad_exit` only when the pad (`against`, or the one named in `cause`) is a neighbour pin of the connection's terminal
pad: same footprint and centre distance <= 1.5 x that terminal pad's nearest same-footprint pitch (row and diagonal
neighbours, BGA too). Any other pad of the part is `static`/`foreign_copper` as before. RST vs U1.15 / U1.20 (13-15 mm
from U1.1) stops hiding as pad_exit; CS0 vs U1.10 (2 mm from U1.9) stays pad_exit.

### Invariants and tests (targeted, rule 14c)
- Unit (viacut.rs): via in a 1.0 mm pad gap + foreign track through the gap -> over 0.45 mm (exact by hand); via moved
  out of the gap or foreign wire on the other layer / other route -> 0; same-net crosser -> 0; band through the disc
  in open space -> no cut, 0; gap wide enough -> 0. Each is a mutant killer (drop the via item, drop the
  through-disc crossing, wrong layer test, own-net bound).
- Fixture (synthetic, tiny): two SMD pads 1.0 mm apart, a via-needing wire leaving the upper pad and a wire through the
  gap: CapModel predicts 1 over-full before/0 after the via moves (or fails the old build). wiccon GUI model: RST
  cluster predicted; the loop's pick draws 0 there (VM). topo_reroute52 drawn-clean test un-ignored if it passes
  (it runs the edges model; checked first whether its failure is CM1 at all).
- violation_class unit table: neighbour pin, diagonal neighbour, far pin of the same part, other part.
- Ladder seq (rounds=1, cached base): every changed connection judged; runtime of the extra term reported.
