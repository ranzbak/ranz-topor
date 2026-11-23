"""PROPOSED test patterns for draw-via-neck (docs/dev/design/draw-via-neck.md): the metric stage (ERBS drawing) shifts a
jammed via before it necks a neighbour, necks only where the room ends, and enters a pad along its axis.

NOT in the test suite: the user approves patterns first (AGENT_RULES). Built like harness/tw6_patterns.py (reference
copper first, pads around it, a shapely clearance check, KiCad DRC with --drc). Two layers, Default class 0.25 / 0.2,
via 0.6 / 0.3; the neck patterns add a POWER class (0.5 / 0.2) with a 0.2 mm neck floor (.kicad_dru, as fit_patterns'
neck_fit).

1. `via_jam_shift`      (user case Q2) V changes layer next to a fine-pitch pad column; N leaves the pad above it and runs
                        between V's via and a fixed wall. A via jammed against the column leaves N no room (neck or
                        clearance error); the via shifted into the free room below lets N pass at full width.
2. `neck_in_pad_row`    (user case Q3, the neck length) a 0.5 mm POWER wire enters a 0.3 mm pad of a 0.5 mm pitch row: it
                        necks only inside the row and for the round end's clearance past it (0.283 mm past the tip).
3. `neck_shared_channel` (rule (2), second half) the same row; a signal S leaves the next pad and runs beside the POWER wire
                        through a 1.4 mm channel: the neck extends to the channel's end because S needs the room there.
4. `pad_axis_entry`     (user case Q3, the hook) a wire from the lower left into a 0.3 x 1.6 pad of a row: straight out of
                        the tip along the pad axis, turn 45 deg once the diagonal clears the neighbour pad's corner (about
                        one trace radius past the tip); no leg along the pad side.

    python -m harness.dvn_patterns [OUT] [--drc]
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from harness.fit_patterns import CLEAR, DEFAULT_CLASS, W_SIG, Fp, Keepout, Pad, Pattern, Track, Via, write_board
from harness.fx_survivors import clearance_report
from harness.radii_patterns import EPS

STATUS = "PROPOSED (awaiting approval)"
SQ2 = math.sqrt(2.0)
VIA_D = 0.6
W_POW, FLOOR = 0.5, 0.2
POWER = {"name": "POWER", "clearance": CLEAR, "track_width": W_POW, "via_diameter": 0.8, "via_drill": 0.4, "priority": 0}
DRU = ('(version 1)\n(rule "POWER neck floor"\n  (condition "A.NetClass == \'POWER\'")\n'
       f'  (constraint track_width (min {FLOOR:g}mm)))\n')


def _len(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def _r(x: float) -> float:
    return round(x, 4)


def _row(ref: str, nets: dict[int, str], xs: list[float], y: float, size: tuple[float, float]) -> Fp:
    return Fp(ref, [Pad(str(i + 1), nets.get(i + 1), (x, y), size) for i, x in enumerate(xs)], "wall")


# ------------------------------------------------------------------------------------------------ 1. via_jam_shift
def via_jam_shift() -> Pattern:
    return _via_jam(False)


def via_interval_sliver() -> Pattern:
    """1b (lead 2026-11-23, Q2's cause): via_jam_shift plus X, an F.Cu wire below the column that V's B.Cu run must cross:
    the via's interval is the pocket between X, N and the column, which the triangulation cuts into sliver fans from the
    column's pad corners; the pocket still holds the via at its west side."""
    return _via_jam(True)


def _via_jam(sliver: bool) -> Pattern:
    W, H = 14.0, 14.0
    xc = 10.0  # the column's pads: 0.8 (x) x 0.3 (y) at 0.5 mm pitch, y 5.0 .. 7.5 (pin 3 = N, pin 4 = V)
    ys = [5.0 + 0.5 * i for i in range(6)]
    u1 = _row("U1", {3: "N", 4: "V"}, [xc] * 6, 0.0, (0.8, 0.3))
    for p, y in zip(u1.pads, ys):
        p.pos = (xc, y)
    edge = xc - 0.4  # the column's west edge
    half = W_SIG / 2 + CLEAR + EPS
    # N: straight out of pin 3 along its axis, then 45 deg down-left (x + y = cN) to J2.
    xn = 9.2
    cN = xn + 6.0
    j2c = (4.2, cN - 4.2)  # (4.2, 11.0)
    n_pts = [(xc, 6.0), (xn, 6.0), j2c]
    # The wall: F.Cu tracks are not allowed north-west of N's corridor (other copper there, like qmtech's VBUS):
    # parallel to N's diagonal at exactly N's half width + clearance from its centre line.
    off = half * SQ2  # in x + y
    cw = cN - off
    # The wall: the half plane x + y <= cw inside the board's top-left corner (tracks only; vias may stand in it).
    wall = Keepout(["F.Cu"], [(0.6, 0.6), (_r(cw - 0.6), 0.6), (0.6, _r(cw - 0.6))], tracks=True, vias=False)
    # V: straight out of pin 4 along its axis to x 9.4, 45 deg down-left to x 9.0, down to the via; B.Cu down to J1.
    vy = 7.2
    via = (9.0, vy)
    v_f = [(xc, 6.5), (9.4, 6.5), (9.0, 6.9), via]
    j1c = (9.0, 12.0)
    v_b = [via, j1c]
    j1 = Fp("J1", [Pad("1", "V", j1c, (1.0, 1.0), layer="B.Cu")], "terminal")
    j2 = Fp("J2", [Pad("1", "N", j2c, (1.0, 1.0))], "terminal")
    fps_x, conns_x, tracks_x, nets_x = [], [], [], []
    if sliver:
        # X on F.Cu at y 8.3 from J4 (12.5, 8.3) to J3 (8.4, 8.3): V (B.Cu, x 9.0) crosses it, so V's via lies above it, in
        # the pocket x <= 9.1 (column), y <= 7.675 (X), x + y >= 16.08 (N). The expected via (9.0, 7.2) is in it.
        yx = 8.3
        j3c, j4c = (8.4, yx), (12.5, yx)
        fps_x = [Fp("J3", [Pad("1", "X", j3c, (1.0, 1.0))], "terminal"), Fp("J4", [Pad("1", "X", j4c, (1.0, 1.0))], "terminal")]
        conns_x = [("X", "J3.1", "J4.1")]
        tracks_x = [Track("X", "F.Cu", W_SIG, [j3c, j4c])]
        nets_x = ["X"]
    # The jam (what the drawing does today on qmtech Q2): the via at the column's pin 4, 0.32 mm off its edge, on N's
    # straight corridor; N then has to pass between it and the wall: room = dist(via, wall) - via_r - clearance.
    jam = (_r(edge - VIA_D / 2 - CLEAR - EPS), 6.5)
    room_jam = (jam[0] + jam[1] - cw) / SQ2 - VIA_D / 2 - CLEAR
    room = {"via_at_jam": list(jam), "n_room_at_jam_mm": _r(room_jam), "n_needs_mm": W_SIG + CLEAR,
            "via_free_band": "below y 7.0 on V's B.Cu path (x 9.0): the via clears N by >= 0.2 when x + y >= "
                             f"{_r(cN + (VIA_D / 2 + CLEAR + W_SIG / 2) * SQ2)}"}
    return Pattern(
        name="via_interval_sliver" if sliver else "via_jam_shift",
        title=("The via's interval is a sliver pocket beside the column (X crossed on B.Cu): the via goes to the pocket's room"
               if sliver else "A via jammed against a pad column necks its neighbour; shifted into free room it does not"),
        kind="solvable",
        gap_classes=["draw-via-neck (1): the drawing's via site prices the planned room of the neighbour (graded) and moves "
                     "the via before the neighbour necks (TopoR 6.3 manual p. 179 Shift Vias, p. 183)"],
        purpose="Two layers. U1 is a fine-pitch pad column (0.8 x 0.3 pads, 0.5 mm pitch) at x 10, F.Cu. N (pin 3, y 6.0) goes "
                "to J2 (4.2, 11.0) on F.Cu; V (pin 4, y 6.5) comes from J1 (9.0, 12.0), an SMD pad on B.Cu: V needs one via. "
                "North-west of N's straight corridor F.Cu is closed (a tracks-only rule area: other copper there, as "
                "qmtech's VBUS F.Cu trace in user case Q2), exactly N's half width + clearance from N's centre line. "
                f"A via at the column (as Q2: {jam}) leaves N {room_jam:.3f} mm where it needs {W_SIG + CLEAR:.2f}: N necks or "
                "breaks a clearance. Free room for the via: below the column's pin 4 on V's B.Cu path.",
        answer=f"N straight out of pin 3 to x {xn}, then 45 deg down-left to J2 at full width 0.25 ({_len(n_pts):.3f} mm). V straight "
               f"out of pin 4 to x 9.4, 45 deg to x 9.0, down to the via at {via}, B.Cu straight down to J1. 1 via, 0 necks, DRC 0. "
               "Any via site on x 9.0 between y 7.15 and the B.Cu run's end is equally good (same length); the via never "
               "on N's corridor.",
        why="Solvable by construction: N's corridor and the wall were laid out first, the via's band derived from them. The "
            "jammed site is legal for the via itself (it keeps 0.2 to the column), so a test that only checks the via's "
            "own clearance takes it; only the neighbour's planned room shows it is wrong.",
        size=(W, H), layers=2, nets=["N", "V"] + nets_x, fps=[u1, j1, j2] + fps_x,
        connections=[("N", "U1.3", "J2.1"), ("V", "U1.4", "J1.1")] + conns_x, cuts=[], keepouts=[wall],
        classes=[dict(DEFAULT_CLASS)],
        ref_tracks=[Track("N", "F.Cu", W_SIG, n_pts), Track("V", "F.Cu", W_SIG, v_f), Track("V", "B.Cu", W_SIG, v_b)] + tracks_x,
        ref_vias=[Via("V", via)],
        witness={"N": ("F.Cu", n_pts), "V": ("F.Cu", v_f)},
        extra={"dvn": {"ref_len": _r(_len(n_pts) + _len(v_f) + _len(v_b) + sum(_len(t.pts) for t in tracks_x)), "vias": 1, "necks": 0, "room": room,
                       "jam_marks": [list(jam)],
                       "literature": "TopoR 6.3 manual p. 179 (F7 Shift Vias), p. 183 ('most violations are eliminated by "
                                     "procedures for automatic via shifting'); Dayan & Dai 1991 UCSC-CRL-91-40 pp. 2-6 "
                                     "(force-driven movable vias, other nets' wires push)"}})


# ------------------------------------------------------------------------------------------------ 2./3. neck rows
def _neck_row(name: str, channel: bool) -> Pattern:
    W, H = 13.0, 12.0
    yr, pl, pw = 4.0, 0.8, 0.3  # the row: 0.3 x 0.8 pads, 0.5 mm pitch, centres y 4.0, tips at y 4.4
    xs = [5.0 + 0.5 * i for i in range(7)]  # pin 4 (x 6.5) = PWR, pin 3 (x 6.0) = S (channel pattern)
    xp = 6.5
    nets = {4: "PWR"} | ({3: "S"} if channel else {})
    u1 = _row("U1", nets, xs, yr, (pw, pl))
    tip = yr + pl / 2
    # The widest neck between pins 3 and 5 (edges 6.15 / 6.85): 0.7 - 2 x 0.2 = 0.30 (= the pad width; floor 0.2).
    w_neck = _r(1.0 - pw - 2 * CLEAR - 2 * EPS)
    # Full width where its round end clears the neighbour pads' tip corners (lateral 0.35): sqrt(0.45^2 - 0.35^2) past.
    lat = 0.5 - pw / 2
    dy = math.sqrt((W_POW / 2 + CLEAR + EPS) ** 2 - lat ** 2)
    jp = (xp, 10.0)
    j1 = Fp("J1", [Pad("1", "PWR", jp, (1.5, 1.5))], "terminal")
    fps = [u1, j1]
    conns = [("PWR", "U1.4", "J1.1")]
    keep, extra_tr, nets_l = [], [], ["PWR"]
    if not channel:
        y_full = _r(tip + dy)
        tracks = [Track("PWR", "F.Cu", w_neck, [(xp, yr), (xp, y_full)]), Track("PWR", "F.Cu", W_POW, [(xp, y_full), jp])]
        neck_len_past_tip = _r(y_full - tip)
        title = "A wide wire into a narrow pad of a row: neck inside the row only"
        purpose = (f"Two layers, B.Cu closed. PWR (POWER class, 0.5 mm, neck floor {FLOOR} mm) from J1 (6.5, 10) to pin 4 of a "
                   "0.5 mm pitch row of 0.3 x 0.8 pads (centres y 4.0, tips y 4.4). Between pins 3 and 5 the widest wire is "
                   f"{w_neck} mm; below the row the wire is free.")
        answer = (f"PWR straight up the pad axis: {W_POW} mm from J1 to y {y_full}, then {w_neck} mm into the pad centre. The "
                  f"neck is {neck_len_past_tip} mm past the row's tip (where the 0.5 mm round end clears the neighbour "
                  "pins' tip corners by 0.2), nowhere else. Today's zone (a circle round the pad centre to the necked "
                  "pass's far edge end plus one pitch) reaches about 0.8 mm past the tip (qmtech Q3: 0.37-0.65 mm).")
        why = "Solvable by construction: the straight line is the only shortest path; the neck geometry follows from the pins."
    else:
        # S: from pin 3 straight down its axis, beside PWR, through a channel (F.Cu closed west of it and east of PWR)
        # down to y 6.4, then 45 deg down-left to J3.
        y_ch = 6.4
        xs_ = 6.0
        # Channel walls: west of S at S's half width + clearance, east of PWR (necked) at its half width + clearance.
        x_w = _r(xs_ - W_SIG / 2 - CLEAR - EPS)
        x_e = _r(xp + w_neck / 2 + CLEAR + EPS)
        keep = [Keepout(["F.Cu"], [(0.6, tip + 0.05), (x_w, tip + 0.05), (x_w, y_ch), (0.6, y_ch)], tracks=True, vias=True),
                Keepout(["F.Cu"], [(x_e, tip + 0.05), (W - 0.6, tip + 0.05), (W - 0.6, y_ch), (x_e, y_ch)], tracks=True, vias=True)]
        # Full width once S has turned away: S's diagonal from (6.0, y_ch) down-left (x + y = 6.0 + y_ch); PWR's full
        # piece starts where its round end clears both S's corner and the channel's east wall end.
        # The full-width end cap at (6.5, y) must keep 0.2 from S (corner then diagonal): distance from (6.5, y) to
        # S's polyline >= 0.25 + 0.2 + 0.125 = 0.575.
        need = W_POW / 2 + CLEAR + W_SIG / 2 + EPS
        # S diagonal line x + y = 6.0 + y_ch: distance from (6.5, y) = (6.5 + y - 6.0 - y_ch) / sqrt2 >= need
        y_diag = 6.0 + y_ch - 6.5 + need * SQ2
        # the corner (6.0, y_ch): sqrt(0.5^2 + (y - y_ch)^2) >= need
        y_corner = y_ch + math.sqrt(max(need * need - 0.25, 0.0))
        # the east wall's lower end corner (x_e, y_ch): distance >= 0.25 + 0.2
        y_wall = y_ch + math.sqrt(max((W_POW / 2 + CLEAR + EPS) ** 2 - (x_e - xp) ** 2, 0.0))
        y_full = _r(max(y_diag, y_corner, y_wall))
        j3c = (_r(6.0 + y_ch - 9.0), 9.0)
        j3 = Fp("J3", [Pad("1", "S", j3c, (1.0, 1.0))], "terminal")
        fps.append(j3)
        conns.append(("S", "U1.3", "J3.1"))
        nets_l.append("S")
        extra_tr = [Track("S", "F.Cu", W_SIG, [(xs_, yr), (xs_, y_ch), j3c])]
        tracks = [Track("PWR", "F.Cu", w_neck, [(xp, yr), (xp, y_full)]), Track("PWR", "F.Cu", W_POW, [(xp, y_full), jp])]
        neck_len_past_tip = _r(y_full - tip)
        title = "A wide wire beside a neighbour in a channel: the neck extends only as far as the neighbour needs the room"
        purpose = (f"neck_in_pad_row's row and PWR wire; S (Default 0.25) leaves pin 3 and must run straight down beside PWR "
                   f"through a channel down to y {y_ch} (F.Cu closed west of S and east of necked PWR), then turns 45 deg to "
                   f"J3 {j3c}. In the channel PWR fits only necked ({w_neck}).")
        answer = (f"S straight down to y {y_ch}, 45 deg down-left to J3. PWR {w_neck} mm from the pad to y {y_full} (where its "
                  f"0.5 mm round end clears S's corner / diagonal and the channel wall's end), {W_POW} mm from there to J1. The "
                  f"neck runs {neck_len_past_tip} mm past the row's tip because S needs that room, not further.")
        why = "Solvable by construction: S and the necked PWR fill the channel at the clearance; the walls are derived from them."
    return Pattern(
        name=name, title=title, kind="solvable",
        gap_classes=["draw-via-neck (2): a neck only where the room ends; longer only where another trace needs the room"],
        purpose=purpose, answer=answer, why=why, size=(W, H), layers=2, nets=nets_l, fps=fps, connections=conns, cuts=[],
        keepouts=[Keepout(["B.Cu"], [(0.0, 0.0), (W, 0.0), (W, H), (0.0, H)])] + keep,
        classes=[dict(DEFAULT_CLASS), dict(POWER)], net_patterns=[{"netclass": "POWER", "pattern": "PWR"}], dru=DRU,
        ref_tracks=tracks + extra_tr, ref_vias=[],
        witness={"PWR": ("F.Cu", [(xp, yr), jp])},
        extra={"dvn": {"ref_len": _r(sum(_len(t.pts) for t in tracks + extra_tr)), "vias": 0, "neck_width": w_neck,
                       "neck_to_y": y_full, "neck_len_past_tip": neck_len_past_tip, "row_tip_y": tip,
                       "literature": "TopoR 6.3 manual p. 56 (the minimum width only where needed, near pads narrower than "
                                     "the wire); user rule 2026-11-05 (a neck may run further back when it frees room for "
                                     "others), 2026-11-23 (full width up to the pad row, neck inside it only)"}})


def neck_in_pad_row() -> Pattern:
    return _neck_row("neck_in_pad_row", False)


def neck_shared_channel() -> Pattern:
    return _neck_row("neck_shared_channel", True)


# ------------------------------------------------------------------------------------------------ 4. pad_axis_entry
def pad_axis_entry() -> Pattern:
    W, H = 13.0, 10.0
    yr, pl, pw = 3.5, 1.6, 0.3  # ADV7511-like row: 0.3 x 1.6 pads, 0.5 mm pitch, tips at y 4.3
    xs = [5.0 + 0.5 * i for i in range(7)]
    xp = 6.5  # pin 4 = SIG
    u1 = _row("U1", {4: "SIG"}, xs, yr, (pw, pl))
    tip = yr + pl / 2
    need = W_SIG / 2 + CLEAR + EPS
    # Turn 45 deg down-left at (xp, yt): the diagonal x + y = xp + yt keeps `need` from pin 3's tip corner (6.15, tip).
    c = (xp - 0.5 + pw / 2, tip)
    yt = _r(c[0] + c[1] + need * SQ2 - xp)
    ye = 7.0
    j1c = (2.0, ye)
    xd = _r(xp + yt - ye)  # where the diagonal reaches y 7
    sig = [(xp, yr), (xp, yt), (xd, ye), j1c]
    j1 = Fp("J1", [Pad("1", "SIG", j1c, (1.0, 1.0))], "terminal")
    # A via of another net (OBS) right of the pad's tip, as qmtech's ADV_SDA via beside ADV_AVDD (Q3): it closes the
    # straight line from J1 to the pad's right side, not the axis exit.
    obs = Fp("Q1", [Pad("1", None, (7.4, 5.4), (0.8, 0.8), "circle")], "obstacle")
    return Pattern(
        name="pad_axis_entry", title="Into a long pad of a row along its axis, turning once clear of the neighbour pin",
        kind="solvable",
        gap_classes=["draw-via-neck (3): the end leg along the pad axis ~1 trace radius past the tip, then the turn; no leg "
                     "along the pad's side (octi Exit / pad-snap price; the ERBS stretch end)"],
        purpose="Two layers, B.Cu closed. SIG (0.25) from J1 (2, 7) to pin 4 of a 0.5 mm pitch row of 0.3 x 1.6 pads (centres "
                f"y {yr}, tips y {tip}), as the ADV7511 row of qmtech Q3. A round 0.8 mm pad of no net at (7.4, 5.4) sits "
                "below-right of the tip, like the ADV_SDA via in Q3.",
        answer=f"SIG straight down the pad axis to y {yt} ({_r(yt - tip)} mm past the tip, about one trace radius: there the 45 "
               f"deg diagonal clears pin 3's tip corner by 0.2), 45 deg down-left to ({xd}, {ye}), then straight to J1 "
               f"({_len(sig):.3f} mm). No leg along the pad's side, no turn inside the row, full width.",
        why="Solvable by construction: the axis leg and the diagonal are tangent to the clearance round pin 3's corner; any "
            "earlier turn breaks it, any later one is longer.",
        size=(W, H), layers=2, nets=["SIG"], fps=[u1, j1, obs], connections=[("SIG", "U1.4", "J1.1")], cuts=[],
        keepouts=[Keepout(["B.Cu"], [(0.0, 0.0), (W, 0.0), (W, H), (0.0, H)])],
        classes=[dict(DEFAULT_CLASS)], ref_tracks=[Track("SIG", "F.Cu", W_SIG, sig)], ref_vias=[],
        witness={"SIG": ("F.Cu", sig)},
        extra={"dvn": {"ref_len": _r(_len(sig)), "vias": 0, "turn_at": [xp, yt], "row_tip_y": tip,
                       "straight_past_tip": _r(yt - tip),
                       "literature": "TopoR 6.3 manual p. 71 ('Don't stretch wire to polygonal pad origin'); user "
                                     "2026-11-23 (straight out ~1 trace radius, then turn; SMD-FANOUT-STAIRCASE)"}})


PATTERNS = {f.__name__: f for f in (via_jam_shift, via_interval_sliver, neck_in_pad_row, neck_shared_channel, pad_axis_entry)}


def generate(name: str, out: Path, *, drc: bool = False) -> dict:
    pat = PATTERNS[name]()
    bad = clearance_report(pat)
    if bad:
        raise SystemExit(f"{name}: reference breaks a clearance: {bad}")
    d = Path(out) / name
    paths = {"problem": write_board(pat, d / "problem" / f"{name}.kicad_pcb", copper=False),
             "reference": write_board(pat, d / "reference" / f"{name}.kicad_pcb", copper=True)}
    doc = {"name": pat.name, "title": pat.title, "status": STATUS, "purpose": pat.purpose, "expected": pat.answer, "why": pat.why,
           "connections": [{"net": n, "from": a, "to": b} for n, a, b in pat.connections],
           "reference_tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": t.pts} for t in pat.ref_tracks],
           "reference_vias": [list(v.pos) for v in pat.ref_vias], **pat.extra.get("dvn", {})}
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
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "dvn-patterns" / "boards"
    for name in PATTERNS:
        print(name, *(str(p) for p in generate(name, out, drc=drc).values()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
