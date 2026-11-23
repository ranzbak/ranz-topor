"""limit-p3p4 (F10, lead 2026-11-23): on a large board routed from scratch (private board P) the soft time limit was
not kept: the layer assignment and rip-up of the initial plan (P3/P4) ran past a 600 s limit for more than 20 min, and
Accept then gave NOTHING_TO_DRAW. Now, through the plugin's run flow with the progress dialog's double:
- once the topological search placed every connection the progress says "first plan ready, Accept will draw it",
  and Accept there writes copper (the plan so far), never NOTHING_TO_DRAW;
- the soft limit with run.limit_grace_s = 0 ends P3/P4 at their next check: the optimizing ends shortly after the
  limit and the plan so far is drawn.

Synthetic board (harness/fixtures.py `make_dense_board`): 2 layers, ~90 crossing two-pad nets on a 1 mm grid; its
P3/P4 take several times its P2 (release engine: P2 about 1 s, P3/P4 about 5 s on main)."""
import json
import time

import pytest

wx = pytest.importorskip("wx")
topor_engine = pytest.importorskip("topor_engine")
pytest.importorskip("pcbnew")

from harness.copy import ProjectCopy  # noqa: E402
from harness.fixtures import DENSE_CLASSES, make_dense_board, write_project_file  # noqa: E402
from harness.model import export_model, load_board  # noqa: E402
from harness.rip import record_widths  # noqa: E402
from harness.rules import resolve_rules  # noqa: E402
from topor import run as run_mod  # noqa: E402
from topor.config import Config  # noqa: E402
from topor.fake import FakeBoard, ScriptedUi  # noqa: E402

READY = "first plan ready, Accept will draw it"


@pytest.fixture(scope="module")
def app():
    try:
        a = wx.App(False)
    except Exception as e:  # noqa: BLE001 - no display
        pytest.skip(f"no display for wx: {e}")
    yield a


class DialogUi(ScriptedUi):
    """The progress dialog's double (the engine never stops at stable by itself). `accept_at_ready`: press Accept at
    the first progress whose stage says the first plan is ready. Records the stages it saw."""

    def __init__(self, accept_at_ready: bool):
        super().__init__(accept=True)
        self.waits_for_ok = True
        self.accept_at_ready = accept_at_ready
        self.stages: list[str] = []

    def progress(self, p):
        self.stages.append(getattr(p, "stage", ""))
        if self.accept_at_ready and READY in getattr(p, "stage", ""):
            return False
        return super().progress(p)


def _dense(tmp_path):
    pcb = make_dense_board(tmp_path / "fx" / "dense.kicad_pcb")
    # valid test: the board minimums match the net class (no fab floor above it)
    pro = write_project_file(pcb, rules={"min_clearance": 0.15, "min_track_width": 0.15, "min_via_diameter": 0.5}, classes=DENSE_CLASSES)
    cp = ProjectCopy(pcb=pcb, pro=pro, dru=None, src_hashes={})
    b = load_board(cp)
    model = export_model(b, resolve_rules(b, dru_text=None, widths_before=record_widths(b)), name="dense")
    return cp, model


def _run(tmp_path, monkeypatch, ui, values):
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(tmp_path / "reports"))
    cp, model = _dense(tmp_path)
    cfg = Config.defaults().with_values({("run", "refill_after_accept"): False, ("optimize", "runs"): 1, **values})
    out = run_mod.run(FakeBoard(cp.pcb, model, project_dir=cp.pcb.parent), ui, cfg, topor_engine, tmp_root=tmp_path / "work")
    return out


def test_accept_at_first_plan_ready_writes_copper(app, tmp_path, monkeypatch):
    ui = DialogUi(accept_at_ready=True)
    out = _run(tmp_path, monkeypatch, ui, {("run", "timeout_s"): 0.0})
    assert any(READY in s for s in ui.stages), sorted(set(ui.stages))
    codes = [f.code for f in out.findings] if getattr(out, "findings", None) else []
    assert out.error is None and "NOTHING_TO_DRAW" not in codes, (out.status, out.error)
    assert out.items, "Accept at the first plan writes copper"
    assert out.routed == out.total > 0, (out.routed, out.total)


def test_the_soft_limit_ends_p3_p4_and_draws_the_plan_so_far(app, tmp_path, monkeypatch):
    ui = DialogUi(accept_at_ready=False)
    t = time.monotonic()
    out = _run(tmp_path, monkeypatch, ui, {("run", "timeout_s"): 2.0, ("run", "limit_grace_s"): 0.0})
    wall = time.monotonic() - t
    tl = json.loads((out.report.dir / "result.json").read_text())["time_limit"]
    assert tl["reached"] is True and tl["mode"] == "soft", tl
    # P3/P4 stopped at their next check after the limit (main: the rip-up rounds ran on, ~4 s here, > 1000 s on P);
    # P2, the first plan, is not cut, so the bound allows for it ending after the 2 s limit on a loaded machine
    assert tl["overrun_s"] < 2.0, tl
    assert tl["grace_hit"] is True, tl
    assert out.items and out.routed == out.total > 0, (out.routed, out.total)
    log = (out.report.dir / "log.txt").read_text()
    assert "initial plan: the time limit plus its grace passed" in log or "initial plan: the time limit plus its grace passed" in json.dumps(
        json.loads((out.report.dir / "result.json").read_text())), "the round says the plan so far was kept"
    print(f"limit run: {wall:.1f} s wall")
