"""mcp-drc live gate: drc -> drc_spots -> cleanup in ONE real KiCad 10 session (Xephyr, isolated home) on a copy of
the public wiccon-2023 board. Deselected by default (marker ``gui``):

    PYTHONPATH=.:plugin .venv/bin/pytest tests/mcp_server/test_mcp_drc_gui.py -m gui -s

"Routed" = the hand-routed copy plus one synthetic short (a track of net A from an A track end to the nearest B
track end on the same layer), "input" = the stripped copy (harness.strip). Needs Xephyr, metacity, xdotool, wmctrl,
xwd, KiCad 10 and the board under ~/work/kicad/projects (read only; copied).
"""
from __future__ import annotations

import math
import re
import shutil
import sys
import uuid
from pathlib import Path

import pytest

from harness import strip
from harness.paths import BENCH_BOARDS
from mcp_server import kicad_gui as G
from mcp_server import tools

pytestmark = pytest.mark.gui
ROOT = Path(__file__).resolve().parents[2]

SEG = re.compile(r'\(segment\s+\(start ([-\d.]+) ([-\d.]+)\)\s+\(end ([-\d.]+) ([-\d.]+)\)\s+\(width ([\d.]+)\)\s+'
                 r'\(layer "([^"]+)"\)\s+\(net (?:"([^"]*)"|(\d+))\)\s+\((?:uuid|tstamp)\s+"?([^")\s]+)')


def add_short(pcb: Path, skip: set = frozenset()) -> tuple[str, str]:
    """Append one F.Cu track of net A joining the closest pair of A / B track ends (A != B; tracks whose uuid is
    in `skip` are not used). O(n^2) in ends."""
    text = pcb.read_text()
    ends = [((float(m[1]), float(m[2])), (float(m[3]), float(m[4])), m[5], m[6], m[7] or m[8])
            for m in SEG.finditer(text) if m[6] == "F.Cu" and m[9] not in skip]
    best = None
    for i, a in enumerate(ends):
        for b in ends[i + 1:]:
            if a[4] == b[4]:
                continue
            for pa in a[:2]:
                for pb in b[:2]:
                    d = math.dist(pa, pb)
                    if d > 0.05 and (best is None or d < best[0]):
                        best = (d, pa, pb, a)
    assert best, "no F.Cu tracks of two nets"
    _, pa, pb, a = best
    net = f'"{a[4]}"' if not a[4].isdigit() else a[4]
    seg = (f'\t(segment (start {pa[0]} {pa[1]}) (end {pb[0]} {pb[1]}) (width {a[2]}) (layer "F.Cu") (net {net}) '
           f'({"tstamp" if "(tstamp " in text else "uuid"} "{uuid.uuid4()}"))\n')
    i = text.rstrip().rindex(")")
    pcb.write_text(text[:i] + seg + text[i:])
    return a[4], seg


def need_wiccon() -> Path:
    """The wiccon-2023 snapshot, looked up when the test RUNS (never at import / collection: without scratch/boards,
    e.g. GitHub CI, BENCH_BOARDS raises and a collection error would stop the whole pytest run); absent -> skip."""
    try:
        p = Path(BENCH_BOARDS["wiccon-2023"])
    except (RuntimeError, KeyError) as ex:
        pytest.skip(f"wiccon-2023 board snapshot not on this host: {ex}")
    if not p.is_file():
        pytest.skip(f"wiccon-2023 board not on this host: {p}")
    return p


def add_dangling(pcb: Path, length: float = 0.2) -> str:
    """Append a `length` mm F.Cu stub at right angles from the middle of the longest non-GND F.Cu track: one end on
    that track, the other free (KiCad: track_dangling, a warning in wiccon's project). Returns its uuid."""
    text = pcb.read_text()
    segs = [((float(m[1]), float(m[2])), (float(m[3]), float(m[4])), m[5], m[7] or m[8])
            for m in SEG.finditer(text) if m[6] == "F.Cu" and "GND" not in (m[7] or "")]
    a, b, w, net = max(segs, key=lambda t: math.dist(t[0], t[1]))
    d = math.dist(a, b)
    mx, my = (a[0] + b[0]) / 2, (a[1] + b[1]) / 2
    nx, ny = -(b[1] - a[1]) / d, (b[0] - a[0]) / d
    u = str(uuid.uuid4())
    seg = (f'\t(segment (start {mx:.4f} {my:.4f}) (end {mx + nx * length:.4f} {my + ny * length:.4f}) (width {w}) '
           f'(layer "F.Cu") (net {net if net.isdigit() else f"{chr(34)}{net}{chr(34)}"}) '
           f'({"tstamp" if "(tstamp " in text else "uuid"} "{u}"))\n')
    i = text.rstrip().rindex(")")
    pcb.write_text(text[:i] + seg + text[i:])
    return u


@pytest.mark.skipif(not all(shutil.which(t) for t in ("Xephyr", "xdotool", "wmctrl", "xwd", "pcbnew", "metacity",
                                                      "kicad-cli")),
                    reason="needs Xephyr, xdotool, wmctrl, xwd, metacity and KiCad 10")
def test_wiccon_spots_and_cleanup_share_one_kicad(tmp_path, monkeypatch):
    need_wiccon()
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    hand, inp, _ = strip.prepare("wiccon-2023", tmp_path / "boards", name="w")
    skip: set = set()
    for _ in range(3):  # several shorts: their markers are close, so a stale focus (no scroll) would show
        seg = add_short(hand.pcb, skip=skip)[1]
        skip.add(re.search(r'"([0-9a-f-]{36})"\)\)$', seg.strip()).group(1))
    add_dangling(hand.pcb)  # one warning row of ours: the default (errors only) walk must skip it
    rid = tools.register_run(str(hand.pcb), str(inp.pcb), name="w")["run_id"]
    d = tools.drc(rid)
    assert d["rules"] == "rules = .kicad_pro" and d["project_used"]
    assert d["ours_by_severity"]["warning"] >= 1 and d["ours_by_severity"]["error"] >= 2, d["ours_by_severity"]
    assert d["rows"] and all(r["sev"] == "error" for r in d["rows"]) and "warnings" in d["summary"]
    warn_ids = {r["id"] for r in tools.drc(rid, severities=["error", "warning"])["rows"] if r["sev"] == "warning"}
    assert warn_ids
    short = sorted(c for c in d["by_class"] if c.split()[0] in ("shorting_items", "tracks_crossing") and d["by_class"][c]["ours"])
    assert short, d["by_class"]
    starts0 = G.GuiSession.starts_total
    try:
        s = tools.drc_spots(rid, per_class=1)  # >= 2 rows: one per class of ours, all shorts
        assert s["wanted"] >= 2 and s["kicad_starts_this_call"] == 1, s.get("errors")
        recs = s["records"]
        assert len(recs) == s["wanted"] and all(r["match"] for r in recs), [r.get("error") for r in recs]
        assert all(Path(r["board_png"]).is_file() and Path(r["dlg_png"]).is_file() for r in recs)
        # verify r6 N1: the ring is on each row's OWN marker (from the measured view transform), which is on screen
        # (KiCad centres a track row's items, here up to ~5 mm from the marker): the clearance row's ring is not the
        # shorts' ring, and every ring pixel is where that row's marker is on the screen
        assert all(r["marker_visible"] and r["marker_px"] for r in recs), [(r["id"], r["marker_px"]) for r in recs]
        for a, b in zip(recs, recs[1:]):
            if math.dist((a["x"], a["y"]), (b["x"], b["y"])) > 0.3:
                assert a["marker_px"] != b["marker_px"] or a["focus_mm"] != b["focus_mm"], (a["id"], b["id"])
        assert len({tuple(r["marker_px"]) for r in recs}) >= 2, [(r["id"], r["marker_px"]) for r in recs]
        # errors only: Warnings unticked in the live dialog before the walk, the list = kicad-cli's error rows,
        # no warning row visited; both severities counted
        assert s["severities"] == ["error"] and not {r["id"] for r in recs} & warn_ids
        assert s["ours_by_severity"] == d["ours_by_severity"] and s["cli_by_severity"]["warning"] >= 1
        assert s["gui_equals_cli"], (s["gui_violations"], s["cli_violations"])
        assert any(st["step"] == "drc severity filter" for st in G._SESSION.steps)
        boxes = G._SESSION.call("drc_state")["boxes"]
        assert boxes.get("Warnings") is False and boxes.get("Errors") is True, boxes
        pid = s["session"]["pcbnew_pid"]
        c = tools.cleanup(rid)
        assert G.GuiSession.starts_total == starts0 + 1  # the cleanup ran in the drc_spots session
        assert c["session"]["pcbnew_pid"] == pid
        on = sorted(k for k, v in c["options"].items() if v)
        assert on == sorted(G.CLEANUP_LABELS[k] for k in ("short", "vias", "merge", "unconnected"))
        assert sum(c["removed"].values()) >= 1  # the synthetic short (KiCad deletes the track joining two nets)
        assert not [k for k, v in c["drc_by_class"].items() if k in short and v["delta"] >= 0]
        for v in c["newly_unconnected"].values():
            assert {"before", "after", "new", "lost_copper"} <= set(v)
            assert all("items" in n and "conns" in n for n in v["new"])
        assert isinstance(c["newly_unconnected_total"], int)
    finally:
        tools.gui_session("stop")
    assert G.session_status()["session"] is None


@pytest.mark.skipif(not all(shutil.which(t) for t in ("Xephyr", "xdotool", "wmctrl", "xwd", "pcbnew", "metacity",
                                                      "kicad-cli")),
                    reason="needs Xephyr, xdotool, wmctrl, xwd, metacity and KiCad 10")
def test_wiccon_topor_route_connection_ids_end_to_end(tmp_path, monkeypatch):
    """route (result.json) -> one synthetic short between two of OUR tracks -> drc rows carry engine connection ids
    -> cleanup's newly unconnected connections name ids of that result.json."""
    need_wiccon()
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    if not (ROOT / ".venv").is_dir():
        monkeypatch.setenv("TOPOR_VENV", sys.prefix)
    hand, _, _ = strip.prepare("wiccon-2023", tmp_path / "boards", name="w")
    r = tools.route(str(hand.pcb), {"png": False})
    rid = r["run_id"]
    info = tools._run(rid)
    inp, rout = tools._boards(info)
    res = tools._result_or_none(info)
    ids = {c["id"] for c in (res.get("route", res).get("connections") or [])}
    assert ids, "the run's result.json has no connections"
    from mcp_server import drc as D
    add_short(rout, skip=set(D.board_items(inp.read_text())))  # between two tracks topor drew
    d = tools.drc(rid)
    short = [x for x in d["rows"] if x["type"] in ("shorting_items", "tracks_crossing")]
    assert short, d["by_class"]
    conns = {c["conn"] for x in short for o in x["ours"] for c in o["conns"]}
    assert conns and conns <= ids  # our copper -> engine connection ids of this result.json
    try:
        s = tools.drc_spots(rid, cls=short[0]["class"])
        assert s["kicad_starts_this_call"] == 1 and all(x["match"] for x in s["records"]), s["errors"]
        c = tools.cleanup(rid)
        assert c["session"]["reused"]
        got = {cc["conn"] for v in c["newly_unconnected"].values() for n in v["new"] for cc in n["conns"]}
        got |= {i for v in c["newly_unconnected"].values() for lc in v["lost_copper"] for i in lc["conns"]}
        assert c["newly_unconnected_total"] >= 1 and got and got <= ids, c["newly_unconnected"]
    finally:
        tools.gui_session("stop")
    assert G.session_status()["session"] is None
