"""PROPOSED test patterns D8 for TW5 ESCAPE (docs/dev/design/tw5-escape.md), from Polubasov 2010 «TopoR inside»
(Электроника НТБ 6/2010 pp. 104-109, https://www.electronics.ru/journal/article/112; figures on pp. 107-108).

NOT in the test suite: the user approves patterns first (plan decision D8, AGENT_RULES). Built like harness/fx_survivors.py
(reference copper first, pads around it, KiCad DRC with --drc). Two layers; F.Cu = the paper's red layer, B.Cu = green.
Thick circles in the paper = THT pads (here 1.6 mm, drill 0.8), red rectangles = SMD pads on the red layer (1.0 x 0.7 mm).

1. `layering_zero_vias`   fig. 10: wire 1 blocks wire 3 with no via on the board; the layering flips wires 1 and 2
                          (a part without SMD pad: both layers equal), then wire 3 goes straight (fig. 10c, 0 vias).
2. `oscillating_cross`    fig. 11: two SMD wires cross; one of them dives with two vias; the two equal answers must not
                          alternate cycle after cycle (re-lay counter, fading).
3. `layering_escape_fig8` figs. 8-9: five wires, the paper's local minimum has 3 vias; the layering with the same 3 vias
                          lets wires 5 and 2 be laid again better (9b, 9c: 0 vias). Scale from the figure (1 mm = 10 px).

    python -m harness.tw5_patterns [OUT] [--drc]
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from harness.fit_patterns import CLEAR, DEFAULT_CLASS, W_SIG, Fp, Pad, Pattern, Track, Via, write_board
from harness.fx_survivors import clearance_report
from harness.radii_patterns import EPS

STATUS = "PROPOSED (awaiting approval)"
THT, DRILL, SMD = (1.6, 1.6), 0.8, (1.0, 0.7)
SRC = "Polubasov 2010, «TopoR inside», Электроника НТБ 6/2010"


def _tht(ref, net, pos):
    return Fp(ref, [Pad("1", net, pos, THT, "circle", drill=DRILL)], "terminal")


def _smd(ref, net, pos):
    return Fp(ref, [Pad("1", net, pos, SMD)], "terminal")


def _len(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


# ------------------------------------------------------------------------------------------------ 1. fig. 10
def layering_zero_vias() -> Pattern:
    W, H = 16.0, 11.0
    a, b, c = [(2.0, 5.5), (12.0, 5.5)], [(5.0, 2.0), (5.0, 9.0)], [(8.5, 1.5), (8.5, 9.5)]
    fps = [_tht("J1", "W1", a[0]), _tht("J2", "W1", a[1]), _tht("J3", "W2", b[0]), _tht("J4", "W2", b[1]),
           _smd("U1", "W3", c[0]), _smd("U2", "W3", c[1])]
    tr = [Track("W1", "B.Cu", W_SIG, a), Track("W2", "F.Cu", W_SIG, b), Track("W3", "F.Cu", W_SIG, c)]
    return Pattern(
        name="layering_zero_vias", title="Fig. 10: layering helps with no via on the board", kind="solvable",
        gap_classes=["TW5 (b) layering escape: a part without SMD pad flips as a whole (same vias), opening a straight path"],
        purpose="Two layers. W1 (THT J1 - J2, horizontal) and W2 (THT J3 - J4, vertical) cross; W3 runs between two SMD pads on "
                "F.Cu (U1 above W1, U2 below). The paper's local minimum (fig. 10a): W1 on F.Cu, W2 on B.Cu, so W3 (F.Cu: its "
                "pads) detours around J2. Its straight path crosses W1 on the same layer; laying W3 again alone cannot help "
                "(W1 cannot change layer without W2). The layering escape flips W1 and W2 together (fig. 10b, still 0 vias), "
                "then W3 is laid straight (fig. 10c).",
        answer="W1 straight on B.Cu (10 mm), W2 straight on F.Cu (7 mm), W3 straight on F.Cu (8 mm). 0 vias, DRC 0.",
        why="W3 has SMD pads on F.Cu and 0 vias is possible, so W3 is on F.Cu; it crosses W1, so W1 is on B.Cu; W1 crosses W2, "
            "so W2 is on F.Cu. All three straight: the shortest possible, unique.",
        size=(W, H), layers=2, nets=["W1", "W2", "W3"], fps=fps,
        connections=[("W1", "J1.1", "J2.1"), ("W2", "J3.1", "J4.1"), ("W3", "U1.1", "U2.1")], cuts=[],
        classes=[dict(DEFAULT_CLASS)], ref_tracks=tr, ref_vias=[],
        witness={t.net: (t.layer, t.pts) for t in tr},
        extra={"tw5": {"figure": "10", "source": SRC + ", p. 108 fig. 10 (text p. 107)", "ref_len": 25.0, "vias": 0}})


# ------------------------------------------------------------------------------------------------ 2. fig. 11
def oscillating_cross() -> Pattern:
    # The pads 0.55 mm from the board edge: no wire passes between a pad and the edge (0.5 + 0.25 + 0.2 needed), so
    # neither wire can go round the other (tw5r5: with room there topor took a 0-via detour, +4.3 mm).
    W, H = 16.0, 12.0
    a, b = [(1.05, 6.0), (14.95, 6.0)], [(8.0, 1.05), (8.0, 10.95)]
    d = 0.3 + CLEAR + W_SIG / 2 + EPS  # via radius + clearance + half width: the dive's vias next to W1
    fps = [_smd("U1", "W1", a[0]), _smd("U2", "W1", a[1]), _smd("U3", "W2", b[0]), _smd("U4", "W2", b[1])]
    y0, y1 = round(6.0 - d, 4), round(6.0 + d, 4)
    tr = [Track("W1", "F.Cu", W_SIG, a), Track("W2", "F.Cu", W_SIG, [b[0], (8.0, y0)]), Track("W2", "B.Cu", W_SIG, [(8.0, y0), (8.0, y1)]),
          Track("W2", "F.Cu", W_SIG, [(8.0, y1), b[1]])]
    return Pattern(
        name="oscillating_cross", title="Fig. 11: two equal answers must not alternate", kind="solvable",
        gap_classes=["TW5 (b) via-rose re-lays with the re-lay counter (fading): no A-B-A oscillation"],
        purpose="Two layers, SMD pads only (F.Cu). W1 (U1 - U2, horizontal) and W2 (U3 - U4, vertical) cross: one of them dives "
                "to B.Cu with two vias. The two answers cost the same; in the paper laying the wire with the vias again moves the "
                "vias to the other wire, and back (fig. 11): the re-lay counter stops that.",
        answer=f"Both straight; one wire (either) dives with two vias {2 * d:.3f} mm apart, centred on the other wire (W2 shown). "
               "2 vias, DRC 0; the same answer after every later cycle (no alternation).",
        why="Two SMD wires on one layer that cross need exactly one dive of two vias; straight is shortest; the dive is as short "
            "as the vias' clearance to the crossed wire allows.",
        size=(W, H), layers=2, nets=["W1", "W2"], fps=fps,
        connections=[("W1", "U1.1", "U2.1"), ("W2", "U3.1", "U4.1")], cuts=[],
        classes=[dict(DEFAULT_CLASS)], ref_tracks=tr, ref_vias=[Via("W2", (8.0, y0)), Via("W2", (8.0, y1))],
        witness={"W1": ("F.Cu", a), "W2": ("F.Cu", b)},
        extra={"tw5": {"figure": "11", "source": SRC + ", p. 108 fig. 11", "ref_len": 23.8, "vias": 2,
                       "also_optimal": "W1 dives instead of W2 (mirror)"}})


# ------------------------------------------------------------------------------------------------ 3. figs. 8-9
def layering_escape_fig8() -> Pattern:
    """Figure 8's pads at its scale (1 mm = 10 px of the 143 x 107 px figure), shifted 1 mm right and down."""
    W, H = 16.5, 12.5
    p = {"S1a": (2.1, 11.2), "S1b": (7.1, 5.4), "T2a": (5.0, 4.0), "T2b": (13.4, 11.1), "T3a": (5.0, 6.9), "T3b": (14.8, 6.9),
         "T4a": (9.3, 2.5), "T4b": (9.3, 8.3), "S5a": (11.5, 2.5), "S5b": (11.5, 8.4)}
    fps = [_smd("U1", "W1", p["S1a"]), _smd("U2", "W1", p["S1b"]), _tht("J1", "W2", p["T2a"]), _tht("J2", "W2", p["T2b"]),
           _tht("J3", "W3", p["T3a"]), _tht("J4", "W3", p["T3b"]), _tht("J5", "W4", p["T4a"]), _tht("J6", "W4", p["T4b"]),
           _smd("U3", "W5", p["S5a"]), _smd("U4", "W5", p["S5b"])]
    # The optimum at this scale (0 vias): W2 on F.Cu through the gap between U2 and W4, not the paper's left loop (fig. 9c).
    w2 = [p["T2a"], (8.0, 4.0), (8.0, 10.0), (13.4, 10.0), p["T2b"]]
    tr = [Track("W1", "F.Cu", W_SIG, [p["S1a"], (2.1, 5.4), p["S1b"]]), Track("W2", "F.Cu", W_SIG, w2),
          Track("W3", "B.Cu", W_SIG, [p["T3a"], p["T3b"]]), Track("W4", "F.Cu", W_SIG, [p["T4a"], p["T4b"]]),
          Track("W5", "F.Cu", W_SIG, [p["S5a"], p["S5b"]])]
    loop = [p["T2a"], (3.6, 4.0), (3.6, 10.0), (13.4, 10.0), p["T2b"]]
    return Pattern(
        name="layering_escape_fig8", title="Figs. 8-9: the same 3 vias, another layering, then two better re-lays", kind="solvable",
        gap_classes=["TW5 (b) layering escape (random via-optimal layering) + via-rose wires laid again first"],
        purpose="Two layers, five wires at the figure's scale. The paper's local minimum (fig. 8): W1 2 vias, W4 1 via (3 vias), "
                "W5 loops round the whole group on F.Cu. One of the 12 layerings with 3 vias (fig. 9a) lets W5 (9b) and then W2 "
                "(9c) be laid again better: 0 vias.",
        answer=f"0 vias. W1 F.Cu up and right (10.8 mm), W3 B.Cu straight (9.8 mm), W4 and W5 F.Cu straight (5.8, 5.9 mm), W2 on "
               f"F.Cu right of U2, down between U2 and W4 across W3, along y 10 and into J2 ({_len(w2):.1f} mm).",
        why="W5 (SMD on F.Cu) and W4 straight are crossed only by W3, so W3 is on B.Cu; W1 (SMD) on F.Cu; W2 cannot pass right of "
            "J4 (copper to edge) and must cross W3's line on F.Cu or loop left of J3 on B.Cu round W1 (the paper's fig. 9c, "
            f"{_len(loop):.1f} mm): at this scale the F.Cu way is {_len(loop) - _len(w2):.1f} mm shorter with no via. The paper's "
            "9c is the better local state its loop found, not the optimum; decide which end state the test expects.",
        size=(W, H), layers=2, nets=["W1", "W2", "W3", "W4", "W5"], fps=fps,
        connections=[("W1", "U1.1", "U2.1"), ("W2", "J1.1", "J2.1"), ("W3", "J3.1", "J4.1"), ("W4", "J5.1", "J6.1"),
                     ("W5", "U3.1", "U4.1")], cuts=[],
        classes=[dict(DEFAULT_CLASS)], ref_tracks=tr, ref_vias=[],
        witness={t.net: (t.layer, t.pts) for t in tr},
        extra={"tw5": {"figure": "8, 9a-c", "source": SRC + ", p. 107 figs. 8-9", "ref_len": round(sum(_len(t.pts) for t in tr), 2),
                       "vias": 0, "paper_end_state_w2": loop}})


PATTERNS = {"layering_zero_vias": layering_zero_vias, "oscillating_cross": oscillating_cross, "layering_escape_fig8": layering_escape_fig8}


def generate(name: str, out: Path, *, drc: bool = False) -> dict:
    pat = PATTERNS[name]()
    bad = clearance_report(pat)
    if bad:
        raise SystemExit(f"{name}: reference breaks a clearance: {bad}")
    d = Path(out) / name
    paths = {"problem": write_board(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_board(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)}
    doc = {"name": pat.name, "title": pat.title, "status": STATUS, "purpose": pat.purpose, "expected": pat.answer, "why": pat.why,
           "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
           "reference_tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": t.pts} for t in pat.ref_tracks],
           "reference_vias": [list(v.pos) for v in pat.ref_vias], **pat.extra.get("tw5", {})}
    if drc:
        from harness.drc import run_drc

        rep = run_drc(paths["reference"], d / "reference" / "drc.json")
        doc["reference_drc"] = {"violations": len(rep.violations), "unconnected": len(rep.unconnected),
                                "detail": [str(v) for v in rep.violations][:10]}
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    return paths


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    drc = "--drc" in argv
    argv = [a for a in argv if a != "--drc"]
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "tw5-patterns" / "boards"
    for name in PATTERNS:
        print(name, *(str(p) for p in generate(name, out, drc=drc).values()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
