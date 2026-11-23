# pour-pad-join: same-numbered pads in pours, and complete boards must not spin the dialog loop

**What.** On a board KiCad calls complete (DRC: 0 unconnected), the plugin found connections that do not
exist and, in the progress dialog, spun the TopoR loop at tens to thousands of cycles per second until the time
limit. Three generic mechanisms, one fix each.

1. **Same-numbered pads (decompose).** A pour lists the pads it reaches as `REF.NUM`, one entry per pad, so a
   footprint with several pads of one number (connector shields `SH`, mounting pads `MP`, a button's two `2`
   pads) is listed several times. `decompose_with` joins a listed pad with `position` (`engine/src/decompose.rs`
   ~428), which finds the first such pad every time. The other pads stay out of the pour's cluster and each gets a
   pad-to-pour connection that is not needed (private board P: 3 on GND). `pourcheck.rs::pad_indices` already
   reads such a label as "all of them". Fix: every pad carrying that number is its own terminal; when a label
   names more than one pad on a pour layer, the pourcheck geometry (`pourcheck::pour_joined_pads`, the KiCad 10
   zone_filler thermal-spoke rule of pad-pour-via A, raster islands) decides which of them the fill joins. A pad
   the fill reaches is pour-connected (no connection); a pad it does not reach keeps a real connection. Labels
   naming one pad are joined as before (no pourcheck call, copper unchanged). "Reaches" = joined to one of that
   pour's own islands (fill, spokes, tracks, vias), also when the pour is not in the net's main component (round
   2: the main component's pads alone left a second pour of the net without a port, and the real connection from
   its group to the rest of the net was lost).
2. **Nothing to route (fail fast).** With 0 connections in scope the core still builds the projection and the
   plane stubs (P: ~150 s), then the dialog's loop (no cycle cap, TW3 D6; no stop at "Routes stable",
   user 2026-11-22) runs empty cycles: eprom-to-kernal kept whole, 20 s limit, 40 075 cycles per run x 5 runs,
   CPU 70 s on 6 threads, log.txt 3 MB, result.json 5.8 MB. Fix: the route returns at once when the
   decomposition has no connection (stop reason `nothing_to_route`, no projection, no loop, no copper); the plugin
   says so clearly but not as an error (a complete board is a valid input): finding `NOTHING_TO_ROUTE` at info
   severity, a "Topor — Notice" dialog with an information icon ("The board is already fully connected ...
   nothing to route"; with a selection: "Every connection of the selection ... is already made"), and writes
   nothing. Only info findings get the notice look; a warning or error through the same dialog stays
   "Topor — Error". The pours' stitching (classes 2-4) works on the pours as given, so the early end runs it as
   the full route runs it on its picked result: when it adds vias, they are the call's items (stop reason
   `stitch_only`, the normal preview / Accept flow); `NOTHING_TO_ROUTE` only when nothing is added. The pre-flight has no connection count yet (scope and rip
   plan come later), so the check sits at the start of the engine's route.
3. **Unbounded per-cycle text.** The TopoR loop's report line lists every cycle ("per cycle: 1: ..., 2: ...")
   with no bound (5.8 MB for 40 075 cycles; a 600 s dialog run on a board of a few short connections reaches
   tens of MB per run, five runs, in memory and in result.json / report.md). Fix: list the first 20 and the last
   20 cycles and say how many are left out. Text only; copper unchanged.

**Not changed.** Loop, cycle cap, stable rules, time limit, drawing. Copper of every board with connections is
bit-identical (1 changes only which pads count as joined when a footprint has same-numbered pads in a pour;
3 is text).

**Invariants.** A pad the pour does reach is joined to it (no phantom pad-to-pour connection); a pad it does not
reach is not (no lost connection). A run with nothing to route ends in seconds with a clear info finding, never
with quiet empty cycles.

**Literature.** None needed: KiCad's connectivity is the spec (memory "KiCad tools are the spec"); TopoR's loop
works on the wires of the plan, an empty plan has nothing to optimise.

**Tests.** Rust (`engine/tests/decompose_nets.rs`): a footprint with 4 same-numbered `SH` shield pads, all in
the fill, listed 4 times -> 0 connections; 3 in the fill and 1 outside (at every list position), listed 3 times
-> exactly 1 pour connection, from the outside pad; shield pads in a second pour of the net (main component or
not, with or without a singly-listed pad beside them) -> 1 connection between the two groups; one shield pad in
each of two pours -> each joined to its own. Rust: a model with 0 connections -> `route` returns
`nothing_to_route` without a loop; a complete board with a thin pour neck -> `stitch_only` with exactly the vias
a full route stitches (11), and with `stitch_area` its lattice (24); with the classes off -> `nothing_to_route`. Rust: per-cycle text bounded. Python (`harness/fixtures.py` synthetic board,
run flow, dialog double that never presses OK): complete board -> `NOTHING_TO_ROUTE` in seconds, 0 loop cycle
lines. Mutants for each fix.
