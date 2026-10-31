"""FakeBoard / ScriptedUi behave like the real ports (ports.py)."""
from pathlib import Path

import pytest

from topor.config import Config
from topor.fake import FakeBoard, ScriptedUi
from topor.findings import Finding
from topor.ports import (BoardPort, CommitRequest, Decision, Progress, RunSummary, Selection, UiPort)
from toporlib import pcbfile

MM = 1_000_000
MODEL = {"layers": [{"id": 0, "name": "F.Cu"}, {"id": 2, "name": "B.Cu"}],
         "nets": [{"code": 1, "name": "GND"}]}
PCB = ('(kicad_pcb\n\t(version 1)\n\t(segment\n\t\t(start 1 1)\n\t\t(end 2 2)\n\t\t(width 0.2)\n'
       '\t\t(layer "F.Cu")\n\t\t(net "GND")\n\t\t(uuid "old-1")\n\t)\n)\n')
SEG = {"net": 1, "layer": 0, "width": 200_000, "group": "topor",
       "prim": {"kind": "seg", "a": [0, 0], "b": [MM, 0]}}


@pytest.fixture
def pcb(tmp_path):
    p = tmp_path / "proj" / "b.kicad_pcb"
    p.parent.mkdir()
    p.write_text(PCB)
    (p.parent / "b.kicad_pro").write_text("{}")
    (p.parent / "b.kicad_dru").write_text("(version 1)")
    (p.parent / "other.kicad_pro").write_text("{}")
    return p


def test_fake_is_a_board_port(pcb):
    b: BoardPort = FakeBoard(pcb, MODEL)
    assert b.board_name() == "b.kicad_pcb" and b.project_dir() == pcb.parent
    assert b.selection().empty


def test_fake_save_copy_uses_live_text(pcb, tmp_path):
    b = FakeBoard(pcb, MODEL, live_text=PCB.replace("old-1", "live-1"))
    out = b.save_copy(tmp_path / "copy")
    assert out == tmp_path / "copy" / "b.kicad_pcb"
    assert "live-1" in out.read_text() and "old-1" in pcb.read_text()
    assert sorted(p.name for p in out.parent.iterdir()) == ["b.kicad_dru", "b.kicad_pcb", "b.kicad_pro"]
    out2 = FakeBoard(pcb, MODEL).save_copy(tmp_path / "copy2")
    assert out2.read_text() == PCB


def test_fake_commit_applies_and_records(pcb, tmp_path):
    b = FakeBoard(pcb, MODEL)
    res = b.commit(CommitRequest(remove_uuids=["old-1", "ghost"], items=[SEG], message="m"))
    assert (res.removed, res.created, res.grouped) == (1, 1, True)
    assert len(res.created_ids) == 1 and b.commits[0].message == "m"
    text = b.save_copy(tmp_path / "c").read_text()
    spans = pcbfile.find_items(text)
    assert [s.uuid for s in spans] == res.created_ids
    assert '(net "GND")' in text and '(layer "F.Cu")' in text
    assert b.groups == {"topor": res.created_ids}
    b.refill_zones()
    b.run_action("pcbnew.Foo")
    assert b.refills == 1 and b.actions == ["pcbnew.Foo"]
    assert [c[0] for c in b.calls] == ["commit", "save_copy", "refill_zones", "run_action"]
    # the file on disk is untouched
    assert pcb.read_text() == PCB


def test_fake_selection_settable(pcb):
    b = FakeBoard(pcb, MODEL)
    b.selection_value = Selection(footprints=["U1"])
    assert b.selection().footprints == ["U1"] and not b.selection().empty


def test_scripted_ui_records_calls(tmp_path):
    ui: UiPort = ScriptedUi(decisions=Decision(True, ("PAD_OUTSIDE_BOARD",)), accept=False,
                            cancel_at_iteration=3)
    cfg = Config.defaults()
    f = Finding("X", "warning", "m")
    ui.first_run_notice(tmp_path / "topor.toml")
    d = ui.preflight([f], RunSummary(3, 2, 2, "1.0"), cfg)
    assert d.proceed and d.accepted_codes == ("PAD_OUTSIDE_BOARD",)
    assert ui.progress(Progress("route", 2)) is True
    assert ui.progress(Progress("route", 3)) is False
    assert ui.preview(tmp_path / "a.png", None) is False
    ui.done("rejected", None, tmp_path)
    ui.error(f)
    assert [c[0] for c in ui.calls] == ["first_run_notice", "preflight", "progress", "progress",
                                         "preview", "done", "error"]
    assert ui.calls[-2] == ("done", ("rejected", None, tmp_path))


def test_scripted_ui_defaults_and_lists():
    ui = ScriptedUi()
    assert ui.preflight([], RunSummary(), Config.defaults()).proceed
    assert ui.progress(Progress(iteration=10**6)) is True and ui.preview(Path("x"), None) is True
    ui2 = ScriptedUi(decisions=[Decision(False), Decision(True)], accept=[True, False])
    assert [ui2.preflight([], RunSummary(), Config.defaults()).proceed for _ in range(3)] == [False, True, True]
    assert [ui2.preview(Path("x"), None) for _ in range(3)] == [True, False, False]
