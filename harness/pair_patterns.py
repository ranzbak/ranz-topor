"""Via-pair local-minimum patterns. Neither is in the suite; the user decides which stays.

* `pair_move` (shopping list 5.8; NOT yet approved by the user, NEEDS USER APPROVAL): a wire A sits in a via pair only
  because two wires B1, B2 cross the stretch between its vias, and the optimum moves B1 and B2 together round A's end
  so A needs no via at all.
* `pair_swap` (shopping list 5.2 step 4; the user approved this pattern's CONCEPT on 2026-11-11, for diagnosis): two
  wires in each other's one-wire channel; restored unchanged from b5f41d3 below `pair_move` (see the note at the end of
  this docstring for why 5.8 built `pair_move` instead).

Luzin 2013 («Топологичность», docs/literature/luzin2013-flexible-any-direction.pdf, printed p. 104 = PDF p. 9,
fig. 21): (a) re-layering removes no via, (b) a single reroute of the short wire removes them at +36 % length, (c) no
vias at +7 % length "but it cannot be obtained by a single reroute of wires" (fig. 21c: the two long wires bend round
the end of the short one). Luzin, Popov & Popov 2013 (docs/literature/luzin-popov2013-local-via-minimization.pdf,
printed p. 134 = PDF p. 2, step 8.1, fig. 1): "on the layers Lv ∩ Lcross lead the wires crossing the wire joining v
and u round u", then copy the stretch onto Lv and delete the via.

`pair_move` (this file) is fig. 21 on a 2-layer board, rotated: B1, B2 are two 0.5 mm supply rails (net class PWR)
130 mm long across the board, A a 0.25 mm signal across them in the middle, its upper pad A1 40 mm above the rails,
its lower pad A2 27 mm below them. Every pad is SMD on F.Cu. Nothing else is on the board: no walls, no keep-outs;
every constraint is in the PNG (pads, widths, the copper-to-edge band).

* Expected (optimum, price = mm + 25 mm per via): A straight on F.Cu; B1 and B2 on F.Cu dip in a shallow V below A2
  (B1 inside, B2 outside): 0 vias, the rails about 23 mm longer than straight (any-angle; a 45-degree drawing of the
  same topology about 46 mm, still below the 50 mm of two vias).
* Careless = a local minimum of single-wire rip-up and reroute (measured, engine 09c18ca, TopoR loop, steiner off
  and on, seeds 0-2: 2 vias kept): everything straight, A dives under the rails (F.Cu, via, B.Cu, via, F.Cu).
  - A alone: round the rails' ends is about 80 mm longer than straight, more than its two vias even at the loop's
    highest via weight (1.5 x 25 mm per via).
  - B2 alone round A2: A still crosses B1 and keeps both vias; the reroute only earns the crossing it removes (the
    loop prices a crossing at loop_crossing 0.25 x crossing 20 mm x the via emphasis, at most about 10 mm with the
    coefficients' oscillation) and pays about 11.5 mm x 1.56 (a 0.5 mm rail's length weight) = 18 mm.
  - B1 alone round A2 must cross B2 twice.
  - Re-layering: two straight rails across A need A's two vias.
  Only B1 and B2 laid again together round A2 free A (Luzin-Popov 2013 step 8.1; [vias] pair_move).
* Why the router reaches the careless state first (not a hidden block, the route order): wide before thin (shopping
  list 4.3b, memory route-order-and-neck-rule), so the rails are laid first, straight, and A then dives. If the order
  ever lays A first, the rails go round its pad at once and the pattern still expects the optimum.
* Why so large: the three prices that make it a local minimum are fixed lengths (a via 25 mm, the crossing credit up
  to about 10 mm, A's way round the rails' ends against 2 vias at the highest via weight); a smaller board lets the
  sweep solve it one wire at a time (rails 74 mm, A2 1.8 mm below them: both rails went round A2 one after the other,
  each paid by its crossing) or makes A's way round the rails' ends cheapest.

Why 5.8 built `pair_move` beside `pair_swap` (b5f41d3, 5.2 step 4): in `pair_swap` the router reached the 0-via
answer without any pair move (the initial routing already swapped the two wires, measured in 5.2), so it does not
test a local-minimum escape, and it needs walls and plugs to force the situation. It is kept for the user to decide.

Rules and conventions as `harness/via_patterns.py` (rules, price, PNG). `python -m harness.pair_patterns [OUT]` writes
`OUT/pair_move/` and `OUT/pair_swap/` (default `scratch/pair-patterns`). Coordinates in mm, KiCad frame (y down).
Deterministic.
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from harness import via_patterns as vp
from harness.fit_patterns import Fp, Pad, Track

W_SIG = vp.W_NORMAL  # A: Default class 0.25
W_PWR = 0.50  # B1, B2: class PWR
SQ2 = math.sqrt(2.0)
SIZE = (136.0, 74.5)
XA = 68.0  # A's column (the middle)
YA_TOP = 3.0  # A1
Y_B1, Y_B2 = 43.0, 44.2  # the rails
YA_BOT = Y_B1 + 28.0 - 0.875  # A2: B1 passes 28 mm below its own line
XB_L, XB_R = 3.0, 133.0  # rail pads: 65 mm either side of A
SLACK = 0.025  # grid slack on every clearance line


def vee(y0: float, yb: float, xl: float, xr: float, x0: float, x1: float, keep) -> list[tuple[float, float]]:
    """A rail from (xl, y0) to (xr, y0) that dips to yb under x0..x1 along straight (any-angle) flanks: the flat run
    is widened symmetrically by the least amount (0.005 mm steps) for which `keep(polyline)` holds."""
    for k in range(4000):
        a = k * 0.005
        pts = [(xl, y0), (vp.r4(x0 - a), yb), (vp.r4(x1 + a), yb), (xr, y0)]
        if keep(pts):
            return pts
    raise AssertionError("no vee found")


def p_pair_move() -> vp.VPat:
    from shapely.geometry import LineString

    W, H = SIZE
    pad = vp.PAD
    fps = [vp.terminal("A1", "A", XA, YA_TOP), vp.terminal("A2", "A", XA, YA_BOT),
           vp.terminal("B1L", "B1", XB_L, Y_B1), vp.terminal("B1R", "B1", XB_R, Y_B1),
           vp.terminal("B2L", "B2", XB_L, Y_B2), vp.terminal("B2R", "B2", XB_R, Y_B2)]
    c = vp.CLEAR
    a2 = vp.pad_geo(fps[1].pads[0])
    a_track = LineString([(XA, YA_TOP), (XA, YA_BOT)])
    # Expected (any-angle, as the plan's taut paths run): below A2 first B1, then B2, each a clearance apart; straight
    # flanks from the rail pads to the flat run under A2.
    y1 = vp.r4(YA_BOT + pad / 2 + c + W_PWR / 2 + SLACK)
    y2 = vp.r4(y1 + W_PWR + c + SLACK)
    need_a = c + W_PWR / 2 + SLACK  # rail centre line to A's copper
    b1 = vee(Y_B1, y1, XB_L, XB_R, XA - pad / 2, XA + pad / 2,
             lambda pts: LineString(pts).distance(a2) >= need_a - 1e-6 and LineString(pts).distance(a_track) >= need_a + W_SIG / 2 - 1e-6)
    b1_line = LineString(b1)
    b2 = vee(Y_B2, y2, XB_L, XB_R, b1[1][0], b1[2][0],
             lambda pts: LineString(pts).distance(b1_line) >= W_PWR + c + SLACK - 1e-6 and LineString(pts).distance(a2) >= need_a - 1e-6)
    ref = vp.Sol("B1, B2 dip in a V below A's lower pad (fig. 21c); A straight on F.Cu: 0 vias", [
        Track("A", "F.Cu", W_SIG, [(XA, YA_TOP), (XA, YA_BOT)]),
        Track("B1", "F.Cu", W_PWR, b1), Track("B2", "F.Cu", W_PWR, b2)], [],
        "the rails bend round the end of the short wire (Luzin 2013 fig. 21c, Luzin-Popov 2013 step 8.1)")
    # Careless: all straight, A dives: via above B1, B.Cu under both rails, via between B2 and A2.
    v1 = (XA, vp.r4(Y_B1 - W_PWR / 2 - c - vp.MIN_VIA[0] / 2 - SLACK))  # just above B1
    v2 = (XA, vp.r4(Y_B2 + W_PWR / 2 + c + vp.MIN_VIA[0] / 2 + SLACK))  # just below B2
    car = vp.Sol("all straight, A dives under both rails: 2 vias (a local minimum of single-wire reroutes)", [
        Track("A", "F.Cu", W_SIG, [(XA, YA_TOP), v1]), Track("A", "B.Cu", W_SIG, [v1, v2]),
        Track("A", "F.Cu", W_SIG, [v2, (XA, YA_BOT)]),
        Track("B1", "F.Cu", W_PWR, [(XB_L, Y_B1), (XB_R, Y_B1)]),
        Track("B2", "F.Cu", W_PWR, [(XB_L, Y_B2), (XB_R, Y_B2)])],
        [vp.mkvia("A", v1), vp.mkvia("A", v2)],
        "no single wire's reroute gains: A round the rails' ends > 2 vias; B2 alone leaves B1 across A; B1 alone "
        "crosses B2 twice")
    nets = ["A", "B1", "B2"]
    pr, pc = vp.price(ref, nets), vp.price(car, nets)
    # The single-reroute alternative of A (fig. 21b): round the rails' nearer ends, as a taut any-angle estimate.
    xe = XB_L - pad / 2 - c - W_SIG / 2
    a_round = math.dist((XA, YA_TOP), (xe, Y_B1 - pad / 2)) + (Y_B2 - Y_B1 + pad) + math.dist((xe, Y_B2 + pad / 2), (XA, YA_BOT))
    straight = {n: vp.r4(v) for n, v in (("A", YA_BOT - YA_TOP), ("B1", XB_R - XB_L), ("B2", XB_R - XB_L))}
    exp = {
        "kind": "solvable", "layers": 2, "expected_vias": 0, "expected_crossings": 0, "same_layer_crossings": 0,
        "reference": pr, "careless": pc, "straight_mm": straight,
        "expected_shape": {"A": "straight on F.Cu, 0 vias", "B1, B2": "on F.Cu, both pass below A's lower pad A2 "
                           "(B1 inside, B2 outside); 0 vias; the V any-angle or 45 degrees"},
        "alternatives": {
            "A round the rails' ends (fig. 21b, one single reroute)": {"extra_mm_estimate": vp.r4(a_round - straight["A"]),
                                                                      "vs": "2 vias = 50 mm"},
            "rails round A's upper pad A1": "a deeper V (41 mm instead of 28): about twice as long",
            "rails dive under A": "4 vias",
            "A on B.Cu end to end": "2 vias anyway (A's pads are SMD on F.Cu)"},
        "local_minimum": "the careless state is what the router builds first (wide rails laid first, straight; A then "
                         "dives) and no single-wire reroute, re-layering or label move improves it (measured: engine "
                         "09c18ca, TopoR loop, steiner off and on, seeds 0-2, 2 vias kept); only B1 and B2 laid again "
                         "together round A2 (Luzin-Popov 2013 step 8.1, [vias] pair_move) reach the optimum",
        "judgement_calls": [
            "rails 0.5 mm (class PWR) so the route order (wide first) lays them first; with equal widths A (shorter) "
            "would be laid first and the rails would bend round it in the initial routing: no local minimum then",
            "rails 130 mm long with A in the middle so A's way round their ends (about +80 mm) costs more than its two "
            "vias at the loop's highest via weight (fig. 21b says +36 % in Luzin's case; here the prices set the scale)",
            "A2 27 mm below the rails so one rail's way round it (about 11.5 mm, x 1.56 for a 0.5 mm rail) costs more "
            "than the crossing it removes (loop_crossing 0.25 x crossing 20 mm x via emphasis up to 1.5, oscillating): "
            "with A2 close to the rails the sweep frees A one rail at a time and the pattern tests nothing",
            "price 25 mm per via is the user rule; the expected answer holds for any via price above about 12 mm "
            "(any-angle V) or 23 mm (45-degree V)"],
        "literature": [
            "Luzin 2013, «Топологичность» (docs/literature/luzin2013-flexible-any-direction.pdf), printed p. 104 = PDF "
            "p. 9, fig. 21a-c: re-layering removes no via; a single reroute removes them at +36 %; the +7 % variant "
            "without vias cannot be reached by a single reroute",
            "Luzin, Popov & Popov 2013 (docs/literature/luzin-popov2013-local-via-minimization.pdf), printed p. 134 = PDF "
            "p. 2, steps 1-11 and fig. 1: step 8.1 leads the wires crossing the stretch between two adjacent vias round "
            "the other via, then the stretch moves onto the neighbours' layer and the via goes",
            vp.LIT["topor63"]],
    }
    return vp.VPat("pair_move", "Two rails cross a short wire's stretch: they move together round its end, not the wire "
                   "down two vias", SIZE, fps,
                   [("A", "A1.1", "A2.1"), ("B1", "B1L.1", "B1R.1"), ("B2", "B2L.1", "B2R.1")], ref, car,
                   f"0 vias: A straight on F.Cu; B1 and B2 dip in a V below A's lower pad A2 on F.Cu; {pr['length_mm']:.2f} mm, "
                   f"price {pr['price']:.1f}. Careless (all straight, A dives under both rails, a local minimum of "
                   f"single-wire reroutes): 2 vias, {pc['length_mm']:.2f} mm, price {pc['price']:.1f}. A round the rails' "
                   f"ends instead: about +{a_round - straight['A']:.0f} mm.",
                   "All pads SMD on F.Cu. The rails are wide (0.5 mm) and laid first, straight; A then needs two vias to "
                   "pass under them. B2 alone round A2 leaves B1 across A (still two vias); B1 alone crosses B2; A round "
                   "the rails' ends is about 80 mm longer. Moving both rails together round A2 frees A for about 23 mm "
                   "of rail length (any-angle).",
                   exp, vp.classes([{"name": "PWR", "clearance": vp.CLEAR, "track_width": W_PWR,
                                     "via_diameter": vp.VIA_CLASS[0], "via_drill": vp.VIA_CLASS[1], "priority": 0}]),
                   net_patterns=[{"netclass": "PWR", "pattern": "B*"}],
                   zooms=[((XA - 12.0, Y_B1 - 4.0, XA + 12.0, YA_BOT + 4.0), "A's crossing with the rails / the rails round A2")],
                   colors={"A": vp.PALETTE[0], "B1": vp.PALETTE[1], "B2": vp.PALETTE[2]})


vp.BUILDERS["pair_move"] = p_pair_move


# ---- pair_swap (5.2 step 4, restored unchanged from b5f41d3; constants prefixed SW_). Two nets A and B run left to
# right through two one-wire channels between fixed walls; each channel closed on one layer by a plug; the swap
# (A through channel 2 on F.Cu, B through channel 1 on B.Cu) needs no via, each straight needs two.
SW_CH = 0.90  # channel width: one 0.25 mm track with 0.20 clearances needs 0.65, two need 1.10
SW_Y1, SW_Y2 = 4.0, 8.0  # channel centres
SW_X0, SW_X1 = 7.0, 15.0  # wall extent
SW_XP = 11.0  # plug centre
SW_SIZE = (22.0, 12.0)


def _rect(num, net, x0, y0, x1, y1, layer):
    return Pad(num, net, ((x0 + x1) / 2, (y0 + y1) / 2), (x1 - x0, y1 - y0), layer=layer)


def p_pair_swap() -> vp.VPat:
    W, H = SW_SIZE
    e = vp.EDGE
    c1, c2 = (SW_Y1 - SW_CH / 2, SW_Y1 + SW_CH / 2), (SW_Y2 - SW_CH / 2, SW_Y2 + SW_CH / 2)
    ov = 0.05  # plugs overlap their walls (one copper body per net)
    fps = []
    # F.Cu: top wall + plug in channel 1 + middle wall are one body (net WF1); the bottom wall is WF2.
    fps.append(Fp("WF1", [_rect("1", "WF1", SW_X0, e, SW_X1, c1[0], "F.Cu"), _rect("2", "WF1", SW_XP - 0.4, c1[0] - ov, SW_XP + 0.4, c1[1] + ov, "F.Cu"),
                          _rect("3", "WF1", SW_X0, c1[1], SW_X1, c2[0], "F.Cu")], "wall"))
    fps.append(Fp("WF2", [_rect("1", "WF2", SW_X0, c2[1], SW_X1, H - e, "F.Cu")], "wall"))
    # B.Cu: middle wall + plug in channel 2 + bottom wall (WB1); the top wall is WB2.
    fps.append(Fp("WB1", [_rect("1", "WB1", SW_X0, c1[1], SW_X1, c2[0], "B.Cu"), _rect("2", "WB1", SW_XP - 0.4, c2[0] - ov, SW_XP + 0.4, c2[1] + ov, "B.Cu"),
                          _rect("3", "WB1", SW_X0, c2[1], SW_X1, H - e, "B.Cu")], "wall"))
    fps.append(Fp("WB2", [_rect("1", "WB2", SW_X0, e, SW_X1, c1[0], "B.Cu")], "wall"))
    A1, B1, A2, B2 = (2.0, SW_Y1), (2.0, SW_Y2), (20.0, SW_Y1), (20.0, SW_Y2)
    fps += [vp.terminal("A1", "A", *A1), vp.terminal("B1", "B", *B1, layer="B.Cu"),
            vp.terminal("A2", "A", *A2), vp.terminal("B2", "B", *B2, layer="B.Cu")]
    w = W_SIG
    # Expected: swapped. A on F.Cu through channel 2, B on B.Cu through channel 1; two crossings on different layers.
    ref = vp.Sol("A through channel 2 on F.Cu, B through channel 1 on B.Cu: 0 vias, 2 crossings on different layers",
                 [Track("A", "F.Cu", w, [A1, (6.0, SW_Y2), (16.0, SW_Y2), A2]), Track("B", "B.Cu", w, [B1, (6.0, SW_Y1), (16.0, SW_Y1), B2])], [])
    va, vb, ua, ub = (5.0, SW_Y1), (5.0, SW_Y2), (17.0, SW_Y1), (17.0, SW_Y2)
    car = vp.Sol("each straight through its own channel (A channel 1, B channel 2): two vias each",
                 [Track("A", "F.Cu", w, [A1, va]), Track("A", "B.Cu", w, [va, ua]), Track("A", "F.Cu", w, [ua, A2]),
                  Track("B", "B.Cu", w, [B1, vb]), Track("B", "F.Cu", w, [vb, ub]), Track("B", "B.Cu", w, [ub, B2])],
                 [vp.mkvia("A", va), vp.mkvia("B", vb), vp.mkvia("A", ua), vp.mkvia("B", ub)],
                 "a local minimum of single-wire reroutes: moving A (or B) alone into the other channel over-fills it")
    pr, pc = vp.price(ref, ["A", "B"]), vp.price(car, ["A", "B"])
    exp = {
        "kind": "solvable", "layers": 2, "expected_vias": 0, "expected_crossings": 2, "same_layer_crossings": 0,
        "reference": pr, "careless": pc,
        "minimum_vias_proof": "walls on both layers leave only the two channels; channel 1 is F.Cu-closed by the plug, "
                              "channel 2 B.Cu-closed; A's pads are on F.Cu, B's on B.Cu: A in channel 2 and B in channel 1 "
                              "change no layer",
        "local_minimum": "each channel holds one 0.25 mm track (0.90 free, two need 1.10): from the careless state a "
                         "single-wire reroute of A into channel 2 (or B into channel 1) over-fills it; A and B must move "
                         "together (the pair move of brief 5.2 step 4)",
        "judgement_calls": ["the swap costs about 5.7 mm of length (the diagonals) and saves 2 vias (50 mm at 25 mm/via)",
                            "A's pads both on F.Cu, B's both on B.Cu: the swapped channels need no via, the own channels two each"],
        "literature": ["Polubasov 2010 (docs/literature/polubasov2010-topor-inside.html) «Оптимизация расслоения»: a "
                       "local minimum where no single-wire reroute improves the evaluation (fig. 7 point 1)",
                       vp.LIT["topor63"]],
    }
    return vp.VPat("pair_swap", "Two wires in each other's channel: a local minimum of single-wire reroutes", SW_SIZE, fps,
                   [("A", "A1.1", "A2.1"), ("B", "B1.1", "B2.1")], ref, car,
                   f"0 vias: A through channel 2 on F.Cu, B through channel 1 on B.Cu, crossing twice on different layers; "
                   f"{pr['length_mm']:.1f} mm, price {pr['price']:.1f}. Careless (the shortest paths laid one at a time, a local "
                   f"minimum of single-wire reroutes): 4 vias, {pc['length_mm']:.1f} mm, price {pc['price']:.1f}.",
                   "Each channel holds exactly one track and is closed on one layer by a plug; A's pads are on F.Cu, B's "
                   "on B.Cu, so the swapped channels need no via. Moving one wire alone into the "
                   "other channel over-fills it.",
                   exp, vp.classes(), zooms=[((0.5, 2.0, 8.0, 10.0), "left: dive sites / swap crossing"),
                                             ((9.0, 2.0, 13.0, 10.0), "plugs in the channels")],
                   colors={"A": vp.PALETTE[0], "B": vp.PALETTE[1], "WF1": vp.GREY, "WF2": vp.GREY, "WB1": "#b5b5b5", "WB2": "#b5b5b5"})


vp.BUILDERS["pair_swap"] = p_pair_swap


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "pair-patterns"
    for name in ("pair_move", "pair_swap"):
        g = vp.generate(name, out)
        doc = json.loads(g["expected"].read_text())
        print(name, doc["kicad"], doc["expected"]["reference"], doc["expected"]["careless"])
        print(g.get("png"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
