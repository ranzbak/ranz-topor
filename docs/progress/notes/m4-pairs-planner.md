# M4 planner part: diff pairs decided in the topology (shopping list 5.7)

Branch `m4-pairs` off main 7e320c1 (2026-11-14). Scope (user 2026-11-14): the planner part of M4 — gateways and
fan-out zones, polarity parity, swap sites priced against two single tracks. The drawing realisation ("realised from
ERBS") waits for the rebuilt ERBS (branch radii-m3); drawing changes here are only what the planner change needs.

## 1. What exists on main (D7, merged 8689a2f + later via-pair work)

- `topo/diffpair.rs` `plan`: pairs by KiCad's `MatchDpSuffix` (plugin), partner pads, N connections mirrored onto the
  P tree. **Polarity fallback**: `swaps()` tested whether the straight pad-to-pad lines of P and N cross (or lie on one
  line); such connection pairs were routed as two single tracks before any routing. This is the "polarity fallback"
  the code-vs-literature review moved to M4: it decides from pad positions only, ignores the pair's exit directions
  (dp_uturn_parity: the two lines are collinear, so it routed a pair that needs one via pair as two singles) and never
  considers a flip at a layer change.
- `alias`: the planning model; the lead (P) wire carries the pair as one envelope `p / cos 22.5 + w`; N pads carry the P
  net; a P-net copper bridge between partner pads that have room between them; the pair via is the two-via disc.
- Drawing (`realise`): offsets the lead's drawn centre line by +-p/2; **drawing-side zip** (`pick_zip`: the nearest
  point from the pad where both stubs clear); via pairs from `via_pair` (axis square to the bisector of in/out, which
  degenerates at a turn-back: M4-PAIR2, the N B.Cu track over the P via on the private 2-layer badge); P keeps one
  side for the whole connection, `polarity_cross` only reported.
- D7b / D7c / d7d-wip (commits c24c67f, 64d62a8, 7e13f4d, fe35b9c, ccd6aff; never merged): gateways before P2
  (`exit_dir`, `plan_fan`), gateway nodes in the projection, a one-via-pair "swap leg" at an end, swap-vs-single
  pricing (`swap_price`, `congestion`). Reused here: `exit_dir`, `plan_fan`, the static violation probe and the
  crowding measure (ported to today's API); not reused: gateway nodes in the projection (the projection is built
  once per call since 4.2 and the lead's terminals stay pad centres), the one-pair swap leg (superseded by the user's
  two-via-pair swap, 2026-11-10).

## 2. What 5.7 needs, and what this stage does

| 5.7 item | literature / rule | done here |
| --- | --- | --- |
| gateways + fan-out zones decided in the topology | KiCad PNS `DP_GATEWAYS::BuildFromPrimitivePair` (docs/dev/kicad-diff-pairs.md 3.2); TopoR's zipped pair is one wire with a movable slider at each end (TopoR 6.3 manual p. 171); user rule 2026-11-08 (straight out, symmetric 45 deg, gateway) | `dpplan::plan_fan` per pair end before the topology (graded run-out); the gateways decide polarity and the swap sites. Reservation in the planning model: `[diffpair] fanout = "bridge"` (default, the D7 bridge) or `"gateway"` (the fan-out tracks as lead-net copper, opt-in: in dp_gateway and dp_bus_crossing those polygons bend the lead's own way into its pads, section 4) |
| polarity parity, a U-turn at a layer change counts as a swap | user rule 3 (2026-11-08/10), pattern dp_uturn_parity; TopoR 6.3 p. 173 (a layer switch adds a slider and a pair of vias; "Swap Nets" removes an overlap) | handedness at the gateways (planar invariant), per-via-pair keep/flip price from the plan's turn angle, exact parity solve after the via sites are final; the drawing's via pair follows the planned flip (fixes M4-PAIR2) |
| swap sites priced by crowding; default the cheaper of swap or two singles; a swap needs room for two via pairs | user 2026-11-09 / 2026-11-10 (rules 3b, 4), patterns dp_swap_needed / dp_swap_no_room | `dpplan::plan_ends`: every site along the gateway line on both sides, priced by 4 vias + excursion + `via_squeeze` crowding + clearance breaks (graded); two singles priced by `dp_uncoupled` over the connection + 2 hop vias + 2 x `dp_via_unpaired` + one `crossing`; `[diffpair] swap = via_pair / via_pair_always / single` |
| pair values from Board Setup | user 2026-11-10 (rule 3c) | unchanged (D7 reads `diff_pair_width/gap/via_gap`; the pair via is the smallest the board allows) |

Not here (drawing, waits for ERBS): drawing a planned swap (the U excursion; until then `dpplan::SWAPS_DRAWN = false`
routes a planned swap as two single tracks, which draw legal copper, and the swap stays the plan's recorded decision),
the hop of two single tracks with coupling outside the hop zone, fan-out tracks drawn from the planned gateway instead
of `pick_zip`.

Drawing changes, and why each is strictly needed by the planner change: (1) `via_pair` takes the planned keep/flip of
each via pair (the plan decides it; without this the drawing re-derives the axis from the bisector and M4-PAIR2's
turn-back stays wrong); (2) `realise` keeps P on the side the parity solution gives per run between via pairs instead of
one side for the whole connection (otherwise a planned flip is drawn as a crossing); (3) report lines for planned flips
and planned swaps. Nothing else in the drawing changed.

## 3. Cost terms (planner mm, next to the existing terms)

Existing: `length` 1/mm, `via` 25/via (TopoR 6.3 manual pp. 72-74: vias are a criterion, counted), `crossing` 20,
`via_squeeze` 5 per fully squeezed via neighbour, `violation` 1000, `dp_uncoupled` 1/mm, `dp_skew` 10/mm,
`dp_via_unpaired` 25/via, `dp_polarity_cross` 1000.

New:

- **`dp_via_turn`** (10 per mm, default = the `dp_skew` weight it stands for): a via pair at a layer change whose
  path turns by theta. The vias' axis rotated by phi from square-to-travel makes the two tracks of each leg differ by
  `pv sin phi` (pv = via centre pitch, KiCad `EffectiveDiffPairViaGap` + diameter, doc 3.4). Keep: axis square to the
  bisector, phi = theta/2 on both legs -> `2 pv sin(theta/2)`. Flip: axis along the bisector, phi = 90 - theta/2 ->
  `2 pv cos(theta/2)`. Smooth in theta, equal at 90 degrees, no cut-off; same form as KiCad's per-bend skew
  `2 p tan(theta/2)` (doc 3.3). At pv = 0.7 mm a flip on a straight run costs 1.4 mm of skew (14), at a turn-back 0.
- **Parity** (no weight of its own): the number of flips must be odd exactly when the handedness differs between the
  gateways. On one layer a pair is a ribbon in the plane and cannot change sides whatever its path; only a via pair can.
  `solve_parity` takes each via pair's cheaper option and, if the parity is wrong, changes the one with the smallest
  difference (exact for a sum of independent binary choices with a parity constraint).
- **Swap site** = `4 x via + excursion x length + crowding x via_squeeze + 10 x via x (breaks + depth/mm)` (the break
  price is D7c's `swap_price`; graded, never dropped). The footprint is pattern dp_swap_needed's (`s1u_swap`,
  `swap_footprint_min`): both via pairs with the axis along the line, the down leg shifted by `s` on a 45-degree
  diagonal, 50 um slack past each clearance as the pattern builds it; a shortfall of 1 um or less (nm rounding of
  the quantised probe) is no break.
- **Two single tracks** = `dp_uncoupled(gateway-to-gateway mm) + 2 x via + 2 x dp_via_unpaired + crossing`.

Hand check against the approved patterns (U = 1/mm here, the patterns' price model used U = 2): dp_swap_needed has
room, so the swap (about 100 + 2h + small crowding) beats two singles (about 120 + connection length); in
dp_swap_no_room every site breaks clearance (10 x 25 per break) and two singles win.

## 4. Results

(filled in below after the runs)
