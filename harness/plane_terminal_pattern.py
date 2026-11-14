"""Test pattern `plane_terminal`: own-net pours are terminals of their net (shopping list 4.4, user rule 2026-11-10).

User rule (2026-11-10): "Routing through an existing plane, since it's shared by all traces of that plane should not
be added to its distance. This does mean, planes need to be connected to their nets like they are pads for that net,
with the difference that they can have vias, even when via in pad is disabled."

Three questions on one small 2-layer board (30 x 20 mm, rules as `harness/fit_patterns.py`: clearance 0.2, track
0.25, via 0.6 / 0.3, copper-edge 0.5):

1. VCC: pad J1.1 (F.Cu) far left; a VCC pour on F.Cu covers the right part of the board (x >= 18) and holds C1.1.
   The pour is a terminal of VCC: J1.1's trace ends on it at its near edge (15.5 mm, no via). Careless answer
   (pads only): J1.1 to C1.1 through the pour, 20.0 mm.
2. GND pair: R1.2 and R2.1 (F.Cu, 1.5 mm apart face to face); a GND pour on B.Cu over the whole board. Both need a
   via onto the plane; the plane may carry vias and a via on it is the net's copper, so the second pad ends on the
   first pad's via: one shared via midway (stubs 1.25 + 1.25 mm). Careless: one via per pad (2 vias). Sharing is
   allowed outside BGA footprints (user 2026-11-13: "no shared vias under bga chips, shared vias are okay outside of
   BGA footprints"; `[planes] share_vias` = "outside_bga", the default); with "never" each pad gets its own via.
3. GND single: C1.2 (F.Cu) stands inside the VCC F.Cu pour; its shortest connection is one via straight onto the
   B.Cu GND plane beside it (the VCC pour refills around it).

Expected (cost = length + 25 mm per via, TopoR 6.3 manual p. 184): 15.5 + 2.5 + 1.3 mm, 2 vias. Careless: 20.0 + 2 x
~1.2 + 1.3 mm, 3 vias.

Same conventions as `harness/plane_via_pattern.py` (branch `plane-via-pattern`): solvable by construction (the
reference copper is drawn first; KiCad DRC of the reference with the zones refilled must show 0 violations and 0
unconnected), deterministic. Coordinates in mm, KiCad frame (y down). `python -m harness.plane_terminal_pattern OUT`
writes OUT/plane_terminal/{problem,reference}/, expected.json and plane_terminal.png.

Needs user approval before it joins the test suite (shopping list rules).
"""
from __future__ import annotations

import json
import sys
from dataclasses import dataclass, field
from pathlib import Path

import pcbnew

from harness.fit_patterns import DEFAULT_CLASS, MM, RULES, W_SIG, Fp, Pad, Track, Via, build_board
from harness.fixtures import write_project_file

W, H = 30.0, 20.0
PAD = (1.0, 1.45)  # R_0805 pad (x along the part axis, y across)
VCC_POUR_X = 18.0  # VCC pour on F.Cu: x >= this
J1 = (3.0, 10.0)
C1_1, C1_2 = (23.0, 10.0), (24.9, 10.0)
R1_1, R1_2 = (5.6, 4.0), (7.5, 4.0)
R2_1, R2_2 = (10.0, 4.0), (11.9, 4.0)
SHARED_VIA = (8.75, 4.0)  # midway between the GND faces 8.0 and 9.5
C1_VIA = (26.2, 10.0)  # beside C1.2 (face 25.4): 0.5 mm of via edge to the pad
VCC_END = (VCC_POUR_X + 0.5, 10.0)  # the trace ends 0.5 mm inside the VCC fill
VIA_MM = 25.0


@dataclass
class PTPattern:
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


def plane_terminal() -> PTPattern:
    fps = [
        Fp("J1", [Pad("1", "VCC", J1, PAD)], "terminal"),
        Fp("C1", [Pad("1", "VCC", C1_1, PAD), Pad("2", "GND", C1_2, PAD)], "obstacle"),
        Fp("R1", [Pad("1", None, R1_1, PAD), Pad("2", "GND", R1_2, PAD)], "obstacle"),
        Fp("R2", [Pad("1", "GND", R2_1, PAD), Pad("2", None, R2_2, PAD)], "obstacle"),
    ]
    ref_tracks = [
        Track("VCC", "F.Cu", W_SIG, [J1, VCC_END]),
        Track("GND", "F.Cu", W_SIG, [R1_2, SHARED_VIA]),
        Track("GND", "F.Cu", W_SIG, [R2_1, SHARED_VIA]),
        Track("GND", "F.Cu", W_SIG, [C1_2, C1_VIA]),
    ]
    own1, own2 = (R1_2[0], R1_2[1] + 1.35), (R2_1[0], R2_1[1] + 1.35)
    careless_tracks = [
        Track("VCC", "F.Cu", W_SIG, [J1, C1_1]),
        Track("GND", "F.Cu", W_SIG, [R1_2, own1]),
        Track("GND", "F.Cu", W_SIG, [R2_1, own2]),
        Track("GND", "F.Cu", W_SIG, [C1_2, C1_VIA]),
    ]
    return PTPattern("plane_terminal", (W, H), 2, ["GND", "VCC"], fps, ref_tracks,
                     [Via("GND", SHARED_VIA), Via("GND", C1_VIA)], careless_tracks,
                     [Via("GND", own1), Via("GND", own2), Via("GND", C1_VIA)])


def _zone(b: pcbnew.BOARD, net: str, layer: int, rect) -> None:
    z = pcbnew.ZONE(b)
    z.SetLayer(layer)
    z.SetNet(b.FindNet(net))
    ol = pcbnew.SHAPE_LINE_CHAIN()
    x0, y0, x1, y1 = rect
    for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1)):
        ol.Append(pcbnew.VECTOR2I(int(x * MM), int(y * MM)))
    ol.SetClosed(True)
    z.AddPolygon(ol)
    b.Add(z)


def build_variant(pat: PTPattern, variant: str) -> pcbnew.BOARD:
    """variant: problem | reference | careless. Adds the VCC F.Cu pour and the GND B.Cu pour, filled."""
    class View:
        size, layers, nets, fps, keepouts = pat.size, pat.layers, pat.nets, pat.fps, pat.keepouts
        ref_tracks = {"problem": [], "reference": pat.ref_tracks, "careless": pat.careless_tracks}[variant]
        ref_vias = {"problem": [], "reference": pat.ref_vias, "careless": pat.careless_vias}[variant]

    b = build_board(View, copper=variant != "problem")
    _zone(b, "VCC", pcbnew.F_Cu, (VCC_POUR_X, 0.0, W, H))
    _zone(b, "GND", pcbnew.B_Cu, (0.0, 0.0, W, H))
    pcbnew.ZONE_FILLER(b).Fill(b.Zones())
    return b


def write_variant(pat: PTPattern, path: Path, variant: str) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    pcbnew.SaveBoard(str(path), build_variant(pat, variant))
    write_project_file(path, rules=pat.rules, classes=pat.classes)
    return path


def _len(pts) -> float:
    return sum(((b[0] - a[0]) ** 2 + (b[1] - a[1]) ** 2) ** 0.5 for a, b in zip(pts, pts[1:]))


def expected_doc(pat: PTPattern) -> dict:
    ref = {t.net + ":" + "-".join(f"{p[0]:g},{p[1]:g}" for p in t.pts): round(_len(t.pts), 4) for t in pat.ref_tracks}
    car = {t.net + ":" + "-".join(f"{p[0]:g},{p[1]:g}" for p in t.pts): round(_len(t.pts), 4) for t in pat.careless_tracks}
    lr, lc = sum(ref.values()), sum(car.values())
    return {
        # harness.validity: built for the board's own rules ([rules] fab_profile = "board", the default)
        "rules_assumed": {"authority": "board", "margin": 0.0},
        "name": pat.name,
        "title": "Own-net pours are terminals: end on the VCC pour, share a GND plane via, one via onto the GND plane",
        "kind": "solvable",
        "board": {"size": list(pat.size), "layers": pat.layers,
                  "pours": {"VCC": f"F.Cu, x >= {VCC_POUR_X} (holds C1.1)", "GND": "B.Cu, whole board"}},
        "user_rule": "Existing planes are terminals of their net like pads; routing through an own-net plane adds no "
                     "distance; vias onto a plane are allowed even when via-in-pad is off (user 2026-11-10)",
        "expected": {
            "VCC J1.1": f"F.Cu trace from J1.1 ending inside the VCC fill at its near edge (x ~ {VCC_END[0]}): "
                        f"{_len([J1, VCC_END]):.1f} mm, 0 vias, no copper inside the pour",
            "GND R1.2 + R2.1": f"one shared via at {SHARED_VIA}, two F.Cu stubs of 1.25 mm",
            "GND C1.2": f"one via beside the pad ({C1_VIA}), onto the B.Cu GND plane",
            "vias": len(pat.ref_vias),
            "length_mm": round(lr, 3),
            "cost_mm": round(lr + VIA_MM * len(pat.ref_vias), 3),
        },
        "careless": {
            "VCC J1.1": f"pads only: J1.1 to C1.1 through the pour, {_len([J1, C1_1]):.1f} mm",
            "GND": "one via per pad",
            "vias": len(pat.careless_vias),
            "length_mm": round(lc, 3),
            "cost_mm": round(lc + VIA_MM * len(pat.careless_vias), 3),
        },
        "tracks_reference_mm": ref,
    }


def render(pat: PTPattern, doc: dict, out: Path) -> Path:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.patches import Circle, Rectangle

    fig, axes = plt.subplots(1, 2, figsize=(14, 5.6))
    for ax, (label, tracks, vias) in zip(axes, (("expected (4.4: pours are terminals)", pat.ref_tracks, pat.ref_vias),
                                                 ("careless (pads only, one via per pad)", pat.careless_tracks, pat.careless_vias))):
        ax.add_patch(Rectangle((0, 0), W, H, fill=False, ec="#999", lw=1))
        ax.add_patch(Rectangle((0.5, 0.5), W - 1, H - 1, fc="#1f77b4", alpha=0.10, ec="none"))
        ax.add_patch(Rectangle((VCC_POUR_X, 0.5), W - 0.5 - VCC_POUR_X, H - 1, fc="#ff7f0e", alpha=0.22, ec="#ff7f0e", lw=0.8))
        ax.text(VCC_POUR_X + 0.3, 1.4, "VCC pour, F.Cu", color="#c05000", fontsize=8)
        ax.text(0.8, H - 0.9, "GND pour, B.Cu (whole board)", color="#1f5f9f", fontsize=8)
        for f in pat.fps:
            for p in f.pads:
                col = {"VCC": "#ff7f0e", "GND": "#1f77b4", None: "#888"}[p.net]
                ax.add_patch(Rectangle((p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2), *p.size, fc=col, ec="k", lw=0.6))
                ax.text(p.pos[0], p.pos[1] + (1.25 if p.num == "1" else -0.95), f"{f.ref}.{p.num}" + (f" {p.net}" if p.net else ""), ha="center", fontsize=7)
        for t in tracks:
            xs, ys = zip(*t.pts)
            ax.plot(xs, ys, color="#c00000", lw=2.2, solid_capstyle="round")
        for v in vias:
            ax.add_patch(Circle(v.pos, v.dia / 2, fc="#ffd700", ec="k", lw=0.8))
            ax.add_patch(Circle(v.pos, v.drill / 2, fc="w", ec="k", lw=0.4))
        d = doc["expected"] if label.startswith("expected") else doc["careless"]
        ax.set_title(f"{label}\n{d['vias']} vias, {d['length_mm']:.2f} mm copper, cost {d['cost_mm']:.1f} mm (25 mm/via)", fontsize=9)
        ax.set_xlim(-0.5, W + 0.5)
        ax.set_ylim(H + 0.5, -0.5)
        ax.set_aspect("equal")
        ax.tick_params(labelsize=7)
    e = doc["expected"]
    fig.suptitle("plane_terminal (4.4): " + "; ".join(f"{k}: {v}" for k, v in e.items() if k.startswith(("VCC", "GND"))), fontsize=8, wrap=True)
    fig.tight_layout()
    out = Path(out)
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def generate(out: Path, *, png: bool = True) -> dict:
    pat = plane_terminal()
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
    for k, v in generate(Path(argv[0]) if argv else Path("scratch/plane-terminal-pattern")).items():
        print(k, v)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
