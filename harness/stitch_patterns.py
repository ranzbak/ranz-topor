"""PROPOSED test pattern for stitch-vias (docs/dev/design/stitch-vias.md): one 2-layer board, GND poured on F.Cu and
B.Cu ("remove islands: always"), kept signal copper on F.Cu cutting the F.Cu pour into the four kinds of piece the
stitching handles. NOT in the test suite: the user approves the PNG first (AGENT_RULES "New test patterns need user
approval").

    python -m harness.stitch_patterns [OUT]      # OUT/stitch_pieces/problem/stitch_pieces.kicad_pcb + expected.json

Pieces (kept copper: a closed loop of a signal net between two pads 1 mm apart, so the fill cannot pass the gap):
  A cut:     F.Cu loop [14, 4]-[24, 12] around the GND SMD pad C1.1: KiCad keeps the piece (it holds a pad), it is cut off
             -> class 1, at least 2 vias (one per 25 mm²), the first near C1.1;
  B dropped: F.Cu loop [28, 4]-[38, 14], no GND copper inside: KiCad would remove the piece -> class 2, vias keep it;
  C thin:    the F.Cu fill above y = 20 joins the rest only through a 0.7 mm neck between TP2 and TP3 and holds no
             GND via or THT pad -> class 3, vias;
  D none:    an F.Cu loop [3, 23]-[10, 28] with a B.Cu loop under it: no solid GND on another layer -> no via (never
             dangling).
Expected: vias on the 5 mm lattice from the grid origin where legal; nothing else changes. One signal connection (SIG,
J1.2 -> R1.1) is routed so the run has a result to stitch.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import harness  # noqa: F401  (KiCad isolated before pcbnew)
import pcbnew

from harness import drc_patterns as D

STATUS = "PROPOSED (awaiting approval)"
MM = D.MM
ZONE_CLEAR, ZONE_MIN = 0.15, 0.25


def _loop(net: str, layer: str, ref: str, x0: float, y0: float, x1: float, y1: float):
    """Two SMD pads of `net` at (x0, y0) and (x0, y0 + 1) and the loop between them around [x0, y0]-[x1, y1]."""
    pads = [D.Pad("1", net, (x0, y0), (0.6, 0.6), layer=layer), D.Pad("2", net, (x0, y0 + 1.0), (0.6, 0.6), layer=layer)]
    trk = D.Trk(net, layer, D.W, [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0 + 1.0)], locked=True)
    return D.Fp(ref, pads), trk


def pattern() -> D.Pat:
    fps, kept = [], []
    for args in (("SA", "F.Cu", "LA", 14, 4, 24, 12), ("SB", "F.Cu", "LB", 28, 4, 38, 14),
                 ("SD", "F.Cu", "LD", 3, 23, 10, 28), ("SE", "B.Cu", "LE", 2.5, 22.5, 10.5, 28.5)):
        f, t = _loop(*args)
        fps.append(f)
        kept.append(t)
    # The thin neck: SIG4 from TP1 (edge) to TP2, SIG5 from TP3 to TP4 (edge); TP2 / TP3 edges 1.0 mm apart.
    tp = lambda ref, net, x: D.Fp(ref, [D.Pad("1", net, (x, 20.0), (0.8, 0.8))])  # noqa: E731
    fps += [tp("TP1", "SIG4", 0.9), tp("TP2", "SIG4", 26.0), tp("TP3", "SIG5", 27.8), tp("TP4", "SIG5", 39.1)]
    kept += [D.Trk("SIG4", "F.Cu", D.W, [(0.9, 20.0), (26.0, 20.0)], locked=True),
             D.Trk("SIG5", "F.Cu", D.W, [(27.8, 20.0), (39.1, 20.0)], locked=True)]
    fps.append(D.Fp("C1", [D.Pad("1", "GND", (19.0, 8.0), (0.9, 0.95))]))
    fps.append(D.Fp("J1", [D.Pad("1", "GND", (4.0, 13.73), (1.7, 1.7), drill=1.0), D.Pad("2", "SIG", (4.0, 16.27), (1.7, 1.7), "circle", drill=1.0)]))
    fps.append(D.Fp("R1", [D.Pad("1", "SIG", (9.0, 16.27), (0.9, 0.95))]))
    return D.Pat(
        name="stitch_pieces", cluster="stitch-vias", title="Stitching vias: cut, dropped, thin and no-second-layer pieces",
        purpose="stitch-vias classes 1-3 and the never-dangling rule on one board",
        answer="A >= 3 vias (first near C1.1), B >= 4 vias, C vias above y = 20, D none; all on the 5 mm lattice where legal",
        why="GND is poured on both layers; a via joins an F.Cu piece to the solid B.Cu pour",
        size=(40.0, 30.0), nets=["GND", "SIG", "SIG4", "SIG5", "SA", "SB", "SD", "SE"], fps=fps,
        connections=[("SIG", "J1.2", "R1.1")], ref_tracks=[D.Trk("SIG", "F.Cu", D.W, [(4.0, 16.27), (9.0, 16.27)])],
        kept=kept,
        expect={"vias_min": {"A": 3, "B": 4, "C": 2, "D": 0},
                "pieces": {"A": [14, 4, 24, 12], "B": [28, 4, 38, 14], "C": [0, 20, 40, 30], "D": [3, 23, 10, 28]}},
        marks=[{"xy": [19, 3], "text": "A cut"}, {"xy": [33, 3], "text": "B dropped"}, {"xy": [27, 21], "text": "C thin neck"},
               {"xy": [6, 22], "text": "D no B.Cu"}])


def add_zones(b: pcbnew.BOARD, size) -> None:
    """GND zones over the whole board on F.Cu and B.Cu, remove islands always, then filled."""
    net = b.FindNet("GND")
    w, h = size
    for layer in (pcbnew.F_Cu, pcbnew.B_Cu):
        z = pcbnew.ZONE(b)
        z.SetLayer(layer)
        z.SetNet(net)
        z.SetLocalClearance(int(ZONE_CLEAR * MM))
        z.SetMinThickness(int(ZONE_MIN * MM))
        z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
        z.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL)
        ol = z.Outline()
        ol.NewOutline()
        for x, y in ((0.5, 0.5), (w - 0.5, 0.5), (w - 0.5, h - 0.5), (0.5, h - 0.5)):
            ol.Append(int(x * MM), int(y * MM))
        b.Add(z)
    pcbnew.ZONE_FILLER(b).Fill(b.Zones())


def generate(out: Path) -> Path:
    pat = pattern()
    d = Path(out) / pat.name
    paths = {}
    for kind, copper in (("problem", False), ("reference", True)):
        p = d / kind / f"{pat.name}.kicad_pcb"
        p.parent.mkdir(parents=True, exist_ok=True)
        b = D.build_board(pat, copper=copper)
        add_zones(b, pat.size)
        pcbnew.SaveBoard(str(p), b)
        D.write_project_file(p, rules=dict(D.RULES), classes=[dict(D.DEFAULT_CLASS)])
        paths[kind] = p
    doc = {"status": STATUS, "name": pat.name, "title": pat.title, "purpose": pat.purpose, "answer": pat.answer,
           "why": pat.why, "expect": pat.expect, "marks": pat.marks}
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    return paths["problem"]


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "stitch-patterns" / "boards"
    print(generate(out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
