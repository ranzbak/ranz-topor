"""PROPOSED test patterns for neck-regrow (docs/dev/design/neck-regrow.md): a trace never widens again after a neck.

NOT in the test suite: the user approves patterns first (AGENT_RULES). Built like harness/dvn_patterns.py (reference copper
first, pads around it, a shapely clearance check, KiCad DRC with --drc). Two layers, B.Cu closed. PWR is POWER class
0.5 / 0.2 with a 0.2 mm neck floor (.kicad_dru, as dvn_patterns); the walls are unconnected pads.

1. `neck_both_ends` (R-a) PWR runs straight from P1 to P2; each end sits in a 2 mm channel between two wall pads where
                    only a 0.3 mm wire fits. Expected: ONE neck, the whole wire at 0.3 (a neck at each end with full
                    width between them is narrow-wide-narrow; user 2026-11-07 / 2026-11-26).
2. `neck_reentry`   (R-b) PWR leaves P1 through a 2 mm channel open to the right only, turns round the upper wall's end and
                    runs back left above it to P2. Expected: necked in the channel only, full width from the mouth on,
                    also on the way back past P1 (a zone that is a circle round P1 narrows that leg again).

    python -m harness.neckrg_patterns [OUT] [--drc]
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from harness.dvn_patterns import DRU, FLOOR, POWER, W_POW
from harness.fit_patterns import CLEAR, DEFAULT_CLASS, Fp, Keepout, Pad, Pattern, Track, write_board
from harness.fx_survivors import clearance_report
from harness.radii_patterns import EPS

STATUS: dict[str, str] = {}
SQ2 = math.sqrt(2.0)
W_NECK = 0.3  # the widest wire between the walls: wall gap 0.3 + 2 x (CLEAR + EPS)
WALL_T = 0.3  # wall pad thickness (y)


def _len(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def _r(x: float) -> float:
    return round(x, 4)


def _walls(ref: str, x0: float, x1: float, y: float) -> Fp:
    """Two unconnected wall pads from x0 to x1 above and below y, their inner edges W_NECK/2 + CLEAR + EPS from y."""
    off = W_NECK / 2 + CLEAR + EPS + WALL_T / 2
    c, ln = (0.5 * (x0 + x1), x1 - x0)
    return Fp(ref, [Pad("1", None, (_r(c), _r(y - off)), (_r(ln), WALL_T)), Pad("2", None, (_r(c), _r(y + off)), (_r(ln), WALL_T))], "wall")


def _base(name, title, purpose, answer, why, size, fps, tracks, conns, keep, extra) -> Pattern:
    W, H = size
    return Pattern(
        name=name, title=title, kind="solvable",
        gap_classes=["neck-regrow: at most one necked stretch per wire, touching a pad end or the whole wire"],
        purpose=purpose, answer=answer, why=why, size=size, layers=2, nets=["PWR"], fps=fps, connections=conns, cuts=[],
        keepouts=[Keepout(["B.Cu"], [(0.0, 0.0), (W, 0.0), (W, H), (0.0, H)])] + keep,
        classes=[dict(DEFAULT_CLASS), dict(POWER)], net_patterns=[{"netclass": "POWER", "pattern": "PWR"}], dru=DRU,
        ref_tracks=tracks, ref_vias=[], witness={"PWR": ("F.Cu", [p for t in tracks for p in t.pts])},
        extra={"nrg": {"ref_len": _r(sum(_len(t.pts) for t in tracks)), "vias": 0, **extra,
                       "literature": "TopoR 6.3 manual p. 56 (the minimum width only where needed, near pads narrower than "
                                     "the wire); user rules 2026-11-05 (a longer neck costs no capacity), 2026-11-07 (never "
                                     "narrow-wide-narrow), 2026-11-26 (a trace never widens again downstream of a neck)"}})


# ------------------------------------------------------------------------------------------------ 1. neck_both_ends
def neck_both_ends() -> Pattern:
    W, H, y = 13.0, 6.0, 3.0
    p1, p2 = (1.5, y), (11.5, y)
    fps = [Fp("P1", [Pad("1", "PWR", p1, (0.3, 0.3))], "terminal"), Fp("P2", [Pad("1", "PWR", p2, (0.3, 0.3))], "terminal"),
           _walls("WL", 1.0, 3.5, y), _walls("WR", 9.5, 12.0, y),
           # unconnected 0.3 mm pads 1.5 mm off the line in the free middle: the middle is crossed by several triangulation
           # edges with ample room (the two ends' bottlenecks are separate, as on a real board)
           Fp("M", [Pad(str(i + 1), None, (4.5 + (i // 2), y + (1.5 if i % 2 else -1.5)), (0.3, 0.3)) for i in range(10)], "obstacle")]
    tracks = [Track("PWR", "F.Cu", W_NECK, [p1, p2])]
    return _base(
        "neck_both_ends", "A wire that must neck at both ends: one neck end to end, never narrow-wide-narrow",
        f"Two layers, B.Cu closed. PWR (POWER class, {W_POW} mm, neck floor {FLOOR} mm) from P1 {p1} to P2 {p2}, two 0.3 mm "
        f"square pads. Each pad sits in a 2.5 mm channel between two unconnected wall pads (0.3 mm thick) where the widest "
        f"wire is {W_NECK} mm; between the channels (x 3.5 .. 9.5) the line is free (small unconnected pads 1.5 mm off it).",
        f"PWR straight from P1 to P2 at {W_NECK} mm all the way: one necked stretch over the whole wire. Full width in the "
        "middle would widen the trace again after its neck (narrow-wide-narrow), which the user's rule forbids; the narrow "
        "ends set the current anyway (user 2026-11-05).",
        "Solvable by construction: the straight line is the only shortest path; the channels force the neck width.",
        (W, H), fps, tracks, [("PWR", "P1.1", "P2.1")], [],
        {"neck_stretches": 1, "neck_width": W_NECK, "whole_wire_neck": True})


# ------------------------------------------------------------------------------------------------ 2. neck_reentry
def neck_reentry() -> Pattern:
    W, H, y = 8.0, 7.0, 4.5
    p1 = (1.6, y)
    lat = W_NECK / 2 + CLEAR + EPS  # the channel walls' inner edges from the axis
    need = W_POW / 2 + CLEAR + EPS  # a full-width centre line from wall copper
    off = lat + WALL_T / 2
    xl, xu = 3.5, 6.0  # right ends of the lower and the (longer) upper wall
    low = Pad("1", None, (_r(0.5 * (0.6 + xl)), _r(y + off)), (_r(xl - 0.6), WALL_T))
    upp = Pad("2", None, (_r(0.5 * (0.6 + xu)), _r(y - off)), (_r(xu - 0.6), WALL_T))
    walls = Fp("WL", [low, upp], "wall")
    cap = Fp("WC", [Pad("1", None, (0.95, y), (0.3, 0.3))], "wall")  # the channel's closed left end
    ub, ut = y - lat, y - lat - WALL_T  # the upper wall's lower and upper edge (KiCad y down)
    yf = _r(ub + need)  # full width below the upper wall
    # Necked along the axis until the full-width centre line at yf clears the lower wall's corner (xl, y + lat).
    xf = _r(xl + math.sqrt(max(need * need - (y + lat - yf) ** 2, 0.0)))
    q = need * (SQ2 - 1)
    xr = _r(xu + need)
    yb = _r(ut - need)  # the leg back left above the upper wall
    p2 = (1.6, _r(yb - 1.5))
    neck = [p1, (_r(xf - (yf - y)), y), (xf, yf)]
    full = [(xf, yf), (_r(xu + q), yf), (xr, _r(ub + q)), (xr, _r(ut - q)), (_r(xu + q), yb), (_r(p2[0] + (yb - p2[1])), yb), p2]
    fps = [Fp("P1", [Pad("1", "PWR", p1, (0.3, 0.3))], "terminal"), Fp("P2", [Pad("1", "PWR", p2, (1.0, 1.0))], "terminal"), walls, cap]
    tracks = [Track("PWR", "F.Cu", W_NECK, neck), Track("PWR", "F.Cu", W_POW, full)]
    d_back = _r(math.dist(p1, (p2[0] + (yb - p2[1]), yb)))
    return _base(
        "neck_reentry", "A neck out of a channel, then back past its own pad: full width after the neck, also on the way back",
        f"Two layers, B.Cu closed. PWR (POWER class, {W_POW} mm, neck floor {FLOOR} mm) from P1 {p1} (0.3 mm pad) to P2 {p2}. P1 "
        f"sits near the closed left end of a channel between two unconnected wall pads where only {W_NECK} mm fits; the lower "
        f"wall ends at x {xl}, the upper one runs on to x {xu}. P2 lies above, so PWR leaves the channel, runs under the upper "
        f"wall, turns round its end and comes back left above it, {d_back} mm from P1's centre at the nearest.",
        f"PWR {W_NECK} mm from P1 to ({xf}, {yf}) (where the 0.5 mm wire clears the lower wall's corner and the upper wall), "
        f"then {W_POW} mm under the upper wall, round its end and back left above it to P2. One necked stretch, at P1. The leg "
        "back above the wall has room for full width and stays full width although it passes P1 again.",
        "Solvable by construction: the walls force the neck in the channel; under and above the upper wall the room is "
        "derived from the full width; the only way to P2 is round the upper wall's right end.",
        (W, H), fps, tracks, [("PWR", "P1.1", "P2.1")], [],
        {"neck_stretches": 1, "neck_width": W_NECK, "neck_to": [xf, yf], "back_leg_y": yb})


PATTERNS = {f.__name__: f for f in (neck_both_ends, neck_reentry)}


def generate(name: str, out: Path, *, drc: bool = False) -> dict:
    pat = PATTERNS[name]()
    bad = clearance_report(pat)
    if bad:
        raise SystemExit(f"{name}: reference breaks a clearance: {bad}")
    d = Path(out) / name
    paths = {"problem": write_board(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_board(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)}
    doc = {"name": pat.name, "title": pat.title, "status": STATUS.get(name, "PROPOSED (awaiting approval)"), "purpose": pat.purpose,
           "expected": pat.answer, "why": pat.why, "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
           "reference_tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": t.pts} for t in pat.ref_tracks],
           **pat.extra.get("nrg", {})}
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
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "nrg-patterns" / "boards"
    for name in PATTERNS:
        print(name, *(str(p) for p in generate(name, out, drc=drc).values()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
