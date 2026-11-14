# M3 detour diagnosis (F, 2026-10-31, ruling M3-R11)

Scope: the top detour offenders after F2–F4 (head fdf8c10): private-badge-b #208 USB+ J1.A6→J1.B6 (3.49), #34 VCC
U3.1→U3.3 (3.03), #197 SPI_SDO TP17.1→J3.2 (2.83), #207 USB+ U7.6→J1.A6 (2.30), plus #45 GND SW1.0→pour (2.32) and
wiccon-2023 #52 GPIO0 U1.18→FLASH1.1 (1.405, the wiccon max). Numbers from a snapshot bench run (threads 4; private-badge-b
60 s budget, timed out in iteration 1 with 148/N, vias 102, detours > 1.5: 12, median 1.0, p90 1.374; wiccon 60/60,
1.56 s, 0 > 1.5). Tool: `gate_diag --detour <ids>` (new, this commit) dumps, on the empty graph, the committed path,
the slot search alone, the ghost, and the empty-graph path re-checked with every other committed path present (edge
cap / need / fits / passable / occupant); `--chord` probes `clearance_bend` along a polyline. Hand routes read from the
snapshot pcb. Ratio = `length_nm / ghost_len_nm`; "hand" = the hand board's copper between the same two pads.

## Per offender

**#208 USB+ J1.A6→J1.B6, 3.49 mm / ghost 1.00 (e + b + c).** USB-C pin pair, 1.0 mm apart with A7 (USB−) between
them at 0.5 mm pitch (0.2 mm pad gap: nothing fits). Ghost 1.00 = straight through A7 (the foreign pad constraint is
*soft*, P5): underestimate (b). The hand routes it on B.Cu around A7's pad tip, 2.93 mm, 0 vias ⇒ a direct DRC-feasible
path exists and the true detour is 1.19. The router takes 2 escape vias (3.49 mm). Search alone on the empty graph:
2-via path est 24.88 mm (leg 0 est 22.6 mm for a 1.245 mm leg); with `max_vias 0` a 0-via path est 34.9 mm. Trace of
the search (temporary `TOPOR_TRACE`, not committed): leaving A6 through its 0.3 mm tip edge, `state_point` puts the
crossing at A6's own corner (90.100, −123.680) (`margin_of` of an own vertex is `r` = 0), which lies 0.2 mm from A7's
corner (needs 0.305); `los` → `clearance_bend` then spirals (apex (89.600, −123.680), apex_len 11.305 mm, `ok` stays
true) and the state is accepted with +11.3 mm per tip crossing. Both tips ⇒ every B.Cu exit costs ≈ +22 mm, so the
via path "wins". Cause class (c): state point ignores foreign vertices other than the edge ends + the bend loop
accumulates length instead of rejecting. Static (identical with/without the other 230 connections).

**#34 VCC U3.1→U3.3, 8.07 mm / ghost 2.67 (c, b).** SOT-23-5, pads 1.325×0.6 roundrect (corner anchors r 0.15),
GND pad U3.2 between the two VCC pads, track 0.5 mm. Hand: around the outer tips at x = 108.5 (corridor between U3.1-3
and U3.4/5 is 0.95 mm for 0.5 + 2×0.2: 25 µm spare each side), 3.63 mm, 0 vias ⇒ true detour 2.2. Capacity model: the
own tip edge e3700/e3733 has `cap = 0.3 − 0.15 − 0.15 = 0.0` < need 0.905 (w + eps + 2 gaps), the long side e3695
cap 0.725 < 0.905: every exit of a 0.5 mm track from its own 0.6 mm pad is "unfit" (crossed as overuse, factor 4·(1 +
overuse/cap)), so the 2-escape-via path (8.07 + 4 mm via cost) beats the corridor. With the others present U3.1's
exits are also occupied by #35 VCC U3.1→C23.1 (same net, charged full clearance). Ghost 2.67 is corner-tight (b).

**#197 SPI_SDO TP17.1→J3.2, 19.18 mm / ghost 6.78 (a + c + b).** Ghost passes the 0.2 mm gap J3.1–J3.2 (soft, b);
search alone 7.24 mm around J3.2's tip crossing its 0.3 mm tip edge as overuse (cap 0.300 < 0.605); hand 13.49 mm
(goes east first) ⇒ true detour 1.42. With the others present the corridor edges carry #61 (J3.1→GND pour) and the
tip stays unfit ⇒ 2-via 17.4 mm path (cost 23.3 vs the overuse-laden direct one). Commit-order (a) on top of (c).

**#207 USB+ U7.6→J1.A6, 5.55 mm / ghost 2.41 (a + c).** Search alone finds the direct 2.41 mm B.Cu path (= ghost,
= hand's geometry; the hand makes it a 0.9 mm T-junction into the A6–B6 trace). With the others present the exits of
U7.6 (pad 0.625×0.25: tip edge cap 0.375, spoke cap 0.195, both < 0.605 even empty) are occupied by #206 USB+
U7.5→U7.6 — the **same net** — and e3944 (cap 0.000) by #205 USB+ U1.14→U7.5: same-net strands are charged full
clearance against each other and against their own pad corners. Batch commit of iteration 1 resolves it to 2 vias;
private-badge-b never reaches iteration 2, so no rip-up (a).

**#45 GND SW1.0→pour, 2.01 mm / ghost 0.87 (c).** Escape via straight below the pad (0.87) needs the pad's own tip
edge e2281: cap 0.600 < need 0.605 by **5 µm** (eps) ⇒ next escape site at 2.07. Tiny ghost inflates the ratio.

**wiccon #52 GPIO0 U1.18→FLASH1.1, 12.3 mm / ghost 8.77 (a + c).** Search alone = ghost 8.77 (west of the U1 pin
row). Committed 11.9 est (iteration 2) east of pins 17/16/15 — the hand does the same (15.9 mm), topor is shorter than
hand. With the others present the ghost is 10.10 mm with one soft edge: FLASH1.1's own tip edge e1511 (cap 0.8) is
crossed by #55 GPIO0 FLASH1.1→TP3.1 (same net) and the second same-net strand needs 1.11 ⇒ unfit ⇒ 11.9. Not a user
problem (< 1.5, shorter than hand); same mechanism (same-net capacity) as #207.

## Common root causes
1. **Own-net / same-net capacity (F6, 5 of 6).** `fits_at` charges `max(c, c_e)` at both edge ends regardless of net
   and subtracts the own corner radius: a pad's own tip / spoke edges are unfit for the pad's own net (0.3 mm tips,
   0.6 mm pads with 0.5 mm tracks, U7's 0.25 mm pads), same-net strands block each other. The search still crosses
   them as overuse (factor 4–8 on the step), which decides via-vs-direct and feeds `overuse_edges` (315 after private-badge-b
   iteration 1) and the "stalled" wiccon loop.
2. **State point + los runaway on own-pad edges (#208, the worst ratio).** Static search-geometry defect, independent
   of negotiation.
3. **Ghost reference (b).** Foreign pads are soft and corners tight, so short same-footprint pairs read 3× where the
   best DRC-feasible path is 1.2× (#208: 1.00 vs hand 2.93; #34: 2.67 vs 3.63; #197: 6.78 vs 13.49).
4. **No second iteration on private-badge-b** (60 s, M3-R13): sibling connections of one net take each other's pad exits in
   the batch commit and are never ripped.

## Fix tried in M3 and withdrawn
F6 as sketched in `m3-gate-diagnosis.md` (own-net end gap 0; also a variant giving the own radius back and letting the
strand overlap it by `min(w/2, r)`), ~120 lines in `graph.rs` / `search.rs` + tests. Measured: eprom two-layer
byte-identical (27/27, 64.628 mm); wiccon 60/60, vias 33 → 30, outside 1357 → 1352 mm, 0.8 s; #207 / #197 / #45 /
wiccon #52 find their direct paths on the empty graph. But `eprom_ripped_front_only_is_the_m2_result` (M2 regression
guard, 27/27 with the +5V zero-slack corridor) drops to 26/27 in **both** variants: an edge at U2.28 (THT, r 0.8)
becomes fit, the loop takes that corridor at ε, taut reports "wall edge grazed mid-span by 455000 nm" (the outline
wall, which the vertex-only capacity model cannot see; the old gap was accidental protection), the loop is stuck and
the zero-slack pass finds no candidate. #208 and #34 are unchanged by F6 (root cause 2 and the cap 0 tip). Not cheap
and bounded ⇒ reverted (patch kept in the session scratchpad only); nothing outcome-affecting is committed, so the R3
snapshot PNGs stand.

## M4 list (estimates)
- **M4-D1 own-net / same-net capacity, wall-aware** (root cause 1): own ends and same-net neighbours charge no gap and
  give the core radius back; the fit of an edge whose end is own-net must additionally respect walls that are not edge
  ends (outline, keepouts, long pad sides) — a wall query per edge at build time (`obs.query_walls` of the edge's
  bbox, cached per edge) or a los check in `leave_vertex`. Regression tests: 0.6 × 0.3 rect pads (search_layers (1)
  must flip to "fits, ghost not soft"), 0.5 mm track through a 0.6 mm roundrect tip, two same-net strands on one tip
  edge, eprom front-only 27/27 kept. 2–3 days; eprom two-layer identity to be re-verified (was identical).
- **M4-D2 state point and los convergence** (root cause 2, #208): `state_point` shrinks the usable sub-segment by every
  foreign vertex within `r + c + w/2 + eps` of the edge (`verts_near` query), and `los` returns `ok = false` when the
  bend loop does not converge (apex not approaching the target / cap hit) instead of accepting the accumulated length.
  Changes search geometry everywhere ⇒ full gate + eprom identity check. 1–2 days.
- **M4-D3 same-net sibling connections** (root cause 4, #207 / #34 / #52): let a connection terminate on same-net
  committed copper (hand-style T-junction, 0.9 mm instead of 5.5 mm for #207) or order a net's connections so the
  shared pad is left once; plus a rip-up pass within the private-badge-b budget (slot-search cost, already M4 per M3-R13). 3–5
  days.
- **M4-D4 detour reference** (root cause 3): a DRC-feasible reference (foreign constraint edges hard, clearance bends
  as in the slot search, no capacity / other nets) instead of the topological ghost, so the ratio measures the detour
  against the best possible path; and the caption should flag ratios whose ghost is < 2 mm. Needs a ruling (M3-R11
  defines the reference). 1 day.
