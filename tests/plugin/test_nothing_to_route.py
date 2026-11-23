"""pour-pad-join (fail fast, lead 2026-11-23): a fully connected board is a valid input with nothing to route. The run
ends right after reading the board with the notice NOTHING_TO_ROUTE (info, no red error), before the engine builds its
map of the board, and never spins the dialog's TopoR loop (no cycle cap, no stop at "Routes stable") through empty
cycles until the time limit. Before: a complete small board ran 40 075 empty cycles per run in 20 s.

Synthetic board (harness/fixtures.py): a connector J1 with four GND shield pads all numbered "SH", all in the B.Cu GND
pour (and the fixture's free GND via); no other copper. The pour lists "J1.SH" four times; the first-match join of the
pour's pad list left three of the four out of the pour (phantom pad-to-pour connections, the private board P)."""
import json
import time

import pytest

wx = pytest.importorskip("wx")
topor_engine = pytest.importorskip("topor_engine")
pytest.importorskip("pcbnew")

from harness.copy import ProjectCopy  # noqa: E402
from harness.fixtures import make_fixture_board, write_project_file  # noqa: E402
from harness.model import export_model, load_board  # noqa: E402
from harness.rip import record_widths  # noqa: E402
from harness.rules import resolve_rules  # noqa: E402
from topor import run as run_mod  # noqa: E402
from topor.config import Config  # noqa: E402
from topor import messages  # noqa: E402
from topor.fake import FakeBoard, ScriptedUi  # noqa: E402
from topor.ports import Selection  # noqa: E402


@pytest.fixture(scope="module")
def app():
    try:
        a = wx.App(False)
    except Exception as e:  # noqa: BLE001 - no display
        pytest.skip(f"no display for wx: {e}")
    yield a


class DialogUi(ScriptedUi):
    """The progress dialog's double that never presses Accept / Cancel (the engine then never stops at stable)."""

    def __init__(self):
        super().__init__(accept=True)
        self.waits_for_ok = True


def _complete_board(tmp_path):
    pcb = make_fixture_board(tmp_path / "fx" / "fixture.kicad_pcb", pads=None, npth=False, locked_track=False,
                             arc=False, copper_text=False, shield_pads=4)
    pro = write_project_file(pcb, rules={}, classes=[])
    cp = ProjectCopy(pcb=pcb, pro=pro, dru=None, src_hashes={})
    b = load_board(cp)
    model = export_model(b, resolve_rules(b, dru_text=None, widths_before=record_widths(b)), name="fixture")
    return cp, model


def test_shield_pads_are_listed_once_per_pad(tmp_path):
    """The exporter's side of the mechanism: one pour entry per reached pad, so a same-numbered pad repeats."""
    _, model = _complete_board(tmp_path)
    gnd = [z for z in model["pours"] if z["pads"]]
    assert len(gnd) == 1 and gnd[0]["pads"] == ["J1.SH"] * 4, gnd[0]["pads"]


@pytest.mark.parametrize("scope", ["whole", "between", "touching"])
def test_complete_board_ends_with_nothing_to_route(app, tmp_path, monkeypatch, scope):
    """`scope`: the whole board (no selection), or J1 selected with that run.scope_mode (pour-pad-join r2: the notice
    then speaks of the selection's connections, not of the whole board)."""
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(tmp_path / "reports"))
    cp, model = _complete_board(tmp_path)
    ui = DialogUi()
    vals = {("run", "timeout_s"): 30.0, ("run", "refill_after_accept"): False}
    if scope != "whole":
        vals[("run", "scope_mode")] = scope
    cfg = Config.defaults().with_values(vals)
    board = FakeBoard(cp.pcb, model, project_dir=cp.pcb.parent,
                      selection=Selection(footprints=["J1"]) if scope != "whole" else None)
    live = board.text
    t = time.monotonic()
    out = run_mod.run(board, ui, cfg, topor_engine, tmp_root=tmp_path / "work")
    router_s = float(out.router_s or 0.0)
    assert out.stop_reason == "nothing_to_route", (out.status, out.stop_reason)
    assert out.total == 0 and not out.items and out.status == "rejected" and not out.cancelled
    # loud, not red: the one notice window, an info finding
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert [(f.code, f.severity) for f in errs] == [("NOTHING_TO_ROUTE", "info")]
    assert errs[0].message == messages.nothing_to_route_cause(scope)
    if scope == "whole":
        assert errs[0].message.startswith("The board is already fully connected")
    else:
        assert errs[0].message.startswith(f"Every connection of the selection (scope mode '{scope}'")
        assert "fully connected" not in errs[0].message and "the rest of the board was not checked" in errs[0].message
    assert "preview" not in ui.names()
    # in seconds: no projection, no loop (the empty loop used to run to the 30 s limit)
    assert router_s < 2.0, router_s
    assert time.monotonic() - t < 25.0
    log = (out.report_dir / "log.txt").read_text()
    assert "loop cycle:" not in log and "nothing to route:" in log
    assert "DRC round 1: nothing added or removed" in log
    rr = json.loads((out.report_dir / "result.json").read_text())
    assert rr["stop_reason"] == "nothing_to_route"
    assert any(f["code"] == "NOTHING_TO_ROUTE" for f in rr["findings"]), rr["findings"]
    assert "Stopped: nothing_to_route (nothing to route" in (out.report_dir / "report.md").read_text()
    # writes nothing: no commit, no zone refill, no KiCad action, the live board text unchanged
    assert (board.commits, board.refills, board.actions) == ([], 0, [])
    assert not [c for c in board.calls if c[0] in ("commit", "refill_zones", "run_action")]
    assert board.text == live

