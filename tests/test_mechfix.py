"""Mechanism fixtures (harness/mechfix.py) routed end to end: strip, topo core, KiCad DRC with refill.

Marker `mechfix` (deselected by default, seconds per case): run with
    TOPOR_ENGINE_SITE=<own wheel site> .venv/bin/python -m pytest tests/test_mechfix.py -m mechfix
`TOPOR_FX_OUT=<dir>` keeps each case's run (and its compare PNG) under <dir>/<case>/.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from harness import mechfix

ROOT = Path(__file__).resolve().parents[1]

pytestmark = pytest.mark.mechfix


def _route(case: str, tmp_path: Path, *extra: str, tag: str = "") -> dict:
    src = getattr(mechfix, case)(tmp_path / "src")
    out = Path(os.environ["TOPOR_FX_OUT"]) / (case + tag) if os.environ.get("TOPOR_FX_OUT") else tmp_path / ("out" + tag)
    env = {**os.environ, "RAYON_NUM_THREADS": os.environ.get("RAYON_NUM_THREADS", "2"), "TOPOR_HAND_LABEL": "unrouted input"}
    r = subprocess.run([str(ROOT / "tools" / "route_board.sh"), str(src), str(out), "--timeout", "60", "--name", case, *extra],
                       env=env, capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stdout[-2000:] + r.stderr[-3000:]
    m = json.loads((out / f"{case}--metrics.json").read_text())
    m["_out"] = str(out)
    return m


def _clean(m: dict) -> None:
    assert m["routed"] == m["connections"], m
    assert m["unconnected"] == 0, m
    assert m["routing_new"] == 0, m


def _result(m: dict) -> dict:
    return json.loads((Path(m["report_dir"]) / "result.json").read_text())["route"]


def _first_round(m: dict) -> None:
    """Round 1 drew every connection: no rescue by the topology feedback (a later round's
    lowered rooms reroute around what the planner did not see, usually as a detour)."""
    picked = [v for v in _result(m)["variants"] if v.get("picked")]
    r1 = [v for v in _result(m)["variants"] if v["label"].startswith("round 1")]
    assert r1 and min(v["errors"] for v in r1) == 0, (picked, r1)


def test_fx_plane_stub_squeeze(tmp_path):
    """The GND stub via keeps out of SIG's corridor and the planner sees it: SIG runs straight,
    drawn in round 1 (HEAD 2bfab0b: via in the corridor, round 1 fails, round 2 detours 1.066)."""
    m = _route("fx_plane_stub_squeeze", tmp_path)
    _clean(m)
    _first_round(m)
    assert m["detour"]["max"] <= 1.02, m["detour"]



def _conns(m: dict, net: str) -> list:
    return [c for c in _result(m)["connections"] if c["name"] == net]


def test_fx_two_sided_overlap(tmp_path):
    """SIG goes on to U from the near pad R (under the front pad M of net K), not from the far
    pad L, and the whole net is about the chain L -> R -> U long (HEAD 2bfab0b: from L, 40.3 mm)."""
    m = _route("fx_two_sided_overlap", tmp_path)
    _clean(m)
    sig = _conns(m, "SIG")
    to_u = [c for c in sig if "U1.1" in (c["from"]["label"], c["to"]["label"])]
    assert len(to_u) == 1 and "R1.1" in (to_u[0]["from"]["label"], to_u[0]["to"]["label"]), to_u
    chain = 6.84 + ((35.25 - 15.37) ** 2 + (17.14 - 3.55) ** 2) ** 0.5
    # 1.15, not 1.0: the drawing's taut path still bends at Steiner points of the sleeve P2 chose
    # (D6a: 34.4 mm, 1.11 x the chain; a separate mechanism).
    assert sum(c["length_nm"] for c in sig) / 1e6 <= 1.15 * chain, [(c["from"]["label"], c["to"]["label"], c["length_nm"]) for c in sig]

def test_fx_copper_graphic_corridor(tmp_path):
    """The planner sees the copper graphic: SIG is drawn in round 1 (HEAD 2bfab0b: planned
    through it, the drawing rejects the run)."""
    m = _route("fx_copper_graphic_corridor", tmp_path)
    _clean(m)
    _first_round(m)

def test_fx_keepout_no_via_site(tmp_path):
    """The crossing's vias keep out of the no-via keepout; everything drawn in round 1."""
    m = _route("fx_keepout_no_via_site", tmp_path)
    _clean(m)
    _first_round(m)
def test_fx_keepout_no_via_inner(tmp_path):
    """Through vias keep out of a no-via keepout on In1.Cu alone (4 layers)."""
    _clean(_route("fx_keepout_no_via_inner", tmp_path))


def test_fx_pinned_crossing(tmp_path):
    """A and B, both held to F.Cu over the foreign B.Cu pad K, do not cross over K (D6a: they
    crossed there, "crossing cannot be layered", one dropped)."""
    _clean(_route("fx_pinned_crossing", tmp_path))


def test_fx_local_pour_far_pads(tmp_path):
    """Every pad of P is joined to the pour's copper: the far pads by traces, no via standing
    outside the pour (D6a: 4 stub vias in bare board, 4 unconnected)."""
    _clean(_route("fx_local_pour_far_pads", tmp_path))


def test_fx_same_layer_pour_stub(tmp_path):
    """P joins the B.Cu pour by a B.Cu track alone (no via, no signal connection to Q)."""
    m = _route("fx_same_layer_pour_stub", tmp_path)
    _clean(m)
    gnd = _conns(m, "GND")
    assert len(gnd) == 1 and gnd[0]["to"]["kind"] == "pour" and gnd[0]["vias"] == 0, gnd


def test_fx_coincident_pad_stub(tmp_path):
    """The pad pair joins the B.Cu pour by one stub and via (no signal connection to Q)."""
    m = _route("fx_coincident_pad_stub", tmp_path)
    _clean(m)
    gnd = _conns(m, "GND")
    assert len(gnd) == 1 and gnd[0]["to"]["kind"] == "pour" and gnd[0]["vias"] == 1, gnd


def test_fx_corner_wrap(tmp_path):
    """No out-and-back spike: no track is the exact reverse of another track of its net on its
    layer (D6b: SIG had one at (11.55, 16.63))."""
    m = _route("fx_corner_wrap", tmp_path)
    _clean(m)
    import pcbnew
    b = pcbnew.LoadBoard(str(next(Path(m["_out"]).glob("fx_corner_wrap-topor/*.kicad_pcb"))))
    segs = {}
    for t in b.GetTracks():
        if t.Type() != pcbnew.PCB_VIA_T:
            s, e = t.GetStart(), t.GetEnd()
            segs[(s.x, s.y, e.x, e.y, t.GetLayer(), t.GetNetCode())] = t
    spikes = [k for k in segs if (k[2], k[3], k[0], k[1], k[4], k[5]) in segs]
    assert not spikes, spikes


def test_fx_narrow_gap_forced(tmp_path):
    """Never drop (M-B1): the only way is a gap too narrow for the track. The connection is drawn
    through it anyway; KiCad's DRC sees it connected, with the clearance violations the engine
    reports (place, type, cause). With [optimize] allow_violations = false it stays unrouted and
    nothing breaks a rule (the old legal-only output)."""
    m = _route("fx_narrow_gap_forced", tmp_path)
    assert m["routed"] == m["connections"] == 1, m
    assert m["unconnected"] == 0, m
    assert m["routing_new"] >= 1 and set(m["drc_by_type_new"]) <= {"clearance"}, m
    viol = _result(m)["violations"]
    assert viol and all(v["kind"] == "clearance" and v["cause"] for v in viol), viol
    assert all(abs(v["at"][0] / 1e6 - 15.0) < 1.0 and abs(v["at"][1] / 1e6 - 10.0) < 1.0 for v in viol), viol
    # The least violating geometry: no copper overlap (a clearance violation, not a short); the
    # pick counts violations first (one against one wall pad beats one against each).
    assert all(0 < v["depth_nm"] < 200_000 for v in viol), viol
    legal = _route("fx_narrow_gap_forced", tmp_path, "--set", "optimize.allow_violations=false", tag="-legal")
    assert legal["routed"] == 0 and legal["routing_new"] == 0, legal
    assert not _result(legal)["violations"], _result(legal)["violations"]


def test_fx_terminal_clinch(tmp_path):
    """Crossing rules (M-B2): B no longer wraps A's terminal A1: A runs straight to A2 and B
    passes A1 on the south (before: A went around B's pad B2, detour 1.55, Luzin & Popov 2012
    Fig. 3a)."""
    m = _route("fx_terminal_clinch", tmp_path)
    _clean(m)
    assert m["vias"] == 0, m
    assert m["detour"]["max"] <= 1.35, m["detour"]


def test_fx_terminal_clinch_no_via(tmp_path):
    """The clinch where A cannot go around B and no via site anywhere near: legal copper, no
    violation (before: A crossed B on one layer, drawn through it as a short, never drop)."""
    m = _route("fx_terminal_clinch_no_via", tmp_path)
    _clean(m)
    assert not _result(m)["violations"], _result(m)["violations"]



def test_fx_neck_bottleneck(tmp_path):
    """Width in P2 (M-C): the plan necks the wide net from a pad end through the 0.7 mm gap, the
    drawing realises it: legal copper, one necked stretch reported (before M-C: drawn through the
    gap at full width as a clearance violation)."""
    m = _route("fx_neck_bottleneck", tmp_path)
    _clean(m)
    r = _result(m)
    assert not r["violations"], r["violations"]
    c = r["connections"][0]
    assert c["neck_nm"] > 0 and 200_000 <= c["neck_width_nm"] < 400_000, c
    assert m["detour"]["max"] <= 1.05, m["detour"]


def test_fx_fine_pitch_exit(tmp_path):
    """Pad exits on fine pitch (M-C, M-45): every wire leaves its pad through the tip along the
    pad's axis, no neighbour pad clipped, no forced copper, every segment at 0/45/90 degrees. The
    P2 side of it (no exit through a side gap) is the engine test
    `topo_mc::a_wire_leaves_a_fine_pitch_pad_through_its_tip`. Open since M-C: the taut chords of
    neighbouring 0.5 mm pins turning at the row's tips ran 0.354 mm apart where 0.45 mm is needed;
    the octilinear drawing leaves along the axis and staggers the bends (each wire hugs the one
    drawn before it at the clearance)."""
    m = _route("fx_fine_pitch_exit", tmp_path)
    _clean(m)
    assert not _result(m)["violations"], _result(m)["violations"]
    tr = _tracks(m, "fx_fine_pitch_exit")
    assert all(_off45(t) < 1e-3 for t in tr), [t for t in tr if _off45(t) >= 1e-3]
    # The segment at each connected pin's centre runs along the pin (horizontal) past its tip.
    for i in range(1, 6):
        y = 9.5 + 0.5 * i
        first = [t for t in tr if any(abs(p[0] - 11.0) < 1e-3 and abs(p[1] - y) < 1e-3 for p in (t[2], t[3]))]
        assert first, (i, tr)
        for t in first:
            assert abs(t[2][1] - t[3][1]) < 1e-4, t
            assert min(t[2][0], t[3][0]) <= 10.2 + 1e-3 or max(t[2][0], t[3][0]) >= 11.8 - 1e-3, t


def _tracks(m: dict, case: str) -> list:
    """Straight tracks of the routed board: `(net, layer, (x, y) a, (x, y) b, width)` in mm."""
    import pcbnew

    b = pcbnew.LoadBoard(str(Path(m["_out"]) / f"{case}-topor" / f"{case}.kicad_pcb"))
    out = []
    for t in b.GetTracks():
        if t.GetClass() == "PCB_TRACK":
            a, c = t.GetStart(), t.GetEnd()
            out.append((t.GetNetname(), t.GetLayer(), (a.x / 1e6, a.y / 1e6), (c.x / 1e6, c.y / 1e6), t.GetWidth() / 1e6))
    return out


def _off45(t) -> float:
    """Perpendicular deviation (mm) of a track's far end from the nearest 0/45/90 degree line
    through its start."""
    import math

    dx, dy = t[3][0] - t[2][0], t[3][1] - t[2][1]
    best = float("inf")
    for k in range(8):
        a = k * math.pi / 4
        best = min(best, abs(dx * math.sin(a) - dy * math.cos(a)))
    return best


def _turns(tr: list, net: str) -> list:
    """Turn angles (degrees) along the chain of `net`'s tracks, in path order (one chain)."""
    import math

    segs = [(t[2], t[3]) for t in tr if t[0] == net and math.dist(t[2], t[3]) > 1e-4]
    key = lambda p: (round(p[0], 4), round(p[1], 4))  # noqa: E731
    deg: dict = {}
    for a, b in segs:
        deg[key(a)] = deg.get(key(a), 0) + 1
        deg[key(b)] = deg.get(key(b), 0) + 1
    start = next(p for s in segs for p in s if deg[key(p)] == 1)
    chain, cur, left = [], start, list(segs)
    while left:
        s = next((s for s in left if key(s[0]) == key(cur) or key(s[1]) == key(cur)), None)
        if s is None:
            break
        left.remove(s)
        nxt = s[1] if key(s[0]) == key(cur) else s[0]
        chain.append((cur, nxt))
        cur = nxt
    out = []
    for (a, b), (_, c) in zip(chain, chain[1:]):
        u, v = (b[0] - a[0], b[1] - a[1]), (c[0] - b[0], c[1] - b[1])
        cos = (u[0] * v[0] + u[1] * v[1]) / (math.hypot(*u) * math.hypot(*v))
        t = math.degrees(math.acos(max(-1.0, min(1.0, cos))))
        if t > 0.05:
            out.append(t)
    return out


def _len(tr: list, net: str) -> float:
    import math

    return sum(math.dist(t[2], t[3]) for t in tr if t[0] == net)


def test_fx_octi_straight(tmp_path):
    """Octilinear (M-45): a nearly straight run stays straight with one short 45 degree jog, no
    longer than 1 % over the straight distance (before: one segment at 1.4 degrees)."""
    m = _route("fx_octi_straight", tmp_path)
    _clean(m)
    tr = _tracks(m, "fx_octi_straight")
    assert all(_off45(t) < 1e-3 for t in tr), tr
    # The elbow: 23.4 mm straight + 0.6 sqrt(2) mm at 45 degrees (the shortest octilinear path).
    assert _len(tr, "SIG") <= 23.4 + 0.6 * 2 ** 0.5 + 0.01, _len(tr, "SIG")
    assert len(_turns(tr, "SIG")) <= 2, _turns(tr, "SIG")


def test_fx_octi_bend(tmp_path):
    """Octilinear (M-45): on a free board the run is an elbow, one 45 degree bend, 10 + 7 sqrt(2)
    mm long (before: one segment at 22.4 degrees)."""
    m = _route("fx_octi_bend", tmp_path)
    _clean(m)
    tr = _tracks(m, "fx_octi_bend")
    assert all(_off45(t) < 1e-3 for t in tr), tr
    t = _turns(tr, "SIG")
    assert len(t) == 1 and abs(t[0] - 45.0) < 0.05, t
    assert abs(_len(tr, "SIG") - (10.0 + 7.0 * 2 ** 0.5)) < 0.05, _len(tr, "SIG")


def test_fx_octi_bundle(tmp_path):
    """Octilinear (M-45): wires around a block keep a constant pitch through their bends: every
    wire at 0/45/90 degrees, each of the inner wires follows the one inside it at the clearance
    (`[drawing] bundle_pull`), so their 45 degree stretches are parallel at width + clearance, and
    the report counts the bends taken together with their skew (before M-45: taut chords at any
    angle; with the octilinear drawing but no pull, each wire took its own shortest staircase and
    the diagonals were 0.5 to 2 mm apart)."""
    import math

    m = _route("fx_octi_bundle", tmp_path)
    _clean(m)
    assert not _result(m)["violations"], _result(m)["violations"]
    tr = _tracks(m, "fx_octi_bundle")
    assert all(_off45(t) < 1e-3 for t in tr), [t for t in tr if _off45(t) >= 1e-3]

    def diag_offset(net):
        # Distance from the origin of the 45-degree line (lower left to upper right) of the wire's
        # bend around the block's corner (12, 12): its 45-degree segment nearest that corner.
        ds = [t for t in tr if t[0] == net and abs(abs(t[3][0] - t[2][0]) - abs(t[3][1] - t[2][1])) < 1e-3
              and (t[3][0] - t[2][0]) * (t[3][1] - t[2][1]) < 0]
        assert ds, (net, [t for t in tr if t[0] == net])
        d = min(ds, key=lambda t: math.dist(((t[2][0] + t[3][0]) / 2, (t[2][1] + t[3][1]) / 2), (12.0, 12.0)))
        return (d[2][0] + d[2][1]) / math.sqrt(2)

    # The inner three follow each other at the clearance (0.25 mm track + 0.2 mm: 0.45 mm centre
    # to centre); the outer one may keep its own lane (the plan spreads it over a wide passage).
    offs = [diag_offset(f"B{i}") for i in range(3)]
    gaps = [abs(b - a) for a, b in zip(offs, offs[1:])]
    assert all(abs(g - 0.45) < 0.01 for g in gaps), (offs, gaps)
    bundles = [w for w in _result(m)["warnings"] if w.startswith("bundles (octilinear)")]
    assert bundles and int(bundles[0].split(": ")[1].split()[0]) >= 3, bundles


def test_fx_octi_no_fit(tmp_path):
    """Octilinear (M-45) where 45 degrees cannot fit: through the 30 degree channel the stretch
    stays any-angle and is reported (any_angle warning with its place and angle); the connection
    is routed without a violation."""
    m = _route("fx_octi_no_fit", tmp_path)
    _clean(m)
    r = _result(m)
    assert not r["violations"], r["violations"]
    assert any("any_angle" in w or "not at 0/45/90 degrees (" in w for w in r["warnings"]), r["warnings"]
    tr = _tracks(m, "fx_octi_no_fit")
    off = [t for t in tr if _off45(t) >= 1e-3]
    assert off and all(5.9 <= min(t[2][0], t[3][0]) and max(t[2][0], t[3][0]) <= 24.1 for t in off), off


# ---------------------------------------------------------------- D7 differential pairs
def _dp(m: dict) -> list:
    return _result(m).get("diff_pairs") or []


def _dp_clean(m: dict, n_pairs: int, coupled_min: float) -> list:
    _clean(m)
    r = _result(m)
    assert not r["violations"], r["violations"]
    dps = _dp(m)
    assert len(dps) == n_pairs, dps
    for d in dps:
        assert d["coupled_pct"] >= coupled_min and not d["polarity_cross"], d
    return dps


def test_fx_dp_straight(tmp_path):
    """D7: a straight pair is drawn as two coupled tracks at the gap, symmetric about the pad pair (skew 0), with
    short fan-out stubs (before D7: DIFF_PAIRS_SKIPPED, both connections unrouted)."""
    m = _route("fx_dp_straight", tmp_path)
    (d,) = _dp_clean(m, 1, 95.0)
    assert d["gap_held"] and abs(d["skew_nm"]) <= 10_000, d
    assert max(d["uncoupled_start_nm"] + d["uncoupled_end_nm"]) <= 500_000, d


def test_fx_dp_bend(tmp_path):
    """D7: a pair through a 45 degree elbow stays coupled; the measured skew is the bend estimate (2 p tan 22.5 per
    bend, opposite bends cancel)."""
    m = _route("fx_dp_bend", tmp_path)
    (d,) = _dp_clean(m, 1, 90.0)
    assert abs(d["skew_nm"] - d["skew_estimate_nm"]) <= 50_000, d


def test_fx_dp_via(tmp_path):
    """D7: a pair that must change layer does so with via pairs (two vias side by side each time, one per half),
    no unpaired via, no violation."""
    m = _route("fx_dp_via", tmp_path)
    (d,) = _dp_clean(m, 1, 70.0)
    assert d["vias_p"] == d["vias_n"] == 2 and m["vias"] == 4, (d, m["vias"])


def test_fx_dp_qfn_fanout(tmp_path):
    """D7: a pair leaving 0.5 mm pitch QFN pins fans out through the pad tips (the uncoupled stretch at the pads)
    and zips at the gap past the pins; the neighbour pins' nets are routed too, nothing violates."""
    m = _route("fx_dp_qfn_fanout", tmp_path)
    (d,) = _dp_clean(m, 1, 70.0)
    assert max(d["uncoupled_start_nm"]) <= 2_500_000, d


def test_fx_dp_narrow(tmp_path):
    """D7, never drop: a pair through a passage too narrow for it is routed (both halves have copper); what does not
    fit is priced and reported, not left open."""
    m = _route("fx_dp_narrow", tmp_path)
    assert m["routed"] == m["connections"] == 2 and m["unconnected"] == 0, m
    assert len(_dp(m)) == 1


def test_fx_dp_two_pairs(tmp_path):
    """D7: two pairs side by side through a bend around a block: both coupled, polarity kept, clear of each other."""
    m = _route("fx_dp_two_pairs", tmp_path)
    _dp_clean(m, 2, 80.0)


def test_fx_dp_no_partner(tmp_path):
    """D7: KiCad's name rule: EN and SIG_P1 have no partner net and route as normal nets; RX_P1 / RX_N1 (digits after
    the polarity) are one pair."""
    m = _route("fx_dp_no_partner", tmp_path)
    (d,) = _dp_clean(m, 1, 95.0)
    assert (d["p_name"], d["n_name"]) == ("RX_P1", "RX_N1"), d


if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-m", "mechfix", "-q"]))


# ---- Inflated sketch U1 (docs/dev/inflated-sketch.md §6.2): what the per-layer cut test sees on the boardlets.

def _cuts(case: str, tmp_path: Path) -> tuple[dict, dict]:
    """Route `case` with the cut report on; the report's plan handed to the drawing (picked run and cycle)."""
    from harness.cutclass import picked
    rep_dir = tmp_path / "cuts"
    m = _route(case, tmp_path, "--set", f"capacity.cut_report={rep_dir}", "--set", "optimize.runs=1")
    res = json.loads((Path(m["report_dir"]) / "result.json").read_text())
    k, c = picked(res)
    rep = json.loads((rep_dir / f"run{k}" / f"cycle{c}.json").read_text())
    return m, rep["final"]


def _layer(fin: dict, name: str) -> list:
    return next(L["cuts"] for L in fin["layers"] if L["layer"] == name)


def _near(pt, xy, tol=0.02) -> bool:
    return abs(pt[0] - xy[0]) <= tol and abs(pt[1] - xy[1]) <= tol


def _cut(cuts: list, a, b) -> list:
    return [c for c in cuts if (_near(c["p"], a) and _near(c["q"], b)) or (_near(c["p"], b) and _near(c["q"], a))]


def test_fx_b1a_corner_bundle_exact_fit(tmp_path):
    """B1a: four tracks through a passage exactly as wide as they need: the cut between the wall
    corners holds all four with slack 0, and no F.Cu cut is over-full."""
    _, fin = _cuts("fx_b1_corner_bundle", tmp_path)
    f = _layer(fin, "F.Cu")
    gap = _cut(f, (15.0, 10.0), (15.0, 8.0))
    assert gap and len(gap[0]["items"]) == 4 and abs(gap[0]["slack"]) <= 0.001, gap
    assert not [c for c in f if c["slack"] < -0.0005], [c for c in f if c["slack"] < -0.0005]


def test_fx_b1b_corner_bundle_one_um_short(tmp_path):
    """B1b: the passage 1 um narrower: exactly one over-full cut, the wall corners', by 1 um."""
    _, fin = _cuts("fx_b1b_corner_bundle", tmp_path)
    f = _layer(fin, "F.Cu")
    over = [c for c in f if c["slack"] < -0.0005]
    assert len(over) == 1 and _cut(over, (15.0, 10.0), (15.0, 8.001)) and -0.0015 <= over[0]["slack"], over


def test_fx_b2_non_neighbour_squeeze(tmp_path):
    """B2: AB, AD, BC and CD fit (slack 0.023 mm), AC (the wall ends, 2.85 mm) holds all six tracks and
    overflows by 0.05 mm. The per-edge model is not asserted (measured U1, it varies by run): it flags
    the diagonal edges AD / BC as over-full through its oblique-crossing factor, and in some runs an
    edge A-C as well."""
    _, fin = _cuts("fx_b2_squeeze", tmp_path)
    f = _layer(fin, "F.Cu")
    ac = [c for c in f if c["p"][0] in (13.575, 16.425) and c["q"][0] in (13.575, 16.425) and abs(c["p"][0] - c["q"][0]) > 2.8]
    assert ac and all(len(c["items"]) == 6 and abs(c["slack"] + 0.05) <= 0.002 for c in ac), ac
    sides = [c for c in f if min(abs(c["p"][0] - 15.0), abs(c["q"][0] - 15.0)) <= 0.06 and abs(c["p"][1] - c["q"][1]) > 0.3]
    assert sides and all(c["slack"] >= -0.0005 for c in sides if len(c["items"]) == 3), sides


def test_fx_b5_other_layer_pad(tmp_path):
    """B5: the F.Cu sheet has the cut between the two F.Cu wall ends across the B.Cu pad, 1.0 mm,
    with the three tracks on it, over-full by 0.55 mm. The projection's edges there end on the B.Cu
    pad, so the per-edge model has no edge between the wall ends; it flags the B.Cu pad's own sides
    (0.4 mm) as over-full F.Cu edge-layers instead (measured U1: the right place, the wrong cut)."""
    _, fin = _cuts("fx_b5_other_layer_pad", tmp_path)
    f = _layer(fin, "F.Cu")
    gap = _cut(f, (14.5, 9.9), (15.5, 9.9)) + _cut(f, (14.5, 10.1), (15.5, 10.1))
    assert gap and all(len(c["items"]) == 3 and abs(c["slack"] + 0.55) <= 0.01 for c in gap), gap
    walls = (14.5, 15.5)
    assert not [e for e in fin["edge_over"] if e["a"][0] in walls and e["b"][0] in walls], fin["edge_over"]


# ---------------------------------------------------------------- cleanup-safe output (KiCad's Cleanup Tracks & Vias)
#: the copper as drawn and the zones as they were: what topor wrote before the cleanup-safe stage
_AS_DRAWN = ("--set", "output.cleanup_safe=false", "--set", "run.commit_fills=false")


def _cleanup_before_after(case: str, tmp_path: Path) -> tuple:
    """The case routed as drawn (old output) and with the defaults; the defaults must leave KiCad's cleanup nothing
    to do (harness.cleanup_check, every option on) and route as cleanly as before."""
    before = _route(case, tmp_path, *_AS_DRAWN, tag="-as-drawn")
    after = _route(case, tmp_path)
    _clean(after)
    assert after["cleanup_unstable"] == 0, after.get("cleanup_summary")
    assert after["routed"] == before["routed"] and after["unconnected"] == before["unconnected"], (before, after)
    return before, after


def _tees_off_vertex(m: dict) -> int:
    """Track ends that stop on the middle of another track of their net and layer (a T that is no shared vertex)."""
    import pcbnew
    pcb = next(iter(sorted((Path(m["_out"]) / f"{m['board']}-topor").glob("*.kicad_pcb"))))
    b = pcbnew.LoadBoard(str(pcb))
    segs = [(t.GetNetCode(), t.GetLayer(), (t.GetStart().x, t.GetStart().y), (t.GetEnd().x, t.GetEnd().y))
            for t in b.GetTracks() if t.Type() == pcbnew.PCB_TRACE_T]
    ends = {}
    for n, l, a, e in segs:
        for p in (a, e):
            ends[(n, l, p)] = ends.get((n, l, p), 0) + 1
    off = 0
    for (n, l, p), k in ends.items():
        if k > 1:
            continue
        for n2, l2, a, e in segs:
            if (n2, l2) != (n, l) or p in (a, e):
                continue
            dx, dy = e[0] - a[0], e[1] - a[1]
            cross = abs(dx * (p[1] - a[1]) - dy * (p[0] - a[0]))
            dot = (p[0] - a[0]) * dx + (p[1] - a[1]) * dy
            if cross <= 2 * max(abs(dx), abs(dy)) and 0 < dot < dx * dx + dy * dy:
                off += 1
                break
    return off


def test_fx_cleanup_in_pad(tmp_path):
    """The 0.17 mm jog inside the round pad (the octilinear diagonal ends short of the centre) is not written: as
    drawn, KiCad's "Delete tracks fully inside pads" removes it; now the trace ends inside the pad (eprom U2)."""
    before, after = _cleanup_before_after("fx_cleanup_in_pad", tmp_path)
    assert before["cleanup_by_rule"].get("track_in_pad", 0) >= 1, before["cleanup_summary"]


def test_fx_cleanup_tee(tmp_path):
    """C joins the A-B trace at (5, 10): as drawn its end stops on the trunk's middle (and the trunk keeps
    collinear pieces KiCad merges); now the trunk is split there, one exact vertex of three track ends."""
    before, after = _cleanup_before_after("fx_cleanup_tee", tmp_path)
    assert _tees_off_vertex(before) >= 1 and before["cleanup_unstable"] >= 1, before["cleanup_summary"]
    assert _tees_off_vertex(after) == 0


def test_fx_cleanup_collinear(tmp_path):
    """The trace along the pad row: as drawn, a vertex at every pad (1 nm apart in y from its neighbours, so
    KiCad's bounding-box merge takes only some of them); now one straight segment along the row."""
    before, after = _cleanup_before_after("fx_cleanup_collinear", tmp_path)
    assert before["cleanup_by_rule"].get("merge_collinear", 0) >= 1, before["cleanup_summary"]


def test_fx_cleanup_stale_fill(tmp_path):
    """The board's stored B.Cu fill is out of date: as drawn (zones untouched), KiCad's cleanup removes the GND
    stub's via (connected on one layer) and then the stub; now Accept writes the refilled zones with the copper."""
    before, after = _cleanup_before_after("fx_cleanup_stale_fill", tmp_path)
    assert before["cleanup_by_rule"].get("dangling_via", 0) >= 1, before["cleanup_summary"]
