"""Test patterns for "fit in the abstract" (shopping list 2.1): small boards with a known answer.

Each pattern is one behaviour of the cut test (P1a `CutModel::flow`, P1b inner-layer room, neck
floors). It is either

- *solvable by construction*: the reference copper is laid out first (track centre lines at the
  wanted pitch), and the walls, terminal pads and nets are derived from it, so the reference passes
  KiCad DRC with 0 violations and 0 unconnected; or
- *unsolvable by design*: exactly one cut whose demand exceeds its capacity by a stated amount, and
  every other way across the barrier costs more. The expected result is that violation, predicted
  and reported, with no wire dropped.

Expected values are computed from the geometry and the board's rules with the formulas written into
`expected.json`, and checked against the numbers worked out by hand that each pattern states
(`hand=`); nothing comes from the engine.

Capacity and demand of a cut `pq` on one layer (the same rule as `docs/dev/inflated-sketch.md` §1.2
and engine `topo/demand.rs::need` / `topo/cutflow.rs`, single clearance class):

- capacity = length of `pq` in the planning norm: Euclidean for `style = "smooth"`, octagonal
  `max(|dx|, |dy|, (|dx| + |dy|) / sqrt 2)` for `"octilinear"` (Staepelaere 1992 eq. 1.2, PDF p. 12).
- demand = sum of the widths of the wires crossing `pq` + between neighbours of different nets the
  larger of their clearances + at each end the end's clearance (a foreign pad: max(pad clearance,
  wire clearance); the board edge: the copper-to-edge clearance).
- Maley 1987 p. 32 writes the same test with the spacing folded into the widths: capacity =
  length − s/2 − s/2, congestion = Σ (w + s); with one clearance s both give the same slack.

Coordinates in mm, KiCad frame (y down). `python -m harness.fit_patterns [OUT]` writes every pattern
(problem/ and reference/ boards, expected.json, the PNG) and the README under OUT (default
`scratch/fit-patterns`). Deterministic: fixed KIID seed, no random numbers.
"""
from __future__ import annotations

import json
import math
import sys
from dataclasses import dataclass, field
from pathlib import Path

import pcbnew

from harness.fixtures import write_project_file

MM = 1_000_000
SQ2 = math.sqrt(2.0)

# Board rules shared by every pattern (Default class 0.25 mm at 0.2 mm, board minimum = the class width so the
# plan cannot neck a signal wire; the neck patterns add a POWER class).
W_SIG = 0.25
CLEAR = 0.2
EDGE = 0.5
HOLE_CLEAR = 0.25
RULES = {"min_clearance": CLEAR, "min_track_width": W_SIG, "min_copper_edge_clearance": EDGE,
         "min_hole_clearance": HOLE_CLEAR, "min_hole_to_hole": 0.25, "min_via_diameter": 0.4,
         "min_through_hole_diameter": 0.3, "min_via_annular_width": 0.05, "max_error": 0.005}
DEFAULT_CLASS = {"name": "Default", "clearance": CLEAR, "track_width": W_SIG, "via_diameter": 0.6,
                 "via_drill": 0.3, "priority": 2147483647}
# QFN-like pad row: 0.30 x 0.80 mm pads at 0.5 mm pitch (0.20 mm between pads: no wire fits, one needs 0.65).
QFN_W, QFN_L, QFN_P = 0.30, 0.80, 0.50

LAYER_IDS = {"F.Cu": pcbnew.F_Cu, "In1.Cu": pcbnew.In1_Cu, "In2.Cu": pcbnew.In2_Cu, "B.Cu": pcbnew.B_Cu}


def _v(x, y):
    return pcbnew.VECTOR2I(int(round(x * MM)), int(round(y * MM)))


def oct_len(dx: float, dy: float) -> float:
    """Octagonal norm (Staepelaere 1992 eq. 1.2, PDF p. 12; engine `topo/sheet.rs::Norm::len`)."""
    x, y = abs(dx), abs(dy)
    return max(x, y, (x + y) / SQ2)


def demand(items: list[tuple[str, float, float]], end_p: tuple, end_q: tuple) -> float:
    """Demand of the wires `items` = [(net, width, clearance)] ordered from p to q across a cut whose ends are
    `("pad", net, clearance)` or `("edge", clearance)`. The rule of `docs/dev/inflated-sketch.md` §1.2 /
    engine `topo/demand.rs::need` (foreign-pad ends only; the own-net half-width rule is not used here)."""
    if not items:
        return 0.0
    n = sum(w for _, w, _ in items)
    for a, b in zip(items, items[1:]):
        if a[0] != b[0]:
            n += max(a[2], b[2])

    def end(e, it):
        if e[0] == "edge":
            return e[1]
        assert e[1] != it[0], "own-net ends are not used in these patterns"
        return max(e[2], it[2])

    return n + end(end_p, items[0]) + end(end_q, items[-1])


# ----------------------------------------------------------------------------------------------- pattern model
@dataclass
class Pad:
    num: str
    net: str | None
    pos: tuple[float, float]
    size: tuple[float, float]
    shape: str = "rect"  # rect | circle
    drill: float | None = None  # PTH (all copper layers) when set
    layer: str = "F.Cu"


@dataclass
class Fp:
    ref: str
    pads: list[Pad]
    role: str  # wall | terminal | obstacle


@dataclass
class Track:
    net: str
    layer: str
    width: float
    pts: list[tuple[float, float]]


@dataclass
class Via:
    net: str
    pos: tuple[float, float]
    dia: float = 0.6
    drill: float = 0.3


@dataclass
class Keepout:
    layers: list[str]
    poly: list[tuple[float, float]]
    tracks: bool = True
    vias: bool = True


@dataclass
class Cut:
    id: str
    layer: str
    p: tuple[float, float]
    q: tuple[float, float]
    fp: tuple[str, str]  # (ref, pad number) of the feature at p
    fq: tuple[str, str]
    at_p: str  # which point of the feature, in words
    at_q: str
    crossing: list[str]  # nets crossing the cut, in order from p to q (the planned topology)
    hand: dict  # numbers worked out by hand: len_e, len_o, need, (slack)
    role: str = "binding"  # binding | context | empty
    note: str = ""


@dataclass
class Pattern:
    name: str
    title: str
    kind: str  # solvable | unsolvable | solvable (smooth) / unsolvable (octilinear)
    gap_classes: list[str]
    purpose: str
    answer: str
    why: str
    size: tuple[float, float]
    layers: int
    nets: list[str]
    fps: list[Fp]
    connections: list[tuple[str, str, str]]  # (net, "REF.pad", "REF.pad")
    cuts: list[Cut]
    keepouts: list[Keepout] = field(default_factory=list)
    classes: list[dict] = field(default_factory=lambda: [dict(DEFAULT_CLASS)])
    net_patterns: list[dict] = field(default_factory=list)
    rules: dict = field(default_factory=lambda: dict(RULES))
    dru: str = ""
    ref_tracks: list[Track] = field(default_factory=list)
    ref_vias: list[Via] = field(default_factory=list)
    witness: dict = field(default_factory=dict)  # net -> (layer, polyline): the planned topology, centre line
    closed: list[dict] = field(default_factory=list)  # other ways across the barrier and what they cost
    extra: dict = field(default_factory=dict)  # neck, wrong-model readings, norm notes
    views: list[list[str]] = field(default_factory=lambda: [["F.Cu", "B.Cu"]])

    @property
    def solvable(self) -> bool:
        return self.kind.startswith("solvable")


def _qfn_row(ref: str, y: float, left_edge: float, n: int, *, first: int = 1) -> Fp:
    """n QFN-like pads (no net) in a row along x, the first pad's left side at `left_edge`."""
    pads = [Pad(str(first + i), None, (left_edge + QFN_W / 2 + i * QFN_P, y), (QFN_W, QFN_L)) for i in range(n)]
    return Fp(ref, pads, "wall")


def _terminals(ref: str, nets: list[str], xs: list[float], y: float, size: float) -> Fp:
    return Fp(ref, [Pad(str(i + 1), n, (x, y), (size, size)) for i, (n, x) in enumerate(zip(nets, xs))], "terminal")


def _strip(w: float, y0: float, y1: float) -> list[tuple[float, float]]:
    return [(0.0, y0), (w, y0), (w, y1), (0.0, y1)]


def _jog(x_from: float, x_to: float, y_end: float, sign: int) -> list[tuple[float, float]]:
    """A 45 degree jog from x_from to x_to that ends at y_end (sign +1 going down, -1 going up)."""
    d = abs(x_to - x_from)
    return [(x_from, y_end - sign * d), (x_to, y_end)] if d > 0 else []


# ----------------------------------------------------------------------------------------------- the patterns
def channel_fit() -> Pattern:
    """1. Three wires through a pad channel with 0.05 mm slack (solvable)."""
    W, H, Yr = 10.2, 12.0, 6.0
    xc = W / 2
    nets = ["S1", "S2", "S3"]
    slack = 0.05
    d = slack / 4  # the slack spread over the 4 gaps of the channel
    pitch = W_SIG + CLEAR + d
    xs = [xc + i * pitch for i in (-1, 0, 1)]  # reference centre lines in the channel: drawn first
    xl = xs[0] - W_SIG / 2 - CLEAR - d  # channel walls derived from them
    xr = xs[-1] + W_SIG / 2 + CLEAR + d
    u1 = _qfn_row("U1", Yr, xl - 8 * QFN_P + (QFN_P - QFN_W), 8)
    u2 = _qfn_row("U2", Yr, xr, 8, first=9)
    tx = [xc - 1.27, xc, xc + 1.27]
    j1 = _terminals("J1", nets, tx, 2.0, 0.8)
    j2 = _terminals("J2", nets, tx, 10.0, 0.8)
    tracks = []
    for n, t, x in zip(nets, tx, xs):
        top =[(t, 2.0)] + (_jog(t, x, Yr - 1.2, +1) or [(x, Yr - 1.2)])
        bot = ([(x, Yr + 1.2), (t, Yr + 1.2 + abs(x - t))] if abs(x - t) > 0 else [(x, Yr + 1.2)]) + [(t, 10.0)]
        pts = _dedup(top + bot)
        tracks.append(Track(n, "F.Cu", W_SIG, pts))
    a, b = (xl, Yr - QFN_L / 2), (xr, Yr - QFN_L / 2)
    cut = Cut("C1", "F.Cu", a, b, ("U1", "8"), ("U2", "9"), "top-right corner", "top-left corner", nets,
              hand={"len_e": 1.60, "len_o": 1.60, "need": 1.55, "slack": 0.05},
              note="Horizontal, so both norms give 1.60. The bottom corners give the same cut (same length).")
    closed = [
        {"way": "between two pads of one row (e.g. U1.7-U1.8)", "capacity": 0.20,
         "demand_one_wire": 0.65, "formula": "w + 2s = 0.25 + 0.40"},
        {"way": "past the end of a row (U1.1 to the left board edge, U2.16 to the right)", "capacity": 0.50,
         "demand_one_wire": 0.95, "formula": "w + s + edge = 0.25 + 0.20 + 0.50"},
        {"way": "on B.Cu", "capacity": 0.0, "formula": "rule area on B.Cu (tracks, vias) over the rows, edge to edge"},
    ]
    return Pattern(
        name="channel_fit", title="Channel, exact fit (3 wires, slack 0.05 mm)", kind="solvable",
        gap_classes=["fit / over-full cut seen by both models (the control: same channel, fits)",
                     "fit / borderline (slack < 0.1 mm)"],
        purpose="The basic cut: three 0.25 mm wires at 0.2 mm clearance through the 1.60 mm channel between two "
                "QFN-like pad rows (0.30 x 0.80 mm pads at 0.5 mm pitch).",
        answer="Cut C1 (U1.8 to U2.9, F.Cu): capacity 1.60, demand 3 x 0.25 + 4 x 0.20 = 1.55, slack +0.05 mm in "
               "both norms. No over-full cut.",
        why="Solvable by construction: the three centre lines were placed first at a pitch of 0.4625 mm (the slack "
            "spread over the four gaps), the walls 0.2125 mm from the outer tracks. The rows reach to 0.5 mm of the "
            "board edge and B.Cu is closed over the rows, so the channel is the only way across.",
        size=(W, H), layers=2, nets=nets, fps=[u1, u2, j1, j2],
        connections=[(n, f"J1.{i + 1}", f"J2.{i + 1}") for i, n in enumerate(nets)],
        cuts=[cut], keepouts=[Keepout(["B.Cu"], _strip(W, Yr - 1.5, Yr + 1.5))],
        ref_tracks=tracks, witness={t.net: (t.layer, t.pts) for t in tracks}, closed=closed)


def _dedup(pts):
    out = []
    for p in pts:
        if not out or abs(out[-1][0] - p[0]) > 1e-9 or abs(out[-1][1] - p[1]) > 1e-9:
            out.append(p)
    return out


def channel_overfull() -> Pattern:
    """2. The channel of pattern 1, four wires (unsolvable: over-full by one wire + clearance - slack)."""
    base = channel_fit()
    W, H = base.size
    Yr, xc = 6.0, W / 2
    nets = ["S1", "S2", "S3", "S4"]
    tx = [xc - 1.905, xc - 0.635, xc + 0.635, xc + 1.905]
    j1 = _terminals("J1", nets, tx, 2.0, 0.8)
    j2 = _terminals("J2", nets, tx, 10.0, 0.8)
    xl, xr = xc - 0.80, xc + 0.80
    g = (xr - xl - 4 * W_SIG) / 5  # the planned topology drawn with the deficit spread evenly (0.12 per gap)
    us = [xl + g + W_SIG / 2 + i * (W_SIG + g) for i in range(4)]
    witness = {n: ("F.Cu", [(t, 2.0), (t, 3.5), (u, Yr - 1.2), (u, Yr + 1.2), (t, 8.5), (t, 10.0)])
               for n, t, u in zip(nets, tx, us)}
    cut = Cut("C1", "F.Cu", (xl, Yr - QFN_L / 2), (xr, Yr - QFN_L / 2), ("U1", "8"), ("U2", "9"),
              "top-right corner", "top-left corner", nets,
              hand={"len_e": 1.60, "len_o": 1.60, "need": 2.00, "slack": -0.40},
              note="Overflow 0.40 = one more wire (w + s = 0.45) minus the 0.05 slack of pattern 1.")
    closed = [dict(c) for c in base.closed]
    closed[0].update({"overflow_one_wire": 0.45})
    closed[1].update({"overflow_one_wire": 0.45})
    return Pattern(
        name="channel_overfull", title="Channel over-full by one wire (4 wires)", kind="unsolvable",
        gap_classes=["fit / over-full cut seen by both models"],
        purpose="Pattern 1's channel with a fourth net that must cross the same pad rows. B.Cu is closed over the "
                "rows by a rule area (tracks and vias), so every wire crosses on F.Cu.",
        answer="Cut C1 (U1.8 to U2.9, F.Cu): demand 4 x 0.25 + 5 x 0.20 = 2.00, capacity 1.60, over-full by 0.40 mm "
               "= (w + s) - 0.05. Expected router result: all four wires routed, one violation at C1 predicted and "
               "reported (deficit 0.40 in total, e.g. 0.08 in each of the 5 gaps), nothing dropped.",
        why="Unsolvable by design: every way across the rows is closed except the channel; the cheapest alternative "
            "for the fourth wire (a 0.20 mm row gap, or 0.50 mm past a row end) is over-full by 0.45 > 0.40, so the "
            "least violation is the one at C1.",
        size=(W, H), layers=2, nets=nets, fps=[base.fps[0], base.fps[1], j1, j2],
        connections=[(n, f"J1.{i + 1}", f"J2.{i + 1}") for i, n in enumerate(nets)],
        cuts=[cut], keepouts=base.keepouts, witness=witness, closed=closed)


def _neck(name: str, floor: float) -> Pattern:
    """3a/3b. A 0.8 mm power wire between two THT header pins 0.84 mm apart; the neck floor (custom rule) decides."""
    W, H, Yr = 5.44, 9.6, 4.0
    xc = W / 2
    w_pow, w_neck = 0.8, 0.44
    r_pin = 0.85
    # Reference first (pattern 3a): the neck centred at xc, 0.44 wide; the pins derived from it.
    xp = w_neck / 2 + CLEAR + r_pin  # 1.27: a 2.54 mm header
    j1 = Fp("J1", [Pad("1", None, (xc - xp, Yr), (1.7, 1.7), "circle", drill=1.0),
                   Pad("2", None, (xc + xp, Yr), (1.7, 1.7), "circle", drill=1.0)], "wall")
    u2 = Fp("U2", [Pad("1", "VBUS", (xc, 1.5), (1.2, 1.2))], "terminal")
    j2 = Fp("J2", [Pad("1", "VBUS", (xc, 8.0), (2.0, 2.0))], "terminal")
    y_full = math.sqrt((r_pin + CLEAR + w_pow / 2) ** 2 - xp ** 2)  # 0.6997: full width clears both pins
    gap = 2 * (xp - r_pin)
    solvable = floor <= gap - 2 * CLEAR + 1e-12
    tracks = [Track("VBUS", "F.Cu", w_neck, [(xc, 1.5), (xc, Yr + 0.70)]),
              Track("VBUS", "F.Cu", w_pow, [(xc, Yr + 0.70), (xc, 8.0)])] if solvable else []
    hand = ({"len_e": 0.84, "len_o": 0.84, "need": 1.20, "slack": -0.36,
             "need_floor": 0.80, "slack_floor": 0.04} if solvable else
            {"len_e": 0.84, "len_o": 0.84, "need": 1.20, "slack": -0.36, "need_floor": 0.90, "slack_floor": -0.06})
    cut = Cut("N1", "F.Cu", (xc - gap / 2, Yr), (xc + gap / 2, Yr), ("J1", "1"), ("J1", "2"),
              "rightmost point of the round pad", "leftmost point of the round pad", ["VBUS"], hand=hand,
              note="Pad to pad (the rings of a 2.54 mm header, 1.7 mm pads): 2.54 - 1.70 = 0.84, horizontal.")
    dru = (f'(version 1)\n(rule "POWER neck floor"\n  (condition "A.NetClass == \'POWER\'")\n'
           f'  (constraint track_width (min {floor:g}mm)))\n')
    classes = [dict(DEFAULT_CLASS), {"name": "POWER", "clearance": CLEAR, "track_width": w_pow, "via_diameter": 0.8,
                                     "via_drill": 0.4, "priority": 0}]
    neck = {
        "nominal_width": w_pow, "floor": floor, "floor_source": ".kicad_dru rule 'POWER neck floor' (track_width min)",
        "max_neck_width": round(gap - 2 * CLEAR, 6), "max_neck_width_formula": "gap - 2 s = 0.84 - 0.40",
        "fits_at_floor": solvable,
        "full_width_clear_from_pin_line": round(y_full, 6),
        "full_width_formula": "sqrt((r_pin + s + w/2)^2 - 1.27^2) = sqrt(1.45^2 - 1.27^2): where the 0.8 mm wire's "
                              "round end clears both pins by 0.2",
        "neck_from": "U2.1 (the nearer pad, 2.5 mm above the pin line; user rule: a neck runs from the pad)",
        "neck_length_from_pad_centre": round(2.5 + y_full, 6),
        "neck_length_from_pad_edge": round(2.5 - 0.6 + y_full, 6),
        "neck_length_from_J2_instead": round(4.0 + y_full, 6),
        "width_ladder_note": "memory route-order-and-neck-rule step 1 (pad width) does not apply (U2.1 is 1.2 mm, wider "
                             "than the wire); step 2 'default width' would give 0.25 (Default class), below this "
                             "pattern's floor 0.40 (custom rule); step 3 'widest that fits' gives 0.44. The expected "
                             "neck width here is the widest that fits; user to confirm the ladder.",
    }
    if solvable:
        neck["reference"] = {"width": w_neck, "from": [xc, 1.5], "to": [xc, Yr + 0.70], "length": 3.20}
    title = ("Wide wire through a pin gap: neck fits at the floor" if solvable
             else "Wide wire through a pin gap: too narrow even at the floor")
    if solvable:
        answer = ("Cut N1 (J1.1 to J1.2): 0.84 mm. At the nominal 0.8 mm the wire alone needs 0.8 + 2 x 0.2 = 1.20 "
                  "(over-full by 0.36); at the 0.40 floor it needs 0.80 (slack +0.04). Expected neck: width 0.44 "
                  "(the widest that fits: gap - 2 s), from U2.1 to 0.6997 mm past the pin line (length 3.1997 from the "
                  "pad centre, 2.5997 from its edge), full width 0.8 from there to J2.1.")
        why = ("Solvable by construction: the reference neck (0.44 mm, centred) was placed first and the header pins "
               "derived from it (centres at 0.22 + 0.20 + 0.85 = 1.27 mm from the wire). The pins reach to 0.6 mm of "
               "the side edges (a wire at the floor needs 0.5 + 0.4 + 0.2 = 1.1) and B.Cu is closed, so the pin gap is "
               "the only way.")
    else:
        answer = ("Cut N1 (J1.1 to J1.2): 0.84 mm; at the 0.50 floor the wire alone needs 0.50 + 2 x 0.20 = 0.90: "
                  "over-full by 0.06 mm even when necked to the floor (0.36 at the nominal width). Expected: the wire "
                  "routed, the violation at N1 predicted and reported (0.06 mm clearance deficit, or a width below the "
                  "0.50 rule: either is the one known impossibility), not dropped.")
        why = ("Unsolvable by design: the same board as neck_fit with the custom neck floor raised from 0.40 to 0.50 "
               "mm; the pin gap stays the only way.")
    return Pattern(
        name=name, title=title, kind="solvable" if solvable else "unsolvable",
        gap_classes=(["fit / over-full cut, static (the wire alone overflows the cut)",
                      "fit / neck not planned (fits at neck floors)"] if solvable else
                     ["fit / over-full cut, static (the wire alone overflows the cut), no neck helps",
                      "fit / no static room"]),
        purpose="A 0.8 mm VBUS wire (POWER class) from J2.1 to U2.1 must pass between the two pins of a 2.54 mm "
                f"THT header (1.7 mm pads, 0.84 mm apart). The POWER neck floor is {floor:.2f} mm (.kicad_dru).",
        answer=answer, why=why, size=(W, H), layers=2, nets=["VBUS"], fps=[j1, u2, j2],
        connections=[("VBUS", "J2.1", "U2.1")], cuts=[cut],
        keepouts=[Keepout(["B.Cu"], _strip(W, 0.0, H))], classes=classes,
        net_patterns=[{"netclass": "POWER", "pattern": "VBUS"}], dru=dru, ref_tracks=tracks,
        witness={"VBUS": ("F.Cu", [(xc, 1.5), (xc, 8.0)])},
        closed=[{"way": "past a header pin to the side edge", "capacity": 0.6,
                 "demand_one_wire_at_floor": round(EDGE + floor + CLEAR, 3), "formula": "edge + floor + s"},
                {"way": "on B.Cu", "capacity": 0.0, "formula": "rule area on B.Cu (tracks, vias) over the board"}],
        extra={"neck": neck})


def neck_fit() -> Pattern:
    return _neck("neck_fit", 0.40)


def neck_too_narrow() -> Pattern:
    return _neck("neck_too_narrow", 0.50)


def model_gap_cut() -> Pattern:
    """4. Maley's non-adjacent cut: every edge of the triangulation has room, the cut across the opening does not."""
    W, H, Yr = 14.3, 16.0, 8.0
    xc = W / 2
    half_open = 1.85  # opening 3.70 between the rows
    b = 0.25  # TP1 / TP2: 0.5 x 0.5 test pads
    dy = 1.40  # TP1's bottom 1.40 above the rows' top
    u1 = _qfn_row("U1", Yr, xc - half_open - 10 * QFN_P + (QFN_P - QFN_W), 10)
    u2 = _qfn_row("U2", Yr, xc + half_open, 10, first=11)
    yb = Yr - QFN_L / 2 - dy - b
    tp1 = Fp("TP1", [Pad("1", None, (xc, yb), (2 * b, 2 * b))], "obstacle")
    tp2 = Fp("TP2", [Pad("1", None, (xc, 2 * Yr - yb), (2 * b, 2 * b))], "obstacle")
    lnets, rnets = ["L1", "L2", "L3", "L4"], ["R1", "R2", "R3", "R4"]
    nets = lnets + rnets
    tx = [xc - 4.5 + i for i in range(4)] + [xc + 1.5 + i for i in range(4)]
    j1 = _terminals("J1", nets, tx, 2.0, 0.6)
    j2 = _terminals("J2", nets, tx, 14.0, 0.6)
    xl, xr = xc - half_open, xc + half_open
    g = (xr - xl - 8 * W_SIG) / 9
    us = [xl + g + W_SIG / 2 + i * (W_SIG + g) for i in range(8)]
    witness = {n: ("F.Cu", [(t, 2.0), (t, Yr - 3.5), (u, Yr - 0.6), (u, Yr + 0.6), (t, Yr + 3.5), (t, 14.0)])
               for n, t, u in zip(nets, tx, us)}
    top = Yr - QFN_L / 2
    a_tr, c_tl = (xl, top), (xr, top)
    a_br, c_bl = (xl, Yr + QFN_L / 2), (xr, Yr + QFN_L / 2)
    b_bl, b_br = (xc - b, top - dy), (xc + b, top - dy)
    d_tl, d_tr = (xc - b, Yr + QFN_L / 2 + dy), (xc + b, Yr + QFN_L / 2 + dy)
    diag = {"len_e": 2.1260, "len_o": 2.1213, "need": 2.00, "slack_e": 0.1260, "slack_o": 0.1213}
    cuts = [
        Cut("AC", "F.Cu", a_tr, c_tl, ("U1", "10"), ("U2", "11"), "top-right corner", "top-left corner", nets,
            hand={"len_e": 3.70, "len_o": 3.70, "need": 3.80, "slack": -0.10},
            note="Crosses the shorter cut BD (3.60 < 3.70), so it is no edge of the triangulation of the pad corners."),
        Cut("AB", "F.Cu", a_tr, b_bl, ("U1", "10"), ("TP1", "1"), "top-right corner", "bottom-left corner", lnets,
            hand=dict(diag), role="context", note="dx 1.60, dy 1.40: Euclid 2.1260, octagonal (dx + dy)/sqrt 2 = 2.1213."),
        Cut("AD", "F.Cu", a_br, d_tl, ("U1", "10"), ("TP2", "1"), "bottom-right corner", "top-left corner", lnets,
            hand=dict(diag), role="context"),
        Cut("BC", "F.Cu", b_br, c_tl, ("TP1", "1"), ("U2", "11"), "bottom-right corner", "top-left corner", rnets,
            hand=dict(diag), role="context"),
        Cut("DC", "F.Cu", d_tr, c_bl, ("TP2", "1"), ("U2", "11"), "top-right corner", "bottom-left corner", rnets,
            hand=dict(diag), role="context"),
        Cut("BD", "F.Cu", b_bl, d_tl, ("TP1", "1"), ("TP2", "1"), "bottom-left corner", "top-left corner", [],
            hand={"len_e": 3.60, "len_o": 3.60, "need": 0.0}, role="empty",
            note="No wire must cross it (L* pass left of TP1 and TP2, R* right of them). The planned L4 and R1 dip under "
                 "TP1 past the line BD and back: two crossings that cancel (entanglement 0, Maley p. 32)."),
    ]
    # Delaunay witness: the circle through the four corners of TP1 / TP2 holds no wall corner.
    cc = (xc, Yr)
    r_tp = math.dist(cc, b_bl)
    r_wall = math.dist(cc, a_tr)
    extra = {
        "per_edge_model": {
            "claim": "Every edge of the triangulation of the pad corners in the opening has room; only the "
                     "non-adjacent cut AC is over-full (the 'model gap' class: per-edge model fits, cut test overflows).",
            "delaunay_check": {"circle_centre": list(cc), "radius_through_TP_corners": round(r_tp, 4),
                               "nearest_wall_corner_distance": round(r_wall, 4),
                               "reading": "1.8927 > 1.8173: the TP1-TP2 rectangle is Delaunay, its vertical sides "
                                          "(BD) split the opening, so no edge joins U1.10 to U2.11."},
            "edge_loads": "AB, AD hold L1-L4; BC, DC hold R1-R4 (4 wires, need 2.00, room 2.12); BD holds none; the "
                          "left/right diagonals of the quadrilaterals (U1.10 corners to TP corners, 2.72) hold 4.",
        }
    }
    return Pattern(
        name="model_gap_cut", title="Non-adjacent cut over-full (Maley model gap)", kind="unsolvable",
        gap_classes=["fit / over-full cut, model gap (per-edge model: fits)"],
        purpose="Eight nets cross a 3.70 mm opening between two QFN-like rows; two 0.5 mm test pads (TP1 above, TP2 "
                "below the opening centre) split it into a left and a right passage. B.Cu is closed (one layer).",
        answer="Cut AC (U1.10 to U2.11 across the opening, F.Cu): demand 8 x 0.25 + 9 x 0.20 = 3.80, capacity 3.70: "
               "over-full by 0.10 mm, in both norms. Every cut that is an edge of the triangulation fits: AB, AD, BC, "
               "DC hold 4 wires each (need 2.00, capacity 2.1213 octagonal / 2.1260 Euclid, slack +0.121 / +0.126); "
               "BD is empty. Expected: the over-full cut AC predicted and reported (0.10 mm), nothing dropped.",
        why="Unsolvable by design: whatever the topology, every wire from the top row of terminals to the bottom row "
            "passes the opening and so crosses AC (the rows reach to 0.5 mm of the edges; B.Cu is closed). AC is a "
            "critical cut (corner to closest point, Maley p. 40) and not shadowed in the octagonal norm (the shadow "
            "half-height 1.85 tan 22.5 = 0.77 < 1.40, sheet.rs shadow rule after Maley p. 55).",
        size=(W, H), layers=2, nets=nets, fps=[u1, u2, tp1, tp2, j1, j2],
        connections=[(n, f"J1.{i + 1}", f"J2.{i + 1}") for i, n in enumerate(nets)],
        cuts=cuts, keepouts=[Keepout(["B.Cu"], _strip(W, 0.0, H))], witness=witness,
        closed=[{"way": "between two pads of one row", "capacity": 0.20, "demand_one_wire": 0.65},
                {"way": "past the end of a row", "capacity": 0.50, "demand_one_wire": 0.95},
                {"way": "on B.Cu", "capacity": 0.0, "formula": "rule area on B.Cu (tracks, vias) over the board"}],
        extra=extra)


def inner_layer_room() -> Pattern:
    """5. 4 layers: the top channel holds 2, the inner layer between the drilled pins holds 3 (solvable with vias)."""
    W, H, Yr = 6.2, 14.0, 7.0
    xc = W / 2
    nets = ["S1", "S2", "S3", "S4", "S5"]
    # Reference first. In1: three wires, slack 0.25 spread over the 4 gaps.
    gi = CLEAR + 0.25 / 4
    xi = [xc + k * (W_SIG + gi) for k in (-1, 0, 1)]
    ring_in = xi[-1] + W_SIG / 2 + gi - xc  # 0.90: inner side of the pin rings
    r_pin = 0.85
    xp = ring_in + r_pin  # 1.75: a 3.5 mm pitch connector
    # F.Cu: one wire in each sub-gap between a ring and TP1, slack 0.05 spread over its 2 gaps.
    gf = CLEAR + 0.05 / 2
    xf = ring_in - gf - W_SIG / 2  # 0.55
    tp_half = xf - W_SIG / 2 - gf  # 0.20: TP1 is 0.40 wide
    j1 = Fp("J1", [Pad("1", None, (xc - xp, Yr), (1.7, 1.7), "circle", drill=1.0),
                   Pad("2", None, (xc + xp, Yr), (1.7, 1.7), "circle", drill=1.0)], "wall")
    tp1 = Fp("TP1", [Pad("1", None, (xc, Yr), (2 * tp_half, 0.8))], "obstacle")
    tx = [xc - 2.2, xc - 1.1, xc, xc + 1.1, xc + 2.2]
    j2 = _terminals("J2", nets, tx, 1.5, 0.7)
    j3 = _terminals("J3", nets, tx, 12.5, 0.7)
    tracks, vias = [], []
    for n, t, x in ((nets[0], tx[0], xc - xf), (nets[4], tx[4], xc + xf)):
        d = abs(x - t)
        tracks.append(Track(n, "F.Cu", W_SIG, [(t, 1.5), (t, Yr - 1.0 - d), (x, Yr - 1.0), (x, Yr + 1.0),
                                               (t, Yr + 1.0 + d), (t, 12.5)]))
    for n, t, x in zip(nets[1:4], tx[1:4], xi):
        tracks.append(Track(n, "F.Cu", W_SIG, [(t, 1.5), (t, 3.0)]))
        tracks.append(Track(n, "F.Cu", W_SIG, [(t, 11.0), (t, 12.5)]))
        vias += [Via(n, (t, 3.0)), Via(n, (t, 11.0))]
        d = abs(x - t)
        pts = _dedup([(t, 3.0), (t, Yr - 1.5 - d), (x, Yr - 1.5), (x, Yr + 1.5), (t, Yr + 1.5 + d), (t, 11.0)])
        tracks.append(Track(n, "In1.Cu", W_SIG, pts))
    ring_l, ring_r = (xc - ring_in, Yr), (xc + ring_in, Yr)
    cuts = [
        Cut("F1", "F.Cu", ring_l, (xc - tp_half, Yr), ("J1", "1"), ("TP1", "1"), "rightmost point of the ring",
            "left side (foot)", ["S1"], hand={"len_e": 0.70, "len_o": 0.70, "need": 0.65, "slack": 0.05}),
        Cut("F2", "F.Cu", (xc + tp_half, Yr), ring_r, ("TP1", "1"), ("J1", "2"), "right side (foot)",
            "leftmost point of the ring", ["S5"], hand={"len_e": 0.70, "len_o": 0.70, "need": 0.65, "slack": 0.05}),
        Cut("I1", "In1.Cu", ring_l, ring_r, ("J1", "1"), ("J1", "2"), "rightmost point of the ring (inner layer)",
            "leftmost point of the ring (inner layer)", ["S2", "S3", "S4"],
            hand={"len_e": 1.80, "len_o": 1.80, "need": 1.55, "slack": 0.25},
            note="Drill to drill: TP1 is on F.Cu only, so on In1 the cut runs between the two pins' rings "
                 "(KiCad keeps the annular rings on inner layers by default)."),
    ]
    extra = {
        "layer_assignment": "Unique in counts: F1 and F2 hold one wire each (two would need 1.10 > 0.70, over-full "
                            "0.40), I1 holds three (four would need 2.00 > 1.80, over-full 0.20); B.Cu and In2.Cu are "
                            "closed to tracks. 5 wires = 1 + 1 + 3; the three In1 wires need 2 vias each (6 vias). "
                            "The reference puts the outer nets S1, S5 on F.Cu.",
        "shared_edge_model": "Wrong reading this pattern exposes: taking the projection's edges J1.1-TP1 and TP1-J1.2 "
                             "(0.70 each) as the In1 room as well gives In1 only 2 wires, so 5 wires look over-full "
                             "(the fifth over-full by 0.40). On In1 TP1 does not exist; the room is I1 = 1.80.",
        "variant_removed_inner_rings": "If the pins' unused inner rings were removed (KiCad padstack option), the In1 cut "
                                       "would be hole to hole: 3.5 - 1.0 = 2.50 with the 0.25 hole clearance at both "
                                       "ends (engine Bound::Hole): room for 4 wires. Not generated.",
    }
    return Pattern(
        name="inner_layer_room", title="Full top, free inner layer (4 layers)", kind="solvable",
        gap_classes=["fit / P1b inner-layer room (the per-layer sheet, not the shared projection edges)",
                     "fit / over-full cut seen by both models (the shared-edge reading would report one here)"],
        purpose="Five nets cross the line of a 2-pin THT connector (3.5 mm pitch, 1.7 mm pads); on F.Cu a 0.40 x 0.80 mm "
                "SMD pad TP1 sits between the pins and leaves two 0.70 mm sub-gaps. B.Cu and In2.Cu are closed to "
                "tracks (vias allowed).",
        answer="F.Cu cuts F1, F2 (pin ring to TP1): 0.70 each, one wire each (need 0.65, slack +0.05). In1 cut I1 (pin "
               "ring to pin ring, drill to drill): 1.80, three wires (need 3 x 0.25 + 4 x 0.20 = 1.55, slack +0.25). "
               "All five fit: two on F.Cu, three on In1 with two vias each.",
        why="Solvable by construction: the In1 centre lines (pitch 0.5125) and the F.Cu centre lines were placed first; "
            "the pin centres (1.75 mm from the middle) and TP1 (0.40 wide) were derived from them. The pins reach to "
            "0.5 mm of the side edges (no wire fits past them on any layer).",
        size=(W, H), layers=4, nets=nets, fps=[j1, tp1, j2, j3],
        connections=[(n, f"J2.{i + 1}", f"J3.{i + 1}") for i, n in enumerate(nets)],
        cuts=cuts, keepouts=[Keepout(["B.Cu", "In2.Cu"], _strip(W, 0.0, H), tracks=True, vias=False)],
        ref_tracks=tracks, ref_vias=vias,
        witness={t.net: (t.layer, t.pts) for t in tracks if len(t.pts) > 2},
        closed=[{"way": "past a pin to the side edge (any layer)", "capacity": 0.5, "demand_one_wire": 0.95},
                {"way": "on B.Cu or In2.Cu", "capacity": 0.0, "formula": "rule area (tracks) over the board"}],
        extra=extra, views=[["F.Cu"], ["In1.Cu"]])


def diagonal_cut_norm() -> Pattern:
    """6. A cut at 22.46 degrees: fits in the Euclidean norm (any-angle), over-full in the octagonal (45 degree)."""
    Yr = 7.0
    dx, dy = 1.50, 0.62  # q - p = (1.50, -0.62): (sqrt 2 - 1) * 1.50 = 0.6213 >= 0.62, so cap_oct = dx (eq. 1.2)
    xa = 0.5 + 8 * QFN_P - (QFN_P - QFN_W)  # 4.30: right side of U1.8
    W = xa + dx + 8 * QFN_P - (QFN_P - QFN_W) + 0.5
    H = 12.8
    y_u2 = Yr - QFN_L - dy  # U2 row centre: its bottom 0.62 above U1's top
    u1 = _qfn_row("U1", Yr, 0.5, 8)
    u2 = _qfn_row("U2", y_u2, xa + dx, 8, first=9)
    p = (xa, Yr - QFN_L / 2)
    q = (xa + dx, y_u2 + QFN_L / 2)
    E = math.hypot(dx, dy)
    u = (dx / E, -dy / E)  # along the cut
    n = (dy / E, dx / E)  # across it, pointing down-right (KiCad y down)
    nets = ["S1", "S2", "S3"]
    slack = E - (3 * W_SIG + 4 * CLEAR)
    d = slack / 4
    ts = [CLEAR + d + W_SIG / 2 + i * (W_SIG + CLEAR + d) for i in range(3)]
    P = [(p[0] + t * u[0], p[1] + t * u[1]) for t in ts]
    k = n[0] / n[1]  # dx per dy along the wires (tan 22.46)
    bot_y = [Yr + 1.0 + 1.194 * i for i in range(3)]  # wire 1 bends first going down, the others diverge
    top_y = [Yr - QFN_L - dy - QFN_L / 2 - 0.6 - 1.194 * (2 - i) for i in range(3)]  # wire 3 bends first going up
    tracks, tx_top, tx_bot = [], [], []
    for net, (px, py), yb, yt in zip(nets, P, bot_y, top_y):
        xb = px + (yb - py) * k
        xt = px + (yt - py) * k
        tx_bot.append(xb)
        tx_top.append(xt)
        tracks.append(Track(net, "F.Cu", W_SIG, [(xt, 1.5), (xt, yt), (xb, yb), (xb, 12.0)]))
    j1 = _terminals("J1", nets, tx_top, 1.5, 0.6)
    j2 = _terminals("J2", nets, tx_bot, 12.0, 0.6)
    cut = Cut("C6", "F.Cu", p, q, ("U1", "8"), ("U2", "9"), "top-right corner", "bottom-left corner", nets,
              hand={"len_e": 1.6231, "len_o": 1.5000, "need": 1.55, "slack_e": 0.0731, "slack_o": -0.0500},
              note="Direction 22.46 deg (tan = 0.62/1.50), next to the octagonal norm's 22.5 deg breakpoint, where the "
                   "two norms differ most (factor cos 22.5 = 0.924). At exactly 45 deg they would agree.")
    extra = {
        "norms": {
            "euclid": "1.6231 >= 1.55: fits with slack 0.0731 (style smooth; the reference crosses at a right angle).",
            "octagonal": "max(1.50, 0.62, 2.12/sqrt 2 = 1.4991) = 1.50 < 1.55: over-full by 0.05 (style octilinear). "
                         "Vertical wires need the horizontal extent 1.50; 135 deg wires the extent (dx + dy)/sqrt 2 = "
                         "1.499 across them; no octilinear direction is better (Staepelaere eq. 1.2).",
            "per_edge_packer": "engine pack.rs stretches each spacing by the octilinear oblique factor "
                               "demand::octi_stretch = 1/cos 22.5 = 1.082 here: 1.55 x 1.082 = 1.678 > 1.6231, the same "
                               "verdict (over-full by about 0.055 on the Euclidean length).",
        },
        "expected_by_style": {"smooth": "fits, slack +0.0731, 0 violations (the reference)",
                              "octilinear": "over-full by 0.0500 at C6: predicted and reported (or the stretch kept "
                                            "any-angle, as fx_octi_no_fit), nothing dropped"},
    }
    return Pattern(
        name="diagonal_cut_norm", title="Diagonal cut: Euclidean fits, octagonal over-full",
        kind="solvable (smooth) / unsolvable (octilinear)",
        gap_classes=["fit / borderline over-full (< 0.1 mm)", "octilinear room (M-45: norm of the planning metric)"],
        purpose="Three nets cross two staggered QFN-like rows: U1's top-right corner and U2's bottom-left corner are "
                "offset by (1.50, 0.62) mm, a cut at 22.46 deg. B.Cu is closed over the rows.",
        answer="Cut C6 (U1.8 to U2.9): Euclidean 1.6231, octagonal 1.5000; demand 3 x 0.25 + 4 x 0.20 = 1.55. Smooth "
               "style: fits (slack +0.073). Octilinear style: over-full by 0.050.",
        why="Solvable for any-angle drawing by construction (the reference wires cross C6 at a right angle at a pitch "
            "of 0.468 mm; the rows were placed at the cut's ends). For 45 degree drawing it is unsolvable by design: "
            "C6 is the only way across (rows to 0.5 mm of the edges, B.Cu closed) and its octagonal length is 0.05 "
            "short.",
        size=(round(W, 6), H), layers=2, nets=nets, fps=[u1, u2, j1, j2],
        connections=[(n, f"J1.{i + 1}", f"J2.{i + 1}") for i, n in enumerate(nets)], cuts=[cut],
        keepouts=[Keepout(["B.Cu"], _strip(W, y_u2 - 1.5, Yr + 1.5))], ref_tracks=tracks,
        witness={t.net: (t.layer, t.pts) for t in tracks},
        closed=[{"way": "between two pads of one row", "capacity": 0.20, "demand_one_wire": 0.65},
                {"way": "past the end of a row", "capacity": 0.50, "demand_one_wire": 0.95},
                {"way": "on B.Cu", "capacity": 0.0, "formula": "rule area on B.Cu (tracks, vias) over the rows"}],
        extra=extra)


PATTERNS = {f.__name__: f for f in (channel_fit, channel_overfull, neck_fit, neck_too_narrow, model_gap_cut,
                                     inner_layer_room, diagonal_cut_norm)}


# ----------------------------------------------------------------------------------------------- expected values
def class_of(pat: Pattern, net: str) -> dict:
    for np_ in pat.net_patterns:
        if np_["pattern"] == net:
            return next(c for c in pat.classes if c["name"] == np_["netclass"])
    return next(c for c in pat.classes if c["name"] == "Default")


def floor_of(pat: Pattern) -> float | None:
    neck = pat.extra.get("neck")
    return neck["floor"] if neck else None


def cut_values(pat: Pattern, c: Cut) -> dict:
    """Lengths, demand and slack of one cut from the geometry and the pattern's rules."""
    dx, dy = c.q[0] - c.p[0], c.q[1] - c.p[1]
    le, lo = math.hypot(dx, dy), oct_len(dx, dy)
    items = [(n, class_of(pat, n)["track_width"], class_of(pat, n)["clearance"]) for n in c.crossing]
    ends = []
    for ref, num in (c.fp, c.fq):
        pad = next(p for f in pat.fps if f.ref == ref for p in f.pads if p.num == num)
        ends.append(("pad", pad.net, CLEAR))
    need = demand(items, ends[0], ends[1])
    out = {"len_e": le, "len_o": lo, "need": need, "slack_e": le - need, "slack_o": lo - need,
           "angle_deg": math.degrees(math.atan2(-dy, dx))}
    fl = floor_of(pat)
    if fl is not None and items:
        floor_items = [(n, min(w, fl), s) for n, w, s in items]
        out["need_floor"] = demand(floor_items, ends[0], ends[1])
        out["slack_floor"] = le - out["need_floor"]
    return out


def check_hand(pat: Pattern, tol: float = 1e-4) -> None:
    """The computed numbers must equal the ones worked out by hand (rounded to 4 decimals)."""
    for c in pat.cuts:
        v = cut_values(pat, c)
        h = c.hand
        pairs = [("len_e", v["len_e"]), ("len_o", v["len_o"]), ("need", v["need"])]
        if "slack" in h:
            pairs.append(("slack", v["slack_e"]))
            assert abs(v["slack_e"] - v["slack_o"]) < tol or "slack_o" in h, (pat.name, c.id)
        for key in ("slack_e", "slack_o", "need_floor", "slack_floor"):
            if key in h:
                pairs.append((key, v[key]))
        for key, got in pairs:
            assert abs(h[key] - got) <= 5e-5 + tol, f"{pat.name} {c.id} {key}: hand {h[key]} computed {got:.6f}"


def _r(x: float) -> float:
    return round(x, 4)


def expected_doc(pat: Pattern) -> dict:
    check_hand(pat)
    cuts = []
    for c in pat.cuts:
        v = cut_values(pat, c)
        items = [{"net": n, "width": class_of(pat, n)["track_width"], "clearance": class_of(pat, n)["clearance"]}
                 for n in c.crossing]
        k = len(items)
        d = {
            "id": c.id, "role": c.role, "layer": c.layer,
            "p": [_r(c.p[0]), _r(c.p[1])], "q": [_r(c.q[0]), _r(c.q[1])],
            "from": f"{c.fp[0]}.{c.fp[1]} {c.at_p}", "to": f"{c.fq[0]}.{c.fq[1]} {c.at_q}",
            "direction_deg": round(v["angle_deg"], 3),
            "length_euclid": _r(v["len_e"]), "length_octagonal": _r(v["len_o"]),
            "ends": "foreign pads (no net), clearance max(0.20 pad, 0.20 wire) = 0.20 at each end",
            "crossing": items,
            "demand_formula": (f"{k} x w + {max(k - 1, 0)} x s (between different nets) + 2 x s (ends)" if k else
                               "no wire crosses: empty cut"),
            "demand": _r(v["need"]),
            "slack_euclid": _r(v["slack_e"]) if k else None,
            "slack_octagonal": _r(v["slack_o"]) if k else None,
            "hand": c.hand,
            "note": c.note,
        }
        if "need_floor" in v:
            d["demand_at_floor"] = _r(v["need_floor"])
            d["slack_at_floor"] = _r(v["slack_floor"])
        if k:
            d["maley_form"] = {"capacity": _r(v["len_e"] - CLEAR), "congestion": _r(sum(i["width"] + i["clearance"]
                                                                                       for i in items)),
                               "formula": "capacity = length - s/2 - s/2; congestion = sum(w + s) (Maley p. 32, "
                                          "spacing folded into the widths): same slack as the demand form"}
        cuts.append(d)
    doc = {
        "name": pat.name, "title": pat.title, "kind": pat.kind, "gap_classes": pat.gap_classes,
        "purpose": pat.purpose, "expected": pat.answer, "why": pat.why,
        "board": {"size_mm": list(pat.size), "copper_layers": pat.layers,
                  "keepouts": [{"layers": k.layers, "tracks": k.tracks, "vias": k.vias,
                                "poly": [[_r(x), _r(y)] for x, y in k.poly]} for k in pat.keepouts]},
        "rules": {"clearance": CLEAR, "track_width": {c["name"]: c["track_width"] for c in pat.classes},
                  "board_min_track_width": pat.rules["min_track_width"], "copper_edge_clearance": EDGE,
                  "hole_clearance": HOLE_CLEAR, "kicad_dru": pat.dru or None,
                  "netclass_patterns": pat.net_patterns},
        "capacity_definition": {
            "capacity": "length of the cut in the planning norm: Euclid (style smooth) or octagonal max(|dx|, |dy|, "
                        "(|dx|+|dy|)/sqrt 2) (style octilinear; Staepelaere 1992 eq. 1.2, PDF p. 12)",
            "demand": "sum of widths + max(s_i, s_j) between neighbours of different nets + at each end the end's "
                      "clearance (foreign pad: max(pad clearance, wire clearance))",
            "sources": ["docs/dev/inflated-sketch.md §1.2 (Maley p. 32 with spacing folded into widths)",
                        "engine topo/demand.rs::need, topo/cutflow.rs (cap = sheet cut length in the norm, no oblique "
                        "factor), topo/sheet.rs::Norm (branch cl-cleanup)"],
            "agreement": "Identical for one clearance class (every pattern here): Maley's capacity L - s with "
                         "congestion sum(w + s) and the engine's need <= L give the same slack.",
        },
        "wires": [{"net": n, "from": a, "to": b, "width": class_of(pat, n)["track_width"],
                   "clearance": class_of(pat, n)["clearance"],
                   "crosses": [c.id for c in pat.cuts if n in c.crossing],
                   "witness_layer": pat.witness[n][0] if n in pat.witness else None,
                   "witness": [[_r(x), _r(y)] for x, y in pat.witness[n][1]] if n in pat.witness else None}
                  for n, a, b in pat.connections],
        "cuts": cuts,
        "other_ways_across": pat.closed,
        "connections": len(pat.connections),
        "reference": ({"tracks": len(pat.ref_tracks), "vias": len(pat.ref_vias),
                       "expected_drc": {"violations": 0, "unconnected": 0}} if pat.solvable else None),
        "problem_expected_drc": {"violations": 0, "unconnected": len(pat.connections)},
    }
    doc.update(pat.extra)
    return doc


# ----------------------------------------------------------------------------------------------- boards
def build_board(pat: Pattern, *, copper: bool) -> pcbnew.BOARD:
    pcbnew.KIID.SeedGenerator(1)
    b = pcbnew.CreateEmptyBoard()
    b.SetCopperLayerCount(pat.layers)
    nets = {}
    for name in pat.nets:
        n = pcbnew.NETINFO_ITEM(b, name)
        b.Add(n)
        nets[name] = n
    W, H = pat.size
    for a, c in (((0, 0), (W, 0)), ((W, 0), (W, H)), ((W, H), (0, H)), ((0, H), (0, 0))):
        s = pcbnew.PCB_SHAPE(b)
        s.SetShape(pcbnew.SHAPE_T_SEGMENT)
        s.SetLayer(pcbnew.Edge_Cuts)
        s.SetWidth(int(0.05 * MM))
        s.SetStart(_v(*a))
        s.SetEnd(_v(*c))
        b.Add(s)
    for f in pat.fps:
        fp = pcbnew.FOOTPRINT(b)
        fp.SetReference(f.ref)
        fp.SetValue(f.ref)
        fp.SetPosition(_v(*f.pads[0].pos))
        fp.Reference().SetLayer(pcbnew.F_Fab)
        fp.Value().SetLayer(pcbnew.F_Fab)
        fp.Value().SetVisible(False)
        b.Add(fp)
        for ps in f.pads:
            p = pcbnew.PAD(fp)
            p.SetNumber(ps.num)
            if ps.drill is None:
                p.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
                ls = pcbnew.LSET()
                for ly in (LAYER_IDS[ps.layer], pcbnew.F_Mask, pcbnew.F_Paste):
                    ls.AddLayer(ly)
                p.SetLayerSet(ls)
            else:
                p.SetAttribute(pcbnew.PAD_ATTRIB_PTH)
                p.SetLayerSet(p.PTHMask())
                p.SetDrillSize(_v(ps.drill, ps.drill))
            p.SetShape(pcbnew.PAD_SHAPE_CIRCLE if ps.shape == "circle" else pcbnew.PAD_SHAPE_RECT)
            p.SetSize(_v(*ps.size))
            p.SetPosition(_v(*ps.pos))
            if ps.net is not None:
                p.SetNet(nets[ps.net])
            fp.Add(p)
    for k in pat.keepouts:
        z = pcbnew.ZONE(b)
        z.SetIsRuleArea(True)
        ls = pcbnew.LSET()
        for ly in k.layers:
            ls.AddLayer(LAYER_IDS[ly])
        z.SetLayerSet(ls)
        z.SetDoNotAllowTracks(k.tracks)
        z.SetDoNotAllowVias(k.vias)
        z.SetDoNotAllowZoneFills(False)
        z.SetDoNotAllowPads(False)
        z.SetDoNotAllowFootprints(False)
        ol = pcbnew.SHAPE_LINE_CHAIN()
        for x, y in k.poly:
            ol.Append(_v(x, y))
        ol.SetClosed(True)
        z.AddPolygon(ol)
        b.Add(z)
    if copper:
        for t in pat.ref_tracks:
            for a, c in zip(t.pts, t.pts[1:]):
                tr = pcbnew.PCB_TRACK(b)
                tr.SetStart(_v(*a))
                tr.SetEnd(_v(*c))
                tr.SetWidth(int(round(t.width * MM)))
                tr.SetLayer(LAYER_IDS[t.layer])
                tr.SetNet(nets[t.net])
                b.Add(tr)
        for v in pat.ref_vias:
            via = pcbnew.PCB_VIA(b)
            via.SetPosition(_v(*v.pos))
            via.SetDrill(int(round(v.drill * MM)))
            via.SetWidth(int(round(v.dia * MM)))
            via.SetNet(nets[v.net])
            b.Add(via)
    return b


def write_board(pat: Pattern, path: Path, *, copper: bool) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    pcbnew.SaveBoard(str(path), build_board(pat, copper=copper))
    write_project_file(path, rules=pat.rules, classes=pat.classes, patterns=pat.net_patterns)
    dru = path.with_suffix(".kicad_dru")
    if pat.dru:
        dru.write_text(pat.dru)
    elif dru.exists():
        dru.unlink()
    return path


def generate(name: str, out: Path, *, png: bool = True) -> dict:
    """Write one pattern under out/<name>/: problem/, reference/ (solvable), expected.json, <name>.png."""
    pat = PATTERNS[name]()
    d = Path(out) / name
    d.mkdir(parents=True, exist_ok=True)
    paths = {"problem": write_board(pat, d / "problem" / f"{name}.kicad_pcb", copper=False)}
    if pat.solvable:
        paths["reference"] = write_board(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)
    doc = expected_doc(pat)
    (d / "expected.json").write_text(json.dumps(doc, indent=2) + "\n")
    paths["expected"] = d / "expected.json"
    if png:
        from harness.fit_patterns_png import render

        paths["png"] = render(pat, doc, d / f"{name}.png")
    return paths


def readme(out: Path) -> Path:
    lines = ["# Fit patterns (shopping list 2.1)", "",
             "Generated by `harness/fit_patterns.py` (branch `fit-patterns`); regenerate with "
             "`.venv/bin/python -m harness.fit_patterns`. Each directory holds `problem/` (pads, rule areas, nets; no "
             "copper), `reference/` (solvable patterns: the reference copper, KiCad DRC 0 violations / 0 unconnected), "
             "`expected.json` (hand-computed cut lengths, demands, slack or overflow, with the formulas) and a PNG "
             "(problem left, reference or the marked impossibility right).", "",
             "Rules on every board: Default class 0.25 mm track at 0.20 mm clearance, board minimum track width 0.25 "
             "(no neck for signals), copper to edge 0.50, hole clearance 0.25. Demand of a cut = sum of widths + "
             "clearances between different nets + the clearance at each end; capacity = the cut's length in the "
             "planning norm (Euclid for smooth, octagonal for octilinear).", "",
             "| Pattern | Kind | Gap class | Key numbers |", "|---|---|---|---|"]
    for name, f in PATTERNS.items():
        pat = f()
        keys = []
        for c in [c for c in pat.cuts if c.role == "binding"]:
            v = cut_values(pat, c)
            key = f"{c.id} ({c.layer}): {v['len_e']:.4f}"
            key += f" (oct {v['len_o']:.4f})" if abs(v["len_e"] - v["len_o"]) > 1e-6 else ""
            key += f", demand {v['need']:.2f}, slack {v['slack_e']:+.4f}"
            if abs(v["slack_e"] - v["slack_o"]) > 1e-6:
                key += f" (oct {v['slack_o']:+.4f})"
            if "need_floor" in v:
                key += f"; at floor {floor_of(pat):.2f}: demand {v['need_floor']:.2f}, slack {v['slack_floor']:+.2f}"
            keys.append(key)
        lines.append(f"| `{name}` | {pat.kind} | {pat.gap_classes[0]} | {'; '.join(keys)} |")
    lines.append("")
    for i, (name, f) in enumerate(PATTERNS.items(), 1):
        pat = f()
        lines += [f"## {i}. `{name}`: {pat.title}", "",
                  f"{pat.purpose} **Gap class:** {'; '.join(pat.gap_classes)}. **Expected:** {pat.answer} "
                  f"**Why {pat.kind}:** {pat.why}", "", f"![{name}]({name}/{name}.png)", ""]
    lines += [
        "## Notes for the review", "",
        "- **Changes to the proposed set.** (a) Case 6 asked for a 45 deg cut; at exactly 45 deg the octagonal and "
        "Euclidean norms are equal (sqrt 2 dx both), so the cut is at 22.46 deg, next to the norm's 22.5 deg "
        "breakpoint where they differ most (factor cos 22.5 = 0.924; Staepelaere 1992 eq. 1.2, PDF p. 12). "
        "(b) Case 3 is two patterns (`neck_fit`, `neck_too_narrow`): the same board, only the POWER neck floor in "
        "the .kicad_dru differs (0.40 / 0.50), so the two answers differ by one number. (c) Case 2's overflow is "
        "one width + one clearance minus case 1's slack (0.45 - 0.05 = 0.40), since it reuses case 1's channel; the "
        "row gaps (0.20) and row-to-edge gaps (0.50) were sized so every other way costs 0.45, making C1 the unique "
        "least violation.",
        "- **Capacity definitions.** For one clearance class the doc (`docs/dev/inflated-sketch.md` §1.2, Maley p. 32 "
        "with spacing folded into widths: capacity L - s, congestion sum(w + s)) and the engine (`topo/demand.rs::need`, "
        "`topo/cutflow.rs`: cap = cut length in the norm, need = sum w + gaps + end clearances, no oblique factor) give "
        "the same slack. They differ only where these patterns do not go: mixed clearances (engine `mixed_clearance` "
        "conservative = s_max on every gap vs pairwise max(s_i, s_j); the doc's territory model w/2 + s_max/2), "
        "own-net ends (half width) and junction points (cap minus half width, `cutflow.rs::half_at`). The per-edge "
        "packer of P2-P4 (`topo/pack.rs`) is a third definition: Euclidean edge length with each spacing times the "
        "oblique factor 1/sin (octilinear: `demand::octi_stretch`), which reaches the same verdict on "
        "`diagonal_cut_norm` (1.55 x 1.082 = 1.678 > 1.623).",
        "- **Circles.** The THT pads are exact circles here; the engine's sheet grows each polygonised circle by its "
        "sagitta, so its capacity on N1, F1, F2, I1 is at most the sagitta (a few um) below the stated value.",
        "- **Neck width.** `neck_fit` expects the widest neck that fits (0.44). The neck-width ladder in memory "
        "(pad width, then default width, then widest that fits) would try the Default width 0.25 first, which is "
        "below this pattern's 0.40 floor; user to confirm which width the expected answer should name.",
        "",
    ]
    p = Path(out) / "README.md"
    p.write_text("\n".join(lines))
    return p


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "fit-patterns"
    for name in PATTERNS:
        paths = generate(name, out)
        print(name, *(str(p) for p in paths.values()))
    print(readme(out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
