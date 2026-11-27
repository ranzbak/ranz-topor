# pick-necks: necking in the final pick (user decisions 2026-11-27)

What: `[optimize] pick = "best"` ranks the drawn variants by fewest errors, fewest connections with a
short (`pick_shorts`), then ONE value `length + mm_per_via x vias + necking` (mm equivalent), then
fewest shrinkages. Necking is not a strict step any more (user, r2: "Let's use necking width reduced
* length * constant"):

    necking = K x sum over drawn copper of (class width - drawn width) x stretch length   [K = neck_mm_per_mm2, mm2]
            + neck_via_mm x (vias smaller than the class / pair via)

Why: a variant that necks a power trace or drops to the minimum via is a worse board than a slightly
longer one, but a strict key let 0.14 mm of graded neck outrank 118 mm-eq of length + vias (qmtech r4,
verifier r1). As a price the trade is graded: necking only turns a pick when it is worth the length.

Measured on the real drawn copper (`criteria()` over `Assembled.items`, `drawn_necking()` in
topo/core.rs): every track segment or arc at its OWN width, so a taper counts each step at its own
width (r1 graded a merged taper at its narrowest width over the whole length: esp +5V 1.60 vs 0.60 mm
in the copper); a track at or above its full width counts 0. Full width = the connection's class
width (`Connection.width`), for the two nets of a diff pair the pair rule width. Full via = the REAL
rules' `via_sizes(net).first()` (`model_real`, a pair's own via), never the alias planning disc of the
lead net (r1 counted every lead-net via of a coupled pair as necked); both nets of a pair are counted
because every drawn item is. Nothing is carried per wire any more (`neck_graded_mm` removed), so the
pair realisation cannot leave a stale number.

Literature: TopoR 6.3 manual pp. 72-73: the variant table is sorted by violations, then length;
vias and shrinkages (width reductions) are criteria of their own, 25 mm/via a good price. Here the
width reduction is priced into the length value by the user's decision (not the manual's order).

Constants (both GUESSES, not measured; user r2: neck_via_mm stays 10):
- `neck_via_mm = 10`: one necked via = 10 mm of extra length (0.4 of a via at 25 mm/via).
- `neck_mm_per_mm2` (K) = 10. Scale: a 0.25 mm signal trace narrowed by 0.1 mm over 5 mm = 0.5 mm2 =
  5 mm-eq (a nudge, a tie-break between near-equal variants); a 0.2 mm cut over 5 mm = 1 mm2 = 10
  mm-eq = one necked via; a 0.6 mm supply trace necked to 0.3 mm over 10 mm = 3 mm2 = 30 mm-eq, about
  one extra via + 5 mm of length. r1 measured 1 neck via per 5-7 mm of graded neck on qmtech, K = 10
  keeps the area term and the via term of the same order. A board with two drawn rows that differ
  only in necking is needed to tune it; until then 0 switches the term off.

Invariants: only `Objective::Best` adds `necking` to its value (`base()`); `errors_weighted`,
`errors_vias_length` and area are unchanged; `necking` is in `covers`/`dominates`, so the archive never
prunes a drawn row that necks less. Plans (the search) carry 0: the search does not price it. The role
picks (errors / shortest / vias) use `Objective::better` too, so their tie-breaks among equals now read
necking as well (r1 note overstated "role picks unchanged"). In the TopoR loop (`loop = cycles`) the
necking decides which drawn rounds are kept (as the final pick); the stable meter's score does not
include it. The round line and dialog name the decider `necking` when the necking turns the
comparison (length + vias alone would pick the other, or tie), else `score`.

Tests: archive.rs `best_pick_necking_is_part_of_the_value`, `necking_is_a_pareto_criterion`; core.rs
`drawn_necking_per_segment_width` (taper, diagonal, per-net width, arcs both ways, via boundary per
net), `necking_term_prices`; python `test_score_delta_has_a_text_for_every_decider`, report fields in
test_report.py.
