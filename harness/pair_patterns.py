"""Pair-swap pattern (shopping list 5.2 step 4, concept approved by the user 2026-11-11 for diagnosis): a local
minimum of single-wire rip-up and reroute that only a move of two wires together leaves.

Two nets A and B run left to right through two one-wire channels between fixed walls (fixed copper on both layers).
In channel 1 an F.Cu plug closes F.Cu (a wire there must be on B.Cu), in channel 2 a B.Cu plug closes B.Cu (F.Cu
there). A's SMD pads are on F.Cu, B's on B.Cu. Straight through their own channels (A channel 1, B channel 2) each needs two
vias (A: F.Cu pad -> B.Cu under the F plug -> F.Cu pad; B the other way round). Swapped (A through channel 2 on F.Cu, B through channel 1 on B.Cu) both vias vanish; A and B then
cross twice, on different layers (legal). A single-wire reroute of either cannot reach the swap: the other channel
holds one wire, so moving one wire there over-fills it (a violation), and every other way is closed by the walls.
TopoR's three procedures (Polubasov 2010, docs/literature/polubasov2010-topor-inside.html: single-wire reroute with
social factors, layer re-optimisation, floating up) change one wire's path or only layers; the expected answer needs
both paths changed at once (Polubasov 2010 figs 7-9: a local minimum of the evaluation function).

Rules and conventions as `harness/via_patterns.py` (walls are pads of single-purpose nets, no hidden keepouts).
`python -m harness.pair_patterns [OUT]` writes `OUT/pair_swap/` (default `scratch/pair-patterns`). Coordinates in mm,
KiCad frame (y down). Deterministic.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

from harness import via_patterns as vp
from harness.fit_patterns import Fp, Pad, Track

W_SIG = vp.W_NORMAL
CH = 0.90  # channel width: one 0.25 mm track with 0.20 clearances needs 0.65, two need 1.10
Y1, Y2 = 4.0, 8.0  # channel centres
X0, X1 = 7.0, 15.0  # wall extent
XP = 11.0  # plug centre
SIZE = (22.0, 12.0)


def _rect(num, net, x0, y0, x1, y1, layer):
    return Pad(num, net, ((x0 + x1) / 2, (y0 + y1) / 2), (x1 - x0, y1 - y0), layer=layer)


def p_pair_swap() -> vp.VPat:
    W, H = SIZE
    e = vp.EDGE
    c1, c2 = (Y1 - CH / 2, Y1 + CH / 2), (Y2 - CH / 2, Y2 + CH / 2)
    ov = 0.05  # plugs overlap their walls (one copper body per net)
    fps = []
    # F.Cu: top wall + plug in channel 1 + middle wall are one body (net WF1); the bottom wall is WF2.
    fps.append(Fp("WF1", [_rect("1", "WF1", X0, e, X1, c1[0], "F.Cu"), _rect("2", "WF1", XP - 0.4, c1[0] - ov, XP + 0.4, c1[1] + ov, "F.Cu"),
                          _rect("3", "WF1", X0, c1[1], X1, c2[0], "F.Cu")], "wall"))
    fps.append(Fp("WF2", [_rect("1", "WF2", X0, c2[1], X1, H - e, "F.Cu")], "wall"))
    # B.Cu: middle wall + plug in channel 2 + bottom wall (WB1); the top wall is WB2.
    fps.append(Fp("WB1", [_rect("1", "WB1", X0, c1[1], X1, c2[0], "B.Cu"), _rect("2", "WB1", XP - 0.4, c2[0] - ov, XP + 0.4, c2[1] + ov, "B.Cu"),
                          _rect("3", "WB1", X0, c2[1], X1, H - e, "B.Cu")], "wall"))
    fps.append(Fp("WB2", [_rect("1", "WB2", X0, e, X1, c1[0], "B.Cu")], "wall"))
    A1, B1, A2, B2 = (2.0, Y1), (2.0, Y2), (20.0, Y1), (20.0, Y2)
    fps += [vp.terminal("A1", "A", *A1), vp.terminal("B1", "B", *B1, layer="B.Cu"),
            vp.terminal("A2", "A", *A2), vp.terminal("B2", "B", *B2, layer="B.Cu")]
    w = W_SIG
    # Expected: swapped. A on F.Cu through channel 2, B on B.Cu through channel 1; two crossings on different layers.
    ref = vp.Sol("A through channel 2 on F.Cu, B through channel 1 on B.Cu: 0 vias, 2 crossings on different layers",
                 [Track("A", "F.Cu", w, [A1, (6.0, Y2), (16.0, Y2), A2]), Track("B", "B.Cu", w, [B1, (6.0, Y1), (16.0, Y1), B2])], [])
    va, vb, ua, ub = (5.0, Y1), (5.0, Y2), (17.0, Y1), (17.0, Y2)
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
    return vp.VPat("pair_swap", "Two wires in each other's channel: a local minimum of single-wire reroutes", SIZE, fps,
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
    g = vp.generate("pair_swap", out)
    doc = json.loads(g["expected"].read_text())
    print("pair_swap", doc["kicad"], doc["expected"]["reference"], doc["expected"]["careless"])
    print(g.get("png"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
