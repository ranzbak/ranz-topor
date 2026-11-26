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
           _walls("WL", 1.0, 3.5, y), _walls("WR", 9.5, 12.0, y)]
    tracks = [Track("PWR", "F.Cu", W_NECK, [p1, p2])]
    return _base(
        "neck_both_ends", "A wire that must neck at both ends: one neck end to end, never narrow-wide-narrow",
        f"Two layers, B.Cu closed. PWR (POWER class, {W_POW} mm, neck floor {FLOOR} mm) from P1 {p1} to P2 {p2}, two 0.3 mm "
        f"square pads. Each pad sits in a 2.5 mm channel between two unconnected wall pads (0.3 mm thick) where the widest "
        f"wire is {W_NECK} mm; between the channels (x 3.5 .. 9.5) the board is free.",
        f"PWR straight from P1 to P2 at {W_NECK} mm all the way: one necked stretch over the whole wire. Full width in the "
        "middle would widen the trace again after its neck (narrow-wide-narrow), which the user's rule forbids; the narrow "
        "ends set the current anyway (user 2026-11-05).",
        "Solvable by construction: the straight line is the only shortest path; the channels force the neck width.",
        (W, H), fps, tracks, [("PWR", "P1.1", "P2.1")], [],
        {"neck_stretches": 1, "neck_width": W_NECK, "whole_wire_neck": True})


# ------------------------------------------------------------------------------------------------ 2. neck_reentry
def neck_reentry() -> Pattern:
    W, H, y = 9.0, 7.0, 4.5
    p1 = (1.6, y)
    x_mouth = 3.5
    wl = _walls("WL", 0.6, x_mouth, y)
    top = wl.pads[0].pos[1] - WALL_T / 2  # the upper wall's upper edge (KiCad y down: smaller y = above)
    # The channel's closed left end: an unconnected pad across it, left of P1 (0.2 from each wall: no 0.3 wire passes).
    cap = Fp("WC", [Pad("1", None, (0.95, y), (0.3, 0.3))], "wall")
    # Necked from P1 to the mouth plus where the full 0.5 mm round end clears both wall corners (lateral lat from the axis).
    lat = W_NECK / 2 + CLEAR + EPS
    need = W_POW / 2 + CLEAR + EPS
    x_full = _r(x_mouth + math.sqrt(max(need * need - lat * lat, 0.0)))
    # Round the upper wall's right end (corner (x_mouth, top)) octilinearly at `need` from the corner: up at x_mouth + need,
    # 45 deg tangent to the circle of radius `need`, left at top - need.
    xr = _r(x_mouth + need)
    y_v = _r(top - need * (SQ2 - 1))
    y_back = _r(top - need)
    x_h = _r(x_mouth + need * (SQ2 - 1))
    p2 = (1.6, _r(y_back - 1.5))
    pts = [p1, (x_full, y), (xr, y), (xr, y_v), (x_h, y_back), (_r(p2[0] + (y_back - p2[1])), y_back), p2]
    fps = [Fp("P1", [Pad("1", "PWR", p1, (0.3, 0.3))], "terminal"), Fp("P2", [Pad("1", "PWR", p2, (1.0, 1.0))], "terminal"), wl, cap]
    tracks = [Track("PWR", "F.Cu", W_NECK, pts[:2]), Track("PWR", "F.Cu", W_POW, pts[1:])]
    d_back = _r(abs(y_back - y))
    return _base(
        "neck_reentry", "A neck out of a channel, then back past its own pad: full width after the neck, also on the way back",
        f"Two layers, B.Cu closed. PWR (POWER class, {W_POW} mm, neck floor {FLOOR} mm) from P1 {p1} (0.3 mm pad) to P2 {p2}. P1 "
        f"sits near the closed end of a channel open to the right (x {x_mouth}), where only {W_NECK} mm fits. P2 lies "
        f"above the channel, so PWR leaves the channel, turns round the upper wall's end and runs back left above it, "
        f"{d_back} mm from P1's centre.",
        f"PWR {W_NECK} mm from P1 to x {x_full} (the mouth plus where the 0.5 mm round end clears the wall corners), then "
        f"{W_POW} mm round the wall end and back left above the wall to P2. One necked stretch, at P1. The leg back above the "
        "wall has room for full width and stays full width although it passes P1 again.",
        "Solvable by construction: the walls force the neck in the channel; above the wall the board is free.",
        (W, H), fps, tracks, [("PWR", "P1.1", "P2.1")], [],
        {"neck_stretches": 1, "neck_width": W_NECK, "neck_to_x": x_full, "back_leg_y": y_back})


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
