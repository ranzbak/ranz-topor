"""last-preview (docs/dev/design/last-preview.md; user 2026-11-19): the progress dialog shows the last round's result,
whatever its score, with the last run's and the best's on a toggle; the score table has a "Last run" row and says
predicted (a TopoR loop plan) or drawn; the round count never goes back ("the run/round displayed goes back to 1 / 1
after a while"); [run] progress_preview / progress_preview_interval_s (since 0.5.0, since-loud: the default until
then).

The engine side (sketches, the read-only hooks, copper bit-equal) is tested in engine/tests/last_preview.rs.
"""
import json
import time

import pytest

from topor import config as config_mod
from topor import preview as preview_mod
from topor import run as run_mod
from topor.config import Config, plugin_value
from topor.ports import Progress
from topor.schema import SETTINGS_BY_KEY

# ---------------------------------------------------------------------------------------------- the option


def test_progress_preview_options_schema_and_since_gate(monkeypatch):
    s = SETTINGS_BY_KEY[("run", "progress_preview")]
    assert (s.default, s.choices, s.type) == ("last", ("last", "run", "best", "off"), "str")
    iv = SETTINGS_BY_KEY[("run", "progress_preview_interval_s")]
    assert (iv.default, iv.unit, iv.range) == (2.0, "s", (0.2, 600))
    assert s.since == iv.since == "0.5.0" and s.plugin_field == "run.progress_preview" and s.engine_field is None
    cfg = Config.defaults().with_values({("run", "progress_preview"): "best", ("run", "progress_preview_interval_s"): 7.0})
    # since-loud: a later release's setting keeps its default in this version (CFG_NOT_YET_AVAILABLE said so)...
    monkeypatch.delenv("TOPOR_UNRELEASED_SETTINGS", raising=False)
    assert plugin_value(cfg, "run", "progress_preview") == "last"
    assert plugin_value(cfg, "run", "progress_preview_interval_s") == 2.0
    # ...unless the opt-in switch is set
    monkeypatch.setenv("TOPOR_UNRELEASED_SETTINGS", "1")
    assert plugin_value(cfg, "run", "progress_preview") == "best"
    assert plugin_value(cfg, "run", "progress_preview_interval_s") == 7.0


def test_progress_preview_value_from_toml_warns_until_available(tmp_path, monkeypatch):
    monkeypatch.delenv("TOPOR_UNRELEASED_SETTINGS", raising=False)
    p = tmp_path / "topor.toml"
    p.write_text('version = 1\n[run]\nprogress_preview = "off"\nprogress_preview_interval_s = 5\n')
    cfg, findings = config_mod.load_config(tmp_path)
    codes = [f.code for f in findings]
    assert codes.count("CFG_NOT_YET_AVAILABLE") == 2, codes
    assert plugin_value(cfg, "run", "progress_preview") == "last"  # the default is what the run uses
    for v in SETTINGS_BY_KEY[("run", "progress_preview")].choices:  # every choice loads (and reaches the UI opted in)
        p.write_text(f'version = 1\n[run]\nprogress_preview = "{v}"\n')
        c, _ = config_mod.load_config(tmp_path)
        assert c.get("run", "progress_preview") == v
        monkeypatch.setenv("TOPOR_UNRELEASED_SETTINGS", "1")
        assert plugin_value(c, "run", "progress_preview") == v
        monkeypatch.delenv("TOPOR_UNRELEASED_SETTINGS")
    p.write_text('version = 1\n[run]\nprogress_preview = "sometimes"\n')
    cfg2, f2 = config_mod.load_config(tmp_path)
    assert cfg2.get("run", "progress_preview") == "last" and "CFG_VALUE" in [f.code for f in f2]


# ---------------------------------------------------------------------------------------------- progress fields


def test_progress_from_json_reads_the_last_preview_fields():
    d = {"phase": "route", "loop_run": 3, "loop_cycle": 37, "loop_restarts": 2, "loop_at_s": 12.5, "loop_errors": 4,
         "loop_open": 1, "loop_violations": 3, "loop_vias": 50, "loop_length_mm": 1234.5, "loop_shrinkages": 2,
         "cycles_total": 140, "last_at_s": 3.0, "runbest_run": 2, "runbest_cycle": 30, "runbest_at_s": 11.0,
         "runbest_kind": "plan", "runbest_errors": 3, "runbest_open": 0, "runbest_violations": 3, "runbest_vias": 48,
         "runbest_length_mm": 1200.0, "runbest_shrinkages": 1, "pick_cycles": [5, 30, 12], "best_kind": "plan",
         "sketch_seq": 9}
    p = Progress.from_json(d)
    assert (p.loop_run, p.loop_cycle, p.loop_restarts, p.loop_at_s, p.cycles_total) == (3, 37, 2, 12.5, 140)
    assert (p.loop_errors, p.loop_open, p.loop_vias, p.loop_length_mm, p.loop_shrinkages) == (4, 1, 50, 1234.5, 2)
    assert (p.runbest_run, p.runbest_cycle, p.runbest_kind, p.runbest_vias) == (2, 30, "plan", 48)
    assert (p.pick_cycles, p.best_kind, p.sketch_seq, p.last_at_s) == ([5, 30, 12], "plan", 9, 3.0)
    old = Progress.from_json({"phase": "route"})  # an engine without them
    assert (old.loop_run, old.cycles_total, old.pick_cycles, old.best_kind, old.sketch_seq) == (0, 0, [], "", 0)


# ---------------------------------------------------------------------------------------------- the run flow's poll


class FakeJob:
    def __init__(self, sketches):
        self.sketches, self.requests, self.fetched = sketches, 0, []

    def request_sketch(self):
        self.requests += 1

    def sketch(self, kind):
        self.fetched.append(kind)
        sk = self.sketches.get(kind)
        return json.dumps(sk) if sk else None


class FakeUi:
    def __init__(self, kind="last"):
        self.kind, self.got = kind, []

    def preview_wanted(self):
        return self.kind

    def sketch(self, kind, sk):
        self.got.append((kind, sk["seq"]))


def test_sketch_poll_asks_at_the_interval_and_hands_over_each_new_sketch_once():
    job = FakeJob({"last": {"seq": 1, "kind": "plan"}, "best": {"seq": 1, "kind": "plan"}})
    ui, sk = FakeUi(), {"asked": -1e9, "seq": -1, "kind": None, "sent": None}
    run_mod.sketch_poll(job, ui, Progress(sketch_seq=1), sk, 0.0, 2.0, False)
    assert job.requests == 1 and ui.got == [("last", 1)]
    run_mod.sketch_poll(job, ui, Progress(sketch_seq=1), sk, 1.0, 2.0, False)  # within the interval, nothing new
    assert job.requests == 1 and job.fetched == ["last"] and ui.got == [("last", 1)]
    run_mod.sketch_poll(job, ui, Progress(sketch_seq=1), sk, 2.5, 2.0, False)
    assert job.requests == 2 and job.fetched == ["last"]
    job.sketches["best"] = {"seq": 2, "kind": "plan"}  # a best-only publish: the last sketch is fetched, not re-sent
    run_mod.sketch_poll(job, ui, Progress(sketch_seq=2), sk, 2.6, 2.0, False)
    assert job.fetched == ["last", "last"] and ui.got == [("last", 1)]
    ui.kind = "best"  # the user switched: fetched at once
    run_mod.sketch_poll(job, ui, Progress(sketch_seq=2), sk, 2.7, 2.0, False)
    assert ui.got == [("last", 1), ("best", 2)]
    job.sketches["last"] = {"seq": 3, "kind": "copper"}
    ui.kind = "last"
    run_mod.sketch_poll(job, ui, Progress(sketch_seq=3), sk, 9.0, 2.0, True)  # done: no request, the last fetched
    assert job.requests == 2 and ui.got[-1] == ("last", 3)
    ui.kind = None  # off: nothing at all
    n = (job.requests, len(job.fetched))
    run_mod.sketch_poll(job, ui, Progress(sketch_seq=4), sk, 20.0, 2.0, False)
    assert (job.requests, len(job.fetched)) == n


def test_sketch_poll_refetches_when_the_best_or_last_run_moves_without_a_new_sketch():
    """verify-last-preview F5: the engine publishes a pick's sketch (seq 5) before it moves best_run / runbest_run, so the
    poll in between fetched the old run's sketch; when the run moves, the image of the Best so far / Last run row follows
    though sketch_seq did not change."""
    job = FakeJob({"best": {"seq": 4, "kind": "plan"}, "run": {"seq": 4, "kind": "plan"}})
    sk = {"asked": -1e9, "seq": -1, "kind": None, "sent": None}
    for kind, field in (("best", "best_run"), ("run", "runbest_run")):
        ui = FakeUi(kind)
        run_mod.sketch_poll(job, ui, Progress(sketch_seq=5, **{field: 1}), sk, 0.0, 2.0, False)
        assert ui.got == [(kind, 4)]
        job.sketches[kind] = {"seq": 5, "kind": "plan"}  # the moved run's sketch, same sketch_seq
        run_mod.sketch_poll(job, ui, Progress(sketch_seq=5, **{field: 1}), sk, 0.1, 2.0, False)
        assert ui.got == [(kind, 4)], "nothing moved: no refetch"
        run_mod.sketch_poll(job, ui, Progress(sketch_seq=5, **{field: 2}), sk, 0.2, 2.0, False)
        assert ui.got == [(kind, 4), (kind, 5)], f"{field} moved: the new run's image"


# ---------------------------------------------------------------------------------------------- the live image (pure)

MODEL = {
    "outline": [{"kind": "seg", "a": [0, 0], "b": [20_000_000, 0]}, {"kind": "seg", "a": [20_000_000, 0],
                "b": [20_000_000, 10_000_000]}, {"kind": "seg", "a": [20_000_000, 10_000_000], "b": [0, 10_000_000]},
                {"kind": "seg", "a": [0, 10_000_000], "b": [0, 0]}],
    "pads": [{"center": [2_000_000, 5_000_000], "layers": [0], "rotation_deg": 90.0,
              "shapes": {"0": {"kind": "rect", "w": 1_000_000, "h": 2_000_000}}},
             {"center": [18_000_000, 5_000_000], "layers": [0, 2], "drill": {"w": 400_000, "h": 400_000},
              "shapes": {"0": {"kind": "circle", "d": 1_000_000}, "2": {"kind": "circle", "d": 1_000_000}}},
             {"center": [10_000_000, 2_000_000], "layers": [2],
              "shapes": {"2": {"kind": "poly", "pts": [[9_500_000, 1_500_000], [10_500_000, 1_500_000],
                                                      [10_000_000, 2_500_000]]}}}],
    "kept": [{"layer": 2, "width": 250_000, "prim": {"kind": "seg", "a": [1_000_000, 9_000_000],
                                                    "b": [5_000_000, 9_000_000]}}],
    "vias": [{"at": [15_000_000, 8_000_000], "d": 600_000}],
}
SEG = {"net": 1, "layer": 0, "width": 200_000, "prim": {"kind": "seg", "a": [2_000_000, 5_000_000],
                                                       "b": [18_000_000, 5_000_000]}}
VIA = {"kind": "via", "net": 1, "at": [10_000_000, 5_000_000], "d": 600_000, "drill": 300_000, "layers": [0, 2]}


def test_live_svg_draws_the_board_and_the_sketch_in_the_outline_frame():
    base = preview_mod.live_base(MODEL)
    assert base["origin"] == (0, 0) and base["size"] == (20.0, 10.0)
    assert base["svg"].count("<polyline") == 5  # 4 outline edges + the kept track
    assert 'rotate(-90.000)' in base["svg"] and "<polygon" in base["svg"] and base["svg"].count("<circle") == 3
    svg = preview_mod.live_svg(base, [SEG, VIA])
    assert svg.startswith("<svg") and 'viewBox="-0.5000 -0.5000 21.0000 11.0000"' in svg
    assert 'points="2.0000,5.0000 18.0000,5.0000"' in svg and 'stroke-width="0.2000"' in svg
    assert '<circle cx="10.0000" cy="5.0000" r="0.3000"' in svg
    # the board is shifted to its outline's corner
    moved = {**MODEL, "outline": [{**e, "a": [e["a"][0] + 5_000_000, e["a"][1]], "b": [e["b"][0] + 5_000_000,
                                                                                     e["b"][1]]}
                                  for e in MODEL["outline"]]}
    assert preview_mod.live_base(moved)["origin"] == (5_000_000, 0)
    assert preview_mod.live_base({"pads": []}) is None


# ---------------------------------------------------------------------------------------------- the dialog

wx = pytest.importorskip("wx")
from topor import ui_wx  # noqa: E402


@pytest.fixture(scope="module")
def app():
    try:
        a = wx.App(False)
    except Exception as e:  # noqa: BLE001 - no display
        pytest.skip(f"no display for wx: {e}")
    yield a


def _loop(**kw):
    base = dict(phase="route", total=40, routed=40, runs=5, round=1, rounds=30, stage="run 3: TopoR loop on the topology",
                loop_run=3, loop_cycle=37, loop_restarts=2, loop_at_s=20.0, loop_errors=4, loop_open=1, loop_vias=50,
                loop_length_mm=1234.0, cycles_total=140, runbest_run=2, runbest_cycle=30, runbest_kind="plan",
                runbest_errors=3, runbest_vias=48, runbest_length_mm=1200.0, best_run=1, best_round=1, best_errors=2,
                best_vias=47, best_length_mm=1190.0, best_kind="plan", pick_cycles=[12, 30, 5])
    base.update(kw)
    return Progress(**base)


def test_score_rows_in_the_topor_loop_say_predicted_and_the_loop_round():
    rows, verdict = ui_wx.score_rows(_loop())
    assert rows["last"] == {"run": "3", "round": "37", "errors": "4", "open": "1", "vias": "50", "length": "1.234 m",
                            "source": "predicted"}
    assert rows["run"]["run"] == "2" and rows["run"]["round"] == "30" and rows["run"]["source"] == "predicted"
    # the best's round is the loop round its pick came from (run 1: round 12), not the whole-route round 1
    assert rows["best"]["run"] == "1" and rows["best"]["round"] == "12" and rows["best"]["source"] == "predicted"
    assert verdict == ui_wx.WORDING["progress"]["vs_predicted"]
    # the loops drew: a drawn round newer than the last loop round is shown as drawn, numbered as a drawing
    drawn = _loop(last_run=2, last_round=1, last_at_s=25.0, last_errors=3, last_vias=49, last_length_mm=1201.0,
                  last_vs_best="worse", last_decider="errors", last_delta=1.0, best_kind="copper",
                  runbest_kind="copper", runbest_cycle=1)
    rows, verdict = ui_wx.score_rows(drawn)
    assert rows["last"]["round"] == "draw 1" and rows["last"]["source"] == "drawn" and rows["last"]["run"] == "2"
    assert rows["best"]["round"] == "draw 1" and rows["best"]["source"] == "drawn"
    assert rows["run"]["round"] == "draw 1" and rows["run"]["source"] == "drawn"
    assert verdict == "Last round is worse than the best: errors +1."


def test_round_context_names_run_restart_round_and_when():
    t = time.mktime((2026, 11, 19, 14, 2, 11, 0, 0, -1))
    assert ui_wx.round_context(_loop(), t, t + 12) == "Last round finished 14:02:11 (12 s ago): run 3, restart 2, round 37"
    assert ui_wx.round_context(_loop(loop_restarts=0), t, t) == "Last round finished 14:02:11 (0 s ago): run 3, round 37"
    assert ui_wx.round_context(Progress(phase="route"), t) == ""


def test_replay_with_a_restart_never_goes_back(app):
    """The user's report: "the run/round displayed goes back to 1 / 1 after a while" (main 12699012: the whole-route
    round is 1 for every run of the TopoR loop, the best's run flips between runs, TW3 restarts). Replayed: two runs
    report loop rounds, run 2 restarts from the table, the best moves from run 2 to run 1, then the loops draw. The
    count shown only goes up, the context names the restart, no row silently shows round 1."""
    seq = [
        _loop(loop_run=2, loop_cycle=1, loop_restarts=0, cycles_total=1, best_run=2, pick_cycles=[0, 1]),
        _loop(loop_run=1, loop_cycle=1, loop_restarts=0, cycles_total=2, best_run=2, pick_cycles=[1, 1]),
        _loop(loop_run=2, loop_cycle=2, loop_restarts=1, cycles_total=3, best_run=2, pick_cycles=[1, 2]),
        _loop(loop_run=2, loop_cycle=3, loop_restarts=2, cycles_total=4, best_run=1, pick_cycles=[1, 2]),
        # a progress that lost the count (e.g. a stale snapshot): never shown as a step back
        _loop(loop_run=1, loop_cycle=2, loop_restarts=0, cycles_total=2, best_run=1, pick_cycles=[2, 2]),
        _loop(loop_run=1, loop_cycle=2, loop_restarts=0, cycles_total=5, best_run=1, pick_cycles=[2, 2],
              last_run=1, last_round=1, last_at_s=30.0, best_kind="copper", phase="route", stage="run 1: drawing"),
    ]
    dlg = ui_wx.ProgressDialog("eprom", 600.0, 30, preview="last")
    try:
        shown, ctx = [], []
        for p in seq:
            dlg.update(p)
            shown.append(int(dlg.iteration_value.GetLabel()))
            ctx.append(dlg.round_context.GetLabel())
        assert shown == [1, 2, 3, 4, 4, 5], shown
        assert all(a <= b for a, b in zip(shown, shown[1:]))
        assert ctx[3].endswith("run 2, restart 2, round 3"), ctx[3]
        best_round = [ui_wx.score_rows(p)[0]["best"]["round"] for p in seq]
        assert best_round == ["1", "1", "2", "1", "2", "draw 1"], best_round  # the pick's loop round, or a drawing
        assert dlg.iteration_value.GetToolTipText() == ui_wx.WORDING["progress"]["round_total_tip"]
    finally:
        dlg.Destroy()


def _sketch(seq, kind="plan", run=3, cycle=37, items=None):
    return {"run": run, "cycle": cycle, "kind": kind, "seq": seq, "at_s": 1.0, "errors": 0, "open": 0,
            "violations": 0, "vias": 1, "length_mm": 16.0, "items": items if items is not None else [SEG, VIA]}


def _pixels(dlg):
    return bytes(dlg.preview_image.GetBitmap().ConvertToImage().GetData())


def test_dialog_shows_the_last_round_by_default_and_switches_on_the_toggle(app):
    dlg = ui_wx.ProgressDialog("eprom", 600.0, 30, preview="last")
    try:
        assert dlg.TAB_ORDER[:3] == ("preview_last", "preview_run", "preview_best")
        assert dlg.preview_kind == "last" and dlg.preview_radio["last"].GetValue()
        assert dlg.preview_caption.GetLabel() == ui_wx.WORDING["progress"]["preview_waiting"]
        dlg.set_board(MODEL)
        blank = _pixels(dlg)
        dlg.set_sketch("last", _sketch(1))
        assert dlg.rendered == ("last", 1)
        assert dlg.preview_caption.GetLabel() == "Last round: run 3, round 37 — predicted plan, not drawn copper"
        first = _pixels(dlg)
        assert first != blank
        # the next round: another plan, the image changes
        dlg.set_sketch("last", _sketch(2, cycle=38, items=[{**SEG, "layer": 2}]))
        assert dlg.rendered == ("last", 2) and _pixels(dlg) != first
        bmp = dlg.preview_image.GetBitmap()
        assert bmp.GetWidth() <= dlg.preview_size.width and bmp.GetHeight() <= dlg.preview_size.height
        # a sketch of another kind is kept, not shown
        dlg.set_sketch("best", _sketch(3, kind="copper", run=1, cycle=1))
        assert dlg.rendered == ("last", 2)
        dlg.show_kind("best")
        assert dlg.preview_kind == "best" and dlg.preview_radio["best"].GetValue() and dlg.rendered == ("best", 3)
        assert dlg.preview_caption.GetLabel() == "Best so far: run 1, round draw 1 — drawn copper"
        dlg.show_kind("run")  # nothing of it yet
        assert dlg.preview_caption.GetLabel() == ui_wx.WORDING["progress"]["preview_waiting"]
        assert dlg.preview_time.GetLabel().startswith("Image of ")
    finally:
        dlg.Destroy()


def test_wx_ui_preview_plumbing(app, monkeypatch):
    class Quiet(ui_wx.WxUi):
        def _present(self, dlg):
            pass

        def _pump(self):
            pass

    ui = Quiet()
    assert ui.preview_wanted() is None, "no image before the pre-flight answered Route (not the routing run)"
    ui._limit_editable = True
    assert ui.preview_wanted() == "last" and ui.preview_interval_s() == 2.0  # the defaults
    ui.set_board(MODEL)
    ui.progress(_loop())
    dlg = ui._progress
    assert dlg.preview_kind == "last" and dlg._base is not None
    ui.sketch("last", _sketch(5))
    assert dlg.rendered == ("last", 5)
    dlg.show_kind("run")
    assert ui.preview_wanted() == "run"
    ui._close_progress()
    ui._preview_mode = "off"
    assert ui.preview_wanted() is None
    ui.progress(_loop())
    assert ui._progress.preview_kind is None and not hasattr(ui._progress, "preview_image")
    ui.sketch("last", _sketch(6))  # ignored without an image
    ui._close_progress()


@pytest.mark.parametrize("unreleased", [False, True])
def test_wx_preflight_reads_the_preview_settings_only_when_available(app, monkeypatch, unreleased):
    """since-gate in WxUi.preflight (verify-last-preview F2): the pinned topor.toml values (off, 7 s) reach the dialog
    only with TOPOR_UNRELEASED_SETTINGS=1; in this version (0.4.x) the image stays on its defaults (last, 2 s)."""
    from topor.ports import Decision, RunSummary
    if unreleased:
        monkeypatch.setenv("TOPOR_UNRELEASED_SETTINGS", "1")
    else:
        monkeypatch.delenv("TOPOR_UNRELEASED_SETTINGS", raising=False)
    cfg = Config.defaults().with_values({("run", "progress_preview"): "off", ("run", "progress_preview_interval_s"): 7.0})

    class Pre:
        def __init__(self, *a):
            pass

        def ShowModal(self):
            return wx.ID_OK

        def decision(self, proceed):
            return Decision(True)

        def Destroy(self):
            pass

    monkeypatch.setattr(ui_wx, "PreflightDialog", Pre)

    class Quiet(ui_wx.WxUi):
        def _present(self, dlg):
            pass

        def _pump(self):
            pass

    ui = Quiet()
    ui.preflight([], RunSummary(board="b"), cfg)
    ui._limit_editable = True
    if unreleased:
        assert ui.preview_wanted() is None and ui.preview_interval_s() == 7.0
    else:
        assert ui.preview_wanted() == "last" and ui.preview_interval_s() == 2.0


def test_post_run_preview_says_the_result_is_what_accept_writes():
    assert ui_wx.WORDING["preview"]["after_heading"] == "Result: the best so far, what Accept writes"


# ---------------------------------------------------------------------------------------------- preview-errors (live dots)


def test_live_svg_takes_the_engines_bare_xy_pairs_and_scales_the_dot_with_the_board():
    """E12/M1/M4: the engine's Sketch.errors_at is a list of bare [x, y] nm pairs (not dicts); the dot is at least
    ERROR_DOT_R_MM and grows with the board's longer side (about 6 px in the dialog's box)."""
    import re
    base = preview_mod.live_base(MODEL)  # 20 x 10 mm
    svg = preview_mod.live_svg(base, [], [[4_000_000, 5_000_000], [12_500_000, 2_000_000]])
    grp = svg[svg.index("topor-errors"):]
    dots = re.findall(r'<circle cx="([\d.]+)" cy="([\d.]+)" r="([\d.]+)"', grp)
    assert [(float(x), float(y)) for x, y, _ in dots] == [(4.0, 5.0), (12.5, 2.0)]
    assert float(dots[0][2]) == preview_mod.ERROR_DOT_R_MM  # small board: the floor
    big = {"origin": (0, 0), "size": (120.0, 80.0), "svg": ""}
    r = float(re.search(r'topor-errors.*?r="([\d.]+)"', preview_mod.live_svg(big, [], [[1_000_000, 1_000_000]]), re.S).group(1))
    assert r == pytest.approx(120.0 / 60.0) and r > preview_mod.ERROR_DOT_R_MM
    assert preview_mod.error_dot_radius(10.0) == preview_mod.ERROR_DOT_R_MM and preview_mod.error_dot_radius(240.0) == 2.0


def test_dialog_hands_the_sketch_error_places_to_the_image(app, monkeypatch):
    """M7: ProgressDialog._render_preview passes sk["errors_at"] to live_svg (a sketch without it passes None)."""
    seen = []
    real = preview_mod.live_svg
    monkeypatch.setattr(preview_mod, "live_svg", lambda base, items, errors=None, **kw: (seen.append(errors), real(base, items, errors, **kw))[1])
    dlg = ui_wx.ProgressDialog("eprom", 600.0, 30, preview="last")
    try:
        dlg.set_board(MODEL)
        dlg.set_sketch("last", {**_sketch(1, kind="copper"), "errors_at": [[4_000_000, 5_000_000]]})
        dlg.set_sketch("last", _sketch(2))
        assert seen == [[[4_000_000, 5_000_000]], None]
    finally:
        dlg.Destroy()


# ---------------------------------------------------------------------------------------------- plan-dots (plan sketch dots)


def _dot_r(svg):
    import re
    return float(re.search(r'topor-errors.*?r="([\d.]+)"', svg, re.S).group(1))


def test_plan_sketch_dots_are_smaller_than_copper_dots_same_yellow():
    """plan-dots (user 2026-11-21 "the yellow markers a bit smaller on the route finder preview"): a plan sketch's dots
    are PLAN_DOT_SCALE (0.6) of the copper dots' radius, at every board size, in the same yellow."""
    assert preview_mod.PLAN_DOT_SCALE == 0.6 and preview_mod.PLAN_DOT_R_MM == pytest.approx(0.6 * preview_mod.ERROR_DOT_R_MM)
    for base in (preview_mod.live_base(MODEL), {"origin": (0, 0), "size": (120.0, 80.0), "svg": ""}):
        pts = [[1_000_000, 1_000_000]]
        copper = preview_mod.live_svg(base, [], pts)
        plan = preview_mod.live_svg(base, [], pts, plan=True)
        assert _dot_r(plan) == pytest.approx(0.6 * _dot_r(copper), abs=1e-4) and _dot_r(plan) < _dot_r(copper)
        fill = 'fill="#%02X%02X%02X"' % preview_mod.ERROR_DOT_RGB
        assert fill in plan[plan.index("topor-errors") - 12:] and fill in copper[copper.index("topor-errors") - 12:]
    assert _dot_r(preview_mod.live_svg(preview_mod.live_base(MODEL), [], [[1, 1]], plan=True)) == pytest.approx(preview_mod.PLAN_DOT_R_MM, abs=1e-4)
    assert preview_mod.live_dot_radius(10.0, True) == pytest.approx(preview_mod.PLAN_DOT_R_MM)
    assert preview_mod.live_dot_radius(10.0, False) == preview_mod.ERROR_DOT_R_MM


def test_dots_legend_names_plan_and_copper_dots():
    """plan-dots: the legend line names what the dots are: the plan's predicted errors (all placed, or n of the
    counted ones, the rest without a place), the drawn copper's rule violations; none without dots."""
    W = ui_wx.WORDING["progress"]
    leg = ui_wx.sketch_dots_legend
    assert leg(None) == "" and leg(_sketch(1)) == "" and leg(_sketch(1, kind="copper")) == ""
    plan = {**_sketch(1), "violations": 2, "errors_at": [[1, 2], [3, 4]]}
    assert leg(plan) == W["preview_dots_plan"].format(n=2) == "Small yellow dots: predicted errors of the plan (2)"
    part = {**_sketch(1), "violations": 3, "errors_at": [[1, 2]]}
    assert leg(part) == W["preview_dots_plan_part"].format(n=1, total=3) and "(1 of 3;" in leg(part)
    assert "no place" in leg({**_sketch(1), "violations": 2, "errors_at": []})
    cu = {**_sketch(1, kind="copper"), "violations": 1, "errors_at": [[1, 2]]}
    assert leg(cu) == "Yellow dots: rule violations of the drawn copper (1)"


def test_dialog_draws_plan_dots_small_and_shows_the_legend(app, monkeypatch):
    """plan-dots: ProgressDialog._render_preview tells live_svg whether the sketch is a plan, and the legend line
    under the image follows the sketch shown."""
    seen = []
    real = preview_mod.live_svg
    monkeypatch.setattr(preview_mod, "live_svg", lambda base, items, errors=None, **kw: (seen.append(kw.get("plan")), real(base, items, errors, **kw))[1])
    dlg = ui_wx.ProgressDialog("eprom", 600.0, 30, preview="last")
    try:
        dlg.set_board(MODEL)
        dlg.set_sketch("last", {**_sketch(1), "violations": 1, "errors_at": [[4_000_000, 5_000_000]]})
        assert dlg.preview_dots.GetLabel() == "Small yellow dots: predicted errors of the plan (1)"
        dlg.set_sketch("last", {**_sketch(2, kind="copper"), "violations": 1, "errors_at": [[4_000_000, 5_000_000]]})
        assert dlg.preview_dots.GetLabel() == "Yellow dots: rule violations of the drawn copper (1)"
        dlg.set_sketch("last", _sketch(3, kind="copper"))
        assert dlg.preview_dots.GetLabel().strip() == ""
        assert seen == [True, False, False]
    finally:
        dlg.Destroy()
