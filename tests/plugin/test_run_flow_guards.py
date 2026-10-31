"""MINOR 10 (kicad-cli pumps the UI, timeout, cancel) and MINOR 11 (board edited during the run) in the run flow.

Real engine only where a routed result is needed; FakeBoard (eprom) + ScriptedUi like test_run_flow.py (whose
fixtures and helpers are reused). No window is ever shown (Ruling C18).
"""
import stat
import time

import pytest

pytest.importorskip("wx")
pytest.importorskip("topor_engine")

from test_run_flow import (CFG, ExplodingEngine, app, do_run, eprom, make_board, reports,  # noqa: E402,F401
                           _report_root)
from topor import run as run_mod  # noqa: E402
from topor.fake import ScriptedUi  # noqa: E402
from toporlib import pcbfile  # noqa: E402


class PhaseUi(ScriptedUi):
    """Cancels (progress -> False) from the first progress call of `cancel_phase` on."""

    def __init__(self, cancel_phase, **kw):
        super().__init__(**kw)
        self.cancel_phase = cancel_phase

    def progress(self, p):
        super().progress(p)
        return p.phase != self.cancel_phase


def _phases(ui):
    return [c[1][0].phase for c in ui.calls if c[0] == "progress"]


def test_run_emits_setup_progress_first(app, eprom, tmp_path, reports):
    _, ui, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False))
    ph = _phases(ui)
    assert ph[0] == "setup" and "preflight_drc" in ph


def test_run_cancel_in_preflight_drc_kills_cli_and_aborts(app, eprom, tmp_path, reports, monkeypatch):
    from toporlib.kicad_cli import KicadCliCancelled

    seen = {}

    def fake_drc(pcb, out_json, *, save_board=False, timeout_s=None, on_poll=None):
        seen["timeout_s"] = timeout_s
        assert on_poll() is False  # the UI said Cancel -> a real run_cli would kill kicad-cli here
        raise KicadCliCancelled("killed")

    monkeypatch.setattr(run_mod, "run_drc", fake_drc)
    board, ui, out = do_run(eprom, tmp_path, ui=PhaseUi("preflight_drc"), engine=ExplodingEngine())
    assert out.status == "aborted_preflight" and seen["timeout_s"] == 120
    assert "preflight" not in ui.names() and board.commits == []


def test_run_timeout_setting_zero_means_no_timeout(app, eprom, tmp_path, reports, monkeypatch):
    seen = {}

    def fake_drc(pcb, out_json, *, save_board=False, timeout_s=None, on_poll=None):
        seen["timeout_s"] = timeout_s
        raise RuntimeError("stop here")

    monkeypatch.setattr(run_mod, "run_drc", fake_drc)
    cfg = CFG.with_values({("run", "kicad_cli_timeout_s"): 0.0})
    do_run(eprom, tmp_path, cfg=cfg, engine=ExplodingEngine(), strict=False)
    assert seen["timeout_s"] is None


def test_run_real_cli_timeout_is_an_error_finding(app, eprom, tmp_path, reports, monkeypatch):
    slow = tmp_path / "kicad-cli"
    slow.write_text("#!/bin/sh\nsleep 20\n")
    slow.chmod(slow.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setenv("KICAD_CLI", str(slow))
    cfg = CFG.with_values({("run", "kicad_cli_timeout_s"): 0.6})
    t0 = time.monotonic()
    board, ui, out = do_run(eprom, tmp_path, cfg=cfg, engine=ExplodingEngine(), strict=False)
    assert time.monotonic() - t0 < 15
    assert out.status == "error" and out.error.code == "KICAD_CLI_TIMEOUT"
    assert "run.kicad_cli_timeout_s" in out.error.fix
    assert board.commits == [] and [c[1][0].code for c in ui.calls if c[0] == "error"] == ["KICAD_CLI_TIMEOUT"]
    assert "preflight_drc" in _phases(ui)  # the UI was pumped while kicad-cli ran
    assert "error KICAD_CLI_TIMEOUT" in (out.report_dir / "report.md").read_text()


# ------------------------------------------------------------------ MINOR 11
class EditingUi(ScriptedUi):
    """Edits the live board while the preview is open, then answers Accept."""

    def __init__(self, board, edit, **kw):
        super().__init__(**kw)
        self.board, self.edit = board, edit

    def preview(self, png, report):
        self.edit(self.board)
        return super().preview(png, report)


def _move_first_footprint(board):
    i = board.text.index("(footprint ")
    j = board.text.index("(at ", i)
    k = board.text.index(")", j)
    x, y = board.text[j + 4:k].split()[:2]
    board.text = board.text[:j] + f"(at {float(x) + 1.0} {y}" + board.text[k:]


def test_run_refuses_to_commit_when_board_changed(app, eprom, tmp_path, reports):
    board = make_board(eprom)
    ui = EditingUi(board, _move_first_footprint, accept=True)
    _, ui, out = do_run(eprom, tmp_path, ui=ui, board=board, strict=False)
    assert out.status == "error" and out.error.code == "BOARD_CHANGED_DURING_RUN"
    assert "run Topor again; do not edit the board while Topor runs" in out.error.fix
    assert board.commits == [] and not any(c[0] == "commit" for c in board.calls)
    assert [c[1][0].code for c in ui.calls if c[0] == "error"] == ["BOARD_CHANGED_DURING_RUN"]
    assert "error BOARD_CHANGED_DURING_RUN" in (out.report_dir / "report.md").read_text()


def test_run_refuses_to_commit_when_a_track_was_deleted(app, eprom, tmp_path, reports):
    def drop_track(board):
        sp = pcbfile.find_items(board.text)[-1]
        board.text = pcbfile.remove_items(board.text, [sp.uuid])

    board = make_board(eprom)
    _, ui, out = do_run(eprom, tmp_path, ui=EditingUi(board, drop_track, accept=True), board=board, strict=False)
    assert out.status == "error" and out.error.code == "BOARD_CHANGED_DURING_RUN" and board.commits == []


def test_run_commits_when_board_unchanged(app, eprom, tmp_path, reports):
    board = make_board(eprom)
    _, ui, out = do_run(eprom, tmp_path, ui=EditingUi(board, lambda b: None, accept=True), board=board)
    assert out.status == "accepted" and len(board.commits) == 1
    assert len([c for c in board.calls if c[0] == "fingerprint"]) == 2


def test_fake_board_fingerprint_sees_moves(eprom):
    b = make_board(eprom, live=False)
    f0 = b.fingerprint()
    assert b.fingerprint() == f0
    _move_first_footprint(b)
    assert b.fingerprint() != f0
