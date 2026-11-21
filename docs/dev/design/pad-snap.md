# pad-snap (A1b): every track end on a pad at the pad centre

**What.** The octilinear DP (`topo/octi.rs`, the first draw, the ERBS octilinear form and the forced
draw all use it) may bend a run's first (last) leg inside the run's own pad. The centre-to-bend stub then
lies wholly inside the pad; topor's tidy (`tidy.rs` step 2, `inside`) drops it, exactly as KiCad's
Cleanup "Delete tracks fully inside pads" would (`tracks_cleaner.cpp` `deleteTracksInPads`: both ends
`HitTest` in the pad and the track polygon minus the pad polygon empty). The track then ends off the
centre, inside the pad (qmtech iter 1: 119 of 581 pad terminals; U6 18 of 25 at 0.71-0.81 mm).

**Why it matters (KiCad is the spec).** KiCad's connectivity connects the end (CN_VISITOR,
`connectivity_algo.cpp`: shape collision), so DRC reports no `track_dangling`
(`connectivity_data.cpp` `TestTrackEndpointDangling`: the pad hits the end within width/2). But the
push-and-shove router links a pad only at its anchor (`pns_node.cpp` `NODE::addSolid` ->
`linkJoint(aSolid->Pos())`): for drag, footprint drag and routing from the end, an off-centre end is a
loose joint, the "dangling" the user sees. Keeping the stub is no fix: the user's own Cleanup runs with
"tracks in pads" on and deletes it again. So the first segment from the centre must leave the pad.

**Mechanism found (pattern `pad_end_centre`, main 33f63998, sequential).** A, B, C end 0.525 mm off
centre. Their taut path's first node is a pivot round the next pin's tip corner, nearer to the centre
than `Exit::reach`, so the DP offers no "axis leg to past the tip" shape (`octi.rs` "(q - p).dot(axis) >
reach"); the elbows of `connectors(centre, pivot)` bend 0.45 mm inside the pad. D (no pivot) leaves at
45 degrees from the centre through the pad side: centred, kept.

**Change (drawing realises the plan's exit, `padexit.rs`: stub from the tip out by s + w/2).**
1. Shapes: at a pad end with an `Exit`, also offer the axis leg to the *leave point* (the nearest axis
   point whose round end sticks out of the pad by `LEAVE_NM` = 10 um > KiCad's max_error 5 um) followed
   by `connectors(leave, q)`, whenever that leave point is short of `q` along the axis (today's
   full-reach shape stays).
2. Price (graded, costly not failed): a first / last leg whose far end lies inside the own pad by at
   least half the width less tidy's `PAD_TOL_NM` (= the stub tidy and Cleanup delete) pays
   `stub_nm x (1 + off / pad extent)`, `stub_nm` = `min(4 x bend_nm, 0.4 x violation weight price)` (`OctiCfg::stub_nm`: short > clearance > stub holds for every weight; 4 x bend_nm = 1 mm at the defaults), off = how far the end lands from the centre. Every pad end,
   elongated or not. No new topor.toml key (an internal drawing price on the bend scale like
   `axis_price`); survivors are counted.
3. Count: the result's `pad ends:` warning line gets "N track ends off their pad centre (largest x mm)",
   measured on the final items after tidy, so a regression is visible on every board.

tidy stays as it is (it mirrors Cleanup). padend's `Moved` anchors (centre illegal, edge-clear) stay
off centre by design and are reported as before.

**Invariants.** Every pad end the DP draws with a legal centred shape ends at the centre after tidy;
the DP never loses a legal shape it had (only shapes added, one price added); copper identical for any
thread / lane count (pure function of the same inputs).

**Tests.** (pattern `pad_end_centre` stays PROPOSED until the user approves it, D1.) `engine/tests/topo_pad_snap.rs` on the pattern's model (both realise modes; fails on main);
a unit test of the leave point and the stub price (square pad and elongated pad); mutants of the
added shape, the price and the leave margin. Ladder qmtech + esp det s0: DRC / unconnected / Cleanup
sets per net vs main, off-centre terminal counts before/after.

**Literature.** KiCad 10.0 sources as cited (the spec). TopoR-side: the plan already fixes the exit
stub past the tip (shopping list 5.10, `padexit.rs`); this makes the drawing honour it.
