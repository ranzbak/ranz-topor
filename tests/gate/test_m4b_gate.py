"""M4b gate, Step 1 (headless): the plugin flow (``topor.run.run``) on FakeBoard snapshot copies with harness-built
models, ScriptedUi(accept=True) and the real engine reproduces the M4b numbers: eprom 27/27, wiccon 62/62,
``routing_new 0`` and ``unconnected 0`` as judged by kicad-cli on the committed copy.

Boards only from the scratch snapshots (harness.paths, ruling C5), copied to tmp; the snapshot dir hash is checked.
The wiccon snapshot is a KiCad-6 file: the TEST re-saves the copy through pcbnew before the plugin flow (ruling
C21; the plugin itself refuses it with PCB_FORMAT_OLD). topor.toml: the first run on a copy without one creates it
(``load_config`` writes it when the run gets ``cfg=None`` and no file exists -- the only case the flow creates it);
the gate run then reads that file with one user edit, ``[run] existing_copper = "rip"`` (the hand routing is ripped).
No wx window is shown (ruling C18; ``wx.App(False)`` only for the preview renderer). Reports go to a tmp
TOPOR_REPORT_ROOT (ruling C1).
"""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT / "plugin") not in sys.path:
    sys.path.insert(0, str(ROOT / "plugin"))

wx = pytest.importorskip("wx")
topor_engine = pytest.importorskip("topor_engine")

from harness.copy import assert_unchanged, copy_project, sha256_tree  # noqa: E402
from harness.paths import BENCH_BOARDS  # noqa: E402
from tests.plugin.test_run_flow import _ok, _result  # noqa: E402  (the run-flow helpers, not a second flow)
from topor import run as run_mod  # noqa: E402
from topor.config import CONFIG_NAME, Config, load_config, update_config  # noqa: E402
from topor.fake import FakeBoard, ScriptedUi  # noqa: E402
from topor.ports import Decision, Selection  # noqa: E402
from topor.report import outcome_text  # noqa: E402
from topor.scope import connections_by_scope, scope_from_selection  # noqa: E402
from toporlib import pcbfile  # noqa: E402
from toporlib.drc import classify  # noqa: E402

REPORT_FILES = ("report.md", "result.json", "drc.json", "preview.png", "log.txt")
EXPECTED = {"eprom-to-kernal": 27, "wiccon-2023": 62}


@pytest.fixture(scope="module")
def app():
    try:
        a = wx.App(False)
    except Exception as e:  # noqa: BLE001 - no display
        pytest.skip(f"no display for wx: {e}")
    if not wx.Display.GetCount():
        pytest.skip("no display for wx")
    yield a


@pytest.fixture(autouse=True)
def _report_root(tmp_path, monkeypatch):
    monkeypatch.setenv("TOPOR_REPORT_ROOT", str(tmp_path / "reports"))


def project(board: str, dst: Path):
    """Snapshot copy (+ pcbnew re-save when older than KiCad 10, test side only) and its harness-built model,
    as in tests/plugin/test_run_flow.py. Returns (FakeBoard, model, pcb path)."""
    from harness import writer
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    cp = copy_project(BENCH_BOARDS[board], dst / board)
    b = load_board(cp)
    ver = pcbfile.file_version(cp.pcb.read_text())
    if ver is None or ver < pcbfile.KICAD10_VERSION:
        writer.save(b, cp.pcb)  # pcbnew SaveBoard: a live save_as copy is KiCad 10 format
        b = load_board(cp)
        assert pcbfile.file_version(cp.pcb.read_text()) >= pcbfile.KICAD10_VERSION
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    model = export_model(b, rules, name=board, dru_text=dru)
    return model, cp.pcb


def numbers_line(board: str, out, c: dict, unconnected: int) -> str:
    f = out.report.facts
    return (f"GATE M4b {board}: routed {out.routed}/{out.total} vias {f.get('vias')} routing_new {c['routing_new']} "
            f"unconnected {unconnected} (new {c['incomplete']['unconnected_new']}) outside {f.get('length_mm')} mm "
            f"router {out.router_s:.2f} s DRC rounds {out.rounds} stop {out.stop_reason} wall {out.wall_s:.1f} s")


def _docs_gen_check() -> subprocess.CompletedProcess:
    env = {**os.environ, "PYTHONPATH": str(ROOT / "plugin") + os.pathsep + str(ROOT)}
    return subprocess.run([sys.executable, "-m", "topor.docs_gen", "--check"], cwd=ROOT, env=env,
                          capture_output=True, text=True)


@pytest.mark.gate
@pytest.mark.parametrize("board", ["eprom-to-kernal", "wiccon-2023"])
def test_m4b_gate_headless_eprom_and_wiccon(app, board, tmp_path):
    src = BENCH_BOARDS[board]
    hashes = sha256_tree(src.parent)
    model, pcb = project(board, tmp_path / "proj")
    toml = pcb.parent / CONFIG_NAME
    assert not toml.exists()

    # first run on a project without topor.toml: the flow creates it (all defaults + docs); declined here
    ui0 = ScriptedUi(decisions=Decision(proceed=False))
    out0 = run_mod.run(FakeBoard(pcb, model, project_dir=pcb.parent), ui0, None, topor_engine,
                       tmp_root=tmp_path / "work0")
    _ok(out0)
    assert out0.status == "aborted_preflight"
    assert toml.is_file() and any(f.code == "CFG_CREATED" for f in out0.findings)
    assert [c[1][0] for c in ui0.calls if c[0] == "first_run_notice"] == [toml]
    created, cfg_findings = load_config(pcb.parent)
    assert cfg_findings == [] and created == Config.defaults()
    assert "existing_copper" in toml.read_text() and "#" in toml.read_text()  # self-documenting

    # the user's one edit, then the gate run reads the file (cfg=None)
    update_config(toml, {"run.existing_copper": "rip"})
    board_port = FakeBoard(pcb, model, project_dir=pcb.parent)
    ui = ScriptedUi(accept=True)
    out = run_mod.run(board_port, ui, None, topor_engine, tmp_root=tmp_path / "work")
    _ok(out)
    assert out.status == "accepted"
    assert not any(f.code == "CFG_CREATED" for f in out.findings)
    assert out.report.facts.get("existing_copper") == "rip"
    assert len(board_port.commits) == 1 and out.commit is not None
    assert board_port.commits[0].items == out.items and set(board_port.commits[0].remove_uuids) == set(out.ripped)
    assert (out.routed, out.total) == (EXPECTED[board], EXPECTED[board])

    # the committed board (FakeBoard's second save_copy) judged by kicad-cli (M4-R6 oracle)
    copy, rep = _result(board_port, out, tmp_path, "committed")
    c = classify(rep, out.base_drc, set(board_port.groups["topor"]), unconnected_baseline=out.orig_drc)
    unconnected = len(rep.unconnected)
    print(numbers_line(board, out, c, unconnected))
    assert c["routing_new"] == 0
    assert unconnected == 0 and c["incomplete"]["unconnected_new"] == 0
    # the flow's own numbers come from kicad-cli on its routed copy: drc.json in the report dir is that report
    assert out.drc["routing_new"] == 0 and out.drc["incomplete"]["unconnected_new"] == 0
    d = out.report_dir
    assert json.loads((d / "drc.json").read_text()) == json.loads(
        (out.routed_pcb.parent / "drc-routed.json").read_text())
    assert out.routed_pcb.is_relative_to(out.work_dir)

    # report dir
    for f in REPORT_FILES:
        assert (d / f).is_file() and (d / f).stat().st_size > 0, f
    res = json.loads((d / "result.json").read_text())
    assert res["status"] == "accepted" and (res["routed"], res["total"]) == (EXPECTED[board], EXPECTED[board])
    assert res["drc"]["routing_new"] == 0
    assert f"Outcome: {outcome_text('accepted')}" in (d / "report.md").read_text()

    # the generated configuration reference matches the schema
    dg = _docs_gen_check()
    assert dg.returncode == 0, dg.stderr
    assert_unchanged(src.parent, hashes)


@pytest.mark.gate
@pytest.mark.parametrize("mode,expected", [("between", 26), ("touching", 30)])
def test_m4b_gate_scope_selection(app, mode, expected, tmp_path):
    board = "eprom-to-kernal"
    src = BENCH_BOARDS[board]
    hashes = sha256_tree(src.parent)
    model, pcb = project(board, tmp_path / "proj")
    sel = Selection(footprints=["U1", "U2"])
    cfg = Config.defaults().with_values({("run", "existing_copper"): "rip", ("run", "scope_mode"): mode})
    plan = scope_from_selection(sel, model, cfg)
    by_scope = connections_by_scope(sel, model, cfg)
    assert plan.mode == mode and plan.connections == by_scope[mode] == expected
    assert by_scope["between"] < by_scope["touching"] <= by_scope["whole"]  # U1+U2 touch every net

    board_port = FakeBoard(pcb, model, selection=sel, project_dir=pcb.parent)
    ui = ScriptedUi(accept=True)
    out = run_mod.run(board_port, ui, cfg, topor_engine, tmp_root=tmp_path / "work")
    _ok(out)
    summary = [c[1][1] for c in ui.calls if c[0] == "preflight"][0]
    assert summary.has_selection and summary.connections_by_scope == by_scope
    assert out.status == "accepted" and len(board_port.commits) == 1
    names = {int(n["code"]): n["name"] for n in model["nets"]}
    rr = json.loads((out.report_dir / "result.json").read_text())["route"]
    routed_nets = {c["name"] for c in rr["connections"] if c["status"] == "routed"}
    item_nets = {names[it["net"]] for it in out.items}
    assert out.routed > 0 and routed_nets and routed_nets <= plan.nets and item_nets <= plan.nets
    assert {c["name"] for c in rr["connections"]} <= plan.nets  # only connections within scope
    assert 0 < out.total <= plan.connections  # the estimate is an upper bound (pours / ties join pads)
    # only copper of scope nets is ripped
    copper = {k["uuid"]: k["net"] for k in model["kept"]} | {v["uuid"]: v["net"] for v in model["vias"]}
    assert out.ripped and {names[copper[u]] for u in out.ripped} <= plan.nets
    copy, rep = _result(board_port, out, tmp_path, "committed")
    c = classify(rep, out.base_drc, set(board_port.groups["topor"]), unconnected_baseline=out.orig_drc)
    print(f"GATE M4b scope {mode} U1+U2: est {by_scope[mode]} routed {out.routed}/{out.total} nets "
          f"{len(routed_nets)}/{len(plan.nets)} vias {out.report.facts.get('vias')} ripped {len(out.ripped)} "
          f"routing_new {c['routing_new']} unconnected {len(rep.unconnected)} (out of scope, not asserted) "
          f"router {out.router_s:.2f} s")
    assert c["routing_new"] == 0 and out.drc["routing_new"] == 0
    assert_unchanged(src.parent, hashes)
