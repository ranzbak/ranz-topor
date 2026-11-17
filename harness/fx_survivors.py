"""PROPOSED test patterns for the open M3 mutant survivors (reports/radii-m3-20261116-2345.md, verify-radii-m3-20261117-0340.md).

NOT in the test suite: the user approves patterns first (AGENT_RULES). Each pattern is a tiny board with a known optimal answer, built
like `harness/radii_patterns.py`: the reference copper is laid out first at the exact radii the board's rules ask for (clearance + half width +
EPS), pads placed around it, so the reference passes KiCad DRC (`--drc`). The pattern's job is to reach the engine branch a mutant changes;
the engine decides whether the copper differs (see the gallery note for the per-mutant reach).

1. `edge_tangent`      feat_side          a pad whose top edge lies exactly on the straight line between two terminals
2. `collinear_row`     tip_side_alt/first, tie_label, vertex_leave_off   three pads whose top edges lie on that line
3. `diag_exit`         tpad_keep, own_tip_keep   a pad left diagonally through its own corner, between two neighbours
4. `via_fold`          erbs_x_any_layer, h_inject_every_layer   two layers, a via-free area, a net with ERBS on both layers
5. `qfn_two_ends`      h_exit_no_b        wire between two fine-pitch pad rows: pad-exit violations at both ends
6. `edge_pads`         h_edge_exit_whole  two pads 0.6 mm from the board edge, wire along the edge

No pattern for corridor_no_push and end_skip_off (the gallery note says why).

Rules on every board: Default class 0.25 mm at 0.20 mm, board minimum track width 0.25, copper to edge 0.50. B.Cu closed by a rule area
(no tracks, no vias) unless the pattern is two-layer. Octilinear, the engine default. mm, KiCad frame (y down).

    python -m harness.fx_survivors [OUT] [--drc]
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from harness.fit_patterns import (CLEAR, DEFAULT_CLASS, RULES, W_SIG, Fp, Keepout, Pad, Pattern, Track, Via,
                                  _strip, write_board)
from harness.radii_patterns import EPS

SQ2 = math.sqrt(2.0)
R = W_SIG / 2 + CLEAR + EPS  # centre line to foreign copper
STATUS = "PROPOSED (awaiting approval)"


def clearance_report(pat: Pattern) -> list[dict]:
    """Layer-aware shapely check of the reference (tracks, vias, pads; different nets at least CLEAR apart; tracks inside
    the copper-to-edge line). The KiCad DRC (--drc) is the authority."""
    from shapely.geometry import LineString, Point, box

    items = []  # (net, name, layers, geom)
    for t in pat.ref_tracks:
        items.append((t.net, f"track {t.net} {t.layer}", {t.layer}, LineString(t.pts).buffer(t.width / 2, quad_segs=16)))
    for v in pat.ref_vias:
        items.append((v.net, f"via {v.net}", {"F.Cu", "B.Cu"}, Point(v.pos).buffer(v.dia / 2, quad_segs=32)))
    for f in pat.fps:
        for p in f.pads:
            g = box(p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2, p.pos[0] + p.size[0] / 2, p.pos[1] + p.size[1] / 2)
            if p.angle:
                from shapely.affinity import rotate

                g = rotate(g, -p.angle, origin=p.pos)
            items.append((p.net, f"pad {f.ref}.{p.num}", {"F.Cu", "B.Cu"} if p.drill else {p.layer}, g))
    bad = []
    for i in range(len(items)):
        for j in range(i + 1, len(items)):
            (na, a, la, ga), (nb, b, lb, gb) = items[i], items[j]
            if (na is not None and na == nb) or not (la & lb) or (a.startswith("pad") and b.startswith("pad")):
                continue
            d = ga.distance(gb)
            if d < CLEAR - 1e-6:
                bad.append({"a": a, "b": b, "gap": round(d, 4)})
    W, H = pat.size
    e = RULES["min_copper_edge_clearance"]
    inner = box(e, e, W - e, H - e)
    for n, a, la, g in items:
        if a.startswith("track") and not inner.contains(g):
            bad.append({"a": a, "b": "edge", "gap": None})
    return bad


def _r(x: float) -> float:
    return round(x, 4)


def _len(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def _closed_bcu(W, H):
    return [Keepout(["B.Cu"], _strip(W, 0.0, H))]


def _bump(xl: float, xr: float, y: float, sign: int = -1):
    """Octilinear bump of a level wire at height y over a span [xl, xr] of foreign copper whose face is on y: legs on the
    chamfer lines at distance R from the corners, flat at y + sign R. Returns the five inner points."""
    return [(xl - R * SQ2, y), (xl - R * SQ2 + R, y + sign * R), (xr + R * SQ2 - R, y + sign * R), (xr + R * SQ2, y)]


T = 0.25  # terminal pad size: the wire's end must lie inside it, so it can leave the pad centre by 0.12 mm at most
ATT = 0.12


def _level(x0: float, x1: float, y: float):
    """The shortest octilinear wire between two small terminal pads at (x0, y), (x1, y) past foreign copper whose face is on y
    (above it, y up = smaller): it leaves each pad at its top edge (ATT above the centre), climbs 45 degrees to y - R and runs
    level. Only R - ATT = 0.207 mm of climb: the pads' own size lets the wire step aside, the foreign copper stays R away."""
    d = R - ATT
    return [(x0, _r(y - ATT)), (_r(x0 + d), _r(y - R)), (_r(x1 - d), _r(y - R)), (x1, _r(y - ATT))]


# ------------------------------------------------------------------------------------------------ 1. edge_tangent
def edge_tangent() -> Pattern:
    W, H = 12.0, 10.0
    y = 5.0
    h = 1.2 / SQ2  # half diagonal of a 1.2 mm square turned by 45 degrees
    j = Fp("J1", [Pad("1", "N", (1.5, y), (T, T))], "terminal")
    k = Fp("J2", [Pad("1", "N", (10.5, y), (T, T))], "terminal")
    o = Fp("O1", [Pad("1", None, (6.0, y + h), (1.2, 1.2), angle=45.0)], "obstacle")  # top vertex exactly on y
    # the offset of the diamond's upper sides by R: y = 5 + |x - 6| - R sqrt 2; level at y - ATT until it meets it, 45 degrees up
    # to y - R (the offset of the vertex), level, mirrored.
    d = R * SQ2 - ATT
    pts = [(1.5, _r(y - ATT)), (_r(6.0 - d), _r(y - ATT)), (_r(6.0 - d + (R - ATT)), _r(y - R)),
           (_r(6.0 + d - (R - ATT)), _r(y - R)), (_r(6.0 + d), _r(y - ATT)), (10.5, _r(y - ATT))]
    straight = 9.0
    return Pattern(
        name="edge_tangent", title="Pad vertex on the straight line", kind="solvable",
        gap_classes=["erbs: feature side when the nearest feature vertex lies on the band (feat_side)"],
        purpose="One wire from J1 to J2 along y = 5. A foreign pad O1 (no net, a 1.2 mm square turned by 45 degrees: a diamond) "
                "sits below the line with its top vertex exactly on it, so the zero-width straight band touches one vertex "
                "(no bend, no wrap) and runs through no part of the pad. The terminals are 0.25 mm pads.",
        answer=f"Level at y = {y - ATT:.2f} (the top edge of the terminals), 45 degrees up to y = {y - R:.3f} over the vertex, "
               f"45 degrees down, level into J2 ({_r(_len(pts))} mm vs the blocked straight {straight} mm). No via, DRC 0.",
        why="The straight line y = 5 passes 0 mm from O1's vertex (0.325 needed), so the wire must step up by 0.327 mm; O1 is "
            "below, so stepping up is the shortest, and only as high and as long as the diamond's offset needs (45 degree legs "
            "parallel to its upper sides at 0.327 mm). Going under is closed (through the pad), round it is longer.",
        size=(W, H), layers=2, nets=["N"], fps=[j, k, o], connections=[("N", "J1.1", "J2.1")], cuts=[],
        keepouts=_closed_bcu(W, H), classes=[dict(DEFAULT_CLASS)], ref_tracks=[Track("N", "F.Cu", W_SIG, pts)],
        witness={"N": ("F.Cu", pts)}, extra={"fx": {"kills": ["feat_side"], "ref_len": _r(_len(pts))}})


# ------------------------------------------------------------------------------------------------ 2. collinear_row
def collinear_row() -> Pattern:
    W, H = 18.0, 10.0
    y = 5.0
    xs = [5.5, 8.5, 11.5]
    j = Fp("J1", [Pad("1", "N", (1.5, y), (T, T))], "terminal")
    k = Fp("J2", [Pad("1", "N", (16.5, y), (T, T))], "terminal")
    row = Fp("O1", [Pad(str(i + 1), None, (x, y + 0.5), (1.4, 1.0)) for i, x in enumerate(xs)], "obstacle")
    xl, xr = xs[0] - 0.7, xs[-1] + 0.7
    pts = _level(1.5, 16.5, y)
    return Pattern(
        name="collinear_row", title="A row of pads whose top edges are collinear with the band", kind="solvable",
        gap_classes=["erbs: collinear obstacle points (tip_side, tie_label, vertex_leave_off)"],
        purpose="One wire from J1 to J2 along y = 5. Three foreign pads (no net, 1.4 x 1.0 mm, 1.6 mm apart, e.g. a row of test "
                "points) have their top edges exactly on that line: six obstacle vertices collinear with the straight band, and "
                "the cuts between neighbouring vertices lie along the band.",
        answer=f"One level run at y = {y - R:.3f} over the whole row, 45 degrees up out of J1's top edge and down into J2's "
               f"({_r(_len(pts))} mm). No dip into the 1.6 mm gaps. No via, DRC 0.",
        why="The straight line y = 5 touches the three pads' corners and is 0.325 mm inside their clearance; the pads are below, "
            "so the wire steps up by the missing 0.327 mm (0.207 mm 45 degree climbs at the 0.25 mm terminals) and runs level "
            "over the row. A dip into a gap would add length.",
        size=(W, H), layers=2, nets=["N"], fps=[j, k, row], connections=[("N", "J1.1", "J2.1")], cuts=[],
        keepouts=_closed_bcu(W, H), classes=[dict(DEFAULT_CLASS)], ref_tracks=[Track("N", "F.Cu", W_SIG, pts)],
        witness={"N": ("F.Cu", pts)},
        extra={"fx": {"kills": ["tip_side_alt", "tip_side_first", "tie_label", "vertex_leave_off"], "ref_len": _r(_len(pts))}})


# ------------------------------------------------------------------------------------------------ 3. diag_exit
def diag_exit() -> Pattern:
    W, H = 12.0, 12.0
    p0, t = (3.0, 9.0), (9.0, 3.0)  # 45 degrees up and right
    j = Fp("J1", [Pad("1", "N", p0, (1.0, 1.0))], "terminal")  # corner (3.5, 8.5) is on the line x + y = 12
    k = Fp("J2", [Pad("1", "N", t, (0.5, 0.5))], "terminal")
    f1 = Fp("F1", [Pad("1", None, (3.0, 7.0), (1.0, 1.0))], "obstacle")  # above-left of the line, 0.707 off
    f2 = Fp("F2", [Pad("1", None, (5.0, 9.0), (1.0, 1.0))], "obstacle")  # below-right, 0.707 off
    pts = [p0, t]
    return Pattern(
        name="diag_exit", title="Diagonal exit through a pad's own corner, between two neighbours", kind="solvable",
        gap_classes=["erbs: tips of cuts that end in the wire's own pad (tpad_keep, own_tip_keep)"],
        purpose="One wire from the square pad J1 (own net) at 45 degrees to J2. The line leaves J1 exactly through its corner. "
                "Two foreign pads F1 (above-left) and F2 (below-right) sit 0.707 mm off the line on either side, so the cuts "
                "from J1's corner to F1's and F2's corners start in the wire's own pad and the band crosses them at the pad.",
        answer=f"The straight 45 degree line from J1's centre to J2, {_r(_len(pts))} mm. No bend, no via, DRC 0 "
               f"(0.707 mm to both pads; 0.325 needed).",
        why="The straight line is the shortest octilinear path (pure 45 degrees) and is clean: the free width between F1 and F2 "
            "along the line is 1.41 mm, the wire needs 0.65. Any bend is longer. The own pad holds no room at its corner, so no "
            "detour round a cut tip there is wanted.",
        size=(W, H), layers=2, nets=["N"], fps=[j, k, f1, f2], connections=[("N", "J1.1", "J2.1")], cuts=[],
        keepouts=_closed_bcu(W, H), classes=[dict(DEFAULT_CLASS)], ref_tracks=[Track("N", "F.Cu", W_SIG, pts)],
        witness={"N": ("F.Cu", pts)}, extra={"fx": {"kills": ["tpad_keep", "own_tip_keep"], "ref_len": _r(_len(pts))}})


# ------------------------------------------------------------------------------------------------ 4. via_fold
def via_fold() -> Pattern:
    W, H = 14.0, 12.0
    kx = 4.0  # right edge of the no-via rule area
    viax = kx + 0.3 + EPS  # rule area edge + via radius
    vy = 6.0 - (W_SIG / 2 + CLEAR + 0.3) - EPS  # the via keeps its clearance to R
    q1 = Fp("J1", [Pad("1", "Q", (2.0, 4.0), (0.5, 0.5))], "terminal")
    q2 = Fp("J2", [Pad("1", "Q", (2.0, 8.0), (0.5, 0.5), layer="B.Cu")], "terminal")
    r1 = Fp("J3", [Pad("1", "R", (1.0, 6.0), (0.5, 0.5))], "terminal")
    r2 = Fp("J4", [Pad("1", "R", (13.0, 6.0), (0.5, 0.5))], "terminal")
    dy = vy - 4.0
    f = [(2.0, 4.0), (viax - dy, 4.0), (viax, vy)]  # level, then 45 degrees down to the via
    b = [(viax, vy), (2.0, vy + (viax - 2.0)), (2.0, 8.0)]  # 45 degrees down-left, then vertical into J2
    tf = Track("Q", "F.Cu", W_SIG, [(_r(x), _r(y)) for x, y in f])
    tb = Track("Q", "B.Cu", W_SIG, [(_r(x), _r(y)) for x, y in b])
    tr = Track("R", "F.Cu", W_SIG, [(1.0, 6.0), (13.0, 6.0)])
    ko = Keepout(["F.Cu", "B.Cu"], [(0.5, 1.5), (kx, 1.5), (kx, 10.5), (0.5, 10.5)], tracks=False, vias=True)
    return Pattern(
        name="via_fold", title="Two layers: a via-free area makes a net fold back across the same line", kind="solvable",
        gap_classes=["drawing: ERBS of other layers as planned room (erbs_x_any_layer); test hook per layer (h_inject_every_layer)"],
        purpose="Two layers. Net Q runs from J1 (front, 2,4) to J2 (back side, 2,8): it needs one via. The area x 0.5-4.0 mm, "
                "y 1.5-10.5 is a no-via rule area (a shielded component). Q's via is therefore right of x 4.3: Q leaves the area "
                "on F.Cu, changes layer, and comes back on B.Cu, so both its stretches cross the line x = 4 (the same "
                "triangulation edges). R is a straight front wire J3-J4 at y = 6, the neighbour of Q's F.Cu stretch on those edges "
                "and 0.625 mm from where Q's via has to stand.",
        answer=f"Q: F.Cu level then 45 degrees down to the via at (x {viax:.3f}, y {vy:.3f}), B.Cu 45 degrees back down-left and "
               f"vertical into J2; R straight (12 mm). One via, DRC 0.",
        why="The via cannot be left of x 4; the nearest legal x is the rule area's edge plus the via radius, the shortest fold. "
            "Q's total octilinear length is smallest for a via as close to the line between the pads as it can stand "
            "(f(y) = oct(2.3, y - 4) + oct(2.3, 8 - y) falls to 6.49 mm at y = 5.3 and rises again towards the pads: 7.26 mm at y = 4); "
            "R at y = 6 keeps the via off y = 5.375 .. 6.625, so it stands at the upper edge of that gap. R needs no bend.",
        size=(W, H), layers=2, nets=["Q", "R"], fps=[q1, q2, r1, r2],
        connections=[("Q", "J1.1", "J2.1"), ("R", "J3.1", "J4.1")], cuts=[], keepouts=[ko],
        classes=[dict(DEFAULT_CLASS)], ref_tracks=[tf, tb, tr], ref_vias=[Via("Q", (_r(viax), _r(vy)))],
        witness={"Q": ("F.Cu", tf.pts), "R": ("F.Cu", tr.pts)},
        extra={"fx": {"kills": ["erbs_x_any_layer", "h_inject_every_layer"], "ref_len": _r(_len(tf.pts) + _len(tb.pts) + 12.0)}})


# ------------------------------------------------------------------------------------------------ 5. qfn_two_ends
def qfn_two_ends() -> Pattern:
    W, H = 12.0, 11.0
    x0, x1, P, PW, PL = 5.0, 8.0, 0.4, 0.2, 0.8  # pad 3 of each row; 0.4 mm pitch, 0.2 x 0.8 mm pads

    def row(ref, y, xc):
        return Fp(ref, [Pad(str(i + 1), "N" if i == 2 else None, (xc + (i - 2) * P, y), (PW, PL)) for i in range(6)], "terminal")

    top, bot = row("U1", 2.0, x0), row("U2", 9.0, x1)
    # The 45 degree leg from (x0, y_leg) clears the right neighbour's pad corner (x0 + P - PW/2, 2.0 + PL/2) by R.
    cx, cy = P - PW / 2, 2.0 + PL / 2
    y_leg = cy - cx + R * SQ2
    pts = [(x0, 2.0), (x0, _r(y_leg)), (x1, _r(y_leg + (x1 - x0))), (x1, 9.0)]
    return Pattern(
        name="qfn_two_ends", title="Fine-pitch pad rows at both ends of one wire (unavoidable pad-exit violation)",
        kind="unsolvable by the rules (0.025 mm at each pad exit)",
        gap_classes=["erbs report: pad-exit violation run from the far end of the ERBS (h_exit_no_b)"],
        purpose="One wire from pad 3 of a 6-pad row U1 (0.2 x 0.8 mm pads, 0.4 mm pitch) at the top to pad 3 of a row U2 at the "
                "bottom, 3 mm to the right. A 0.25 mm wire centred on a pad is 0.175 mm from the neighbours' pad edges "
                "(0.2 needed): the board's minimum track width is 0.25 mm, so it cannot neck. The violation at both pad "
                "exits is unavoidable.",
        answer=f"Out of U1 along the pad axis, 45 degrees right as soon as the leg clears the neighbours' corners (leg start y "
               f"{y_leg:.3f}), vertical into U2 ({_r(_len(pts))} mm). No via. Violations: only the unavoidable exit ones, "
               "wire against the two neighbours at each end, 0.025 mm deep, along the pad length.",
        why="Leaving a pad of a 0.4 mm row the wire is inside the neighbours' clearance for as long as it runs beside them; the "
            "shortest violation leaves along the axis and turns the moment the leg clears the neighbour's corner by the "
            "clearance. The 45 degree leg is the shortest way to the other row; the last run enters U2 along its axis. The "
            "expected violation count is what KiCad reports for the reference (reference_drc).",
        size=(W, H), layers=2, nets=["N"], fps=[top, bot], connections=[("N", "U1.3", "U2.3")], cuts=[],
        keepouts=_closed_bcu(W, H), classes=[dict(DEFAULT_CLASS)], ref_tracks=[Track("N", "F.Cu", W_SIG, pts)],
        witness={"N": ("F.Cu", pts)}, extra={"fx": {"kills": ["h_exit_no_b"], "ref_len": _r(_len(pts))}})


# ------------------------------------------------------------------------------------------------ 6. edge_pads
def edge_pads() -> Pattern:
    W, H = 8.0, 12.0
    xe = 0.6
    j = Fp("J1", [Pad("1", "N", (xe, 2.0), (0.2, 0.5))], "terminal")
    k = Fp("J2", [Pad("1", "N", (xe, 9.0), (0.2, 0.5))], "terminal")
    xw = 0.5 + W_SIG / 2 + EPS
    pts = [(xw, 2.0), (xw, 9.0)]
    return Pattern(
        name="edge_pads", title="Two edge pads, the wire along the board edge", kind="solvable",
        gap_classes=["erbs report: board outline as the exit feature (h_edge_exit_whole)"],
        purpose="One wire between two pads 0.6 mm from the board's left edge (0.2 x 0.5 mm pads: the copper-to-edge clearance "
                "of 0.5 mm holds for the pads, not for a 0.25 mm wire centred on them, which needs 0.625 mm). The wire runs "
                "7 mm along the edge.",
        answer=f"A straight vertical wire at x = {xw:.3f} (0.027 mm right of the pad centres, inside both pads), "
               f"{_r(_len(pts))} mm. No via, DRC 0.",
        why="The straight line between the pad centres is 0.025 mm into the edge clearance for its whole length. Moving it "
            "0.027 mm into the board costs no length, the track ends stay inside the pads, and the clearance holds.",
        size=(W, H), layers=2, nets=["N"], fps=[j, k], connections=[("N", "J1.1", "J2.1")], cuts=[],
        keepouts=_closed_bcu(W, H), classes=[dict(DEFAULT_CLASS)], ref_tracks=[Track("N", "F.Cu", W_SIG, pts)],
        witness={"N": ("F.Cu", pts)}, extra={"fx": {"kills": ["h_edge_exit_whole"], "ref_len": _r(_len(pts))}})


PATTERNS = {f.__name__: f for f in (edge_tangent, collinear_row, diag_exit, via_fold, qfn_two_ends, edge_pads)}


def expected_doc(pat: Pattern) -> dict:
    return {"name": pat.name, "title": pat.title, "gap_classes": pat.gap_classes, "purpose": pat.purpose,
            "expected": pat.answer, "why": pat.why, "status": STATUS,
            "board": {"size_mm": list(pat.size), "copper_layers": pat.layers,
                      "keepouts": [{"layers": k.layers, "tracks": k.tracks, "vias": k.vias} for k in pat.keepouts]},
            "rules": {"clearance": CLEAR, "track_width": W_SIG, "board_min_track_width": pat.rules["min_track_width"],
                      "copper_edge_clearance": pat.rules["min_copper_edge_clearance"], "style": "octilinear (engine default)"},
            "rules_assumed": {"authority": "board", "margin": 0.0, "options": {"rules.fab_profile": "board"}},
            "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
            "reference": {"tracks": len(pat.ref_tracks), "vias": len(pat.ref_vias), "expected_drc": {"violations": 0, "unconnected": 0}},
            "problem_expected_drc": {"violations": 0, "unconnected": len(pat.connections)},
            "reference_tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": t.pts} for t in pat.ref_tracks],
            **pat.extra.get("fx", {})}


def generate(name: str, out: Path, *, drc: bool = False) -> dict:
    pat = PATTERNS[name]()
    bad = clearance_report(pat)
    if bad and not pat.kind.startswith("unsolvable"):
        raise SystemExit(f"{name}: reference breaks a clearance: {bad}")
    d = Path(out) / name
    paths = {"problem": write_board(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_board(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)}
    doc = expected_doc(pat)
    if bad:
        doc["expected_clearance_gaps"] = bad
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
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "fx-survivors" / "boards"
    for name in PATTERNS:
        print(name, *(str(p) for p in generate(name, out, drc=drc).values()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
