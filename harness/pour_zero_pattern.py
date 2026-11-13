"""Test pattern `pour_zero`: routing over a connected own-net pour has no length (user 2026-11-12).

User decision (2026-11-12): "Routing over pours should be zero length, because the trace ends there, given that
the pour is connected to the rest of the net." Builds on the user rule of 2026-11-10 (own-net planes are terminals of
their net like pads, `harness/plane_terminal_pattern.py`).

Three questions on one small 2-layer board (40 x 30 mm, rules as `harness/fit_patterns.py`: clearance 0.2, track
0.25, via 0.6 / 0.3, copper-edge 0.5; the board's own rules are the authority: route with [rules] fab_profile =
"board", `harness/validity.py` must say VALID); the trace end keeps w/2 + zone minimum width (0.25) + clearance = 0.575 mm
inside the fill (the far-terminal margin of 4.4):

1. Through the pour (VCC): pads J1.1 (left) and J2.1 (right), a VCC pour on F.Cu between them (x 14..26) holding
   C1.1. Both pads end on the pour's near edge; the stretch over the pour has no length. Expected 2 x 10.575 mm,
   0 vias. Careless (pads only): J1.1 and J2.1 to C1.1 through the pour, 2 x 16 mm.
2. First reach (VCC): pad J3.1 below the VCC pour with a 16-pin connector row of another part between them
   (y 18, x 8.5..28.6; 0.27 mm gaps, no trace fits). The trace ends where its shortest path first reaches the pour:
   round the row's right end, then up to the pour's lower right corner (about 19.4 mm). Careless: to the pour point
   nearest the pad (behind the row, about 22.7 mm).
3. Island (GND): pad R1.2 next to a GND island on F.Cu (x 7.5..10, no pad in it, zone "remove islands: never");
   the GND copper of the net is the B.Cu pour along the bottom (y >= 20), which holds the mounting pad H1.1. The
   island is no terminal (it is not connected to the rest of the net): R1.2 is wired to the B.Cu pour (one via),
   and the island is joined to R1.2 by a short stub, so it is no longer isolated copper (user 2026-11-12: "connecting
   the gnd island to the GND pad would be even better"). Careless: only the 1.6 mm stub into the island, 0 vias,
   R1.2 and the island left floating (KiCad: unconnected).

Same conventions as `harness/plane_terminal_pattern.py`: solvable by construction (the reference copper is drawn
first; KiCad DRC of the reference with the zones refilled must show 0 violations and 0 unconnected: the island's
`isolated_copper` warning of the problem board is gone because the reference joins it to R1.2), deterministic.
Coordinates in mm, KiCad frame (y down). `python -m harness.pour_zero_pattern OUT` writes
OUT/pour_zero/{problem,reference,careless}/, expected.json and pour_zero.png.

Approved by the user 2026-11-12 ("pour_zero.png looks good, connecting the gnd island to the GND pad would be even
better"; Q3 updated accordingly).
"""
from __future__ import annotations

import json
import math
import sys
from dataclasses import dataclass, field
from pathlib import Path

import pcbnew

from harness.fit_patterns import DEFAULT_CLASS, MM, RULES, W_SIG, Fp, Pad, Track, Via, build_board
from harness.fixtures import write_project_file

W, H = 40.0, 30.0
PAD = (1.0, 1.45)  # R_0805 pad
MARGIN = W_SIG / 2 + 0.25 + 0.2  # far-terminal margin inside the fill
VCC_POUR = (14.0, 1.5, 26.0, 11.5)
ISLAND = (7.5, 12.5, 10.0, 15.5)
GND_BAND = (0.5, 20.0, 39.5, 29.5)
J1, J2, J3 = (4.0, 6.5), (36.0, 6.5), (20.0, 25.0)
C1_1, C1_2 = (20.0, 6.5), (21.9, 6.5)
R1_1, R1_2 = (4.1, 14.0), (6.0, 14.0)
H1 = (34.0, 25.0)
ROW_Y, ROW_X0, ROW_P, ROW_N, ROW_PAD = 18.0, 9.0, 1.27, 16, (1.0, 1.0)
ROW_X1 = ROW_X0 + ROW_P * (ROW_N - 1)
# Round the row's right end: the wire keeps w/2 + clearance from the end pad's corners.
AROUND = ROW_X1 + ROW_PAD[0] / 2 + W_SIG / 2 + 0.2 + 0.05
J3_END = (VCC_POUR[2] - MARGIN, VCC_POUR[3] - MARGIN)
J3_NEAREST = (J3[0], VCC_POUR[3] - MARGIN)
R1_VIA = (R1_2[0], GND_BAND[1] + 0.9)
ISLAND_STUB = (ISLAND[0] + 1.1, R1_2[1])  # a short stub from R1.2 into the cut-off island joins it to the net
VIA_MM = 25.0


@dataclass
class PZPattern:
    """Duck-typed for `fit_patterns.build_board`."""
    name: str
    size: tuple[float, float]
    layers: int
    nets: list[str]
    fps: list[Fp]
    ref_tracks: list[Track]
    ref_vias: list[Via]
    careless_tracks: list[Track]
    careless_vias: list[Via]
    keepouts: list = field(default_factory=list)
    classes: list[dict] = field(default_factory=lambda: [dict(DEFAULT_CLASS)])
    rules: dict = field(default_factory=lambda: dict(RULES))


def pour_zero() -> PZPattern:
    row = [Pad(str(k + 1), None, (ROW_X0 + ROW_P * k, ROW_Y), ROW_PAD) for k in range(ROW_N)]
    fps = [
        Fp("J1", [Pad("1", "VCC", J1, PAD)], "terminal"),
        Fp("J2", [Pad("1", "VCC", J2, PAD)], "terminal"),
        Fp("J3", [Pad("1", "VCC", J3, PAD)], "terminal"),
        Fp("C1", [Pad("1", "VCC", C1_1, PAD), Pad("2", None, C1_2, PAD)], "obstacle"),
        Fp("R1", [Pad("1", None, R1_1, PAD), Pad("2", "GND", R1_2, PAD)], "obstacle"),
        Fp("H1", [Pad("1", "GND", H1, (2.0, 2.0), shape="circle", drill=1.0)], "obstacle"),
        Fp("X1", row, "obstacle"),
    ]
    j3_path = [J3, (AROUND, ROW_Y + 0.9), (AROUND, ROW_Y - 0.9), J3_END]
    ref_tracks = [
        Track("VCC", "F.Cu", W_SIG, [J1, (VCC_POUR[0] + MARGIN, J1[1])]),
        Track("VCC", "F.Cu", W_SIG, [J2, (VCC_POUR[2] - MARGIN, J2[1])]),
        Track("VCC", "F.Cu", W_SIG, j3_path),
        Track("GND", "F.Cu", W_SIG, [R1_2, R1_VIA]),
        Track("GND", "F.Cu", W_SIG, [R1_2, ISLAND_STUB]),
    ]
    careless_tracks = [
        Track("VCC", "F.Cu", W_SIG, [J1, C1_1]),
        Track("VCC", "F.Cu", W_SIG, [J2, (C1_2[0] + 1.3, 8.2), (C1_1[0], 8.2), C1_1]),
        Track("VCC", "F.Cu", W_SIG, [J3, (AROUND, ROW_Y + 0.9), (AROUND, ROW_Y - 0.9), J3_NEAREST]),
        Track("GND", "F.Cu", W_SIG, [R1_2, ISLAND_STUB]),
    ]
    return PZPattern("pour_zero", (W, H), 2, ["GND", "VCC"], fps, ref_tracks, [Via("GND", R1_VIA)],
                     careless_tracks, [])


def _zone(b: pcbnew.BOARD, net: str, layer: int, rect, *, never_remove_islands: bool = False) -> None:
    z = pcbnew.ZONE(b)
    z.SetLayer(layer)
    z.SetNet(b.FindNet(net))
    if never_remove_islands:
        z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_NEVER)
    ol = pcbnew.SHAPE_LINE_CHAIN()
    x0, y0, x1, y1 = rect
    for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1)):
        ol.Append(pcbnew.VECTOR2I(int(x * MM), int(y * MM)))
    ol.SetClosed(True)
    z.AddPolygon(ol)
    b.Add(z)


def build_variant(pat: PZPattern, variant: str) -> pcbnew.BOARD:
    """variant: problem | reference | careless. Adds the VCC F.Cu pour, the GND F.Cu island and the GND B.Cu
    band, filled."""
    class View:
        size, layers, nets, fps, keepouts = pat.size, pat.layers, pat.nets, pat.fps, pat.keepouts
        ref_tracks = {"problem": [], "reference": pat.ref_tracks, "careless": pat.careless_tracks}[variant]
        ref_vias = {"problem": [], "reference": pat.ref_vias, "careless": pat.careless_vias}[variant]

    b = build_board(View, copper=variant != "problem")
    _zone(b, "VCC", pcbnew.F_Cu, VCC_POUR)
    _zone(b, "GND", pcbnew.F_Cu, ISLAND, never_remove_islands=True)
    _zone(b, "GND", pcbnew.B_Cu, GND_BAND)
    pcbnew.ZONE_FILLER(b).Fill(b.Zones())
    return b


def write_variant(pat: PZPattern, path: Path, variant: str) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    pcbnew.SaveBoard(str(path), build_variant(pat, variant))
    write_project_file(path, rules=pat.rules, classes=pat.classes)
    return path


def _len(pts) -> float:
    return sum(math.hypot(b[0] - a[0], b[1] - a[1]) for a, b in zip(pts, pts[1:]))


def expected_doc(pat: PZPattern) -> dict:
    def tl(ts):
        return {t.net + ":" + "-".join(f"{p[0]:g},{p[1]:g}" for p in t.pts): round(_len(t.pts), 4) for t in ts}
    ref, car = tl(pat.ref_tracks), tl(pat.careless_tracks)
    lr, lc = sum(ref.values()), sum(car.values())
    r, c = pat.ref_tracks, pat.careless_tracks
    return {
        # harness.validity: built for the board's own rules (route with [rules] fab_profile = "board")
        "rules_assumed": {"authority": "board", "margin": 0.0},
        "name": pat.name,
        "title": "Routing over a connected own-net pour has no length: the trace ends where it first reaches the pour",
        "kind": "solvable",
        "rules": {"design_rules_mm": pat.rules, "net_classes": pat.classes,
                  "note": "the board's own rules (Board Setup is the authority, no fab profile)"},
        "board": {"size": list(pat.size), "layers": pat.layers,
                  "pours": {"VCC": f"F.Cu {VCC_POUR} (holds C1.1)",
                            "GND island": f"F.Cu {ISLAND}, no pad, remove islands = never (not connected)",
                            "GND": f"B.Cu {GND_BAND} (holds H1.1)"}},
        "user_rule": "Routing over pours should be zero length, because the trace ends there, given that the pour is "
                     "connected to the rest of the net (user 2026-11-12)",
        "expected": {
            "VCC J1.1 + J2.1": f"each ends on the VCC pour's near edge: {_len(r[0].pts):.2f} + {_len(r[1].pts):.2f} mm, "
                               "no copper over the pour",
            "VCC J3.1": f"round the connector row's right end to the pour's lower right corner: {_len(r[2].pts):.2f} mm",
            "GND R1.2": f"to the B.Cu GND pour (via at {R1_VIA}); the F.Cu island joined to R1.2 by a stub "
                        f"({_len(r[4].pts):.2f} mm), so it is no longer isolated",
            "vias": len(pat.ref_vias),
            "unconnected": 0,
            "length_mm": round(lr, 3),
            "cost_mm": round(lr + VIA_MM * len(pat.ref_vias), 3),
        },
        "careless": {
            "VCC J1.1 + J2.1": f"pads only, to C1.1 through the pour: {_len(c[0].pts):.2f} + {_len(c[1].pts):.2f} mm",
            "VCC J3.1": f"to the pour point nearest the pad, behind the row: {_len(c[2].pts):.2f} mm",
            "GND R1.2": "only a stub into the island: R1.2 and the island float (KiCad: unconnected)",
            "vias": len(pat.careless_vias),
            "unconnected": 1,
            "length_mm": round(lc, 3),
            "cost_mm": round(lc + VIA_MM * len(pat.careless_vias), 3),
        },
        "tracks_reference_mm": ref,
    }


def render(pat: PZPattern, doc: dict, out: Path) -> Path:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.patches import Circle, Rectangle

    def rect(ax, r, **kw):
        ax.add_patch(Rectangle((r[0], r[1]), r[2] - r[0], r[3] - r[1], **kw))

    fig, axes = plt.subplots(1, 2, figsize=(15, 6.4))
    for ax, (label, tracks, vias) in zip(axes, (("expected (the trace ends on the connected pour)", pat.ref_tracks, pat.ref_vias),
                                                 ("careless (pads only / nearest point / island)", pat.careless_tracks, pat.careless_vias))):
        ax.add_patch(Rectangle((0, 0), W, H, fill=False, ec="#999", lw=1))
        rect(ax, GND_BAND, fc="#1f77b4", alpha=0.12, ec="#1f77b4", lw=0.6)
        rect(ax, VCC_POUR, fc="#ff7f0e", alpha=0.22, ec="#ff7f0e", lw=0.8)
        rect(ax, ISLAND, fc="none", ec="#1f77b4", lw=0.9, hatch="////")
        ax.text(VCC_POUR[0] + 0.3, VCC_POUR[1] + 0.9, "VCC pour, F.Cu (holds C1.1)", color="#c05000", fontsize=8)
        ax.text(GND_BAND[0] + 0.3, GND_BAND[3] - 0.5, "GND pour, B.Cu (holds H1.1)", color="#1f5f9f", fontsize=8)
        ax.text(ISLAND[0] - 0.2, ISLAND[1] - 0.4, "GND island, F.Cu: no pad,\nremove islands = never", color="#1f5f9f", fontsize=7)
        for f in pat.fps:
            for p in f.pads:
                col = {"VCC": "#ff7f0e", "GND": "#1f77b4", None: "#888"}[p.net]
                if p.shape == "circle":
                    ax.add_patch(Circle(p.pos, p.size[0] / 2, fc=col, ec="k", lw=0.6))
                    ax.add_patch(Circle(p.pos, (p.drill or 0) / 2, fc="w", ec="k", lw=0.4))
                else:
                    ax.add_patch(Rectangle((p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2), *p.size, fc=col, ec="k", lw=0.5))
                if f.ref != "X1" or p.num in ("1", str(ROW_N)):
                    ax.text(p.pos[0], p.pos[1] + 1.3, f"{f.ref}.{p.num}" + (f" {p.net}" if p.net else ""), ha="center", fontsize=6.5)
        for t in tracks:
            xs, ys = zip(*t.pts)
            ax.plot(xs, ys, color="#c00000", lw=2.2, solid_capstyle="round")
        for v in vias:
            ax.add_patch(Circle(v.pos, v.dia / 2, fc="#ffd700", ec="k", lw=0.8))
            ax.add_patch(Circle(v.pos, v.drill / 2, fc="w", ec="k", lw=0.4))
        d = doc["expected"] if label.startswith("expected") else doc["careless"]
        ax.set_title(f"{label}\n{d['unconnected']} unconnected (KiCad DRC), {d['vias']} vias, {d['length_mm']:.2f} mm copper, cost {d['cost_mm']:.1f} mm (25 mm/via)", fontsize=9)
        ax.set_xlim(-0.5, W + 0.5)
        ax.set_ylim(H + 0.5, -0.5)
        ax.set_aspect("equal")
        ax.tick_params(labelsize=7)
    fig.text(0.5, 0.01, "red = F.Cu trace, yellow = via; the trace end keeps 0.575 mm inside the fill (w/2 + zone min width + clearance)",
             ha="center", fontsize=8)
    fig.suptitle("pour_zero: " + "; ".join(f"{k}: {v}" for k, v in doc["expected"].items() if k.startswith(("VCC", "GND"))),
                 fontsize=8, wrap=True)
    fig.tight_layout(rect=(0, 0.03, 1, 0.95))
    out = Path(out)
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def generate(out: Path, *, png: bool = True) -> dict:
    pat = pour_zero()
    d = Path(out) / pat.name
    paths = {v: write_variant(pat, d / v / f"{pat.name}.kicad_pcb", v) for v in ("problem", "reference", "careless")}
    doc = expected_doc(pat)
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    paths["expected"] = d / "expected.json"
    if png:
        paths["png"] = render(pat, doc, d / f"{pat.name}.png")
    return paths


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    for k, v in generate(Path(argv[0]) if argv else Path("scratch/pour-zero-pattern")).items():
        print(k, v)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
