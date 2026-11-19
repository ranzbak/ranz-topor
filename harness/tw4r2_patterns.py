"""TW4 round 2 pattern proposals (branch tw4-r2, docs/dev/design/tw4-r2.md): PNG + text for the user's approval,
not in the test suite before.

1. `via_gap_best_point` (item 2, CM1 via gap): a via must change layer inside a pad gap that two other wires pass
   too. The gap between the straight side of P1 and the round pad P2 is 1.88 mm at its narrowest (y 5.00) and widens
   both ways (1.943 at y 5.35); one via (0.6) and two wires (0.25) at 0.2 clearance need 1.90. Expected: the via in
   the gap where it fits, the straight path, nothing predicted. Careless (a gap cut measured at one fixed point where
   the gap is 1.88): the plan sees an over-full via cut (0.02 mm) and takes the far gap (about 3x as long), or
   reports a violation it does not have.
2. `pour_ring` (item 3, graded pour-cut term): three GND pads reach GND only through the F.Cu pour. The VCC tree of
   V1, V2, V3 can go V1-V3-V2 (12.8 mm) round the GND pads, which with the pad row R between V1 and V2 cuts them off
   the pour (planes are terminals: a cut-off terminal), or trunk V1-V2 + a T branch to V3 (14.2 mm) that leaves them open. Expected: the
   open side, 0 unconnected. Careless: the shorter ring, the three GND pads an island (KiCad: 1 unconnected; B.Cu is
   closed, so no stitch via can join it).

Rules as `harness/fit_patterns.py` (clearance 0.2, track 0.25, via 0.6 / 0.3, copper-edge 0.5); the board's own
rules are the authority. Coordinates in mm, KiCad frame (y down). Deterministic. `python -m harness.tw4r2_patterns
OUT` writes OUT/<name>/{problem,reference,careless}/, expected.json and <name>.png.
"""
from __future__ import annotations

import json
import math
import sys
import textwrap
from dataclasses import dataclass, field
from pathlib import Path

import pcbnew

from harness.fit_patterns import (CLEAR, DEFAULT_CLASS, MM, RULES, W_SIG, Fp, Keepout, Pad, Track, Via,
                                  build_board)
from harness.fixtures import write_project_file

VIA_D, VIA_DRILL = 0.6, 0.3
VIA_MM = 25.0


@dataclass
class TPattern:
    """Duck-typed for `fit_patterns.build_board`."""
    name: str
    title: str
    size: tuple[float, float]
    layers: int
    nets: list[str]
    fps: list[Fp]
    ref_tracks: list[Track]
    ref_vias: list[Via]
    careless_tracks: list[Track]
    careless_vias: list[Via]
    keepouts: list = field(default_factory=list)
    zones: list = field(default_factory=list)  # (net, layer, rect)
    classes: list[dict] = field(default_factory=lambda: [dict(DEFAULT_CLASS)])
    rules: dict = field(default_factory=lambda: dict(RULES))
    text: dict = field(default_factory=dict)


def _len(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


# ------------------------------------------------------------------------------------------- 1. via_gap_best_point
def via_gap_best_point() -> TPattern:
    W, H = 16.0, 10.0
    yr = 5.0                    # centre line of the wall
    xl = 4.1                    # P1's straight right side
    gap0 = 1.88                 # narrowest gap (at y = yr)
    r2 = 1.0                    # P2 radius (THT, 2.0 mm)
    c2 = (xl + gap0 + r2, yr)   # P2 centre
    need = CLEAR + W_SIG + CLEAR + W_SIG + CLEAR + VIA_D + CLEAR  # 1.90
    sl = 0.003                  # DRC slack per clearance in the reference
    xa = xl + CLEAR + W_SIG / 2 + sl
    xb = xa + W_SIG + CLEAR + sl
    xv = xb + W_SIG / 2 + CLEAR + VIA_D / 2 + sl
    yv = 5.35
    assert abs(need - 1.90) < 1e-9
    # the via clears P2 (round) by more than the clearance; the gap is wide enough at its row
    dv = math.dist((xv, yv), c2) - r2 - VIA_D / 2
    assert dv >= CLEAR, dv
    wide = xl + gap0 + r2 - math.sqrt(r2 * r2 - (yv - yr) ** 2) - xl  # horizontal gap at the via's row
    assert wide >= need + 0.04, wide
    # SIG's F.Cu run down to the via clears P2 at its nearest point (y = yr)
    assert c2[0] - r2 - (xv + W_SIG / 2) >= CLEAR
    # The wall: P1 (rect 3.6 x 2.0, 0.5 from the west edge), P2 (round), pins at 0.2 mm gaps, the far gap, P4.
    p1 = Fp("P1", [Pad("1", None, (xl - 1.8, yr), (3.6, 2.0), drill=1.0)], "wall")
    p2 = Fp("P2", [Pad("1", None, c2, (2 * r2, 2 * r2), "circle", drill=1.0)], "wall")
    pins = [c2[0] + 2.2, c2[0] + 4.4]
    p3 = Fp("P3", [Pad(str(i + 1), None, (x, yr), (2.0, 2.0), "circle", drill=1.0) for i, x in enumerate(pins)], "wall")
    far0 = pins[-1] + 1.0       # far gap from here to P4
    p4 = Fp("P4", [Pad("1", None, (15.25, yr), (0.5, 2.0), drill=0.3)], "wall")
    far = (far0 + 15.0) / 2
    nets = ["SIG", "A", "B"]
    u1 = Fp("U1", [Pad("1", "SIG", (xv, 1.0), (0.4, 0.4))], "terminal")
    t1 = Fp("T1", [Pad("1", "SIG", (xv, 9.0), (0.4, 0.4), layer="B.Cu")], "terminal")
    j1 = Fp("J1", [Pad("1", "A", (xa, 1.0), (0.25, 0.25), "circle"), Pad("2", "B", (xb, 1.0), (0.25, 0.25), "circle")], "terminal")
    j2 = Fp("J2", [Pad("1", "A", (xa, 9.0), (0.25, 0.25), "circle"), Pad("2", "B", (xb, 9.0), (0.25, 0.25), "circle")], "terminal")
    pads = [p1, p2, p3, p4, u1, t1, j1, j2]
    # Via centres only in y 4.95 .. 5.40 (rule areas: vias, both layers, 0.3 = via radius outside): in the wall's gaps.
    band = (4.65, 5.70)
    ko = [Keepout(["F.Cu", "B.Cu"], [(0, 0), (W, 0), (W, band[0]), (0, band[0])], tracks=False, vias=True),
          Keepout(["F.Cu", "B.Cu"], [(0, band[1]), (W, band[1]), (W, H), (0, H)], tracks=False, vias=True)]
    ref = [Track("A", "F.Cu", W_SIG, [(xa, 1.0), (xa, 9.0)]), Track("B", "F.Cu", W_SIG, [(xb, 1.0), (xb, 9.0)]),
           Track("SIG", "F.Cu", W_SIG, [(xv, 1.0), (xv, yv)]), Track("SIG", "B.Cu", W_SIG, [(xv, yv), (xv, 9.0)])]
    fv = (far, 5.2)
    car = [Track("A", "F.Cu", W_SIG, [(xa, 1.0), (xa, 9.0)]), Track("B", "F.Cu", W_SIG, [(xb, 1.0), (xb, 9.0)]),
           Track("SIG", "F.Cu", W_SIG, [(xv, 1.0), (xv, 2.0), (far, 2.0), fv]),
           Track("SIG", "B.Cu", W_SIG, [fv, (far, 8.0), (xv, 8.0), (xv, 9.0)])]
    l_ref = _len(ref[2].pts) + _len(ref[3].pts)
    l_car = _len(car[2].pts) + _len(car[3].pts)
    text = {
        "purpose": "A via must change layer inside a pad gap that two other wires (A, B) pass too. Via centres are "
                   "allowed only in y 4.95 .. 5.40 (rule areas), so only inside the wall's two gaps: the near gap "
                   f"between P1's straight side (x {xl}) and the round pad P2 (centre ({c2[0]:.2f}, {c2[1]:.2f}), r {r2}), and the far gap "
                   f"(x {far0:.2f} .. 15.00). SIG: U1.1 (F.Cu, north) to T1.1 (B.Cu, south).",
        "gap": f"near gap {gap0:.3f} mm at y {yr} (its narrowest), {wide:.3f} at y {yv}; via 0.6 + two 0.25 wires at "
               f"0.2 need {need:.2f} (clear + w + clear + w + clear + via + clear)",
        "answer": f"SIG straight down: F.Cu to the via at ({xv:.3f}, {yv}) in the near gap, B.Cu on to T1.1: "
                  f"{l_ref:.2f} mm, 1 via; A and B straight beside it; nothing predicted.",
        "careless": f"the gap cut measured at one fixed point where the gap is {gap0:.2f}: over-full by "
                    f"{need - gap0:.2f} mm, so the plan takes the far gap ({l_car:.2f} mm, {l_car / l_ref:.2f} x) "
                    "or reports a violation the drawing does not have.",
        "why": "Solvable by construction: the reference was laid out first (A, B, the via at 0.203 mm from each "
               f"neighbour, P2 cleared by {dv:.3f}); the wall's other gaps are 0.2 mm, P1 and P4 lie 0.5 mm from the "
               "edges (copper-edge 0.5), so nothing passes elsewhere. The rule: a via sits where it fits in its "
               "triangle (Maley 1987 pp. 32-33: routable iff no critical cut is over-full, for the via at some "
               "position, not at one fixed point).",
        "lengths": {"reference": round(l_ref, 3), "careless": round(l_car, 3), "need": need, "gap_min": gap0,
                    "gap_at_via": round(wide, 4), "via_to_p2": round(dv, 4)},
    }
    return TPattern("via_gap_best_point", "Via in a widening pad gap with two crossers: the via where it fits",
                    (W, H), 2, nets, pads, ref, [Via("SIG", (xv, yv), VIA_D, VIA_DRILL)], car,
                    [Via("SIG", fv, VIA_D, VIA_DRILL)], keepouts=ko, text=text)


# ------------------------------------------------------------------------------------------------- 2. pour_ring
def pour_ring() -> TPattern:
    W, H = 12.0, 10.0
    v1, v2, v3 = (2.0, 8.0), (10.2, 8.0), (6.0, 3.5)
    g = [(5.0, 6.0), (6.0, 6.0), (7.0, 6.0)]
    row = [3.1 + 1.2 * k for k in range(6)]  # 1.0 mm pads at 1.2 pitch (0.2 gaps: the pour cannot pass)
    fps = [Fp("V1", [Pad("1", "VCC", v1, (0.8, 0.8))], "terminal"),
           Fp("V2", [Pad("1", "VCC", v2, (0.8, 0.8))], "terminal"),
           Fp("V3", [Pad("1", "VCC", v3, (0.8, 0.8))], "terminal"),
           Fp("C1", [Pad("2", "GND", g[0], (0.6, 0.8))], "obstacle"),
           Fp("C2", [Pad("2", "GND", g[1], (0.6, 0.8))], "obstacle"),
           Fp("C3", [Pad("2", "GND", g[2], (0.6, 0.8))], "obstacle"),
           Fp("R", [Pad(str(k + 1), None, (x, 8.0), (1.0, 1.0)) for k, x in enumerate(row)], "wall"),
           Fp("H1", [Pad("1", "GND", (1.5, 1.5), (1.6, 1.6), "circle", drill=0.8)], "obstacle")]
    assert row[0] - 0.5 - (v1[0] + 0.4) >= CLEAR - 1e-9 and v2[0] - 0.4 - (row[-1] + 0.5) >= CLEAR - 1e-9
    # open side: V1-V2 north of the row (between the row and the GND pads), V2-V3 on the east
    yt = 7.5 - CLEAR - W_SIG / 2 - 0.003     # under the row's north side
    assert yt - W_SIG / 2 - (6.0 + 0.4) >= CLEAR  # clears the GND pads
    yc = 7.0                                  # the V pads' wires leave north first (the row's corners)
    # V3 joins the trunk with a branch (Steiner T) west of C1: past C1's north-west corner, down to the trunk.
    xb, yb = 4.7 - CLEAR - W_SIG / 2 - 0.005, 5.6 - CLEAR - W_SIG / 2 - 0.005
    ref = [Track("VCC", "F.Cu", W_SIG, [v1, (v1[0], yt), (v2[0], yt), v2]),
           Track("VCC", "F.Cu", W_SIG, [v3, (xb, yb), (xb, yt)])]
    car = [Track("VCC", "F.Cu", W_SIG, [v1, (v1[0], yc), v3]), Track("VCC", "F.Cu", W_SIG, [v3, (v2[0], yc), v2])]
    l_ref, l_car = sum(_len(t.pts) for t in ref), sum(_len(t.pts) for t in car)
    text = {
        "purpose": "GND pads C1.2, C2.2, C3.2 (F.Cu) reach GND only through the F.Cu GND pour (whole board; the net's "
                   "main copper holds H1.1). VCC joins V1, V2, V3. The pad row R (no net, 0.2 mm gaps) lies between "
                   "V1 and V2 south of the GND pads, so the pour cannot pass there. B.Cu is closed (rule area).",
        "answer": f"VCC trunk V1-V2 along the row's north side, V3 joins it with a T west of C1 ({l_ref:.2f} mm, the "
                  "shortest tree that leaves the GND pads open): the pour reaches them from the north-east; 0 unconnected.",
        "careless": f"the shorter tree V1-V3-V2 ({l_car:.2f} mm, {l_ref - l_car:.2f} shorter) rings the GND pads with "
                    "the row: after refill they are an island (KiCad: 1 unconnected; no stitch via, B.Cu closed).",
        "why": "Planes are terminals (user rule 2026-11-10): a plan that cuts a pour terminal off its pour pays a "
               "graded price (costly, not forbidden), as a dropped connection costs. TopoR 6.3 manual p. 121 only "
               "lets the user delete such islands after the pour.",
        "lengths": {"reference": round(l_ref, 3), "careless": round(l_car, 3)},
    }
    return TPattern("pour_ring", "VCC tree round three pour-served GND pads: leave them open",
                    (W, H), 2, ["GND", "VCC"], fps, ref, [], car, [],
                    keepouts=[Keepout(["B.Cu"], [(0, 0), (W, 0), (W, H), (0, H)])],
                    zones=[("GND", "F.Cu", (0.3, 0.3, W - 0.3, H - 0.3))], text=text)


PROPOSED = {f.__name__: f for f in (via_gap_best_point, pour_ring)}


# ------------------------------------------------------------------------------------------------- boards, png
def _zone(b, net: str, layer: int, rect) -> None:
    z = pcbnew.ZONE(b)
    z.SetLayer(layer)
    z.SetNet(b.FindNet(net))
    z.SetPadConnection(pcbnew.ZONE_CONNECTION_FULL)
    z.SetMinThickness(int(0.25 * MM))
    z.SetLocalClearance(int(CLEAR * MM))
    ol = pcbnew.SHAPE_LINE_CHAIN()
    x0, y0, x1, y1 = rect
    for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1)):
        ol.Append(pcbnew.VECTOR2I(int(x * MM), int(y * MM)))
    ol.SetClosed(True)
    z.AddPolygon(ol)
    b.Add(z)


def build_variant(pat: TPattern, variant: str):
    class View:
        size, layers, nets, fps, keepouts = pat.size, pat.layers, pat.nets, pat.fps, pat.keepouts
        ref_tracks = {"problem": [], "reference": pat.ref_tracks, "careless": pat.careless_tracks}[variant]
        ref_vias = {"problem": [], "reference": pat.ref_vias, "careless": pat.careless_vias}[variant]

    b = build_board(View, copper=variant != "problem")
    for net, layer, rect in pat.zones:
        _zone(b, net, {"F.Cu": pcbnew.F_Cu, "B.Cu": pcbnew.B_Cu}[layer], rect)
    if pat.zones:
        pcbnew.ZONE_FILLER(b).Fill(b.Zones())
    return b


def write_variant(pat: TPattern, path: Path, variant: str) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    pcbnew.SaveBoard(str(path), build_variant(pat, variant))
    write_project_file(path, rules=pat.rules, classes=pat.classes)
    return path


def render(pat: TPattern, out: Path) -> Path:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.patches import Circle, Polygon, Rectangle

    col = {"SIG": "#d62728", "A": "#2ca02c", "B": "#9467bd", "VCC": "#ff7f0e", "GND": "#1f77b4", None: "#999"}
    W, H = pat.size
    fig, axes = plt.subplots(1, 2, figsize=(15, 6.2))
    for ax, (label, tracks, vias) in zip(axes, (("expected", pat.ref_tracks, pat.ref_vias),
                                                 ("careless", pat.careless_tracks, pat.careless_vias))):
        ax.add_patch(Rectangle((0, 0), W, H, fill=False, ec="#666", lw=1))
        for net, layer, r in pat.zones:
            ax.add_patch(Rectangle((r[0], r[1]), r[2] - r[0], r[3] - r[1], fc=col[net], alpha=0.10, ec=col[net], lw=0.6))
            ax.text(r[0] + 0.1, r[3] - 0.15, f"{net} pour, {layer}", color=col[net], fontsize=7)
        for k in pat.keepouts:
            xs, ys = zip(*k.poly)
            what = ("vias" if k.vias else "") + (" + tracks" if k.tracks else "")
            ax.add_patch(Polygon(k.poly, closed=True, fc="none", ec="#bbb", lw=0.6, hatch="..", alpha=0.5))
            ax.text(min(xs) + 0.1, min(ys) + 0.35, f"rule area {'/'.join(k.layers)}: no {what}", color="#888", fontsize=6)
        for f in pat.fps:
            for p in f.pads:
                c = col[p.net]
                if p.shape == "circle":
                    ax.add_patch(Circle(p.pos, p.size[0] / 2, fc=c, ec="k", lw=0.5, alpha=0.9))
                else:
                    tr = matplotlib.transforms.Affine2D().rotate_deg_around(p.pos[0], p.pos[1], -p.angle) + ax.transData
                    ax.add_patch(Rectangle((p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2), *p.size, fc=c, ec="k",
                                           lw=0.5, alpha=0.9 if p.layer == "F.Cu" else 0.4, hatch=None if p.layer == "F.Cu" else "//",
                                           transform=tr))
                if p.drill:
                    ax.add_patch(Circle(p.pos, p.drill / 2, fc="w", ec="k", lw=0.3))
                if f.role != "wall" or f.ref in ("P1", "P2"):
                    ax.text(p.pos[0], p.pos[1] - max(p.size) / 2 - 0.12, f"{f.ref}.{p.num}" + (f" {p.net}" if p.net else ""),
                            ha="center", fontsize=6)
        for t in tracks:
            xs, ys = zip(*t.pts)
            ax.plot(xs, ys, color=col[t.net], lw=2.0 if t.layer == "F.Cu" else 2.0, ls="-" if t.layer == "F.Cu" else "--",
                    solid_capstyle="round")
        for v in vias:
            ax.add_patch(Circle(v.pos, v.dia / 2, fc="#ffd700", ec="k", lw=0.7))
            ax.add_patch(Circle(v.pos, v.drill / 2, fc="w", ec="k", lw=0.4))
        ax.set_title(textwrap.fill(f"{label}: " + pat.text["answer" if label == "expected" else "careless"], 95), fontsize=8)
        ax.set_xlim(-0.3, W + 0.3)
        ax.set_ylim(H + 0.3, -0.3)
        ax.set_aspect("equal")
        ax.tick_params(labelsize=7)
    fig.suptitle(f"{pat.name}: {pat.title}\n" + textwrap.fill(pat.text["purpose"], 190), fontsize=8)
    fig.text(0.5, 0.01, textwrap.fill("solid = F.Cu, dashed = B.Cu, yellow = via, hatched pad = B.Cu pad. " + pat.text["why"], 210),
             ha="center", fontsize=7)
    fig.tight_layout(rect=(0, 0.09, 1, 0.88))
    out = Path(out)
    fig.savefig(out, dpi=130)
    plt.close(fig)
    return out


def generate(name: str, out: Path, *, png: bool = True) -> dict:
    pat = PROPOSED[name]()
    d = Path(out) / pat.name
    paths = {v: write_variant(pat, d / v / f"{pat.name}.kicad_pcb", v) for v in ("problem", "reference", "careless")}
    doc = {"name": pat.name, "title": pat.title, "kind": "solvable", "status": "proposed (tw4-r2), awaiting user approval",
           "rules_assumed": {"authority": "board", "margin": 0.0}, **pat.text,
           "connections": [n for n in pat.nets]}
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    paths["expected"] = d / "expected.json"
    if png:
        paths["png"] = render(pat, d / f"{pat.name}.png")
    return paths


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    out = Path(argv[0]) if argv else Path("scratch/tw4r2-patterns")
    for n in PROPOSED:
        for k, v in generate(n, out).items():
            print(n, k, v)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
