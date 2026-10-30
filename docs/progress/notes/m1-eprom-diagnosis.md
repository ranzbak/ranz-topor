# M1 eprom diagnosis — first smoother run (F, 2026-10-30)

Inputs: `.superpowers/sdd/2026-10-30-topor-m1-smoother/eprom-{model,report}.json`, `Options::default()` + `existing_copper=smooth`
(ε = `margin_nm` = 5 000). Traced with a throwaway crate (engine untouched) that replays `smooth()` and prints every tauten pass,
every self-check violation with the exact f64 gap, and the slack of each chain prim against the `ObstacleSet`. All nm, math frame.

## 1. Self-check vs taut stage: zero-slack tangency judged with zero tolerance (all 16 violations)
Every violation is `gap == required` after rounding; exact gaps: **254 999.60 … 254 999.94 nm vs 255 000** (short by
0.06–0.57 nm) and one 254 997.69 (short 2.3 nm, the bogus loop arc of §2). The chains sit exactly on ρ (`tight: d − ρ = −0.0000`):
`chain_geometry` puts the tangent at ρ = r + clr + w/2 + ε, `tauten` tolerates `PEN_TOL = 1 nm`, `quantise_chain` rounds endpoints
(≤ 0.71 nm) and rebuilds short arcs from 3 integer points (wire 25's 0.088 rad arc: centre moves 0.4 µm, points stay within 1 nm), and
`Checker::required` demands `clr + ε` with a strict `gap < req`. Net slack = 0 ± rounding → fails ~always when a chain touches anything.
Refuted alternatives: no obstacle is missing (every partner is in the set and in the seq), vias are anchored, `query_grow` = 1 155 000 ≥
ρ − r, clearance semantics agree (both 255 000), hole/via clearances are not involved. Wires 16, 24, 25, 26 are geometrically correct.

## 2. Wire 19 (A12) carries a 354° loop: drop rule (d) misclassifies a *negative* arc as a legitimate loop
Chain `SASASAS`, arc 2 = `c(189.9,−72.4) r 0.78 Cw sweep 6.1854 rad` (4.82 mm of copper circling GND via 94574eeb). The drop test
reaches (c) with the shortcut 1 000 289 nm from the via centre vs R = 780 000 (220 µm slack, far outside the 10 µm band), then (d)
sees `sweep > π` → "looping" → bbox of triangle (U1:1, via, U2:24) grown by R spans 13.7 × 7.2 mm and contains foreign pads → not dropped.
The arc is not a loop: its exit tangent lies 5.6° *behind* its entry (t_out before t_in in σ order), i.e. the wrap angle is −6° and
`from_tangent_points` folds it to 2π − 6°. Once (b) (centre on the σ side of the shortcut) and (c) (shortcut clear of the disc) hold, a
wrap of < π is the only consistent geometry; a genuine > π wrap has p, q on the same side (fails (b)) or the shortcut cuts the disc
(fails (c)). The self-check only rejected this chain by the §1 sub-nm artefact — with §1 fixed alone the loop would be emitted.
Iteration 1 also failed (`Infeasible U2:24<->U2:23`) because the conservative initial stack membership put U2:24 at R = 1.685 (wire 20
listed on its fan, but it never wraps it); iteration 2 self-heals. Harmless, noted.

## 3. +5V wires 22/23: the hand runs in a zero-slack channel; the wall add rule then explodes
Hand copper along y = 62.45 (KiCad frame): edge at 62.0 → copper gap exactly 0.25; J1:1 top edge 62.90 and J1:3/J1:4 (r 0.85 at
63.75) → copper gap exactly 0.25. Centreline channel at design rules = [62.45, 62.45], width **0**. With ε on both sides the pass-0 chains
(wire 22 `SASASAS` 10.33 mm via→J1:1 corners; wire 23 `SASAS` 12.00 mm J1:4→via) graze the edge wall by exactly **2ε = 10 µm**
(arc apex −62.445 vs limit −62.455). `penetrations` then adds the wall's *nearer endpoint anchor* = a board corner 5–6 mm away
(`edge[152]`/`[155]`), the chain swings across the board, pass 2 adds 20+ pads/holes, pass 3 is `Infeasible` between two of them —
hence the nonsense labels `U2:7<->U1:5` and `J1:1<->hole`. Not implicated: terminal inside foreign inflation (no `InsideObstacle`),
hole_clearance (J1:3 hole ρ 1.055 < pad ρ 1.305; J1:1 hole limit ends 0.205 past the pad edge < wall ρ 0.455; via hole ρ 0.55+w/2 <
copper 0.655+w/2 — **never binds** because annular 0.2 + copper clearance 0.25 = 0.45 > 0.35). The floor question: KiCad's
`hole_clearance` is hole-edge-to-foreign-copper for *every* hole (PTH included), so the PTH figure is the right value for that single
rule; the NPTH figure would under-constrain small-annular PTH pads. Coordinator hypothesis refuted; the 0.35 floor costs nothing here.
These two wires are honestly infeasible at any ε > 0 (and at ε = 0 only an axis-aligned integer line fits): fallback is the right outcome.

## 4. Arcs and length
Chains before the self-check hold 9 arcs (16: 2, 19: 3 incl. the loop, 24: 1, 25: 2, 26: 1); 0 emitted because all five arc-bearing
wires died in §1. The 20 taut wires are obstacle-free 2.0047 mm hops (pattern `S`, sketch seq empty) — no wrap expected.
Length (centre-to-centre on both sides, as `hand_len` is built): hops 2.0047 vs 2.016–2.068; w16 9.19 vs 12.06; w24 4.09 vs 4.37;
w25 10.59 vs 11.03; w26 7.12 vs 7.44; w19 24.29 vs 21.62 **only because of the loop** (≈ 19.5 without it). In-pad copper is
2 × r per wire = **43.4 mm of the 27-wire total** (1.6 of every 2.0 mm hop). Projection after fixes 1–3 (22/23 fallback): emitted ≈ 115.0 mm
≤ 121.46 mm hand centre-to-centre, but > 112.886 mm KiCad hand track length, because 8.6 mm of hand "connector stubs" are not track
copper. The gate must compare like for like. Principled fix: report both lengths **outside pad copper** (clip the hand polyline and the
emitted prims at the terminal pad skeleton; `dist_prim_skeleton ≤ 0` is inside) and gate on those; keep emitting to the pad centre
(KiCad convention, what the hand does for 20/27 wires, robust connectivity). Clipping the emitted copper at the pad edge is possible
but risks connectivity (end must stay inside copper) and changes nothing electrically — not for M1.

## 5. Fix plan (minimal, ordered; every loop stays capped; no clearance relaxed)
1. **`taut.rs::is_slack`** — delete the looping/bbox branch: after (b)+(c) hold, `sweep > π` is a negative arc; always use the centre-
   triangle test (e) then (f). Test `tests/taut_chain.rs::negative_arc_is_dropped`: start (186,−69.79), goal (202.74,−78.62), anchors
   U1:1 r_core 0.8 at (187.5,−71) Cw, via r 0.4 at (189.9,−72.4) Cw, U2:24 r 0.8 at (201.24,−77.41) Ccw, foreign pads at (187.5,−73.54)
   and (201.24,−72.33), clr 250 000, w 250 000, ε 5 000 → pattern `SASAS`, no arc with sweep > π, length < 20 mm. Eprom: w19 24.29 → ≈ 19.5 mm.
2. **`smooth.rs::smooth`** — run the self-check with `Options { margin_nm: 0, ..opts.clone() }` (design rules; ε is build headroom, not a
   rule — the engine's rounding budget is ε − 2 nm). **Decision for coordinator**: self-check at design clearance rather than at ε.
   `py.rs::check_items` unchanged. Test `tests/smooth_engine.rs::tangent_chain_passes_self_check`: the `l_route` fixture plus a foreign
   pad tangent to the straight hop (centre distance exactly r + clr + w/2 + ε) → status `taut`, and `Checker::new(…, ε)` on the output
   reports only sub-2 nm shortfalls (documents why). Eprom: wires 16, 19, 24, 25, 26 taut → 25/27 wires, 24/25 nets, 8 arcs.
3. **`taut.rs::penetrations`/`tauten`** — wall grazed mid-span: when the chosen endpoint anchor does not cover the penetration point
   (`point_prim(c_e, prim) ≥ rho_anchor(e) + SLACK_BAND_NM`) return `Err(Infeasible("wall <label> grazed mid-span by <d> nm"))` instead of
   adding it (one pass, not 64). Test `tests/taut_chain.rs::wall_grazed_mid_span_fails_fast` (wall (−5,y)→(15,y), straight chain
   10 µm inside ρ_wall at x = 5) → `Infeasible` containing "mid-span", pass count 1. Eprom: wires 22/23 report the true 10 µm cause.
   Follow-up (M1.5 design): foot anchors on walls so a parallel run (bitangent of two equal foot circles) is expressible.
4. **`smooth.rs` report** — add `length_outside_pads_nm` / `hand_outside_pads_nm` (wire, net, totals); harness gate uses them.
   Test `tests/smooth_engine.rs::outside_pad_length_excludes_pad_radius`: 1.6 mm pads 2.0047 mm apart → 404 700 nm. **Decision for
   coordinator**: gate = emitted-outside ≤ hand-outside (projection: ≈ 71.6 vs ≈ 78 mm).
5. Keep `hole_clearance` floor 0.35 (PTH); not implicated. Stack-order fan weakness, single-ε `dyn_clearance`, ε-band hysteresis, "SS"
   merge: not implicated on this board (no shared wrapped anchor, nearest wire pair 2.5 mm apart).

Expected eprom after 1–4: 25/27 wires, 24/25 nets taut (+5V partial: 1/3), 8 arcs, 0 self-check failures, every taut wire shorter than
its hand wire; +5V 22/23 fallback with reason "wall edge grazed mid-span by 10000 nm" (hand at zero slack).
