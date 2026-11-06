"""wx dialogs (B9). Dialogs are constructed but NEVER shown (Ruling C18: DISPLAY may be the live desktop)."""
import re
from pathlib import Path

import pytest

wx = pytest.importorskip("wx")

from topor import ui_wx  # noqa: E402
from topor.config import Config  # noqa: E402
from topor.findings import Finding  # noqa: E402
from topor.ports import Decision, Progress, RunSummary  # noqa: E402

NOTE = Path(__file__).resolve().parents[2] / "docs" / "progress" / "notes" / "m4-ui-wireframes.md"
B8_PREFIXES = ("md_", "sec_", "sum_", "outcome_")  # rows owned by the run report, not by dialogs


@pytest.fixture(scope="module")
def app():
    try:
        a = wx.App(False)
    except Exception as e:  # no display
        pytest.skip(f"no display for wx: {e}")
    if not wx.Display.GetCount():
        pytest.skip("no display for wx")
    yield a


class FakeReport:
    def __init__(self, d):
        self.dir = d

    def summary_lines(self):
        return ["Topor 0.4.0 — b — t", "Routed 27/27 connections · 12 nets · 2 layers"] + ["x"] * 5


FINDINGS = [
    Finding("DEFAULTS_USED", "info", "3 built-in defaults in use", "none"),
    Finding("PAD_OUTSIDE_BOARD", "warning", "2 pads lie outside the board outline",
            "Move the parts inside the outline.", tuple(f"U{i}.1" for i in range(12))),
    Finding("VIA_NOT_SET_UP", "warning", "No via sizes set up", "Set vias in Board Setup."),
]
SUMMARY = RunSummary(27, 12, 2, "0.4.0", board="eprom", has_selection=True)


def wording_rows():
    out, dialog = {}, None
    for line in NOTE.read_text().split("## Wording", 1)[1].splitlines():
        if line.startswith("### "):
            dialog = line[4:].strip()
        elif line.startswith("| ") and dialog and not line.startswith("| dialog") and not line.startswith("|---"):
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            d, wid, kind, text = cells[0], cells[1], cells[2], "|".join(cells[3:])
            out.setdefault(d, []).append((wid, kind, text.replace("\\n", "\n")))
    return out


def _regex(template):
    parts = re.split(r"\{[a-z_0-9]+\}", template)
    return re.compile("(?s)^" + ".*".join(re.escape(p) for p in parts) + "$")


def _texts(w):
    if isinstance(w, wx.Choice):
        return list(w.GetStrings())
    if isinstance(w, wx.TextCtrl):
        return [w.GetValue()]
    return [re.sub(r"\s+", " ", wx.Control.RemoveMnemonics(w.GetLabel()))]  # Wrap() inserts newlines


def _build_all(tmp_path):
    png = tmp_path / "p.png"
    wx.Image(40, 20).SaveFile(str(png), wx.BITMAP_TYPE_PNG)
    rd = tmp_path / "rep"
    rd.mkdir()
    ctx = ui_wx.UiContext(report_dir=rd, log_path=rd / "log.txt",
                          versions={"engine": "0.4.0", "plugin": "0.4.0", "kipy": "0.8", "kicad": "10.0.8"})
    prog = ui_wx.ProgressDialog("eprom", 60.0, 30)
    prog.update(Progress("route", 3, 12, 27, 4, 12.0))
    return {
        "first_run": ui_wx.FirstRunDialog(tmp_path / "topor.toml"),
        "preflight": ui_wx.PreflightDialog(FINDINGS, SUMMARY, Config.defaults()),
        "progress": prog,
        "preview": ui_wx.PreviewDialog(png, FakeReport(rd), "eprom"),
        "report": ui_wx.DoneDialog("accepted", FakeReport(rd), rd),
        "error": ui_wx.ErrorDialog(Finding("KICAD_CLI_MISSING", "error", "kicad-cli was not found.", "Install it.",
                                           ("a", "b")), ctx),
    }


def test_wx_ui_dialogs_construct_headless(app, tmp_path):
    dlgs = _build_all(tmp_path)
    expected_tab = {
        "first_run": ["toml_path", "open_file", "continue"],
        "preflight": ["findings", "cause", "accept", "scope_whole", "scope_between",
                      "scope_touching", "existing", "respect_locked", "partial_routes", "timeout", "threads",
                      "save_toml", "help", "cancel", "route"],
        "progress": ["cancel"],
        "preview": ["zoom", "image", "summary", "report_dir", "open_report_folder", "reject", "accept"],
        "report": ["report_dir", "open_report_folder", "close"],
        "error": ["details", "copy_details", "open_report_folder", "close"],
    }
    mins = {"first_run": (440, 200), "preflight": (640, 560), "progress": (460, 240), "preview": (800, 520),
            "report": (440, 220), "error": (520, 320)}
    default = {"first_run": wx.ID_OK, "preflight": wx.ID_OK, "progress": wx.ID_CANCEL, "preview": wx.ID_OK,
               "report": wx.ID_OK, "error": wx.ID_OK}
    esc = {**{k: wx.ID_OK for k in ("first_run", "report", "error")}, "preflight": wx.ID_CANCEL,
           "progress": wx.ID_CANCEL, "preview": wx.ID_CANCEL}
    try:
        for name, d in dlgs.items():
            order = [n for n in d.child_ids() if n in expected_tab[name]]
            assert order == expected_tab[name], name
            assert tuple(d.TAB_ORDER) == tuple(expected_tab[name]), name
            assert d.GetEscapeId() == esc[name], name
            assert d.GetDefaultItem().GetId() == default[name], name
            assert d.GetAffirmativeId() == default[name] if name != "progress" else True
            mn = d.FromDIP(wx.Size(*mins[name]))
            assert d.GetSize().width >= mn.width and d.GetSize().height >= mn.height, name
            assert not d.IsShown(), name
        # preflight specifics: findings sorted warn, warn, note; row 1 selected; cause pane filled
        p = dlgs["preflight"]
        assert [f.code for f in p.findings] == ["PAD_OUTSIDE_BOARD", "VIA_NOT_SET_UP", "DEFAULTS_USED"]
        assert p.list.GetFirstSelected() == 0
        assert "and 2 more (all in the report)" in p.cause.GetValue()
        assert p.accept_cb.IsShown() and "ignore_pads" in p.accept_cb.GetLabel()
        p.list.Select(1)  # VIA_NOT_SET_UP: not acceptable -> checkbox hidden
        p._show_finding(1)
        assert not p.accept_cb.IsShown()
        p._show_finding(0)
        assert p.route.IsEnabled() and not p.status.IsShown()
        # mnemonics: every button carries one
        for b in p.buttons.values():
            if b.GetName() != "cancel":
                assert "&" in b.GetLabel()
        assert "&" in dlgs["preview"].buttons["accept"].GetLabel()
        # decision round trip: defaults change nothing; changes show up as overrides
        assert p.decision(False) == Decision(False)
        d0 = p.decision(True)
        assert d0.proceed and d0.accepted_codes == () and d0.overrides == {"scope_whole": False}
        p.accept_cb.SetValue(True)
        p._on_accept(None)
        p.threads.SetValue(4)
        p.save_toml.SetValue(True)
        d1 = p.decision(True)
        assert d1.accepted_codes == ("PAD_OUTSIDE_BOARD",) and d1.overrides["threads"] == 4 and d1.save_to_toml
    finally:
        for d in dlgs.values():
            d.Destroy()


def test_wx_ui_wording_matches_wireframes(app, tmp_path):
    rows = wording_rows()
    assert set(rows) == set(ui_wx.WORDING) | {"report"}
    dlgs = _build_all(tmp_path)
    found = 0
    try:
        for dname, rs in rows.items():
            dlg = dlgs[dname]
            for wid, kind, text in rs:
                if wid.startswith(B8_PREFIXES):
                    continue
                assert ui_wx.WORDING[dname][wid] == text, (dname, wid)  # the code's string table = the note
                rx = _regex(text)
                if wid == "title":
                    assert rx.match(dlg.GetTitle()), (dname, wid, dlg.GetTitle())
                    found += 1
                    continue
                w = dlg.widget(wid)
                if w is not None and wid != "dialog":
                    assert any(rx.match(t) for t in _texts(w)), (dname, wid, _texts(w))
                    found += 1
                elif kind == "choice":
                    strings = [s for c in dlg.GetChildren() if isinstance(c, wx.Choice) for s in c.GetStrings()]
                    assert text in strings, (dname, wid)
                    found += 1
                else:
                    assert kind in ("text", "label", "heading", "button", "checkbox") and dname in (
                        "preflight", "progress", "error", "report", "preview"), (dname, wid)
                    # buttons / checkboxes must exist as widgets
                    assert kind not in ("button",) or wid in ("copied",), (dname, wid)
        assert found > 70
        # list columns + severity words live in the findings list
        lst = dlgs["preflight"].list
        cols = [lst.GetColumn(i).GetText() for i in range(3)]
        assert cols == ["Severity", "Code", "Message"]
        assert [lst.GetItemText(i) for i in range(3)] == ["Warn", "Warn", "Note"]
    finally:
        for d in dlgs.values():
            d.Destroy()


class _QuietUi(ui_wx.WxUi):
    shown = 0

    def _present(self, dlg):  # never Show() a real window under test
        self.shown += 1

    def _pump(self):
        pass


def test_progress_returns_false_after_cancel(app):
    ui = _QuietUi()
    assert ui.progress(Progress("route", 3, 12, 27, 4, 12.0)) is True
    dlg = ui._progress
    assert dlg.routed_value.GetLabel() == "12 / 27" and dlg.elapsed_value.GetLabel() == "0:12 / 10:00"
    assert dlg.iteration_value.GetLabel() == "3 of up to 30"  # no round from the engine: [optimize] rounds as cap
    assert dlg.gauge.GetValue() == 12
    evt = wx.CommandEvent(wx.wxEVT_BUTTON, wx.ID_CANCEL)
    evt.SetEventObject(dlg.cancel_button)
    dlg.cancel_button.GetEventHandler().ProcessEvent(evt)
    assert dlg.phase.GetLabel() == ui_wx.WORDING["progress"]["phase_cancelling"]
    assert not dlg.cancel_button.IsEnabled()
    assert ui.progress(Progress("route", 4, 13, 27, 4, 13.0)) is False
    assert ui.progress(Progress("drc", 0, 13, 27, 4, 14.0)) is False
    assert ui.shown == 1  # one dialog, shown once
    ui._close_progress()
    assert ui._progress is None


def test_progress_phase_pending_and_nolimit(app):
    ui = _QuietUi()
    ui._timeout_s = 0.0
    assert ui.progress(Progress("setup", 0, 0, 27, 0, 1.0)) is True
    d = ui._progress
    assert d.phase.GetLabel() == "Preparing geometry"
    assert d.routed_value.GetLabel() == "– / 27"
    assert d.elapsed_value.GetLabel() == "0:01 / no limit"
    ui.progress(Progress("weird", 1, 1, 27, 0, 2.0))
    assert d.phase.GetLabel() == "weird"
    ui._close_progress()


def test_preflight_error_disables_route(app):
    err = Finding("CFG_BAD", "error", "bad value", "fix it")
    d = ui_wx.PreflightDialog([err, FINDINGS[0]], RunSummary(board="b"), Config.defaults())
    try:
        assert not d.route.IsEnabled()
        assert d.status.IsShown() and d.status.GetLabel() == "Cannot route: 1 error(s) — see Fix above."
        assert not d.scope_between.IsEnabled() and d.scope_whole.GetValue()  # nothing selected
        assert d.decision(True).overrides == {}  # no selection -> no scope keys
        empty = ui_wx.PreflightDialog([], RunSummary(board="b"), Config.defaults())
        try:
            assert empty.cause.GetValue() == ui_wx.WORDING["preflight"]["no_findings"] and empty.route.IsEnabled()
        finally:
            empty.Destroy()
    finally:
        d.Destroy()


def test_preview_missing_image_keeps_accept(app, tmp_path):
    d = ui_wx.PreviewDialog(tmp_path / "nope.png", FakeReport(tmp_path / "unwritten"), "b")
    try:
        assert d.widget("no_image") is not None
        assert d.ok_button.IsEnabled() and d.open_btn.IsEnabled()
    finally:
        d.Destroy()


def test_accept_checkbox_relabels_and_keeps_state(app):
    fs = [Finding("PAD_OUTSIDE_BOARD", "warning", "m", "f"), Finding("DRILL_OUTSIDE_FILTER", "warning", "m", "f")]
    d = ui_wx.PreflightDialog(fs, RunSummary(board="b"), Config.defaults())
    try:
        W = ui_wx.WORDING["preflight"]
        got = {}
        for i, f in enumerate(d.findings):
            d._show_finding(i)
            got[f.code] = wx.Control.RemoveMnemonics(d.accept_cb.GetLabel())
        assert got == {"DRILL_OUTSIDE_FILTER": W["accept_drills"], "PAD_OUTSIDE_BOARD": W["accept_pads"]}
        d._show_finding(0)
        d.accept_cb.SetValue(True)
        d._on_accept(None)
        d._show_finding(1)
        assert not d.accept_cb.GetValue()
        d._show_finding(0)
        assert d.accept_cb.GetValue()
        assert d.decision(True).accepted_codes == (d.findings[0].code,)
    finally:
        d.Destroy()


def test_scope_count_follows_radio_and_done_variants(app, tmp_path):
    s = RunSummary(10, 2, 2, "v", board="b", has_selection=True,
                   connections_by_scope={"whole": 10, "between": 4, "touching": 7})
    d = ui_wx.PreflightDialog([], s, Config.defaults())
    try:
        assert d.summary_label.GetLabel().startswith("4 connections")
        d.scope_touching.SetValue(True)
        d._update_count()
        assert d.summary_label.GetLabel().startswith("7 connections")
        d.scope_whole.SetValue(True)
        d._update_count()
        assert d.summary_label.GetLabel().startswith("10 connections")
    finally:
        d.Destroy()
    rep = FakeReport(tmp_path)
    rep.stop_reason = "cancel"
    for outcome, wid in (("accepted", "done_accepted_cancelled"), ("rejected", "done_rejected"),
                         ("cancelled", "done_cancelled")):
        dd = ui_wx.DoneDialog(outcome, rep, tmp_path)
        try:
            w = dd.widget(wid)
            assert w is not None, wid
            assert _regex(ui_wx.WORDING["report"][wid]).match(re.sub(r"\s+", " ", w.GetLabel()))
        finally:
            dd.Destroy()


def test_done_dialog_refill_hint_and_post_commit_warning(app, tmp_path):
    """Ruling C28: after Accept the done notice says whether zones were refilled (press B when not); ruling C30:
    a step that failed after the commit is shown there too."""
    W = ui_wx.WORDING["report"]
    cases = (({"zones_refilled": False}, "done_refill_hint"), ({"zones_refilled": True}, "done_refilled"))
    for facts, wid in cases:
        rep = FakeReport(tmp_path)
        rep.facts = facts
        dd = ui_wx.DoneDialog("accepted", rep, tmp_path)
        try:
            w = dd.widget(wid)
            assert w is not None, wid
            assert re.sub(r"\s+", " ", w.GetLabel()) == W[wid]
            other = "done_refilled" if wid == "done_refill_hint" else "done_refill_hint"
            assert dd.widget(other) is None
        finally:
            dd.Destroy()
    assert "press B (Edit → Fill All Zones)" in W["done_refill_hint"]
    rep = FakeReport(tmp_path)
    rep.facts = {"zones_refilled": False, "post_commit_failed": [Finding(
        "POST_COMMIT_STEP_FAILED", "warning", "The routes were committed, then zone refill failed: AS_BUSY.",
        "Ctrl+Z (Edit → Undo) removes the routes.")]}
    dd = ui_wx.DoneDialog("accepted", rep, tmp_path)
    try:
        w = dd.widget("done_post_commit")
        assert w is not None and "zone refill failed: AS_BUSY" in re.sub(r"\s+", " ", w.GetLabel())
        assert "Ctrl+Z" in re.sub(r"\s+", " ", w.GetLabel())
    finally:
        dd.Destroy()
    dd = ui_wx.DoneDialog("rejected", rep, tmp_path)  # nothing applied: no refill hint
    try:
        assert dd.widget("done_refill_hint") is None and dd.widget("done_post_commit") is None
    finally:
        dd.Destroy()


def test_elapsed_format_hours():
    assert ui_wx._fmt_time(72) == "1:12" and ui_wx._fmt_time(3725) == "1:02:05"


class _Key:
    """Minimal wx.KeyEvent stand-in for the char hooks."""

    def __init__(self, code, mods=False):
        self.code, self.mods, self.skipped = code, mods, False

    def GetKeyCode(self):
        return self.code

    def HasAnyModifiers(self):
        return self.mods

    def Skip(self):
        self.skipped = True


def test_preview_enter_accepts_while_zoom_has_focus(app, tmp_path, monkeypatch):
    """Live test (tools/gui_e2e.sh, K1): Enter in the preview did nothing while the Zoom choice had the focus."""
    d = _build_all(tmp_path)
    try:
        p = d["preview"]
        monkeypatch.setattr(wx.Window, "FindFocus", staticmethod(lambda: p.zoom))
        ended = []
        p.EndModal = ended.append
        p._on_char_hook(_Key(wx.WXK_RETURN))
        assert ended == [wx.ID_OK]
        ev = _Key(wx.WXK_ESCAPE)
        p._on_char_hook(ev)
        assert ev.skipped and ended == [wx.ID_OK]
        monkeypatch.setattr(wx.Window, "FindFocus", staticmethod(lambda: p.buttons["reject"]))
        ev = _Key(wx.WXK_RETURN)
        p._on_char_hook(ev)  # a focused button activates itself
        assert ev.skipped and ended == [wx.ID_OK]
    finally:
        for x in d.values():
            x.Destroy()


def test_buttons_never_narrower_than_their_label(app, tmp_path):
    """Live test (K1): a fixed 88 DIP width cut "Open report folder" to "pen report fold"."""
    d = _build_all(tmp_path)
    try:
        for name in ("preview", "report", "error"):
            for b in d[name].buttons.values():
                assert b.GetMinSize().width >= b.GetBestSize().width, (name, b.GetLabel())
    finally:
        for x in d.values():
            x.Destroy()


def test_progress_names_the_engine_stage_round_and_times(app):
    """K1 (honest about long runs): the topological core's stage and round, router time against the limit and
    the total wall time are shown; the round stays while the plugin's own DRC phase runs."""
    dlg = ui_wx.ProgressDialog("qmtech", 600.0, 4)
    try:
        dlg.update(Progress("route", 2, 120, 426, 30, 75.0, "drawing the traces", 2, 4, 90.0))
        assert dlg.phase.GetLabel() == "Routing — drawing the traces"
        assert dlg.iteration_value.GetLabel() == "2"  # M-B0: [optimize] rounds is only a safety cap
        assert dlg.elapsed_value.GetLabel() == "1:15 / 10:00" and dlg.total_value.GetLabel() == "1:30"
        dlg.update(Progress("drc", 4, 346, 426, 187, 150.0, "", 0, 0, 170.0))
        assert dlg.phase.GetLabel() == ui_wx.WORDING["progress"]["phase_drc"]
        assert dlg.iteration_value.GetLabel() == "2"
    finally:
        dlg.Destroy()


def test_progress_routes_stable_turns_cancel_into_ok(app):
    """M-B0 (user 2026-11-06): when the gain is below the threshold the dialog shows "Routes stable" and the
    Cancel button reads OK; same action (stop, keep the best result so far, which the dialog shows)."""
    ui = _QuietUi()
    p = Progress("route", 1, 300, 426, 150, 40.0, "drawing the traces", 3, 30, 45.0, gain_pct=2.5, runs=5,
                 runs_stable=2, best_routed=341, best_total=426, best_length_mm=2310.0, best_vias=180, best_run=3)
    assert ui.progress(p) is True
    dlg = ui._progress
    W = ui_wx.WORDING["progress"]
    assert dlg.phase.GetLabel() == "Routing — drawing the traces"
    assert dlg.cancel_button.GetLabel().replace("&", "") == "Cancel"
    assert dlg.gain_value.GetLabel() == "2.5 % / 4 rounds"
    assert dlg.runs_value.GetLabel() == "2 of 5 stable"
    assert dlg.best_value.GetLabel() == "Best so far: 341 / 426 routed, 2.31 m, 180 vias (run 3)"
    q = Progress("route", 1, 300, 426, 150, 60.0, "topological search", 7, 30, 65.0, stable=True, gain_pct=0.1,
                 runs=5, runs_stable=5, best_routed=346, best_total=426, best_length_mm=2300.0, best_vias=181,
                 best_run=1)
    assert ui.progress(q) is True
    assert dlg.phase.GetLabel() == W["phase_stable"] == "Routes stable"
    assert dlg.cancel_button.GetLabel().replace("&", "") == "OK"
    assert dlg.GetTitle().endswith("— Routes stable")  # also in the window list (the live test waits for it)
    assert dlg.cancel_button.GetId() == wx.ID_CANCEL  # same button, same action
    assert dlg.best_value.GetLabel() == "Best so far: 346 / 426 routed, 2.30 m, 181 vias (run 1)"
    evt = wx.CommandEvent(wx.wxEVT_BUTTON, wx.ID_CANCEL)
    evt.SetEventObject(dlg.cancel_button)
    dlg.cancel_button.GetEventHandler().ProcessEvent(evt)
    assert dlg.phase.GetLabel() == W["phase_finishing"]
    assert ui.progress(q) is False
    ui._close_progress()
