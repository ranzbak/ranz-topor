"""PROPOSED test patterns for bundle-turn (RCA reports/bundle-turn-rca-*.md): a bundle that fits straight at clearance fits
through its turns too (nested corners), and keeps full clearance away from the one wire's unavoidable bottleneck.

Built like harness/neckrg_patterns.py (reference copper first, pads around it, a shapely clearance check with circles for
round pads, KiCad DRC with --drc). Two layers, B.Cu closed, Default class 0.25 / 0.2 (board minimum width = class width:
no neck). Walls are unconnected pads (grey), keepouts hatched.

1. `turn_fork_corner` (bug 1, qmtech JS1 bus at RN2) N1..N3 come down-right at pitch and turn down into a corridor
                      between an unconnected wall and a pad column that fits N2 + N3 exactly. N1 ends on the column's top
                      pad T1 (the fork: N2 passes T1's corner, N1 stops there). Expected: nested 45 deg corners at pitch;
                      N1 keeps its outer slot down to T1 (bending early puts it into N2's slot: N2 then cannot fit).
2. `hole_bundle_room` (bug 2, qmtech JS1 bus under H2) A, B, C come down-right and turn right under a 3 mm round PTH pad
                      (over the top is keepout), with 3 mm of free board below. B ends in a pocket whose entrance (two
                      unconnected pads, 0.5 mm apart) is narrower than B needs (0.65): B's run is forced. Expected: one
                      bend per wire, nested at pitch, A at clearance to the hole; the only violation is B in the entrance.

    python -m harness.bundle_turn_patterns [OUT] [--drc]
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from harness.fit_patterns import CLEAR, DEFAULT_CLASS, W_SIG, Fp, Keepout, Pad, Pattern, Track, write_board
from harness.radii_patterns import EPS

STATUS: dict[str, str] = {}
SQ2 = math.sqrt(2.0)
NEED = W_SIG / 2 + CLEAR + EPS  # a wire's centre line from foreign copper
P = W_SIG + CLEAR + EPS  # the bundle pitch (centre to centre)
LIT = ("TopoR 6.3 manual p. 237 (nominal clearance wherever wires can be set apart, minimal only in the narrowest spots), "
       "p. 239 (wires stay further apart where there is room), p. 240 (FAQ: DRC violations although there is plenty of room); "
       "Bast, Brosi & Storandt 2020 sec. 4.3 p. 7 (room kept for lines drawn later)")


def _r(x: float) -> float:
    return round(x, 4)


def _pt(p) -> tuple[float, float]:
    return (_r(p[0]), _r(p[1]))


def _len(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def _rect(ref: str, num: str, x0: float, x1: float, y0: float, y1: float) -> Pad:
    return Pad(num, None, (_r(0.5 * (x0 + x1)), _r(0.5 * (y0 + y1))), (_r(x1 - x0), _r(y1 - y0)))


def _box(x0, y0, x1, y1) -> list[tuple[float, float]]:
    return [(_r(x0), _r(y0)), (_r(x1), _r(y0)), (_r(x1), _r(y1)), (_r(x0), _r(y1))]


def _base(name, title, purpose, answer, why, size, nets, fps, tracks, conns, keep, extra) -> Pattern:
    W, H = size
    return Pattern(
        name=name, title=title, kind="solvable",
        gap_classes=["bundle-turn: parallel wires at clearance keep their pitch through 45 deg turns and away from bottlenecks"],
        purpose=purpose, answer=answer, why=why, size=size, layers=2, nets=nets, fps=fps, connections=conns, cuts=[],
        keepouts=[Keepout(["B.Cu"], _box(0.0, 0.0, W, H))] + keep, classes=[dict(DEFAULT_CLASS)],
        ref_tracks=tracks, ref_vias=[], witness={t.net: ("F.Cu", t.pts) for t in tracks},
        extra={"bt": {"ref_len": {t.net: _r(_len(t.pts)) for t in tracks}, "bends": {t.net: len(t.pts) - 2 for t in tracks},
                      "vias": 0, **extra, "literature": LIT}})


# ------------------------------------------------------------------------------------------------ 1. turn_fork_corner
def turn_fork_corner() -> Pattern:
    W, H = 9.0, 8.6
    xr, yt = 7.0, 5.0  # the pad column (0.8 x 0.65 pads at 0.8 pitch, as RN2): T1 terminal, then two unconnected pads
    x2 = xr - 0.4 - NEED  # N2 at clearance from the column, N3 one pitch left, N1 one pitch right (its outer slot)
    x3, x1 = x2 - P, x2 + P
    y3 = yt - 1.0  # N3's (inner) corner; the outer corners sit P (sqrt2 - 1) higher each (offset curves, mitred)
    y2 = y3 - P * (SQ2 - 1)
    y1 = y2 - P * (SQ2 - 1)
    c = {1: x1 - y1, 2: x2 - y2, 3: x3 - y3}  # the diagonals x - y = c
    K = 5.0  # sources on the line x + y = K (perpendicular to the diagonals: P apart)
    src = {i: ((K + c[i]) / 2, (K - c[i]) / 2) for i in c}
    yb = yt + 2.6  # N2, N3 targets below the corridor
    wall = (x3 - NEED - 0.4, x3 - NEED, y3 + 0.3, yt + 1.925)  # left wall pad: x0, x1, y0, y1
    fps = [Fp("T", [Pad("1", "N1", (xr, yt), (0.8, 0.65)), Pad("2", None, (xr, yt + 0.8), (0.8, 0.65)),
                    Pad("3", None, (xr, yt + 1.6), (0.8, 0.65))], "terminal"),
           Fp("WL", [_rect("WL", "1", *wall)], "wall"),
           Fp("S", [Pad(str(i), f"N{i}", _pt(src[i]), (W_SIG, W_SIG), shape="circle") for i in (1, 2, 3)], "terminal"),
           Fp("B", [Pad("2", "N2", _pt((x2, yb)), (W_SIG, W_SIG), shape="circle"),
                    Pad("3", "N3", _pt((x3, yb)), (W_SIG, W_SIG), shape="circle")], "terminal")]
    tracks = [Track("N1", "F.Cu", W_SIG, [_pt(src[1]), _pt((x1, y1)), _pt((x1, yt - (xr - x1))), (xr, yt)]),
              Track("N2", "F.Cu", W_SIG, [_pt(src[2]), _pt((x2, y2)), _pt((x2, yb))]),
              Track("N3", "F.Cu", W_SIG, [_pt(src[3]), _pt((x3, y3)), _pt((x3, yb))])]
    return _base(
        "turn_fork_corner", "A bundle that fits straight fits through its 45 deg turn: nested corners, the outer wire keeps its slot",
        f"Two layers, B.Cu closed, Default 0.25 / 0.2 (pitch {_r(P)} mm). N1, N2, N3 leave three round pads P apart on a "
        f"45 deg line (upper left) and run down-right. A pad column (0.8 x 0.65 at 0.8 pitch, like RN2) stands at x {xr}: "
        f"its top pad T1 is N1's target, the two below are unconnected. Left of it an unconnected wall pad leaves a corridor "
        f"that fits N2 and N3 at exactly clearance (x {_r(x2)} and {_r(x3)}); N2 and N3 end below the corridor.",
        f"All three one 45 deg turn, nested: corners at ({_r(x1)}, {_r(y1)}), ({_r(x2)}, {_r(y2)}), ({_r(x3)}, {_r(y3)}) (each "
        f"outer corner {_r(P * (SQ2 - 1))} mm higher), verticals at pitch. N1 runs down in its outer slot x {_r(x1)} and jogs "
        "45 deg into T1's centre. Bending N1 earlier (same length) puts it into N2's slot past T1's corner, and N2 no longer "
        "fits: a hair violation or a neck there is the drawing's fault, not the plan's.",
        "Solvable by construction: every wire takes an octilinear shortest path (dy > dx: one diagonal, one vertical); the "
        "corridor fixes N2 and N3 to their slots; N1's form is one of several equal-length forms, the only one that leaves "
        "N2 its slot (user rule 2026-11-27: if the traces can exist vertically they can all make a 45 deg turn).",
        (W, H), ["N1", "N2", "N3"], fps, tracks, [("N1", "S.1", "T.1"), ("N2", "S.2", "B.2"), ("N3", "S.3", "B.3")], [],
        {"pitch": _r(P), "check_window": _box(x3 - 1.0, y1 - 1.0, xr + 0.5, yt + 2.0), "allowed_violations": []})


# ------------------------------------------------------------------------------------------------ 2. hole_bundle_room
def hole_bundle_room() -> Pattern:
    W, H = 14.5, 9.0
    cx, cy, dia = 6.5, 3.0, 3.0  # a mounting-hole pad (PTH, unconnected)
    R = dia / 2 + NEED  # A's centre line from the hole centre (tangent on the diagonal and on the horizontal)
    ya, ca = cy + R, cx - cy - R * SQ2
    y = {i: ya + i * P for i in range(3)}
    c = {i: ca - i * P * SQ2 for i in range(3)}
    K = 3.6
    src = {i: ((K + c[i]) / 2, (K - c[i]) / 2) for i in range(3)}
    corner = {i: (c[i] + y[i], y[i]) for i in range(3)}
    yb = y[1]
    xe0, xe1 = 11.6, 12.3  # the pocket entrance: two unconnected pads 0.5 mm apart (B needs 0.65: forced, unavoidable)
    xt, xtb = 11.0, 12.95
    nets = ["A", "B", "C"]
    fps = [Fp("H", [Pad("1", None, (cx, cy), (dia, dia), shape="circle", drill=2.0)], "obstacle"),
           Fp("E", [_rect("E", "1", xe0, xe1, yb - 0.65, yb - 0.25), _rect("E", "2", xe0, xe1, yb + 0.25, yb + 0.65)], "wall"),
           Fp("S", [Pad(str(i + 1), nets[i], _pt(src[i]), (W_SIG, W_SIG), shape="circle") for i in range(3)], "terminal"),
           Fp("T", [Pad("1", "A", _pt((xt, y[0])), (W_SIG, W_SIG), shape="circle"),
                    Pad("2", "B", _pt((xtb, yb)), (W_SIG, W_SIG), shape="circle"),
                    Pad("3", "C", _pt((xt, y[2])), (W_SIG, W_SIG), shape="circle")], "terminal")]
    keep = [Keepout(["F.Cu"], _box(cx - dia / 2, 0.0, W, cy)),  # over the hole: closed (the way round is below)
            Keepout(["F.Cu"], _box(xe1, yb - 1.0, 13.9, yb - 0.6)), Keepout(["F.Cu"], _box(xe1, yb + 0.6, 13.9, yb + 1.0)),
            Keepout(["F.Cu"], _box(13.5, yb - 0.6, 13.9, yb + 0.6))]  # the pocket round B's target
    ends = {0: (xt, y[0]), 1: (xtb, yb), 2: (xt, y[2])}
    tracks = [Track(nets[i], "F.Cu", W_SIG, [_pt(src[i]), _pt(corner[i]), _pt(ends[i])]) for i in range(3)]
    return _base(
        "hole_bundle_room", "A bundle round a mounting hole keeps full clearance while there is room; the forced wire breaks the rule only at its bottleneck",
        f"Two layers, B.Cu closed, Default 0.25 / 0.2 (pitch {_r(P)} mm). A, B, C leave three round pads on a 45 deg line "
        f"(upper left) for targets on the right; the straight lines run through a {dia} mm round PTH pad at ({cx}, {cy}), "
        f"the way over it is keepout, below it the board is free for 3 mm. B ends in a pocket (keepout) whose entrance is "
        f"two unconnected pads 0.5 mm apart at x {xe0}..{xe1}: B needs 0.65, so its run is forced (unavoidable).",
        f"Each wire one 45 deg turn under the hole, nested at pitch: A tangent to the hole at clearance (corner "
        f"({_r(corner[0][0])}, {_r(corner[0][1])})), B and C one pitch further out each. B's only violation: "
        f"{_r(0.25 - W_SIG / 2)} mm to both entrance pads over {_r(xe1 - xe0)} mm. Nothing within clearance of the hole, no stairs.",
        "Solvable by construction (up to B's entrance): the octilinear shortest paths round the hole's clearance octagon; "
        "the room below makes a squeeze at the hole a missed alternative; a forced wire relaxes clearance where it must, "
        "not along its whole run (TopoR manual p. 237: minimal clearance only in the narrowest spots).",
        (W, H), nets, fps, tracks, [("A", "S.1", "T.1"), ("B", "S.2", "T.2"), ("C", "S.3", "T.3")], keep,
        {"pitch": _r(P), "check_window": _box(cx - 3.5, cy - 0.5, xe0 - 0.2, H - 0.5),
         "allowed_violations": [{"net": "B", "with": "E", "box": _box(xe0 - 0.5, yb - 0.7, xe1 + 0.5, yb + 0.7)}]})


PATTERNS = {f.__name__: f for f in (turn_fork_corner, hole_bundle_room)}


def pad_geom(p: Pad):
    from shapely.geometry import Point, box

    if p.shape == "circle":
        return Point(p.pos).buffer(p.size[0] / 2, quad_segs=32)
    g = box(p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2, p.pos[0] + p.size[0] / 2, p.pos[1] + p.size[1] / 2)
    if p.angle:
        from shapely.affinity import rotate

        g = rotate(g, -p.angle, origin=p.pos)
    return g


def violations(pat: Pattern, segs) -> list[dict]:
    """Different-net copper closer than CLEAR on F.Cu (`segs` = [(layer, net, width, pts)]), circles for round pads; pads
    among themselves are the board's. [{a, b, gap, at}]: `at` = the midpoint of the shortest gap."""
    from shapely.geometry import LineString
    from shapely.ops import nearest_points

    items = [(net, f"track {net}", LineString(pts).buffer(w / 2, quad_segs=16)) for layer, net, w, pts in segs if layer == "F.Cu"]
    items += [(p.net, f"pad {f.ref}.{p.num}", pad_geom(p)) for f in pat.fps for p in f.pads if p.drill or p.layer == "F.Cu"]
    bad = []
    for i in range(len(items)):
        for j in range(i + 1, len(items)):
            (na, a, ga), (nb, b, gb) = items[i], items[j]
            if (na is not None and na == nb) or (a.startswith("pad") and b.startswith("pad")):
                continue
            d = ga.distance(gb)
            if d < CLEAR - 1e-6:
                p, q = nearest_points(ga, gb)
                bad.append({"a": a, "b": b, "gap": round(d, 4), "at": (round(0.5 * (p.x + q.x), 3), round(0.5 * (p.y + q.y), 3))})
    return bad


def allowed(pat: Pattern, v: dict) -> bool:
    for al in pat.extra["bt"]["allowed_violations"]:
        xs, ys = [q[0] for q in al["box"]], [q[1] for q in al["box"]]
        nets = {v["a"].split()[1].split(".")[0], v["b"].split()[1].split(".")[0]}
        if al["net"] in nets and al["with"] in nets and min(xs) <= v["at"][0] <= max(xs) and min(ys) <= v["at"][1] <= max(ys):
            return True
    return False


def generate(name: str, out: Path, *, drc: bool = False) -> dict:
    pat = PATTERNS[name]()
    bad = [v for v in violations(pat, [(t.layer, t.net, t.width, t.pts) for t in pat.ref_tracks]) if not allowed(pat, v)]
    if bad:
        raise SystemExit(f"{name}: reference breaks a clearance: {bad}")
    d = Path(out) / name
    paths = {"problem": write_board(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_board(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)}
    doc = {"name": pat.name, "title": pat.title, "status": STATUS.get(name, "PROPOSED (awaiting approval)"), "purpose": pat.purpose,
           "expected": pat.answer, "why": pat.why, "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
           "reference_tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": t.pts} for t in pat.ref_tracks],
           **pat.extra.get("bt", {})}
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
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "bt-patterns" / "boards"
    for name in PATTERNS:
        print(name, *(str(p) for p in generate(name, out, drc=drc).values()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
