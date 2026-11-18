# exist-term: existing copper as terminals of the topological core (design note)

**What.** A connection whose end is existing copper (an existing via, a track end inside a pad or via, a T
end on a track, a free / dangling track end) is routed by the topological core instead of failing with
"terminal is an existing via, track end or stub: not supported ... (S2a)" (Q14(d)). Existing tracks of every
net become obstacles of the projection (today only the drawing sees them).

**Why.** A half-routed board (user 2026-11-17, a private half-routed 2-layer board,
`dangling_ends = "continue"`) leaves almost all of its opens for this reason. Own-net copper is a terminal like a pad
(memories planes-are-terminals, through-own-pad-routing, qmtech "existing-copper terminals").

**Literature.** TopoR keeps fixed wires and vias as part of the design in every mode except placement and
grows new wires from them: "When you move a via or component that is incident to a fixed wire, an unfixed wire
is created between the object you are moving and the fixed wire"; "The fixed state of wires and vias is taken
into account in all modes except Placement mode" (TopoR 6.3 user manual, `eremex2017-topor63-user-manual.pdf`
pp. 110-111, "Working with Fixed Wires and Vias"; same text in `eremex-topor50-help.txt` 6/wires_fix.htm).
Fixed nets keep their topology while the rest is routed from scratch ("flexible fixing", ibid. p. 76).

**Model (decided in the projection, the drawing only realises).**
1. *Terminal resolution* (core, one function for both ends): `Via(i)` = the via's projection obstacle (already
   drilled, centred). A kept-track end at `q` on layer `L` that lies inside an own-net pad or via obstacle on
   `L` = that pad / via (same copper cluster; the wire ends at its centre). Any other kept end: the whole kept
   copper of its cluster is the terminal (lead 2026-11-17): *kept-end terminals* (projection obstacles of the
   net on `L`, a disc of radius 0.4 w inside the track's copper, centre vertex) every `2 (w + s)` along each kept
   track of the cluster, the ends included, none inside own pads / vias; the planned end is always one. The
   cluster's terminals, pads and vias are one component before any wire (`State::pre_join`, D5 net trees), so
   the plan joins where it is cheapest (a free T on the track).
2. *Continued dangling end* (`dangling_ends = "continue"` and the end has a direction `d`): the terminal is a
   *lead*: the stadium `q -> q' = q + d·L` of the track's width, centre vertex at `q'`. The plan starts the wire
   at `q'`; the drawing prepends `q` to the end run, so the new copper leaves the end along `d` (G1). `L` =
   the longest length up to one pitch (w + s of the connection, option `[search] dangling_lead_mm`, 0 = one
   pitch) whose stadium keeps the clearance to every foreign obstacle of the projection and the outline. r4
   (lead decision 2026-11-18), option `[search] dangling_lead_via`, default on: the lead also gets a *via end*,
   the longest `t <= L` where a via centred at `q + d·t` keeps the clearance to foreign copper on every layer, its drill
   the hole clearance and its disc the edge clearance (exact: per item edge the line's capsule interval, the
   largest feasible candidate end; fuzz `lead_len_via_end_matches_an_exact_reference_on_random_boards`, 20000
   boards). r5 (lead decision, memory "power via rules"): the via is the net class via first (the lead may
   end a little shorter); the smaller Board Setup sizes, the tightest last, only where the net class via
   fits nowhere on the lead and `pi x drill >= trace width` (`vias::lead_via_sizes`; test
   `a_lead_ends_at_the_net_class_via_first_and_the_smaller_one_as_fallback`). The via end is used only when the plan puts a layer change at the lead end: the wire's first via
   with its pass interval starting at the terminal (the layer may change before the first pass), or its last
   with the interval reaching the end; topologically the via may sit anywhere there, so the drawing places it
   at the via end (if every check holds against the copper drawn so far), draws the lead up to it and no run
   on the lead's layer (`Drawer::lead_via_site`; a via the plan puts after a pass stays in its interval, test
   `a_lead_via_stays_in_the_plans_pass_interval`). Leads that need no via keep their track-only length `L` (r3 cut every lead to the
   via end: 0.175 mm shorter on the fixture, and on the badge the shorter lead plus a far via folded the trace
   back over it). Cause of the via end: a via the plan forced at the lead end sat 23 um inside a foreign hole
   clearance on the badge (tests `a_via_at_a_cut_lead_keeps_its_clearance`,
   `a_lead_ends_where_the_smallest_via_fits_when_the_plan_changes_layer_there`). Fold-back (r3 verifier F4): a
   run that starts or ends at a lead end continues the hand track, so the octilinear drawing prices its first
   (last) leg's turn against the lead's direction: back along the lead is a full reversal, never drawn
   (`octi` `lead_in` / `lead_out`; test `a_wire_leaving_a_lead_does_not_fold_back_along_it`). Open: the plan's
   via-site test does not know the via end yet (a via drawn there counts as "via without a site" in P4), and
   own-net drilled items are not in the via-end check (KiCad hole_to_hole; the drawing's checker refuses such a
   via, which then goes to its planned site); no room
   ahead (L below 10 um, a degenerate lead) = the plain end disc plus a report note (costly, not failed: the plan
   picks the angle). *Root (A14), only where the decomposition picked the end*: the MST edge of the cluster takes the nearest port pair (`decompose.rs`, `closest`), and the connection is rooted at the dangling end only when that port is the end (the search then starts at the lead alone, `src/dst.retain`). When another port of the same cluster (its pad, a via) is nearer the goal, the edge is pad-pad and the hand stub stays dangling (r1 verifier probe; known gap, test `known_gap_a_nearer_cluster_pad_takes_the_root`, decision D2). The angle after the lead is the graded
   factor `dangling_turn` (1 - cos), opt-in (weight 0) until the plan measures it where the trace leaves the lead.
3. *Kept tracks as obstacles* (`BuildOpts::kept_tracks`, on for the route): every kept track on a projection
   layer (arcs as chords, sagitta 0.02 mm as the drawing) = a stadium polygon of its net on its layer, no
   centre. Foreign wires keep their clearance to it in P2 (fixed copper is an obstacle, p. 110). Overlaps with
   pads / other tracks of a chain are left to the triangulation's crossing splitting (noded fallback, as the
   overlapping copper graphics already are). The drawing keeps its exact segment entities and skips these
   polygons (no double copper). Same-net wires pass own copper freely (as own pads).
4. Pour ends with a via / kept terminal (`(Via|Kept, Pour)`): not in this round (none on the badge); a precise
   reason replaces the S2a text. The "pad has no net centre vertex" case is diagnosed; fixed here only if it
   shares the cause.

**Invariants.** A board without kept tracks builds the identical projection (no new obstacles, same ids): public
ladder copper = main (copper_hash). Every kept-end terminal lies on its own copper on its layer (no new open);
a lead keeps the clearance (no new DRC); obstacle ids of pads / vias / keepouts / shapes / plane extras unchanged
(kept obstacles come after them).

**Tests.** Engine (route_with on JSON models, release, < 60 s each): lone existing via as terminal (wire ends on
it, 0 DRC); dangling track end with `continue` (first segment along `d` within 1 degree, joins the end) and with
`fixed`; a stub into a T (end on another track's body); an own-net via whose pad side is the far layer (wire ends
on the via on either layer); kept track of another net crossing the direct line (wire goes around, no crossing);
a board without kept copper gives the same items as before. Mutants on each new branch. Badge (VM, private):
opens from existing-copper terminals 47 -> <= 2, no new shorts. Public ladder: copper_hash = main.
