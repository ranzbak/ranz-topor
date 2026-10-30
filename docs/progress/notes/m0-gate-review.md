# M0 gate review (F, 2026-10-30, main@e4fed3c, read-only)

## Verdict: PASS WITH CONDITIONS
Gate criteria (45 §1) met on the coordinator's run (gate-run.txt: cargo 75 tests green, pytest 63 default +
9 gate/scaling green). Numbers signed off: fuzz fixture seeds 1/2/3 = 1000/2814, 1000/2544, 1000/3620 attempts,
truncated=false, routing_new=0; eprom (ripped) 1000/20252, routing_new=0; negative-margin control (−20 µm)
59–98 clearance violations (oracle sees the items). Baseline reproduced: eprom 80/0/0 segs/arcs/vias, 189.934 mm,
DRC 93, RMSE 0.0036; wiccon 381/0/36, 1831.971 mm, DRC 48, free vias 11/25 exact, RMSE 0.0003; dcsgone
1369/14/203, 4167.082+43.197 mm, DRC 192 (plan 210, diff only in 3 library/env types), RMSE 0.0025.
Runtime 5.1 / 6.5 / 12.3 s (< 60 s). Grid 1k→100k: candidates/query 4.87→4.92 (1.01×), 4.29e-4→5.10e-4 s (1.19×).
Conditions: C1–C3 below must be closed in M1 before the checker is trusted with vias/multiple netclasses.

## Complexity (45 §5a)
- fuzz.rs: single `while` capped by `attempts >= cap` (50·n), counter incremented before every `continue`;
  self-check via Checker grids + own per-layer Grid (never all-pairs). OK. Minor: own cell is a 2 mm constant
  (fuzz.rs:146), not derived from the rules; a netclass clearance+margin > 2 mm would be missed by the
  3×3 query (fuzz-only, not the checker).
- check.rs: every test is grid query → exact gap; items vs earlier items through `seen_grids`. No O(n²) in
  item or obstacle count. Polygon costs are O(vertices) per candidate (edge obstacles + `region_hit` walks the
  same edges again + `point_in_poly`): bounded, but redundant. Cell = max_w+max_clr+margin; `hole_to_hole` is
  not in `max_clr` (check.rs:183–208) — covered today only because max_w ≥ via Ø; add it explicitly.
- grid.rs: insert bounded by MAX_ITEM_CELLS (65 536) else side list; query O(min(box cells, occupied cells))
  + |big|; clamped i64 indices. OK. The 20× seconds bound (test_grid_scaling.py:12) is loose but the 4×
  candidates bound is the real O(1) assertion.
- dist.rs: closed-form, finite candidate sets, no iteration. OK.

## Ledger rulings (coordinator rotation 2)
- manylinux_2_34 wheel tag: agree (pip accepts on glibc ≥ 2.34; fallback documented).
- build.sh with KICAD_DOCUMENTS_HOME=scratchpad: agree.
- Checker validated end-to-end by the gate, no T6 re-review: agree for M0 only. The gate exercises tracks of
  non-zero nets vs pads/holes/kept copper/vias/zone/edge on one netclass; it does not exercise via items,
  multiple netclasses, keepouts or net ties — see C1–C3.
- track_dangling ≥ 1 instead of ≥ 1000: agree (KiCad merges touching same-net items; criterion is
  routing_new == 0). Add `board track count == 1000` after `write_items` so a partial drop is caught directly.
- dcsgone DRC 192 vs 210: agree (per-type equality outside courtyards/lib/thermal is the right assertion).
- dcsgone free vias 2/201 vs 7/196: agree with deferring the rip policy, disagree with the assertion.
  `kept + removed == 203` is a tautology (every via is one or the other) — pin the measured (2, 201) with
  the DEVIATION comment so a regression is visible. `GetIsFree()` is KiCad's "free via" flag, which stitching
  vias placed with the via tool do not carry; README step 1 ("keep stitching/fence vias") implies "no track
  of its net touches it on the original board" (worker's endpoint-free count 6 is the closest). Must be
  settled before M3 (dcsgone, bornhack ~877 stitching vias) — not an M0 blocker.
- Image compare flatten+crop: agree (RMSE ≤ 0.0036 after; 0.02 threshold generous but fine for hand images).

## Findings
- C1 Important (M1 blocker) check.rs:256 — pad obstacle `own_clr` = local override or 0; the pad's own
  netclass clearance is omitted. KiCad takes max(item class, pad class); a pad on a class with the larger
  clearance is under-checked by the checker AND by fuzz placement (model.rs:402 same formula). Passed only
  because all gate boards resolve to one clearance. Fix: `local.unwrap_or(clr(p.net))`.
- C2 Important (M2) check.rs:270–283, 600–621 — existing vias are copper-only obstacles (model.holes holds
  pad drills only, model.py:159) and new vias inserted into `seen` carry no hole: new via vs existing/new via
  hole-to-hole and new track vs via hole_clearance are unchecked; same-net exemption then lets two new
  same-net vias overlap. Safe today only because annulus ≥ 0.05 mm makes copper clearance imply hole
  clearance. M1 fuzz must emit vias (ledger T6 deferred item).
- C3 Minor check.rs:426–447 — containment by start point is sound for closed outer rings (a connected prim
  not touching an edge is wholly in or out), but outer-ring-only export makes a polygon hole a false
  positive; keepout `req` reported as 0. Acceptable for M0.
- Minor tests/gate/test_m0_gate.py:103 — tautological assertion (see ruling above).
- Minor fuzz coverage — net 0 items, keepouts and net-tie pads are never generated; the gate cannot see
  checker errors there.
