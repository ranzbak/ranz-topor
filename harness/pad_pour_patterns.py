"""PROPOSED test pattern for pad-pour-via (docs/dev/design/pad-pour-via.md): a GND pad in a 0.5 mm-pitch row whose
thermal spokes the neighbours' fanouts all cut, joined by a dog-bone to the B.Cu pour. NOT in the test suite: the
user approves the PNG first (AGENT_RULES "New test patterns need user approval").

    python -m harness.pad_pour_patterns [OUT]      # OUT/pad_dogbone/problem/pad_dogbone.kicad_pcb + expected.json

Board 20 x 20 mm, GND poured on F.Cu and B.Cu (zone clearance 0.25, minimum width 0.25, thermal relief 0.5 gap /
0.5 spoke). Row U1 at y = 10: pads 1, 2, 4, 5 (signal nets S1..S4) at x = 9.0, 9.5, 10.5, 11.0, GND pad 3 at x = 10.0,
all 0.3 x 1.2 mm. Kept copper: each signal pad's 0.25 mm fanout runs 2.8 mm up (-y) to a 0.4 mm test pad; a kept
signal track (S5, between two test pads) crosses 1.24 mm below the row. Spokes of U1.3: left / right in the
neighbours' clearance, up into the 0.25 mm channel between the fanouts (below the minimum width: no fill), down
under S5: all cut, U1.3 is cut off. One routed connection (SIG, J1.2 -> R1.1) so the run has a result to stitch.
Expected (the optimal answer): a dog-bone from U1.3's centre straight up the channel (the net width fits) to a GND
via just past the test pads (about 3.3 mm from the centre), in the solid B.Cu pour; no other change. The radial pad
stub has no answer (its 3 mm reach ends before the via room starts).
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
ZONE_CLEAR, ZONE_MIN = 0.25, 0.25
ROW_Y, FAN = 10.0, 2.8


def pattern() -> D.Pat:
    row, kept, tps = [], [], []
    for k, (x, net) in enumerate(((9.0, "S1"), (9.5, "S2"), (10.0, "GND"), (10.5, "S3"), (11.0, "S4"))):
        row.append(D.Pad(str(k + 1), net, (x, ROW_Y), (0.3, 1.2)))
        if net != "GND":
            tps.append(D.Fp(f"TP{k + 1}", [D.Pad("1", net, (x, ROW_Y - FAN), (0.4, 0.4), "circle")]))
            kept.append(D.Trk(net, "F.Cu", 0.25, [(x, ROW_Y), (x, ROW_Y - FAN)], locked=True))
    yb = ROW_Y + 1.24
    tps += [D.Fp("TP6", [D.Pad("1", "S5", (7.0, yb), (0.4, 0.4), "circle")]), D.Fp("TP7", [D.Pad("1", "S5", (13.0, yb), (0.4, 0.4), "circle")])]
    kept.append(D.Trk("S5", "F.Cu", 0.25, [(7.0, yb), (13.0, yb)], locked=True))
    fps = [D.Fp("U1", row)] + tps
    fps.append(D.Fp("J1", [D.Pad("1", "GND", (3.0, 15.73), (1.7, 1.7), drill=1.0), D.Pad("2", "SIG", (3.0, 18.27), (1.7, 1.7), "circle", drill=1.0)]))
    fps.append(D.Fp("R1", [D.Pad("1", "SIG", (8.0, 18.27), (0.9, 0.95))]))
    return D.Pat(
        name="pad_dogbone", cluster="pad-pour-via", title="Dog-bone via for a pour pad whose spokes are all cut",
        purpose="pad-pour-via B: the axis dog-bone where the radial pad stub has no answer",
        answer="U1.3: stub straight up the channel, GND via past the test pads (~3.3 mm out) in the B.Cu pour",
        why="the fanouts leave no spoke and no via room near the pad; the pad axis is the only way out",
        size=(20.0, 20.0), nets=["GND", "SIG", "S1", "S2", "S3", "S4", "S5"], fps=fps,
        connections=[("SIG", "J1.2", "R1.1")],
        ref_tracks=[D.Trk("SIG", "F.Cu", D.W, [(3.0, 18.27), (8.0, 18.27)]),
                    D.Trk("GND", "F.Cu", 0.25, [(10.0, ROW_Y), (10.0, ROW_Y - 3.3)])],
        kept=kept,
        expect={"dogbone": {"pad": "U1.3", "via_x": 10.0, "via_y_max": ROW_Y - 3.0},
                "pieces": {"U1.3": [9.3, 5.5, 10.7, 10.7]}},
        marks=[{"xy": [10.0, 5.0], "text": "dog-bone via"}, {"xy": [10.0, 12.2], "text": "U1.3 spokes all cut"}])


def add_zones(b: pcbnew.BOARD, size) -> None:
    """GND zones over the whole board on F.Cu and B.Cu (thermal relief), then filled."""
    net = b.FindNet("GND")
    w, h = size
    for layer in (pcbnew.F_Cu, pcbnew.B_Cu):
        z = pcbnew.ZONE(b)
        z.SetLayer(layer)
        z.SetNet(net)
        z.SetLocalClearance(int(ZONE_CLEAR * MM))
        z.SetMinThickness(int(ZONE_MIN * MM))
        z.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL)
        z.SetThermalReliefGap(int(0.5 * MM))
        z.SetThermalReliefSpokeWidth(int(0.5 * MM))
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
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "pad-pour-patterns" / "boards"
    print(generate(out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
