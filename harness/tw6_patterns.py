"""PROPOSED test patterns for TW6 VIA-SITES (docs/dev/design/tw6-via-sites.md, section "Tests").

NOT in the test suite: the user approves patterns first (AGENT_RULES). Built like harness/tw5_patterns.py (reference copper
first, pads around it, a shapely clearance check, KiCad DRC with --drc). Two layers, net class 0.25 / 0.2, via 0.6 / 0.3.

1. `site_or_stay`   a crossing whose via has no room (inside a fine-pitch pad channel) against a 0-via path round the
                    pin row. The plan must price "no via site" (TW6 part A/B) and stay on its layer.
2. `lead_via_end`   a dangling hand track continued by a lead; the lead's track-only end lies between two pads (a wire
                    passes, a via does not) and the target is on B.Cu. The via goes where it fits on the lead (the
                    lead's via end, `KeptEndOut::via_end`, TW6 part C), not to the track-only end.

    python -m harness.tw6_patterns [OUT] [--drc]
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from harness.fit_patterns import CLEAR, DEFAULT_CLASS, QFN_L, QFN_P, QFN_W, W_SIG, Fp, Keepout, Pad, Pattern, Track, Via, write_board
from harness.fx_survivors import clearance_report
from harness.radii_patterns import EPS

STATUS = "PROPOSED (awaiting approval)"
SMD = (1.0, 0.7)
VIA_D = 0.6


def _len(pts) -> float:
    return sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))


def _r(x: float) -> float:
    return round(x, 4)


# ------------------------------------------------------------------------------------------------ 1. site_or_stay
def site_or_stay() -> Pattern:
    W, H, Yc = 16.0, 12.0, 6.0
    gap = 0.8  # channel between the two rows' pad edges: a wire needs 0.65 (fits), a via 1.0 (does not)
    y1, y2 = Yc - gap / 2 - QFN_L / 2, Yc + gap / 2 + QFN_L / 2  # row centres 5.2, 6.8
    xs = [3.5 + i * QFN_P for i in range(19)]  # 3.5 .. 12.5
    xb = 8.0  # BLK's two pads face each other across the channel
    u1 = Fp("U1", [Pad(str(i + 1), "BLK" if x == xb else None, (x, y1), (QFN_W, QFN_L)) for i, x in enumerate(xs)], "wall")
    u2 = Fp("U2", [Pad(str(i + 21), "BLK" if x == xb else None, (x, y2), (QFN_W, QFN_L)) for i, x in enumerate(xs)], "wall")
    ib = xs.index(xb)
    p1, p2 = (1.5, Yc), (14.5, Yc)
    j1, j2 = Fp("J1", [Pad("1", "SIG", p1, SMD)], "terminal"), Fp("J2", [Pad("1", "SIG", p2, SMD)], "terminal")
    # B.Cu outside x 3.4 .. 12.6 is taken (rule area: tracks and vias), modelling other B.Cu copper next to the ICs: a
    # B.Cu dive of SIG has its vias between the strips, i.e. in the channel (no room) or above / below the rows.
    strips = [Keepout(["B.Cu"], [(2.6, 0.0), (3.4, 0.0), (3.4, H), (2.6, H)]),
              Keepout(["B.Cu"], [(12.6, 0.0), (13.4, 0.0), (13.4, H), (12.6, H)])]
    top = y1 - QFN_L / 2  # 4.8
    half = W_SIG / 2 + CLEAR + EPS  # wire centre to foreign copper
    yd = _r(top - half)  # the detour's line above U1
    # 45 deg legs tangent to the clearance round the rows' outer top corners (xs[0] - QFN_W/2, top) and its mirror,
    # leaving / entering the SMD pads where those lines cross y 6 (inside the pads: copper there is the pad's own)
    cl, cr = xs[0] - QFN_W / 2, xs[-1] + QFN_W / 2
    s_l = cl + top - half * math.sqrt(2)  # x + y on the left leg
    s_r = cr - top + half * math.sqrt(2)  # x - y on the right leg
    e1, e2 = (_r(s_l - Yc), Yc), (_r(s_r + Yc), Yc)
    assert p1[0] <= e1[0] <= p1[0] + SMD[0] / 2 and p2[0] - SMD[0] / 2 <= e2[0] <= p2[0]
    a, b = _r(s_l - yd), _r(s_r + yd)
    sig = [e1, (a, yd), (b, yd), e2]
    blk = [(xb, y1), (xb, y2)]
    straight = _len([p1, p2])
    # the via room in the channel: centre to pad edge gap/2, a via needs VIA_D/2 + CLEAR
    room = {"channel_half_width": gap / 2, "via_needs": VIA_D / 2 + CLEAR, "wire_needs": W_SIG / 2 + CLEAR,
            "row_end_window_left": [_r(3.4 + VIA_D / 2), _r(xs[0] - QFN_W / 2 - VIA_D / 2 - CLEAR)],
            "row_end_window_right": [_r(xs[-1] + QFN_W / 2 + VIA_D / 2 + CLEAR), _r(12.6 - VIA_D / 2)]}
    return Pattern(
        name="site_or_stay", title="A crossing whose via has no room, against a 0-via path round the pin row", kind="solvable",
        gap_classes=["TW6 A/B: 'no via site' is a graded price in S; the 0-via alternative is priced against it"],
        purpose="Two layers. Two fine-pitch pad rows U1 (y 5.2) and U2 (y 6.8), 0.30 x 0.80 pads at 0.5 mm pitch, leave a "
                "0.8 mm channel along y 6 (a 0.25 wire needs 0.65 and fits; a 0.6 via needs 1.0 and does not). BLK joins the "
                "two facing pads at x 8 straight across the channel. SIG (SMD J1 (1.5, 6) to J2 (14.5, 6), F.Cu) has its "
                "straight line through the channel: it crosses BLK, so the straight way needs a B.Cu dive whose two vias sit "
                "in the channel (no room). B.Cu outside x 3.4 .. 12.6 is a rule area (other B.Cu copper), so no via fits at "
                "the rows' ends either (the left window would be x 3.7 .. 2.85, empty). The class of the failure: qmtech "
                "via-site-forced connections (ADV_SCL, ADV_DDCSCL: a crossing next to a fine-pitch row).",
        answer=f"SIG on F.Cu round U1, octilinear and tight: from J1.1 at x {e1[0]} 45 deg up to y {yd} (the legs touch the "
               f"clearance round the rows' outer top corners), along it, 45 deg down into J2.1 at x {e2[0]} ({_len(sig):.3f} mm; the "
               f"straight line, {straight:.1f} mm centre to centre, would cross BLK). BLK straight across the channel (1.6 mm). "
               "0 vias, DRC 0. Mirror (round U2, below) is equally optimal.",
        why="Every B.Cu dive of SIG needs its vias outside the channel and between the strips: above U1 or below U2, i.e. SIG "
            "first leaves the channel on F.Cu, which is already the detour; the dive then adds 2 vias to the same length. "
            "Straight through the channel crosses BLK on F.Cu. So the shortest legal answer has 0 vias and is the detour. "
            "Today's flat crossing price in the search sees the straight crossing + 2 vias, not that the vias have no site.",
        size=(W, H), layers=2, nets=["SIG", "BLK"], fps=[j1, j2, u1, u2],
        connections=[("SIG", "J1.1", "J2.1"), ("BLK", f"U1.{ib + 1}", f"U2.{ib + 21}")], cuts=[], keepouts=strips,
        classes=[dict(DEFAULT_CLASS)], ref_tracks=[Track("SIG", "F.Cu", W_SIG, sig), Track("BLK", "F.Cu", W_SIG, blk)], ref_vias=[],
        witness={"SIG": ("F.Cu", sig), "BLK": ("F.Cu", blk)},
        extra={"tw6": {"ref_len": _r(_len(sig) + _len(blk)), "vias": 0, "straight_len": straight, "room": room,
                       "style": "octilinear (the engine's drawing); the track ends inside the pads where the legs cross y 6",
                       "no_site_marks": [[xb - 0.6, Yc], [xb + 0.6, Yc]],
                       "also_optimal": "SIG round U2 (mirror, below)",
                       "literature": "Luzin-Popov 2013 p. 133 (S = sum k_i x_i: the via and its site are terms of S); "
                                     "Dayan & Dai 1993 p. 13 Table 1 (via cost per point in the layering shortest path)"}})


# ------------------------------------------------------------------------------------------------ 2. lead_via_end
def lead_via_end() -> Pattern:
    W, H, Y = 14.0, 12.0, 6.0
    end, lead = (8.0, Y), 1.5
    xo, half = 9.5, 0.3  # the obstacle pads (no net), 0.6 x 0.6, centres y 5.25 and 6.75: 0.9 mm between their edges
    o_y = (Y - 0.75, Y + 0.75)
    r1 = Fp("R1", [Pad("1", "SIG", (3.0, Y), (1.0, 1.0))], "terminal")
    r2 = Fp("R2", [Pad("1", "SIG", (12.5, Y), (1.0, 1.0), layer="B.Cu")], "terminal")
    ob = Fp("Q1", [Pad("1", None, (xo, o_y[0]), (2 * half, 2 * half)), Pad("2", None, (xo, o_y[1]), (2 * half, 2 * half))], "obstacle")
    hand = Track("SIG", "F.Cu", W_SIG, [(3.0, Y), end])
    # the via room on y = Y: the pads' near corners are (xo - half, Y -+ 0.45); a via needs VIA_D/2 + CLEAR from them
    need = VIA_D / 2 + CLEAR + 1e-5  # the engine's 10 nm margin: the via end sits on the room limit
    dy = (o_y[1] - half) - Y  # 0.45
    xv = _r(xo - half - math.sqrt(need * need - dy * dy))  # the lead's via end (last point with room)
    xv2 = _r(xo + half + math.sqrt(need * need - dy * dy))  # first point with room past the pads
    lim = VIA_D / 2 + CLEAR  # the exact room limits (the drawing's via end sits on them, 8.982)
    ok_x = [[8.0, round(xo - half - math.sqrt(lim * lim - dy * dy) + 5e-4, 3)], [round(xo + half + math.sqrt(lim * lim - dy * dy) - 5e-4, 3), 12.5]]
    tip = (end[0] + lead, Y)
    sig_f = [end, (xv, Y)]
    sig_b = [(xv, Y), r2.pads[0].pos]
    return Pattern(
        name="lead_via_end", title="Dangling lead: the via goes where it fits on the lead, not to its track-only end", kind="solvable",
        gap_classes=["TW6 C: the plan knows a dangling lead's via end (lead_len / KeptEndOut::via_end) and prices it as a site"],
        purpose=f"Two layers. A hand-started F.Cu track (SIG, 0.25) from R1.1 (3, 6) ends at nothing at (8, 6), heading +x. The "
                f"lead that continues it ([search] dangling_ends = \"continue\", dangling_lead_mm = {lead}) reaches (9.5, 6), "
                "between the two pads of Q1 (no net, 0.6 x 0.6 at (9.5, 5.25) and (9.5, 6.75), 0.9 mm between their edges: "
                "a wire fits, a via does not). The target R2.1 is an SMD pad on B.Cu at (12.5, 6): SIG needs one via. "
                f"Where it fits on the lead: x <= {xv} (the lead's via end); past Q1: x >= {xv2}; at the track-only lead end "
                "(9.5, 6): no room (0.45 to the pads, 0.5 needed).",
        answer=f"SIG continues F.Cu from (8, 6) to a via at the lead's via end ({xv}, 6), then B.Cu straight to R2.1. 1 via, DRC "
               f"0, new copper {_len(sig_f) + _len(sig_b):.2f} mm. Equally short: the via anywhere on y 6 with room "
               f"(x 8 .. {xv}, or x >= {xv2} with SIG on F.Cu through Q1's gap). The failure it tests: the plan changes "
               "layer at the track-only lead end (9.5, 6), a via without a site (a clearance error, or a forced via).",
        why="Solvable by construction: the straight line from the hand end to R2.1 is free on both layers except for the "
            "via's room between Q1's pads; R2.1 is on B.Cu only, so one via is the minimum; every point on y 6 gives the "
            "same length; the lead's via end is the point the drawing takes under [search] dangling_lead_via (default on).",
        size=(W, H), layers=2, nets=["SIG"], fps=[r1, r2, ob],
        connections=[("SIG", "R1.1", "R2.1")], cuts=[],
        classes=[dict(DEFAULT_CLASS)],
        ref_tracks=[Track("SIG", "F.Cu", W_SIG, sig_f), Track("SIG", "B.Cu", W_SIG, sig_b)], ref_vias=[Via("SIG", (xv, Y))],
        witness={"SIG": ("F.Cu", sig_f)},
        kept_tracks=[hand],
        extra={"tw6": {"ref_len": _r(_len(hand.pts) + _len(sig_f) + _len(sig_b)), "vias": 1, "via_end": [xv, Y],
                       "via_ok_x": ok_x, "tip": list(tip), "no_site_marks": [list(tip)],
                       "options": {"search.dangling_ends": "continue", "search.dangling_lead_mm": lead,
                                   "search.dangling_lead_via": True},
                       "problem_drc_allowed": ["track_dangling"],
                       "literature": "lead finding exist-term r4 (fc3539b0, plan 2026-11-18 lead line); TopoR manual "
                                     "p. 179 (F7 Shift Vias: vias moved to legal positions)"}})


PATTERNS = {"site_or_stay": site_or_stay, "lead_via_end": lead_via_end}


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
           "kept_tracks": [{"net": t.net, "layer": t.layer, "width": t.width, "pts": t.pts} for t in pat.kept_tracks],
           "reference_vias": [list(v.pos) for v in pat.ref_vias], **pat.extra.get("tw6", {})}
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
    out = Path(argv[0]) if argv else Path(__file__).resolve().parents[1] / "scratch" / "tw6-patterns" / "boards"
    for name in PATTERNS:
        print(name, *(str(p) for p in generate(name, out, drc=drc).values()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
