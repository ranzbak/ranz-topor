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
import uuid
from pathlib import Path

import pytest

from harness import strip
from harness.paths import BENCH_BOARDS
from mcp_server import kicad_gui as G
from mcp_server import tools

pytestmark = pytest.mark.gui

SEG = re.compile(r'\(segment\s+\(start ([-\d.]+) ([-\d.]+)\)\s+\(end ([-\d.]+) ([-\d.]+)\)\s+\(width ([\d.]+)\)\s+'
                 r'\(layer "([^"]+)"\)\s+\(net (?:"([^"]*)"|(\d+))\)')


def add_short(pcb: Path) -> tuple[str, str]:
    """Append one F.Cu track of net A joining the closest pair of A / B track ends (A != B). O(n^2) in ends."""
    text = pcb.read_text()
    ends = [((float(m[1]), float(m[2])), (float(m[3]), float(m[4])), m[5], m[6], m[7] or m[8])
            for m in SEG.finditer(text) if m[6] == "F.Cu"]
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


@pytest.mark.skipif(not all(shutil.which(t) for t in ("Xephyr", "xdotool", "wmctrl", "xwd", "pcbnew", "metacity",
                                                      "kicad-cli")),
                    reason="needs Xephyr, xdotool, wmctrl, xwd, metacity and KiCad 10")
@pytest.mark.skipif(not Path(BENCH_BOARDS["wiccon-2023"]).is_file(), reason="wiccon-2023 board not on this host")
def test_wiccon_spots_and_cleanup_share_one_kicad(tmp_path, monkeypatch):
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    hand, inp, _ = strip.prepare("wiccon-2023", tmp_path / "boards", name="w")
    add_short(hand.pcb)
    rid = tools.register_run(str(hand.pcb), str(inp.pcb), name="w")["run_id"]
    d = tools.drc(rid)
    assert d["rules"] == "rules = .kicad_pro" and d["project_used"]
    short = sorted(c for c in d["by_class"] if c.split()[0] in ("shorting_items", "tracks_crossing") and d["by_class"][c]["ours"])
    assert short, d["by_class"]
    starts0 = G.GuiSession.starts_total
    try:
        s = tools.drc_spots(rid, per_class=1)  # >= 2 rows: one per class of ours, all shorts
        assert s["wanted"] >= 2 and s["kicad_starts_this_call"] == 1, s.get("errors")
        recs = s["records"]
        assert len(recs) == s["wanted"] and all(r["match"] for r in recs), [r.get("error") for r in recs]
        assert all(Path(r["board_png"]).is_file() and Path(r["dlg_png"]).is_file() for r in recs)
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
