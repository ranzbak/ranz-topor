# pick-necks: necking in the final pick (user decision 2026-11-27)

What: `[optimize] pick = "best"` ranks the drawn variants by fewest errors, fewest connections with a
short (`pick_shorts`), least necking, least length + `mm_per_via` x vias, fewest shrinkages.
Necking = sum over necked pieces of the drawn copper of `necked mm x (1 - w_neck / w_full)` plus
`[optimize] neck_via_mm` per necked via (a via smaller than its net's class via).

Why: a variant that necks a power trace or drops to the minimum via is a worse board than a
slightly longer one; today only the count of necked stretches (shrinkages) is a last tie-break.

Literature: TopoR 6.3 manual pp. 72-73: the variant table is sorted by violations, then length;
vias and shrinkages (width reductions) are criteria of their own, 25 mm/via a good price. The order
here puts the width reduction before length by the user's decision (not the manual's order).

Where the numbers come from (no geometry recomputed): `settle()` in topo/core.rs already holds
every drawn neck `(pos, w_neck, len)` of a wire (`WireOut.necks`) and the wire's full width; the
via sizes are in `Outcome.vias` and the class via is `vias::via_sizes(net)[0]`. `criteria()` sums
them into `Criteria.{neck_mm, neck_vias, necking}`. Plans (the search) carry 0: the search is
unchanged, and `necking` is in `covers`/`dominates` so the archive never prunes a drawn row that
necks less.

Invariants: only `Objective::Best` reads `necking`; `errors_weighted`, `errors_vias_length`, area
and the role picks (errors/shortest/vias) are unchanged. Equal necking = the old order exactly.

Default `neck_via_mm` 10: see the report (measured on the ladder, det seed 0).

Tests: archive.rs `best_pick_necking_beats_length` (necking beats length, equal necking falls back
to length, errors first, other picks ignore it); report fields in python tests.
