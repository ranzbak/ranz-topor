"""B5 run flow: save_as copy -> rules -> model -> route (progress / cancel) -> DRC loop -> SVG preview ->
Accept = one commit / Reject = nothing -> report folder.

Real engine, real kicad-cli, FakeBoard (eprom snapshot copy + harness-built model; pcbnew only here, in the
test fixture) and ScriptedUi. wx needs an App for the preview renderer: `wx.App(False)`, no window is ever
shown (Ruling C18). Every report folder goes under a tmp TOPOR_REPORT_ROOT (Ruling C1).
"""
import json
import math
import re
import shutil
from pathlib import Path

import pytest

wx = pytest.importorskip("wx")
topor_engine = pytest.importorskip("topor_engine")

from harness.paths import board_pcb  # noqa: E402
from topor import preview, run as run_mod  # noqa: E402
from topor.config import Config  # noqa: E402
from topor.fake import FakeBoard, ScriptedUi  # noqa: E402
from topor.ports import Report  # noqa: E402
from toporlib import pcbfile  # noqa: E402
from toporlib.drc import classify, run_drc  # noqa: E402

BOARD = "eprom-to-kernal"
MARKER = "11111111-2222-4333-8444-555555555555"  # a valid uuid: kicad-cli re-serialises saved boards
LIVE_EDIT = (f'\t(gr_line\n\t\t(start 190 70)\n\t\t(end 191 70)\n\t\t(stroke\n\t\t\t(width 0.1)\n'
             f'\t\t\t(type default)\n\t\t)\n\t\t(layer "Cmts.User")\n\t\t(uuid "{MARKER}")\n\t)\n')

CFG = Config.defaults().with_values({("run", "existing_copper"): "rip", ("run", "timeout_s"): 60.0})


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


@pytest.fixture(scope="module")
def reports(tmp_path_factory):
    mp = pytest.MonkeyPatch()
    root = tmp_path_factory.mktemp("reports")
    mp.setenv("TOPOR_REPORT_ROOT", str(root))
    yield root
    mp.undo()


class Eprom:
    pcb: Path
    model: dict
    text: str


@pytest.fixture(scope="module")
def eprom(tmp_path_factory):
    """Project copy of the snapshot + the harness-built model (pcbnew: test side only)."""
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.rules import resolve_rules

    d = tmp_path_factory.mktemp("proj")
    cp = copy_project(board_pcb(BOARD), d / "eprom-to-kernal")
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    e = Eprom()
    e.model = export_model(b, rules, name=BOARD, dru_text=dru)
    e.pcb = cp.pcb
    e.text = cp.pcb.read_text()
    return e


def make_board(eprom, live=True, **kw):
    text = eprom.text.rstrip()
    live_text = text[:-1].rstrip("\n") + "\n" + LIVE_EDIT + ")\n" if live else None
    return FakeBoard(eprom.pcb, eprom.model, live_text=live_text, project_dir=eprom.pcb.parent, **kw)


def _ok(out):
    if out.status == "error":
        pytest.fail("run failed:\n" + (out.report_dir / "log.txt").read_text()[-2500:])


def do_run(eprom, tmp_path, *, ui=None, cfg=CFG, engine=topor_engine, board=None, strict=True):
    board = board or make_board(eprom)
    ui = ui or ScriptedUi()
    out = run_mod.run(board, ui, cfg, engine, tmp_root=tmp_path / "work")
    if strict:
        _ok(out)
    return board, ui, out


# ------------------------------------------------------------------ engine doubles
class ProxyJob:
    """Delegates to a real RouteJob; `done()` stays False for `hold` polls so the progress loop runs."""

    def __init__(self, real, hold=1, patch=None):
        self.real, self.hold, self.polls, self.cancelled, self.patch = real, hold, 0, False, patch

    def progress(self):
        self.polls += 1
        d = json.loads(self.real.progress())
        d["iteration"] = max(d.get("iteration", 0), 1)
        return json.dumps(d)

    def done(self):
        return self.polls >= self.hold and self.real.done()

    def cancel(self):
        self.cancelled = True  # recorded only: the real job has to finish for a deterministic result

    def wait(self):
        out = self.real.wait()
        return self.patch(out) if self.patch else out


class ProxyEngine:
    def __init__(self, hold=1, patch=None):
        self.hold, self.patch, self.jobs = hold, patch, []

    def default_options(self):
        return topor_engine.default_options()

    def route_async(self, m, o):
        j = ProxyJob(topor_engine.route_async(m, o), self.hold, self.patch)
        self.jobs.append(j)
        return j


class ExplodingEngine:
    def default_options(self):
        return topor_engine.default_options()

    def route_async(self, m, o):
        raise AssertionError("the engine must not be called")


class PanicJob:
    n = 0

    def progress(self):
        self.n += 1
        return json.dumps({"phase": "route", "iteration": 1, "routed": 0, "total": 27, "vias": 0, "elapsed_s": 0.1})

    def done(self):
        return self.n >= 1  # the thread ended (panicked); wait() raises

    def cancel(self):
        pass

    def wait(self):
        raise ValueError("routing thread panicked")


class PanicEngine(ExplodingEngine):
    def route_async(self, m, o):
        return PanicJob()


def _mark_partial(out_json):
    d = json.loads(out_json)
    d["partial"] = 1
    for it in d["items"][:3]:
        it["group"] = "topor-partial"
    return json.dumps(d)


def _nothing_routed(out_json):
    d = json.loads(out_json)
    d.update(items=[], routed=0, stop_reason="cancel", timed_out=True, connections=[], vias=0)
    return json.dumps(d)


# ------------------------------------------------------------------ shared full flows
@pytest.fixture(scope="module")
def accepted(app, eprom, tmp_path_factory, reports):
    """Accept flow with both KiCad follow-up actions switched on."""
    cfg = CFG.with_values({("run", "kicad_cleanup"): True, ("run", "kicad_teardrops"): True})
    tmp = tmp_path_factory.mktemp("acc")
    board = make_board(eprom)
    ui = ScriptedUi(accept=True)
    out = run_mod.run(board, ui, cfg, topor_engine, tmp_root=tmp / "work")
    _ok(out)
    return board, ui, out, tmp


@pytest.fixture(scope="module")
def rejected(app, eprom, tmp_path_factory, reports):
    tmp = tmp_path_factory.mktemp("rej")
    board = make_board(eprom)
    ui = ScriptedUi(accept=False)
    eng = ProxyEngine(hold=2)
    out = run_mod.run(board, ui, CFG, eng, tmp_root=tmp / "work")
    _ok(out)
    return board, ui, out, eng


def _result(board, out, tmp, name):
    """DRC of the committed board (FakeBoard's second save_copy) judged by kicad-cli."""
    copy = board.save_copy(tmp / name)
    rep = run_drc(copy, tmp / f"{name}.json")
    return copy, rep


# ------------------------------------------------------------------ tests
def test_run_uses_save_as_copy_not_disk_file(rejected, eprom):
    board, ui, out, _ = rejected
    assert [c[0] for c in board.calls].count("save_copy") == 1
    rm = next(c for c in board.calls if c[0] == "read_model")
    assert Path(rm[1][1]) == out.pcb_copy and out.pcb_copy.is_relative_to(out.work_dir)
    assert MARKER not in eprom.pcb.read_text()
    temp = [out.pcb_copy, out.ripped_pcb, out.routed_pcb]
    assert len({p for p in temp}) == 3
    for p in temp:
        assert p.is_file() and MARKER in p.read_text(), p  # every temp board derives from the live state


def test_run_accept_single_commit(accepted, tmp_path):
    board, ui, out, tmp = accepted
    assert out.status == "accepted"
    assert len(board.commits) == 1
    req = board.commits[0]
    model_kept = {k["uuid"] for k in out.model["kept"]} | {v["uuid"] for v in out.model["vias"]}
    assert req.remove_uuids and set(req.remove_uuids) == set(out.ripped) <= model_kept
    assert req.items == out.items and len(req.items) > 27
    assert req.message == f"Topor: routed {out.routed}/{out.total}"
    assert req.group is True and {it["group"] for it in req.items} == {"topor"}
    assert board.refills == 1
    assert board.actions == ["pcbnew.GlobalEdit.cleanupTracksAndVias", "pcbnew.GlobalEdit.editTeardrops"]
    names = ui.names()
    assert names.index("preview") < names.index("done")
    assert [c for c in ui.calls if c[0] == "done"][0][1][0] == "accepted"
    # the second save_copy holds the commit; kicad-cli judges it (M4b gate numbers)
    copy, rep = _result(board, out, tmp, "after")
    spans = pcbfile.find_items(copy.read_text())
    res_ids = {s.uuid for s in spans}
    assert not (set(req.remove_uuids) & res_ids)
    assert len([u for u in res_ids if u in set(board.groups["topor"])]) == len(req.items)
    c = classify(rep, out.base_drc, set(board.groups["topor"]), unconnected_baseline=out.orig_drc)
    print(f"GATE eprom: routed {out.routed}/{out.total} routing_new {c['routing_new']} "
          f"unconnected_new {c['incomplete']['unconnected_new']} (all {c['incomplete']['unconnected_items']})")
    assert (out.routed, out.total) == (27, 27)
    assert c["routing_new"] == 0 and c["incomplete"]["unconnected_new"] == 0
    assert out.drc["routing_new"] == 0 and out.drc["incomplete"]["unconnected_new"] == 0
    # the normal geometry has arcs (so the segments test below means something)
    assert any(it.get("prim", {}).get("kind") == "arc" for it in req.items)


def test_run_reject_leaves_board_untouched(rejected, eprom):
    board, ui, out, _ = rejected
    assert out.status == "rejected" and out.commit is None
    assert board.commits == [] and board.refills == 0 and board.actions == []
    assert board.text.count(MARKER) == 1 and eprom.text == eprom.pcb.read_text()
    assert not any(c[0] in ("commit", "refill_zones", "run_action") for c in board.calls)
    done = [c for c in ui.calls if c[0] == "done"]
    assert len(done) == 1 and done[0][1][0] == "rejected" and done[0][1][2] == out.report_dir
    assert "rejected" in (out.report_dir / "report.md").read_text()


def test_run_cancel_keeps_best_result_and_previews(app, eprom, tmp_path, reports):
    eng = ProxyEngine(hold=2)
    ui = ScriptedUi(accept=False, cancel_at_iteration=1)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng)
    assert eng.jobs[0].cancelled  # progress() returned False -> job.cancel()
    assert out.status == "rejected" and out.routed > 0  # preview was shown after the cancelled route
    assert "preview" in ui.names() and board.commits == []
    assert out.report.stop_reason in ("cancel", "converged")  # the job had finished by itself in this double
    assert out.cancelled is True


def test_run_cancel_with_nothing_routed_is_cancelled(app, eprom, tmp_path, reports):
    eng = ProxyEngine(hold=2, patch=_nothing_routed)
    ui = ScriptedUi(accept=True, cancel_at_iteration=1)
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=eng)
    assert out.status == "cancelled" and board.commits == []
    assert "preview" not in ui.names()
    assert [c[1][0] for c in ui.calls if c[0] == "done"] == ["cancelled"]


def test_run_aborts_on_error_finding(app, eprom, tmp_path, reports):
    live = eprom.text.replace('(2 "B.Cu" signal)', '(2 "B.Cu" signal)\n\t\t(4 "In1.Cu" signal)', 1)
    assert live != eprom.text
    board = FakeBoard(eprom.pcb, eprom.model, live_text=live, project_dir=eprom.pcb.parent)
    board, ui, out = do_run(eprom, tmp_path, board=board, engine=ExplodingEngine())
    assert out.status == "aborted_preflight"
    assert any(f.code == "LAYER_ROLES_NEEDED" and f.severity == "error" for f in out.findings)
    pf = [c for c in ui.calls if c[0] == "preflight"]
    assert len(pf) == 1 and any(f.code == "LAYER_ROLES_NEEDED" for f in pf[0][1][0])
    assert board.commits == [] and "progress" not in ui.names()


def test_run_declined_preflight_aborts(app, eprom, tmp_path, reports):
    from topor.ports import Decision

    ui = ScriptedUi(decisions=Decision(proceed=False))
    board, ui, out = do_run(eprom, tmp_path, ui=ui, engine=ExplodingEngine())
    assert out.status == "aborted_preflight" and board.commits == []


def test_run_segments_geometry_has_no_arcs(app, eprom, tmp_path, reports):
    cfg = CFG.with_values({("output", "geometry"): "segments"})
    board, ui, out = do_run(eprom, tmp_path, cfg=cfg)
    assert out.status == "accepted" and out.routed == out.total == 27
    assert out.items and all(it.get("prim", {}).get("kind") != "arc" for it in out.items)
    assert board.commits[0].items == out.items
    new = {s.kind for s in pcbfile.find_items(pcbfile.append_items(
        "(kicad_pcb\n)\n", out.items, layer_names={0: "F.Cu", 2: "B.Cu"},
        net_names={n["code"]: n["name"] for n in out.model["nets"]}))}
    assert "arc" not in new
    assert out.drc["routing_new"] == 0


def test_run_partial_routes_group(app, eprom, tmp_path, reports):
    cfg = CFG.with_values({("run", "kicad_cleanup"): True, ("run", "partial_routes"): True})
    board, ui, out = do_run(eprom, tmp_path, cfg=cfg, engine=ProxyEngine(hold=1, patch=_mark_partial))
    assert out.status == "accepted"
    req = board.commits[0]
    groups = [it["group"] for it in req.items]
    assert groups.count("topor-partial") == 3 and set(groups) == {"topor", "topor-partial"}
    assert board.actions == []  # cleanup skipped when partial routes exist (30 section 4.2)
    assert any(f.code == "CLEANUP_SKIPPED_PARTIAL" for f in out.findings)
    assert set(board.groups) == {"topor", "topor-partial"}


def test_run_engine_panic_reported_not_crashed(app, eprom, tmp_path, reports):
    board, ui, out = do_run(eprom, tmp_path, engine=PanicEngine(), strict=False)
    assert out.status == "error" and board.commits == []
    errs = [c[1][0] for c in ui.calls if c[0] == "error"]
    assert len(errs) == 1 and errs[0].code == "INTERNAL" and errs[0].severity == "error"
    assert "panicked" in errs[0].message and "log.txt" in errs[0].fix and "debug.md" in errs[0].fix
    log = (out.report_dir / "log.txt").read_text()
    assert "panicked" in log and "Traceback" in log
    assert "done" not in ui.names() and "preview" not in ui.names()
    ctx = [c for c in ui.calls if c[0] == "set_context"]
    assert ctx and ctx[-1][1][1] == out.report_dir  # error dialog can open the report folder


def test_run_preview_png_written_with_caption(rejected):
    board, ui, out, _ = rejected
    png = out.report_dir / "preview.png"
    assert png.is_file() and png.stat().st_size > 0
    pv = [c for c in ui.calls if c[0] == "preview"]
    assert len(pv) == 1 and pv[0][1][0] == png and pv[0][1][1] is out.report
    assert isinstance(out.report, Report)
    lines = out.report.summary_lines()
    assert 0 < len(lines) <= 20
    assert lines[1].startswith("Routed 27/27 connections")
    img = wx.Image(str(png))
    assert img.IsOk() and img.GetWidth() > 400 and img.GetHeight() > 200
    # something is drawn (not a blank sheet) and the new-copper highlight tint is present
    px = img.GetData()
    colours = {bytes(px[i:i + 3]) for i in range(0, len(px), 3 * 7)}
    assert len(colours) > 8
    assert any(bytes(c) in colours for c in (preview.PREVIEW_HIGHLIGHT_RGB, preview.PREVIEW_HIGHLIGHT_FALLBACK_RGB))
    # the five report files, the outcome line rewritten after the decision
    d = out.report_dir
    for f in ("report.md", "preview.png", "result.json", "drc.json", "log.txt"):
        assert (d / f).is_file() and (d / f).stat().st_size > 0, f
    assert "Outcome: rejected" in (d / "report.md").read_text()
    res = json.loads((d / "result.json").read_text())
    assert res["status"] == "rejected" and res["routed"] == 27 and res["total"] == 27
    # context handed to the UI before the preview, and the phases shown
    names = ui.names()
    assert names.index("set_context") < names.index("preview")
    phases = [c[1][0].phase for c in ui.calls if c[0] == "progress"]
    for ph in ("preflight_drc", "drc", "preview"):
        assert ph in phases, phases
    assert phases.index("preflight_drc") < phases.index("drc") < phases.index("preview")
    assert "route" in phases or "done" in phases


def test_report_written_before_preview(app, eprom, tmp_path, reports):
    """Ruling C15/Q2: report.md and preview.png exist when preview() is called."""
    seen = {}

    class Ui(ScriptedUi):
        def preview(self, png, report):
            seen["files"] = sorted(p.name for p in Path(report.dir).iterdir())
            seen["outcome"] = (Path(report.dir) / "report.md").read_text()
            return super().preview(png, report)

    do_run(eprom, tmp_path, ui=Ui(accept=False), engine=ProxyEngine(hold=1, patch=None))
    assert "report.md" in seen["files"] and "preview.png" in seen["files"]


# ------------------------------------------------------------------ preview helpers (no board needed)
def test_arcs_to_segments_error_bound_and_ends():
    r = 3 * 1_000_000
    arc = {"net": 1, "layer": 0, "width": 200_000, "group": "topor", "wire": 4,
           "prim": {"kind": "arc", "start": [r, 0], "mid": [int(r * math.cos(0.7)), int(r * math.sin(0.7))],
                    "end": [int(r * math.cos(1.4)), int(r * math.sin(1.4))]}}
    seg = {"net": 1, "layer": 0, "width": 200_000, "prim": {"kind": "seg", "a": [0, 0], "b": [5, 5]}}
    via = {"kind": "via", "net": 1, "at": [1, 1], "d": 600_000, "drill": 300_000, "layers": [0, 2]}
    out = preview.arcs_to_segments([arc, seg, via], 5000)
    assert out[-2:] == [seg, via]
    segs = out[:-2]
    assert len(segs) > 3 and all(s["prim"]["kind"] == "seg" and s["group"] == "topor" and s["wire"] == 4 for s in segs)
    assert segs[0]["prim"]["a"] == arc["prim"]["start"] and segs[-1]["prim"]["b"] == arc["prim"]["end"]
    for a, b in zip(segs, segs[1:]):
        assert a["prim"]["b"] == b["prim"]["a"]
    for s in segs:  # chord midpoint within max_err of the circle
        a, b = s["prim"]["a"], s["prim"]["b"]
        mid = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
        assert abs(math.hypot(*mid) - r) <= 5000 + 2


def test_layer_colours_from_export():
    svg = ('<g style="fill:#000000; fill-opacity:1.0000;stroke:#000000; stroke-opacity:1.0000;\n">\n</g>\n'
           '<g style="fill:#840000; fill-opacity:1.0000; stroke:none;">\n<circle />\n</g>\n'
           '<g style="fill:#008400; fill-opacity:1.0000; stroke:none;">\n</g>\n'
           '<g style="fill:#FFFFFF; fill-opacity:1.0000; stroke:none;">\n</g>\n')
    assert preview.layer_colours(svg) == {0: (132, 0, 0), 2: (0, 132, 0)}
    assert preview.contrast((0, 0, 0), (255, 255, 255)) == pytest.approx(21.0)
    assert preview.pick_tint((255, 255, 255)) == preview.PREVIEW_HIGHLIGHT_RGB
    assert preview.pick_tint((53, 53, 53)) == preview.PREVIEW_HIGHLIGHT_FALLBACK_RGB


def test_accepted_findings_update_config(tmp_path):
    from topor.config import load_config
    from topor.findings import Finding
    from topor.ports import Decision

    cfg, _ = load_config(tmp_path)
    toml = tmp_path / "topor.toml"
    fs = [Finding("PAD_OUTSIDE_BOARD", "warning", "x", "y", ("U1.3", "J2.1"))]
    new = run_mod.apply_accepted_findings(Decision(True, accepted_codes=("PAD_OUTSIDE_BOARD",)), fs, cfg, toml)
    assert new.get("nets", "ignore_pads") == ["U1.3", "J2.1"]
    assert "U1.3" in toml.read_text()


# ------------------------------------------------------------------ ruling C20 / C21
def test_parse_zone_fills_and_version(eprom):
    z = pcbfile.parse_zone_fills(eprom.text)
    assert z and all(isinstance(k, str) for k in z)
    polys = [p for v in z.values() for p in v]
    assert polys and all(p["layer"].endswith(".Cu") and len(p["pts"]) >= 3 for p in polys)
    assert pcbfile.file_version(eprom.text) >= pcbfile.KICAD10_VERSION
    assert pcbfile.file_version("(kicad_pcb (version 20211014) (generator pcbnew)\n)") == 20211014
    assert pcbfile.file_version("garbage") is None


def test_run_refuses_pre_kicad10_file(app, eprom, tmp_path, reports):
    old = re.sub(r"\(version \d+\)", "(version 20211014)", eprom.text, count=1)
    board = FakeBoard(eprom.pcb, eprom.model, live_text=old, project_dir=eprom.pcb.parent)
    board, ui, out = do_run(eprom, tmp_path, board=board, engine=ExplodingEngine())
    assert out.status == "aborted_preflight" and board.commits == []
    f = [x for x in out.findings if x.code == "PCB_FORMAT_OLD"]
    assert f and f[0].severity == "error" and "KiCad 10" in f[0].fix


def test_run_wiccon_refill_gives_harness_connection_count(app, tmp_path_factory, reports):
    """Ruling C20: pour reach / dangling ends are recomputed on the refilled ripped copy (harness order
    rip -> refill -> export), so the engine sees the same connections as the harness model (62)."""
    from harness import writer
    from harness.copy import copy_project
    from harness.model import export_model, load_board
    from harness.rip import record_widths, rip_tracks
    from harness.rules import resolve_rules

    d = tmp_path_factory.mktemp("wic")
    cp = copy_project(board_pcb("wiccon-2023"), d / "w")
    b = load_board(cp)
    writer.save(b, cp.pcb)  # the snapshot is a KiCad 6 file; a live save_as copy is KiCad 10 format
    b = load_board(cp)
    dru = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru, widths_before=record_widths(b))
    model = export_model(b, rules, name="wiccon", dru_text=dru)
    board = FakeBoard(cp.pcb, model, project_dir=cp.pcb.parent)
    out = run_mod.run(board, ScriptedUi(accept=False), CFG, topor_engine, tmp_root=d / "work")
    _ok(out)
    rip_tracks(b, mode="all", scope=None, rf_nets=set())
    ref = json.loads(topor_engine.route(json.dumps(export_model(b, rules, name="wiccon", dru_text=dru)),
                                        json.dumps({"threads": 4, "wall_clock_s": 60.0})))
    print(f"wiccon connections: flow {out.total} harness {ref['connections_total']}")
    assert out.total == ref["connections_total"] == 62


def test_run_report_has_every_section(app, eprom, tmp_path, reports):
    """B8: the real flow writes a report.md with all nine sections and real numbers."""
    _, _, out = do_run(eprom, tmp_path, ui=ScriptedUi(accept=False))
    md = (out.report_dir / "report.md").read_text()
    for s in ("Summary", "Connections", "Vias", "Length", "DRC", "Findings", "Settings", "Files", "Versions"):
        assert f"\n## {s}\n" in md, s
    f = out.report.facts
    assert f["routed"] == 27 and f["vias_before"] is not None and f["length_before_mm"] is not None
    assert f["versions"]["engine"] and "non-default" in "\n".join(out.report.summary_lines())
    assert "Total " in md and "routing_new" in md and "Outside pads" in md
