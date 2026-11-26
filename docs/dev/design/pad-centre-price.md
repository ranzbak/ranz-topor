# pad-centre-price: off-centre in-pad trace ends as a graded drawing price

**What (user 2026-11-26).** KiCad's real Cleanup keeps an off-centre trace end inside its pad and an in-pad T join
(fixture `tests/fixtures/kicad_accepts/pad_centre`), so pad-centre distance is a hint today
(`tidy::PadEndBreach::hint`). The user wants it to be a soft price: the drawing should prefer a pad entry/end at
the centre wherever that costs little, and the run reports how many off-centre ends are left.

**Where the off-centre ends come from (main a18b3716, C1 det s0, `pad_centre_check.py`, before Cleanup).** qmtech 40
hints (25 Terminal, 12 PassThrough, 2 Tee, 1 Junction), esp 22, wiccon 5, eprom 0. Most Terminals are a first leg of
a run from the pad centre that lies wholly inside the pad: tidy (and KiCad's `deleteTracksInPads`) drops it, the run
then starts at the leg's far joint `j`, `|j - c|` off the centre (qmtech R12.2 0.106 mm at 45 deg, RN3.2 0.15 / 0.23
mm either side, U3.4 0.5 mm, L2.1 / L3.1 / F4.1 0.02-0.04 mm along the axis). The octilinear DP already prices that
leg (`octi::stub_price`, pad-snap: `stub_nm x (1 + off / extent)`, about 1-2 mm at the defaults), so where it still
draws one, every centred alternative it had cost more than ~1-2 mm (bend + jog prices of the leave shapes) or was
illegal. The VM diagnosis (`TOPO_PADSNAPDBG` PADSTUB wire ids + `TOPO_RUNPROV` run provenance) names which drawing
path made each one; that table goes into the report.

**Change.**
1. New factor `[factors] pad_off_centre` (weight, mm of length per unit; **default 0 = off = main's copper**). The
   unit: one end `x` pitches off its pad's centre costs `x + x^2` units (`x = off / (w + s)`; smooth, 0 at the centre,
   no cutoff; the `planned_room` shape). `off` is the distance from the PAD centre (`obstacles[ob].center`, also for a
   branch end in its pad) of the end the copper will have after KiCad's Cleanup: the leg's far joint `j` when the
   first (last) leg is one Cleanup deletes (KiCad's test, the same predicate `stub_price` uses), else the run's own
   end (constant across the DP's choices, so not priced).
2. The DP's `stub(end, j)` price = pad-snap's stub price + `weight x (x + x^2)`, the sum capped at 0.8 x one violation
   unit (`STUB_VIOL_SHARE` rule: short > clearance > off-centre end holds for every weight). Costly, not failed: no
   shape is removed. Other drawing paths that leave such a leg (from the diagnosis) get the same price or are listed
   as open items.
3. Warning count (every run, any weight): `pad_off_centre: N trace ends inside a pad of their net off its centre
   (Terminal a, Junction b, Tee c, PassThrough d), largest x mm`, from `tidy::pad_end_breaches` hints on the written
   copper (KiCad keeps them: a hint, not a failure). The old `pad ends off centre` line stays.

**Invariants.** weight 0: copper bit-identical to main (copper hash, 4 boards). Any weight: no new DRC, unconnected,
detour > 1.5 or vias against main per connection; real KiCad Cleanup deletes nothing main keeps; copper identical
for any thread / lane count.

**Tests.** Unit: price 0 at the centre and for a leg that leaves the pad, grows with `off`, weight 0 = old price, cap
below one violation for every weight. Pattern (PROPOSED, PNG to the lead first): a run into a small pad whose base
copper ends off centre where a centred exit costs no extra length / via / detour; base (weight 0) off centre, tip
centred. Boards det s0 4 boards weight 0 (hash = main) and 2-3 weights; default proposal from the numbers.

**Literature.** KiCad 10.0 as the spec: `tracks_cleaner.cpp` `deleteTracksInPads` (the leg Cleanup deletes),
`pns_node.cpp` `NODE::addSolid` (PNS links a pad at its anchor only: why the centre matters for drag). Price shape:
Bast, Brosi & Storandt 2020 sec. 4.3 (graded prices in the octilinear DP), as `planned_room`.
