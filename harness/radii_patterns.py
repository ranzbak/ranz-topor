"""Test patterns for M3 (shopping list 6.2: radii, spokes, octilinear; Q1 attachment stacks, Q2 nested radii).

Each pattern is a small single-layer board with a known optimal answer, built the fit-pattern way
(`harness/fit_patterns.py`): the reference copper is laid out first at the exact radii and pitches the board's rules
ask for, and pads and nets are placed around it. The reference must pass KiCad DRC with 0 violations and 0
unconnected (`python -m harness.radii_patterns --drc`).

The patterns test the drawing, not the fit. Every pattern fits with room to spare except where it says so. What they
check is whether the drawing puts each wire where the plan's topology and radii put it, whatever order the wires are
drawn in.

1. `three_diagonals`: three parallel 45 degree wires from a 0.8 mm row into three neighbouring module pads at 0.7 mm.
   The two outer wires are PWR (0.40 mm), so by the route-order rule (thick before thin) the middle one is drawn last.
   The middle wire must keep its planned corridor (plan P6, wiccon 3.3v).
2. `nested_wraps`: three wires wrap the corner of a large foreign pad, nested. The inner-most is the thin one (drawn
   last). Its net sorts last, so wire-id order is the reverse of the inside-out order (Dayan 1997 PDF 125: attached
   nets kept inside-out). Expected: concentric octagonal wraps at the stack radii.
3. `distant_radius`: pad A, two wires w1 and w2 over it, and a third wire w3 that passes above two pads B and C. The
   zero-width band of w3 touches nothing. A's radius through w1 and w2 (1.625 mm) still sets w3's height, across the
   triangle edge B-C (Popov & Popov 2012 pp. 120-121, Figs. 5-6). w3 is the shortest wire, so it is drawn first.

Rules on every board (the fit-pattern rules): Default class 0.25 mm at 0.20 mm, PWR class 0.40 mm at 0.20 mm, board
minimum track width 0.25, copper to edge 0.50. B.Cu is closed by a rule area (no tracks, no vias): one layer.
Style: octilinear, the engine default. Coordinates are mm in the KiCad frame (y down).

    python -m harness.radii_patterns [OUT] [--drc]

writes `OUT/<name>/{problem,reference}/<name>.kicad_pcb`, `expected.json`, `<name>.png` and `OUT/README.md`. The
default OUT is `scratch/agent-wt/m3-runs/patterns`. Deterministic.
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from harness.fit_patterns import (CLEAR, DEFAULT_CLASS, RULES, W_SIG, Fp, Keepout, Pad, Pattern, Track, _strip,
                                  write_board)

SQ2 = math.sqrt(2.0)
T22 = math.tan(math.radians(22.5))
W_PWR = 0.40
# Every planned gap is the clearance plus EPS, so the reference passes KiCad DRC after rounding to nm.
EPS = 0.002
PWR_CLASS = {"name": "PWR", "clearance": CLEAR, "track_width": W_PWR, "via_diameter": 0.6, "via_drill": 0.3,
             "priority": 0}
CLASSES = [dict(DEFAULT_CLASS), PWR_CLASS]


def width_of(pat: Pattern, net: str) -> float:
    for np_ in pat.net_patterns:
        if np_["pattern"] == net:
            return next(c for c in pat.classes if c["name"] == np_["netclass"])["track_width"]
    return W_SIG


def _len(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def _oct_dist(a, b) -> float:
    dx, dy = abs(b[0] - a[0]), abs(b[1] - a[1])
    return max(dx, dy) + (SQ2 - 1.0) * min(dx, dy)


def _closed_bcu(W, H):
    return [Keepout(["B.Cu"], _strip(W, 0.0, H))]


def _r(x: float) -> float:
    return round(x, 4)


# ----------------------------------------------------------------------------------------------- 1. three diagonals
def three_diagonals() -> Pattern:
    """1. Three parallel 45 degree wires, the middle (thin) one drawn last, into neighbouring module pads."""
    W, H = 12.0, 14.0
    nets = ["P1", "S", "P2"]  # left outer, middle, right outer
    x0, h_bot, h_top = 3.0, 0.8, 0.7  # J1 x, J1 pitch, U1 pitch
    y_j, y_u, y_b = 12.0, 2.0, 10.5  # J1 row, U1 row, bottom bends (all at one height: h_bot = d sqrt 2)
    D = 5.0  # diagonal run of the left wire
    xt = x0 + D  # U1.3, the left wire's target
    j1 = Fp("J1", [Pad(str(i + 1), n, (x0 + i * h_bot, y_j), (0.55, 0.55)) for i, n in enumerate(nets)], "terminal")
    u1 = Fp("U1", [Pad(str(i + 1), (nets[i - 2] if 2 <= i <= 4 else None), (xt + (i - 2) * h_top, y_u), (0.45, 0.9))
                   for i in range(7)], "terminal")
    tracks = []
    for i, n in enumerate(nets):
        xs = x0 + i * h_bot
        xe = xt + i * h_top
        t = xe - xs  # diagonal run of wire i
        pts = [(xs, y_j), (xs, y_b), (xe, y_b - t), (xe, y_u)]
        tracks.append(Track(n, "F.Cu", W_PWR if n.startswith("P") else W_SIG, pts))
    d_perp = h_bot / SQ2
    need_ps = W_PWR / 2 + CLEAR + W_SIG / 2
    lengths = {t.net: _len(t.pts) for t in tracks}
    oct_d = {t.net: _oct_dist(t.pts[0], t.pts[-1]) for t in tracks}
    expected = {
        "nets": {t.net: {"from": f"J1.{i + 1}", "to": f"U1.{i + 3}", "width": t.width, "layer": "F.Cu",
                         "length": _r(lengths[t.net]), "octilinear_distance": _r(oct_d[t.net]),
                         "reference": [[_r(x), _r(y)] for x, y in t.pts]} for i, t in enumerate(tracks)},
        "draw_order_today": ["P1 / P2 (0.40 mm, widest first)", "S (0.25 mm) last"],
        "order_on_every_cut": ["P1", "S", "P2"],
        "numbers": {"diagonal_pitch_perpendicular": _r(d_perp), "needed_P_to_S": _r(need_ps),
                    "slack_per_gap": _r(d_perp - need_ps), "vertical_pitch_J1": h_bot, "vertical_pitch_U1": h_top,
                    "top_bend_stagger": _r(h_bot - h_top)},
        "judge": ["KiCad DRC 0 violations, 0 unconnected", "no via", "every wire's length = its octilinear distance "
                  "(+0.5 % for the pad entry)", "S lies between P1 and P2 on every horizontal line"],
    }
    return Pattern(
        name="three_diagonals", title="Three diagonals, the middle one drawn last", kind="solvable",
        gap_classes=["drawing: planned corridor of a later wire (P6)"],
        purpose="Three wires from J1 (0.8 mm pitch) up and to the right into three neighbouring pads of U1 (0.7 mm "
                "pitch; the other U1 pads have no net). P1 and P2 are 0.40 mm (PWR), S is 0.25 mm. Today's draw "
                "order is widest first, so S is drawn last, between two wires that are already copper.",
        answer="Three nested octilinear wires: vertical from J1, 45 degrees, vertical into U1, S between P1 and P2 "
               f"everywhere. Each at its octilinear distance (P1 {oct_d['P1']:.3f}, S {oct_d['S']:.3f}, P2 "
               f"{oct_d['P2']:.3f} mm). No via, DRC 0. The diagonals are {d_perp:.4f} mm apart (need "
               f"{need_ps:.3f}); the bends into U1 are staggered by {h_bot - h_top:.2f} mm per wire.",
        why="Solvable by construction: the centre lines were laid first (bottom bends on one line, so the 0.8 mm "
            "vertical pitch becomes a 0.566 mm diagonal pitch), the pads placed on their ends. Any monotone "
            "octilinear shape has the same length, so the answer's shape is one of many; nesting, length and DRC are "
            "what is judged.",
        size=(W, H), layers=2, nets=nets, fps=[j1, u1],
        connections=[(n, f"J1.{i + 1}", f"U1.{i + 3}") for i, n in enumerate(nets)], cuts=[],
        keepouts=_closed_bcu(W, H), classes=[dict(c) for c in CLASSES],
        net_patterns=[{"netclass": "PWR", "pattern": "P1"}, {"netclass": "PWR", "pattern": "P2"}],
        ref_tracks=tracks, witness={t.net: (t.layer, t.pts) for t in tracks}, extra={"m3": expected})


# ----------------------------------------------------------------------------------------------- 2. nested wraps
def nested_wraps() -> Pattern:
    """2. Three wires wrap one pad corner, nested; the inner-most (thin) is drawn last and has the highest net id."""
    W, H = 11.5, 14.0
    corner = (7.0, 5.0)  # M1's top-right corner
    m1 = Fp("M1", [Pad("1", None, (5.0, 9.24), (4.0, 8.48))], "obstacle")  # x 3.0..7.0, y 5.0..13.48 (0.52 to edge)
    nets = ["A", "B", "C"]  # outer, middle, inner (net ids ascending outward to inward)
    widths = {"A": W_PWR, "B": W_PWR, "C": W_SIG}
    rho = {}
    rho["C"] = CLEAR + EPS + W_SIG / 2
    rho["B"] = rho["C"] + W_SIG / 2 + CLEAR + EPS + W_PWR / 2
    rho["A"] = rho["B"] + W_PWR / 2 + CLEAR + EPS + W_PWR / 2
    j1y = {"A": 2.4, "B": 3.4, "C": 4.4}
    j2x = {"A": 9.6, "B": 8.6, "C": 7.6}
    y_j2 = 12.5
    j1 = Fp("J1", [Pad(str(i + 1), n, (1.0, j1y[n]), (0.6, 0.6)) for i, n in enumerate(nets)], "terminal")
    j2 = Fp("J2", [Pad(str(i + 1), n, (j2x[n], y_j2), (0.6, 0.6)) for i, n in enumerate(["C", "B", "A"])],
            "terminal")
    tracks = []
    chamfer_c = {}
    for n in nets:
        # The wire's 45 degree leg lies on its octagon's chamfer line x - y = (cx - cy) + rho sqrt 2 (the side of the
        # octagon of in-radius rho about the corner that faces up-right): horizontal from J1, down the chamfer line,
        # vertical into J2. The longest diagonal the corner allows, so the shortest octilinear wire.
        c = (corner[0] - corner[1]) + rho[n] * SQ2
        chamfer_c[n] = c
        y0, x1 = j1y[n], j2x[n]
        pts = [(1.0, y0), (c + y0, y0), (x1, x1 - c), (x1, y_j2)]
        tracks.append(Track(n, "F.Cu", widths[n], pts))
    lengths = {t.net: _len(t.pts) for t in tracks}
    expected = {
        "stack_at_M1_corner": {"vertex": list(corner), "inside_out": ["C", "B", "A"],
                               "radius": {n: _r(rho[n]) for n in ("C", "B", "A")},
                               "formula": "C: s + w_C/2; B: rho_C + w_C/2 + s + w_B/2; A: rho_B + w_B/2 + s + w_A/2 (each s + 0.002 DRC margin)"},
        "net_id_order": nets, "draw_order_today": ["B (0.40, shorter)", "A (0.40)", "C (0.25) last"],
        "nets": {t.net: {"width": t.width, "length": _r(lengths[t.net]), "chamfer_line": f"x - y = {chamfer_c[t.net]:.4f}",
                         "reference": [[_r(x), _r(y)] for x, y in t.pts]} for t in tracks},
        "judge": ["KiCad DRC 0 violations, 0 unconnected", "no via", "length of each wire = reference (+0.5 %)",
                  "inside-out order at the corner C, B, A (C nearest M1)"],
    }
    return Pattern(
        name="nested_wraps", title="Nested wraps around one pad corner (attachment stack)", kind="solvable",
        gap_classes=["drawing: wrap order at a vertex (Q1)"],
        purpose="Three wires from J1 (left) to J2 (bottom) must pass the top-right corner of M1, a 4 x 8.48 mm pad "
                "without a net that reaches to 0.52 mm of the bottom edge (a wire needs 0.95 there). A and B are 0.40 mm (PWR), C is 0.25 mm. "
                "C is the inner-most wrap and is drawn last; its net id is the highest.",
        answer=f"Concentric wraps of M1's corner, inside-out C, B, A at radii {rho['C']:.3f} / {rho['B']:.3f} / "
               f"{rho['A']:.3f} mm. Each wire: horizontal from J1, one 45 degree leg along its octagon's side, vertical "
               f"into J2 (lengths A {lengths['A']:.3f}, B {lengths['B']:.3f}, C {lengths['C']:.3f} mm). No via, DRC 0.",
        why="Solvable by construction: each 45 degree leg lies on the side of the octagon of its stack radius about "
            "the corner; the radii differ by exactly the pitch of neighbouring wires, so the legs are parallel at "
            "the clearance. No shorter octilinear wire exists in this topology (its diagonal is the longest the "
            "corner allows); the only other way (below M1) is closed.",
        size=(W, H), layers=2, nets=nets, fps=[m1, j1, j2],
        connections=[("A", "J1.1", "J2.3"), ("B", "J1.2", "J2.2"), ("C", "J1.3", "J2.1")], cuts=[],
        keepouts=_closed_bcu(W, H), classes=[dict(c) for c in CLASSES],
        net_patterns=[{"netclass": "PWR", "pattern": "A"}, {"netclass": "PWR", "pattern": "B"}],
        ref_tracks=tracks, witness={t.net: (t.layer, t.pts) for t in tracks},
        extra={"m3": expected, "corner": corner, "rho": rho})


# ----------------------------------------------------------------------------------------------- 3. distant radius
def distant_radius() -> Pattern:
    """3. A's radius through W1 and W2 sets the height of W3, whose own band touches no feature (Popov Fig. 6)."""
    W, H = 16.0, 12.0
    A = (8.0, 8.0)
    r = 0.4
    obst = Fp("TP", [Pad("1", None, A, (2 * r, 2 * r), "circle")], "obstacle")
    nets = ["W1", "W2", "W3"]
    rho = {"W1": r + CLEAR + EPS + W_SIG / 2}
    rho["W2"] = rho["W1"] + W_SIG + CLEAR + EPS
    rho["W3"] = rho["W2"] + W_SIG + CLEAR + EPS
    # Terminal rows 0.5 apart. Only W1's straight line runs into A (near its top); W2's and W3's clear A alone (a
    # wire alone needs y <= A - rho W1 = 7.273), so they bow only for the wires inside them: the wave. Over A is
    # the optimum: a level-45-level detour of height h adds 2 h (sqrt 2 - 1); all over (heights 0.277, 0.229,
    # 0.181) adds 0.569; W1 under instead (1.177 down, W2 and W3 then straight) adds 0.975.
    ys = {"W1": 7.55, "W2": 7.05, "W3": 6.55}
    j1 = Fp("J1", [Pad(str(i + 1), n, (1.0, ys[n]), (0.3, 0.3)) for i, n in enumerate(nets)], "terminal")
    j2 = Fp("J2", [Pad(str(i + 1), n, (15.0, ys[n]), (0.3, 0.3)) for i, n in enumerate(nets)], "terminal")
    # Every wire: level from J1, a 45 degree leg on the side of A's octagon of its stack radius (x + y = Ax + Ay -
    # rho sqrt 2, KiCad frame y down), level over A at Ay - rho, mirrored down to J2. W3's octagon is the one A's
    # radius reaches through W1 and W2.
    tops = {n: A[1] - rho[n] for n in nets}
    tracks = []
    for n in nets:
        y0, top = ys[n], tops[n]
        c = A[0] + A[1] - rho[n] * math.sqrt(2.0)
        left = [(1.0, y0), (c - y0, y0), (c - top, top)]
        right = [(2 * A[0] - x, y) for x, y in reversed(left)]
        tracks.append(Track(n, "F.Cu", W_SIG, left + right))
    lengths = {t.net: _len(t.pts) for t in tracks}
    k = 2.0 * (math.sqrt(2.0) - 1.0)
    over_all = sum(k * (ys[n] - tops[n]) for n in nets)
    w1_under = k * (A[1] + rho["W1"] - ys["W1"])
    expected = {
        "radius_from_A": {n: _r(rho[n]) for n in nets},
        "w3": {"zero_width_band": f"straight at y = {ys['W3']} (touches no feature)",
               "height_over_A_needed": _r(tops["W3"]),
               "distant": "W3's band does not wrap A; A's cut up to the board edge crosses W1, W2, W3 (radius 0.4 + "
                          "3 x 0.2 + 2 x 0.25 + 0.125 = 1.625, + 3 x 0.002 margin)"},
        "over_not_under": {"all_over_excess": _r(over_all), "w1_under_excess": _r(w1_under)},
        "draw_order_today": ["W3 (shortest) first", "W2", "W1"],
        "nets": {t.net: {"width": t.width, "length": _r(lengths[t.net]),
                         "reference": [[_r(x), _r(y)] for x, y in t.pts]} for t in tracks},
        "judge": ["KiCad DRC 0 violations, 0 unconnected", "no via", f"W3 at y <= {_r(tops['W3']) + 0.01} over A (x = 8)",
                  "lengths = reference (+0.5 %)", "order on A's vertical cut: W1, W2, W3"],
    }
    return Pattern(
        name="distant_radius", title="Distant pad radius (Popov wave)", kind="solvable",
        gap_classes=["drawing: radius of a distant pad (Q2)"],
        purpose="Three 0.25 mm wires from J1 (left) to J2 (right) past the round pad A (no net). The straight lines of "
                "Only W1's straight line runs into A (near its top), so W1 goes over A. W2's and W3's straight lines (their "
                "zero-width bands) clear A and touch no feature: only A's radius through the wires inside them makes "
                "them bow, W2 through W1, W3 through W1 and W2.",
        answer=f"W1 wraps A at {rho['W1']:.3f} mm; W2 bows up to y = {tops['W2']:.3f} (radius {rho['W2']:.3f}, "
               f"through W1) and W3 up to y = {tops['W3']:.3f} (A's radius for it, {rho['W3']:.3f} mm, through W1 and "
               f"W2), although their own bands are straight at y = {ys['W2']} / {ys['W3']}. Each wire: level, one 45 degree leg on A's octagon, level over A, mirrored. Lengths "
               f"W1 {lengths['W1']:.3f}, W2 {lengths['W2']:.3f}, W3 {lengths['W3']:.3f} mm. No via, DRC 0.",
        why="Optimal by construction: every wire hugs the octagon of its stack radius about A (the shortest "
            "octilinear way round), the 45 degree legs 0.452 mm apart (pitch 0.45), the level legs 0.5 mm apart at "
            f"the terminals. All three over A add {over_all:.3f} mm; sending W1 under A instead (W2, W3 then "
            f"straight) adds {w1_under:.3f} mm.",
        size=(W, H), layers=2, nets=nets, fps=[obst, j1, j2],
        connections=[(n, f"J1.{i + 1}", f"J2.{i + 1}") for i, n in enumerate(nets)], cuts=[],
        keepouts=_closed_bcu(W, H), classes=[dict(c) for c in CLASSES], ref_tracks=tracks,
        witness={t.net: (t.layer, t.pts) for t in tracks}, extra={"m3": expected, "A": A, "rho": rho, "ys": ys})


PATTERNS = {f.__name__: f for f in (three_diagonals, nested_wraps, distant_radius)}
# User 2026-11-13: three_diagonals and nested_wraps approved; distant_radius reworked (W1 / W2 had no reason to bend
# over A), awaiting confirmation.
STATUS = {"three_diagonals": "approved 2026-11-13", "nested_wraps": "approved 2026-11-13",
          "distant_radius": "PROPOSED (reworked 2026-11-13), needs approval"}


# ----------------------------------------------------------------------------------------------- checks
def clearance_report(pat: Pattern) -> list[dict]:
    """Shapely check of the reference: every pair of different-net copper (tracks, pads) at least the clearance apart,
    tracks at least the edge clearance from the outline. The KiCad DRC is the authority; this catches a wrong number
    before the board is written."""
    from shapely.geometry import LineString, Point, box

    items = []
    for t in pat.ref_tracks:
        items.append((t.net, f"track {t.net}", LineString(t.pts).buffer(t.width / 2, quad_segs=16)))
    for f in pat.fps:
        for p in f.pads:
            g = (Point(p.pos).buffer(p.size[0] / 2, quad_segs=32) if p.shape == "circle" else
                 box(p.pos[0] - p.size[0] / 2, p.pos[1] - p.size[1] / 2, p.pos[0] + p.size[0] / 2,
                     p.pos[1] + p.size[1] / 2))
            items.append((p.net, f"pad {f.ref}.{p.num}", g))
    bad = []
    for i in range(len(items)):
        for j in range(i + 1, len(items)):
            (na, a, ga), (nb, b, gb) = items[i], items[j]
            if na is not None and na == nb:
                continue
            if a.startswith("pad") and b.startswith("pad"):
                continue
            d = ga.distance(gb)
            if d < CLEAR - 1e-6:
                bad.append({"a": a, "b": b, "gap": round(d, 4)})
    W, H = pat.size
    inner = box(RULES["min_copper_edge_clearance"], RULES["min_copper_edge_clearance"],
                W - RULES["min_copper_edge_clearance"], H - RULES["min_copper_edge_clearance"])
    for n, a, g in items:
        if a.startswith("track") and not inner.contains(g):
            bad.append({"a": a, "b": "edge", "gap": None})
    return bad


def expected_doc(pat: Pattern) -> dict:
    return {
        "name": pat.name, "title": pat.title, "kind": pat.kind, "gap_classes": pat.gap_classes,
        "purpose": pat.purpose, "expected": pat.answer, "why": pat.why, "status": STATUS[pat.name],
        "board": {"size_mm": list(pat.size), "copper_layers": pat.layers,
                  "keepouts": [{"layers": k.layers, "tracks": k.tracks, "vias": k.vias} for k in pat.keepouts]},
        "rules": {"clearance": CLEAR, "track_width": {c["name"]: c["track_width"] for c in pat.classes},
                  "board_min_track_width": pat.rules["min_track_width"],
                  "copper_edge_clearance": pat.rules["min_copper_edge_clearance"],
                  "netclass_patterns": pat.net_patterns, "style": "octilinear (engine default)"},
        "rules_assumed": {"authority": "board", "margin": 0.0, "options": {"rules.fab_profile": "board"}},
        "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
        "reference": {"tracks": len(pat.ref_tracks), "vias": 0, "expected_drc": {"violations": 0, "unconnected": 0}},
        "problem_expected_drc": {"violations": 0, "unconnected": len(pat.connections)},
        **pat.extra.get("m3", {}),
    }


def generate(name: str, out: Path, *, png: bool = True, drc: bool = False) -> dict:
    pat = PATTERNS[name]()
    bad = clearance_report(pat)
    if bad:
        raise SystemExit(f"{name}: reference breaks a clearance: {bad}")
    d = Path(out) / name
    d.mkdir(parents=True, exist_ok=True)
    paths = {"problem": write_board(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_board(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)}
    doc = expected_doc(pat)
    if drc:
        from harness.drc import run_drc

        rep = run_drc(paths["reference"], d / "reference" / "drc.json")
        doc["reference_drc"] = {"violations": len(rep.violations), "unconnected": len(rep.unconnected),
                                "detail": [str(v) for v in rep.violations][:10]}
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    paths["expected"] = d / "expected.json"
    if png:
        from harness.radii_patterns_png import render

        paths["png"] = render(pat, doc, d / f"{name}.png")
    return paths


def readme(out: Path) -> Path:
    lines = ["# M3 radii patterns (three_diagonals, nested_wraps approved 2026-11-13; distant_radius proposed)", "",
             "Generated by `harness/radii_patterns.py` (branch `radii-m3`). Each directory: `problem/` (pads, rule "
             "areas, nets), `reference/` (the expected copper, KiCad DRC 0 / 0), `expected.json`, the PNG (problem "
             "left, reference right, radii and order marked).", ""]
    for i, (name, f) in enumerate(PATTERNS.items(), 1):
        pat = f()
        lines += [f"## {i}. `{name}`: {pat.title}", "", f"{pat.purpose}", "", f"**Expected:** {pat.answer}", "",
                  f"**Why solvable:** {pat.why}", "", f"![{name}]({name}/{name}.png)", ""]
    p = Path(out) / "README.md"
    p.write_text("\n".join(lines))
    return p


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    drc = "--drc" in argv
    argv = [a for a in argv if a != "--drc"]
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "agent-wt" / "m3-runs" / "patterns"
    for name in PATTERNS:
        paths = generate(name, out, drc=drc)
        print(name, *(str(p) for p in paths.values()))
    print(readme(out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
