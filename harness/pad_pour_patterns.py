"""PROPOSED test pattern for pad-pour-via (docs/dev/design/pad-pour-via.md): a GND pad of a pin row whose thermal
spokes are all cut once its neighbours' fanouts are routed, joined by a dog-bone to the B.Cu pour. NOT in the test
suite: the user approves the PNG first (AGENT_RULES "New test patterns need user approval").

    python -m harness.pad_pour_patterns [OUT]      # OUT/pad_dogbone/problem/pad_dogbone.kicad_pcb + expected.json

Board 20 x 20 mm, GND poured on F.Cu and B.Cu (zone clearance 0.25, minimum width 0.25, thermal relief 0.5 gap /
0.5 spoke, spokes at 90 degrees), board clearance 0.15, tracks 0.2. Row U1 at y = 10, 0.55 mm pitch: S1, GND pad 2 at
x = 10, S2, all 0.3 x 1.2 mm; J1.1 (through-hole GND, bottom left) holds the pour's main component. Kept copper: a
locked S5 track 1.24 mm below the row (covers the bottom spoke's test point); the left / right spoke test points lie
in the neighbours' clearance. Before routing the top spoke stands (0.25 mm from the neighbours), so U1.2 is joined
and no plane stub is planned. Routed: S1 and S2 (net class Power, 0.4 mm) straight up 2.8 mm to test pads; they
leave the channel between them 0.2 mm of fill, below the minimum width, so the top spoke goes too: U1.2 is cut off by
the routed copper, which is what the dog-bone (stitching, after routing) is for. Run with kept copper
(`tools/route_board.sh ... --keep-copper`). Expected (the optimal answer): the fanouts straight; a 0.2 mm dog-bone
from U1.2's centre straight up the channel to a GND via just past the test pads, in the solid B.Cu pour. The radial
pad stub has no answer (no via fits within its 3 mm reach).
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
ROW_Y = 10.0
PITCH, FAN, FAN_W = 0.55, 2.8, 0.4


def pattern() -> D.Pat:
    row = [D.Pad("1", "S1", (10.0 - PITCH, ROW_Y), (0.3, 1.2)), D.Pad("2", "GND", (10.0, ROW_Y), (0.3, 1.2)),
           D.Pad("3", "S2", (10.0 + PITCH, ROW_Y), (0.3, 1.2))]
    yb, yt = ROW_Y + 1.24, ROW_Y - FAN
    fps = [D.Fp("U1", row),
           D.Fp("TP1", [D.Pad("1", "S1", (10.0 - PITCH, yt), (0.4, 0.4), "circle")]),
           D.Fp("TP2", [D.Pad("1", "S2", (10.0 + PITCH, yt), (0.4, 0.4), "circle")]),
           D.Fp("TP6", [D.Pad("1", "S5", (7.0, yb), (0.4, 0.4), "circle")]), D.Fp("TP7", [D.Pad("1", "S5", (13.0, yb), (0.4, 0.4), "circle")])]
    # a through-hole GND pad far off: the pour's main component holds a pad (KiCad and the estimate judge the
    # row pad cut off from it)
    fps.append(D.Fp("J1", [D.Pad("1", "GND", (3.0, 16.0), (1.7, 1.7), drill=1.0)]))
    kept = [D.Trk("S5", "F.Cu", D.W, [(7.0, yb), (13.0, yb)], locked=True)]
    fans = [D.Trk(n, "F.Cu", FAN_W, [(x, ROW_Y), (x, yt)]) for n, x in (("S1", 10.0 - PITCH), ("S2", 10.0 + PITCH))]
    # the via nearest the row on the axis: clearance to the test pads / fanout caps (radius 0.2) at +-PITCH
    yv = yt - ((0.2 + D.CL + D.VIA_D / 2) ** 2 - PITCH ** 2) ** 0.5
    return D.Pat(
        name="pad_dogbone", cluster="pad-pour-via", title="Dog-bone via for a pour pad whose spokes are all cut",
        purpose="pad-pour-via B: the axis dog-bone where the radial pad stub has no answer",
        answer=f"S1 / S2 straight up; U1.2: {D.W} mm stub straight up the channel, GND via just past the test pads (y {yv:.2f}) in the B.Cu pour",
        why="the routed fanouts cut the last spoke and leave no via room within the radial stub's 3 mm; the pad axis is the only way out",
        size=(20.0, 20.0), nets=["GND", "S1", "S2", "S5"], fps=fps,
        connections=[("S1", "U1.1", "TP1.1"), ("S2", "U1.3", "TP2.1")],
        ref_tracks=fans + [D.Trk("GND", "F.Cu", D.W, [(10.0, ROW_Y), (10.0, yv)])],
        ref_vias=[D.Via("GND", (10.0, yv))],
        kept=kept,
        classes=[{"name": "Power", "track_width": FAN_W}], class_patterns=[{"netclass": "Power", "pattern": "S1"}, {"netclass": "Power", "pattern": "S2"}],
        expect={"dogbone": {"pad": "U1.2", "via_x": 10.0, "via_y": round(yv, 3), "via_y_tol": 0.05},
                "pieces": {"U1.2": [9.75, 9.0, 10.25, 11.0]}},
        marks=[{"xy": [10.0, yv - 0.6], "text": "dog-bone via"}, {"xy": [10.0, 12.2], "text": "U1.2 spokes all cut"}])


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
        for fp in b.GetFootprints():  # KiCad's footprint-library default for a rect SMD pad (the API's default is 45)
            for pd in fp.Pads():
                pd.SetThermalSpokeAngleDegrees(90.0)
        add_zones(b, pat.size)
        pcbnew.SaveBoard(str(p), b)
        D.write_project_file(p, rules=dict(D.RULES), patterns=pat.class_patterns,
                             classes=[dict(D.DEFAULT_CLASS)] + [{**D.DEFAULT_CLASS, "priority": 0, **c} for c in pat.classes])
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
