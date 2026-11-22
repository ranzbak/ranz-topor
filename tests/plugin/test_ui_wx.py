"""wx dialogs (B9). Dialogs are constructed but NEVER shown (Ruling C18: DISPLAY may be the live desktop)."""
import json
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
        "error": ui_wx.ErrorDialog(Finding("KICAD_CLI_MISSING", "error", "kicad-cli was not found.", "Install it.",
                                           ("a", "b")), ctx),
    }


def test_wx_ui_dialogs_construct_headless(app, tmp_path):
    dlgs = _build_all(tmp_path)
    expected_tab = {
        "first_run": ["toml_path", "open_file", "continue"],
        "preflight": ["findings", "cause", "accept", "use_default", "scope_whole", "scope_between",
                      "scope_touching", "existing", "respect_locked", "drawing_style", "grid_snap", "timeout", "timeout_clock",
                      "save_toml", "help", "cancel", "route"],
        "progress": ["limit", "limit_set", "limit_add5", "limit_add15", "cancel", "accept"],
        "preview": ["zoom", "image_before", "image_after", "summary", "findings", "cause", "report_dir",
                    "open_report_folder", "reject", "accept"],
        "error": ["details", "copy_details", "open_report_folder", "close"],
    }
    mins = {"first_run": (440, 200), "preflight": (640, 420), "progress": (460, 240), "preview": (800, 560),
            "error": (520, 320)}
    default = {"first_run": wx.ID_OK, "preflight": wx.ID_OK, "progress": wx.ID_OK, "preview": wx.ID_OK,
               "error": wx.ID_OK}
    esc = {**{k: wx.ID_OK for k in ("first_run", "error")}, "preflight": wx.ID_CANCEL,
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
        assert all("&" in b.GetLabel() for b in dlgs["progress"].buttons.values())  # Accept and Cancel
        assert "&" in dlgs["preview"].buttons["accept"].GetLabel()
        # decision round trip: defaults change nothing; changes show up as overrides
        assert p.decision(False) == Decision(False)
        d0 = p.decision(True)
        assert d0.proceed and d0.accepted_codes == () and d0.overrides == {"scope_whole": False}
        assert d0.use_defaults == () and not p.use_default_cb.IsShown()  # no pinned line among FINDINGS
        p.accept_cb.SetValue(True)
        p._on_accept(None)
        p.timeout.SetValue(30)
        p.save_toml.SetValue(True)
        d1 = p.decision(True)
        assert d1.accepted_codes == ("PAD_OUTSIDE_BOARD",) and d1.overrides["timeout_s"] == 30.0 and d1.save_to_toml
        # trace shape (user 2026-11-07: "make it kicad accessible"): default octilinear, smooth is an override
        assert p.drawing_style.GetSelection() == 0 and "drawing_style" not in d1.overrides
        p.drawing_style.SetSelection(1)
        assert p.decision(True).overrides["drawing_style"] == "smooth"
        # grid snap tick: default off = unticked, no override; ticking = prefer
        assert p.grid_snap.Get3StateValue() == wx.CHK_UNCHECKED and "grid_snap" not in d1.overrides
        p.grid_snap.Set3StateValue(wx.CHK_CHECKED)
        assert p.decision(True).overrides["grid_snap"] == "prefer"
        assert p.grid_line.GetLabel() == "Grid: not known (headless run)"
    finally:
        for d in dlgs.values():
            d.Destroy()


def test_wx_ui_wording_matches_wireframes(app, tmp_path):
    rows = wording_rows()
    rows.pop("report", None)  # the note's historical "Finished" section: the window is gone (user 2026-11-19)
    assert set(rows) == set(ui_wx.WORDING)
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
                        "preflight", "progress", "error", "preview"), (dname, wid)
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
    assert dlg.routed_value.GetLabel() == "12 / 27" and dlg.elapsed_value.GetLabel() == "0:12 / 10:00 · CPU 0:00"
    assert dlg.iteration_value.GetLabel() == "3 of up to 30"  # no round from the engine: [optimize] rounds as cap
    assert dlg.gauge.GetValue() == 12
    evt = wx.CommandEvent(wx.wxEVT_BUTTON, wx.ID_CANCEL)
    evt.SetEventObject(dlg.cancel_button)
    dlg.cancel_button.GetEventHandler().ProcessEvent(evt)
    assert dlg.phase.GetLabel() == "Cancel pressed: stopping, nothing is taken… 0 s"
    assert not dlg.cancel_button.IsEnabled() and not dlg.accept_button.IsEnabled()
    assert ui.stop_discards() is True and ui.ok_at_press() is False
    dlg.cancelled_at -= 12.4
    assert ui.progress(Progress("route", 4, 13, 27, 4, 13.0, "drawing the traces")) is False
    assert dlg.phase.GetLabel() == "Cancel pressed: stopping, nothing is taken… 12 s"
    assert ui.progress(Progress("route", 4, 13, 27, 4, 13.0)) is False
    assert ui.progress(Progress("drc", 0, 13, 27, 4, 14.0)) is False
    assert dlg.phase.GetLabel() == ui_wx.WORDING["progress"]["phase_drc"]  # the DRC of the result keeps its label
    assert ui.shown == 1  # one dialog, shown once


def test_progress_accept_keeps_the_best_and_names_the_stage_cancel_after_it_drops(app):
    """User 2026-11-22: Accept = stop and take the best so far (the engine finishes its stage; the label names it);
    ``stop_discards`` stays False. A Cancel after the Accept still drops the run."""
    ui = _QuietUi()
    assert ui.progress(Progress("route", 3, 12, 27, 4, 12.0)) is True
    dlg = ui._progress
    assert ui.stop_discards() is False and ui.ok_at_press() is None
    _press(dlg.accept_button)
    assert dlg.phase.GetLabel() == "Accept pressed: finishing the current stage… 0 s"
    assert not dlg.accept_button.IsEnabled() and dlg.cancel_button.IsEnabled()  # Cancel stays possible
    dlg.cancelled_at -= 12.4
    assert ui.progress(Progress("route", 4, 13, 27, 4, 13.0, "drawing the traces")) is False
    assert dlg.phase.GetLabel() == "Accept pressed: finishing the current stage (drawing the traces)… 12 s"
    assert ui.stop_discards() is False and ui.ok_at_press() is False  # not stable at the press
    _press(dlg.cancel_button)
    assert ui.stop_discards() is True and not dlg.cancel_button.IsEnabled()
    ui._close_progress()


class _Key:
    def __init__(self, code):
        self.code, self.skipped = code, False

    def GetKeyCode(self):
        return self.code

    def Skip(self):
        self.skipped = True


def test_progress_keys_esc_cancels_enter_never_stops_the_run(app):
    """Esc = Cancel. Enter never presses Accept or Cancel, whatever has the focus (a stray Enter during an unattended run
    must neither take a result nor drop the run); in the limit field it sets the limit, on a limit button it is passed on."""
    ui = _QuietUi()
    ui.progress(Progress("route", 1, 1, 27, 0, 1.0))
    dlg = ui._progress
    assert dlg.GetEscapeId() == wx.ID_CANCEL and not dlg.cancel_button.HasFocus()  # never starts on Cancel
    for btn in (dlg.accept_button, dlg.cancel_button):
        btn.SetFocus()
        e = _Key(wx.WXK_RETURN)
        dlg._on_key(e)
        assert not dlg.cancelled and not e.skipped, "Enter on " + btn.GetName() + " does nothing"
    dlg.limit_set.Enable()
    dlg.limit_set.SetFocus()
    e = _Key(wx.WXK_RETURN)
    dlg._on_key(e)
    assert e.skipped and not dlg.cancelled, "Enter on a limit button is the native press"
    e = _Key(wx.WXK_ESCAPE)
    dlg._on_key(e)
    assert dlg.discarded and dlg.cancelled and ui.stop_discards()
    ui._close_progress()


def _press(btn):
    evt = wx.CommandEvent(wx.wxEVT_BUTTON, btn.GetId())
    evt.SetEventObject(btn)
    btn.GetEventHandler().ProcessEvent(evt)


def test_progress_time_limit_buttons_and_field(app):
    """User 2026-11-07: the time limit can be changed while the router runs (+5 / +15 min, a new value, 0 = none,
    lower down to the time spent = stop now); the request waits until the run flow takes it."""
    ui = _QuietUi()
    ui._limit_editable = True
    ui._timeout_s = 600.0
    assert ui.progress(Progress("route", 3, 12, 27, 4, 75.0)) is True
    d = ui._progress
    assert d.limit.GetValue() == 600 and d.limit_open()
    assert ui.time_limit_request() is None  # nothing asked yet
    _press(d.limit_add5)
    assert d.timeout_s == 900 and d.limit.GetValue() == 900
    assert d.elapsed_value.GetLabel() == "1:15 / 15:00 · CPU 0:00"  # the elapsed line shows the new limit at once
    _press(d.limit_add15)
    assert d.timeout_s == 1800
    assert ui.time_limit_request() == 1800 and ui.time_limit_request() is None  # taken once
    # the run flow reports the limit it applies; the field follows
    ui.progress(Progress("route", 3, 12, 27, 4, 76.0, limit_s=1800.0))
    assert d.timeout_s == 1800 and d.elapsed_value.GetLabel() == "1:16 / 30:00 · CPU 0:00"
    # a value in the field: Set limit (Enter in the field does the same)
    d.limit.SetValue(1200)
    _press(d.limit_set)
    assert ui.time_limit_request() == 1200
    # lowered below the time spent: the time spent (stop at the next check, best result kept)
    d.limit.SetValue(30)
    d.apply_field()
    assert ui.time_limit_request() == pytest.approx(76.0)
    assert not d.limit_open() and not d.limit_add5.IsEnabled() and not d.limit_set.IsEnabled()  # limit reached
    ui._close_progress()


def test_progress_time_limit_zero_none_and_disabled_states(app):
    ui = _QuietUi()
    ui._limit_editable = True
    ui._timeout_s = 600.0
    ui.progress(Progress("route", 1, 1, 27, 0, 10.0))
    d = ui._progress
    d.limit.SetValue(0)
    d.apply_field()
    assert ui.time_limit_request() == 0.0 and d.timeout_s == 0
    assert d.elapsed_value.GetLabel() == "0:10 · CPU 0:00 / no limit"
    assert d.limit_set.IsEnabled() and not d.limit_add5.IsEnabled() and not d.limit_add15.IsEnabled()
    d.add_minutes(5)  # nothing to add to "no limit"
    assert ui.time_limit_request() is None
    d.request_limit(1e9)  # capped at the run.timeout_s range
    assert d.timeout_s == d.LIMIT_MAX_S
    # the DRC / preview after the routing: the limit counts router time only
    ui.progress(Progress("drc", 1, 1, 27, 0, 12.0, limit_s=d.LIMIT_MAX_S))
    assert not d.limit_open() and not d.limit.IsEnabled()
    ui._close_progress()
    # after Cancel nothing can be changed
    ui.progress(Progress("route", 1, 1, 27, 0, 10.0))
    d = ui._progress
    d.cancel()
    assert not d.limit_open() and not d.limit_add15.IsEnabled()
    d.add_minutes(15)
    assert ui.time_limit_request() is None
    ui._close_progress()


def test_progress_time_limit_cpu_clock_and_not_before_preflight(app):
    ui = _QuietUi()  # before the pre-flight answer (the pre-flight DRC): no limit to change yet
    ui.progress(Progress("preflight_drc", 0, 0, 0, 0, 0.0))
    d = ui._progress
    assert not d.limit_open() and not d.limit.IsEnabled() and not d.limit_add5.IsEnabled()
    ui._close_progress()
    d = ui_wx.ProgressDialog("eprom", 2400.0, 30, timeout_clock="cpu")
    try:
        assert d.widget("limit_unit_cpu") is not None and d.widget("limit_unit_wall") is None
        assert d.limit.GetName() == "Time limit (s CPU time)"  # accessible name of the field
        p = Progress("route", 1, 1, 27, 0, 100.0)
        p.cpu_s = 2000.0
        d.update(p)
        d.limit.SetValue(1000)
        d.apply_field()
        assert d.limit_request == 2000.0  # clamped to the CPU time spent, the run's clock
    finally:
        d.Destroy()


def test_window_icons(app, tmp_path, monkeypatch):
    light, dark = ui_wx.icon_files(False), ui_wx.icon_files(True)
    assert [p.name for p in light] == ["topor-light-24.png", "topor-light-64.png"]
    assert [p.name for p in dark] == ["topor-dark-24.png", "topor-dark-64.png"]
    b = ui_wx.window_icons(dark=False)
    assert b is not None and b.GetIconCount() == 2
    d = ui_wx.FirstRunDialog(tmp_path / "topor.toml")
    try:
        assert d.GetIcons().GetIconCount() == 2  # every dialog (_Base) carries the bundle
    finally:
        d.Destroy()
    # missing or broken files never break a dialog
    (tmp_path / "icons").mkdir()
    (tmp_path / "icons" / "topor-light-24.png").write_bytes(b"not a png")
    assert ui_wx.window_icons(tmp_path / "icons", dark=False) is None
    assert ui_wx.window_icons(tmp_path / "nowhere") is None
    monkeypatch.setattr(ui_wx, "ICON_DIR", tmp_path / "nowhere")
    monkeypatch.setattr(ui_wx, "window_icons", lambda icon_dir=None, dark=None: None)
    d = ui_wx.ProgressDialog("eprom", 60.0, 30)
    try:
        assert d.GetIcons().GetIconCount() == 0
    finally:
        d.Destroy()


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


def test_scope_count_follows_radio(app, tmp_path):
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
        for name in ("preview", "error"):
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
        assert dlg.elapsed_value.GetLabel() == "1:15 / 10:00 · CPU 0:00" and dlg.total_value.GetLabel() == "1:30"
        dlg.update(Progress("drc", 4, 346, 426, 187, 150.0, "", 0, 0, 170.0))
        assert dlg.phase.GetLabel() == ui_wx.WORDING["progress"]["phase_drc"]
        assert dlg.iteration_value.GetLabel() == "2"
    finally:
        dlg.Destroy()


def test_progress_says_the_soft_time_limit_is_waiting_for_the_step(app):
    """M2.TL (user 2026-11-13): when the limit is reached the dialog names the step the engine finishes and says
    Cancel stops now; after Cancel it says Cancel again, not the limit."""
    dlg = ui_wx.ProgressDialog("qmtech", 600.0, 4)
    try:
        p = Progress("route", 1, 120, 426, 30, 601.0, "layer assignment, rip-up and reroute", 1, 4, 610.0)
        p.limit_reached = True
        p.notice = "time limit reached, waiting for the current step to finish (layer assignment, rip-up and reroute); press Accept to stop now and keep the best"
        dlg.update(p)
        assert dlg.phase.GetLabel() == ("Time limit reached, waiting for the current step to finish (layer assignment, "
                                        "rip-up and reroute); press Accept to stop now and keep the best") 
        dlg.cancelled = True
        dlg.update(p)
        assert not dlg.phase.GetLabel().startswith("Time limit reached"), dlg.phase.GetLabel()
    finally:
        dlg.Destroy()


class _QueuedPressUi(_QuietUi):
    """The press sits in the event queue until the next pump (as a real click between two polls)."""
    queued = False
    button = "cancel_button"

    def _pump(self):
        if self.queued and self._progress is not None:
            self.queued = False
            _press(getattr(self._progress, self.button))


@pytest.mark.parametrize("button", ["cancel_button", "accept_button"])
@pytest.mark.parametrize("stable_first", [False, True])
def test_the_press_is_judged_by_the_state_it_was_pressed_in(app, button, stable_first):
    """verify-sl3 F2: Accept clicked before "Routes stable" is not a stable accept even when the progress the dialog
    gets next is the first one that says stable; one pressed after it is (``ok_at_press`` latched at the press, None
    until a press). Cancel is never a stable accept."""
    ui = _QueuedPressUi()
    ui.button = button
    kw = dict(runs=1, runs_stable=1, best_routed=27, best_total=27, best_run=1, best_round=1)
    first = Progress("route", 1, 0, 27, 0, 1.0, "topological search", 3, 30, 2.0, stable=stable_first, **kw)
    stable = Progress("route", 1, 0, 27, 0, 1.2, "topological search", 4, 30, 2.2, stable=True, **kw)
    assert ui.progress(first) is True and ui.ok_at_press() is None
    dlg = ui._progress
    assert dlg.cancel_button.GetLabel().replace("&", "") == "Cancel"  # the buttons never change their label
    assert dlg.accept_button.GetLabel().replace("&", "") == "Accept"
    ui.queued = True
    assert ui.progress(stable) is False
    assert ui.ok_at_press() is (stable_first and button == "accept_button")
    assert ui.stop_discards() is (button == "cancel_button")
    assert dlg.cancel_button.GetLabel().replace("&", "") == "Cancel"  # the stable progress changes no label
    want = "Cancel pressed" if button == "cancel_button" else "Accept pressed"
    assert dlg.phase.GetLabel().startswith(want), dlg.phase.GetLabel()
    ui._close_progress()


def test_progress_routes_stable_keeps_both_buttons(app):
    """M-B0 (user 2026-11-06): when the gain is below the threshold the dialog shows "Routes stable". Since
    2026-11-22 the buttons stay Accept / Cancel (no OK) and the run goes on; Accept then is a stable accept."""
    ui = _QuietUi()
    p = Progress("route", 1, 300, 426, 150, 40.0, "drawing the traces", 3, 30, 45.0, gain_pct=2.5, runs=5,
                 runs_stable=2, best_routed=341, best_total=426, best_length_mm=2310.0, best_vias=180, best_run=3,
                 best_round=2, best_errors=85, best_open=85)
    assert ui.progress(p) is True
    dlg = ui._progress
    W = ui_wx.WORDING["progress"]
    assert dlg.phase.GetLabel() == "Routing — drawing the traces"
    assert dlg.cancel_button.GetLabel().replace("&", "") == "Cancel"
    assert dlg.gain_value.GetLabel() == "2.5 % / 4 rounds"
    assert dlg.runs_value.GetLabel() == "2 of 5 stable"
    assert _row(dlg, "best") == ["3", "2", "85", "85", "180", "2.310 m", "drawn"]
    assert _row(dlg, "last") == ["–"] * 7, "no round judged yet"
    q = Progress("route", 1, 300, 426, 150, 60.0, "topological search", 7, 30, 65.0, stable=True, gain_pct=0.1,
                 runs=5, runs_stable=5, best_routed=346, best_total=426, best_length_mm=2300.0, best_vias=181,
                 best_run=1, best_round=4, best_errors=80, best_open=80)
    assert ui.progress(q) is True
    assert dlg.phase.GetLabel() == W["phase_stable"] == "Routes stable"
    assert dlg.cancel_button.GetLabel().replace("&", "") == "Cancel"  # no OK: the buttons never change
    assert dlg.accept_button.GetLabel().replace("&", "") == "Accept"
    assert dlg.GetTitle().endswith("— Routes stable")  # also in the window list (the live test waits for it)
    assert dlg.cancel_button.GetId() == wx.ID_CANCEL and dlg.accept_button.GetId() == wx.ID_OK
    assert dlg.accept_button.IsEnabled() and not ui.stop_discards()  # stable does not stop the run
    assert _row(dlg, "best") == ["1", "4", "80", "80", "181", "2.300 m", "drawn"]
    _press(dlg.accept_button)
    assert dlg.phase.GetLabel() == "Accept pressed: finishing the current stage… 0 s"
    assert ui.progress(q) is False and ui.ok_at_press() is True and not ui.stop_discards()
    ui._close_progress()


def _row(dlg, row):
    return [dlg.score_cells[row][c].GetLabel() for c, *_ in dlg.SCORE_COLS]


def _scored(**kw):
    base = dict(phase="route", total=426, routed=426, runs=5, best_run=1, best_round=1, best_errors=3, best_open=0,
                best_vias=166, best_length_mm=7384.0, best_total=426, best_routed=426, last_run=3, last_round=2,
                last_errors=5, last_open=0, last_vias=168, last_length_mm=7412.0, last_vs_best="worse",
                last_decider="errors", last_delta=2.0)
    base.update(kw)
    return Progress(**base)


def test_score_rows_mark_worse_and_better_on_the_deciding_criterion():
    """User 2026-11-11: "It would help if I can see the current score, and the best score in the UI". The rows
    follow the pick order; the verdict names the first criterion that differs (why a round is not the best)."""
    rows, verdict = ui_wx.score_rows(_scored())
    assert rows["last"] == {"run": "3", "round": "2", "errors": "5", "open": "0", "vias": "168", "length": "7.412 m",
                            "source": "drawn"}
    assert rows["best"] == {"run": "1", "round": "1", "errors": "3", "open": "0", "vias": "166", "length": "7.384 m",
                            "source": "drawn"}
    assert verdict == "Last round is worse than the best: errors +2."
    worse = "Last round is worse than the best: "
    assert ui_wx.score_rows(_scored(last_decider="vias", last_delta=2.0))[1] == worse + "vias +2."
    assert ui_wx.score_rows(_scored(last_decider="open", last_delta=1.0))[1] == worse + "open +1."
    new = _scored(last_vs_best="new best", last_decider="length", last_delta=-30.0)
    assert ui_wx.score_rows(new)[1] == "Last round is the new best: length -0.030 m."
    assert ui_wx.score_rows(_scored(last_vs_best="new best", last_decider="", last_delta=0.0))[1] == \
        "The first result is the best so far."
    assert ui_wx.score_rows(_scored(last_vs_best="same", last_decider=""))[1] == \
        "Last round scores the same as the best; the earlier result stays."
    assert ui_wx.score_rows(_scored(last_vs_best="tie", last_decider=""))[1] == \
        "Last round scores the same as the best and is the best now (ties go to the lower run)."
    assert ui_wx.score_rows(_scored(last_vs_best="better", last_decider="open", last_delta=-1.0))[1] == \
        "Last round is better (open -1), but the variant table kept the best."
    assert ui_wx.score_rows(_scored(last_decider="shrinkages", last_delta=1.0))[1] == \
        "Last round is worse than the best: shrinkages +1."
    assert ui_wx.score_rows(_scored(last_decider="score", last_delta=12.25))[1] == \
        "Last round is worse than the best: weighted score +12.2 mm."
    # nothing judged yet: the rows keep "–", no verdict
    rows, verdict = ui_wx.score_rows(Progress(phase="route", total=426))
    assert rows == {"last": None, "run": None, "best": None} and verdict == ""


def test_progress_dialog_shows_the_score_rows_at_its_size(app):
    dlg = ui_wx.ProgressDialog("qmtech", 600.0, 4)
    try:
        size = dlg.GetSize()
        assert _row(dlg, "last") == ["–"] * 7 and _row(dlg, "best") == ["–"] * 7
        dlg.update(_scored())
        assert _row(dlg, "last") == ["3", "2", "5", "0", "168", "7.412 m", "drawn"]
        assert _row(dlg, "best") == ["1", "1", "3", "0", "166", "7.384 m", "drawn"]
        assert dlg.score_verdict.GetLabel() == "Last round is worse than the best: errors +2."
        dlg.update(_scored(last_run=2, last_round=3, last_errors=3, last_vias=166, last_length_mm=7350.0,
                           best_run=2, best_round=3, best_length_mm=7350.0, last_vs_best="new best",
                           last_decider="length", last_delta=-34.0))
        assert _row(dlg, "last") == _row(dlg, "best") == ["2", "3", "3", "0", "166", "7.350 m", "drawn"]
        assert dlg.score_verdict.GetLabel() == "Last round is the new best: length -0.034 m."
        dlg._grow_to_best()
        assert dlg.GetSize() == size, "the score fits the dialog's size"
        for row in dlg.SCORE_ROWS:  # every cell is shown in full (fixed widths hold the widest values)
            for c, *_ in dlg.SCORE_COLS:
                cell = dlg.score_cells[row][c]
                assert cell.GetSize().width >= cell.GetTextExtent(cell.GetLabel()).width, (row, c)
    finally:
        dlg.Destroy()


def test_progress_shows_cpu_time_next_to_router_time(app):
    """Router time and the CPU time of the run (all threads) side by side; the limit sits on the clock it counts."""
    wall = ui_wx.ProgressDialog("qmtech", 600.0, 4)
    cpu = ui_wx.ProgressDialog("qmtech", 2400.0, 4, timeout_clock="cpu")
    try:
        p = Progress("route", 2, 120, 426, 30, 130.0, "", 2, 4, 150.0, cpu_s=465.0)
        wall.update(p)
        cpu.update(p)
        assert wall.elapsed_value.GetLabel() == "2:10 / 10:00 · CPU 7:45"
        assert cpu.elapsed_value.GetLabel() == "2:10 · CPU 7:45 / 40:00"
    finally:
        wall.Destroy()
        cpu.Destroy()


def test_splash_shows_name_logo_version_and_status_and_closes_at_the_first_window(app):
    """User 2026-11-08: the router shows a splash at once (start-up takes a few seconds): logo, "Ranz Topor",
    version, a status line; one window, its status updated in place; closed when the progress dialog opens."""
    ui = _QuietUi()
    ui.splash("Starting the router…", "0.4.0")
    sp = ui._splash
    assert sp is not None and ui.shown == 1
    assert sp.FindWindowByName("title").GetLabel() == "Ranz Topor"
    assert sp.FindWindowByName("subtitle").GetLabel() == "Topological router 0.4.0"
    assert sp.FindWindowByName("logo") is not None  # plugin/icons/topor-*-128.png
    notice = sp.FindWindowByName("notice").GetLabel()  # GPL-3.0 start-up notice
    assert "GPL-3.0-only" in notice and "ABSOLUTELY NO WARRANTY" in notice
    assert sp.status.GetLabel() == "Starting the router…"
    ui.splash("Connecting to KiCad…")
    assert ui._splash is sp and ui.shown == 1 and sp.status.GetLabel() == "Connecting to KiCad…"
    ui.progress(Progress("setup", 0, 0, 27, 0, 0.0))
    assert ui._splash is None and ui._progress is not None
    ui._close_progress()


def test_splash_closes_before_a_modal_window(app):
    ui = _QuietUi()
    ui.splash("Reading the board…")
    ui._close_progress()  # what preflight / first_run_notice / error / preview / done do before ShowModal
    assert ui._splash is None


def test_logo_file_falls_back_to_the_largest_window_icon(tmp_path):
    (tmp_path / "topor-light-24.png").write_bytes(b"x")
    (tmp_path / "topor-light-64.png").write_bytes(b"x")
    assert ui_wx.logo_file(False, tmp_path).name == "topor-light-64.png"
    (tmp_path / "topor-light-128.png").write_bytes(b"x")
    assert ui_wx.logo_file(False, tmp_path).name == "topor-light-128.png"
    assert ui_wx.logo_file(True, tmp_path) is None


# ------------------------------------------------------------------ UI polish (user 2026-11-11: "implement all 4")
def _drc_report(tmp_path, n=9, findings=()):
    from topor.report import RunReport
    facts = dict(version="0.4.0", board="eprom", timestamp="t", routed=27, total=27, nets=25, layers=2, partial=0,
                 failed=0, vias=0, vias_before=17, length_mm=113.4, length_before_mm=112.9, routing_new=n,
                 preexisting=0, unconnected=0, drc_by_type_new={"clearance": n - 1, "shorting_items": 1} if n else {},
                 findings=list(findings))
    return RunReport(tmp_path, facts)


def test_findings_rows_carry_their_severity_colour_and_the_list_fits_its_rows(app):
    """Problems visible at a glance: error red, warning amber, note grey (light / dark pair), icon + word kept; the
    list is as tall as its rows; no errors and no warnings read "No problems found"."""
    err = Finding("CFG_VALUE", "error", "bad", "fix")
    d = ui_wx.PreflightDialog(FINDINGS + [err], SUMMARY, Config.defaults())
    one = ui_wx.PreflightDialog([FINDINGS[0]], SUMMARY, Config.defaults())
    none = ui_wx.PreflightDialog([], SUMMARY, Config.defaults())
    try:
        for i, f in enumerate(d.findings):
            assert d.list.GetItemTextColour(i) == ui_wx.tone_colour(f.severity), f.code
        assert [f.severity for f in d.findings] == ["error", "warning", "warning", "info"]
        assert ui_wx.tone_colour("error") != ui_wx.tone_colour("warning") != ui_wx.tone_colour("info")
        assert d.findings_heading.GetLabel() == "Findings — 1 errors, 2 warnings, 1 notes"
        assert one.list.GetMinSize().height < d.list.GetMinSize().height  # 1 row vs 4 rows
        assert one.findings_heading.GetLabel() == "No problems found (1 notes)"
        assert one.findings_heading.GetForegroundColour() == ui_wx.tone_colour("ok")
        assert none.findings_heading.GetLabel() == "No problems found"
        assert not none.list.IsShown() and not none.cause.IsShown()
    finally:
        for x in (d, one, none):
            x.Destroy()


def test_tone_colours_follow_the_theme(monkeypatch):
    monkeypatch.setattr(ui_wx, "window_is_dark", lambda: False)
    light = {t: ui_wx.tone_colour(t).Get()[:3] for t in ui_wx.TONE_RGB}
    monkeypatch.setattr(ui_wx, "window_is_dark", lambda: True)
    dark = {t: ui_wx.tone_colour(t).Get()[:3] for t in ui_wx.TONE_RGB}
    assert light == {t: v[0] for t, v in ui_wx.TONE_RGB.items()} and dark == {t: v[1] for t, v in ui_wx.TONE_RGB.items()}
    from topor.preview import contrast
    for t, (lt, dk) in ui_wx.TONE_RGB.items():  # readable on GTK's default light / dark window colour
        assert contrast(lt, (255, 255, 255)) >= 4.5 and contrast(dk, (36, 36, 36)) >= 4.5, t


def test_preflight_buttons_help_left_cancel_route_right_route_primary(app):
    d = ui_wx.PreflightDialog([FINDINGS[0]], SUMMARY, Config.defaults())
    try:
        d.SetSize(d.FromDIP(wx.Size(900, 700)))
        d.Layout()
        h, c, r = (d.buttons[n].GetPosition().x for n in ("help", "cancel", "route"))
        assert h < d.GetClientSize().width / 2 < c < r  # Help left; Cancel, Route right (GTK order)
        assert d.GetDefaultItem() is d.route  # Enter = Route
        assert d.route.GetFont().GetWeight() == wx.FONTWEIGHT_BOLD
        assert d.buttons["cancel"].GetFont().GetWeight() != wx.FONTWEIGHT_BOLD
        hint = d.widget("timeout_hint")
        assert hint.GetForegroundColour() == wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT)
        assert hint.GetPosition().y > d.timeout.GetPosition().y  # under the field
    finally:
        d.Destroy()


def test_summary_parts_key_facts_first_alert_and_no_report_path(tmp_path):
    alert, key, rest = ui_wx.summary_parts(_drc_report(tmp_path))
    assert alert == "⚠ DRC new 9 (clearance 8, shorting_items 1) — see findings"
    assert [k.split()[0] for k in key] == ["Routed", "DRC", "Vias", "Length"]
    assert not any(ln.startswith("Report: ") for ln in rest + key)
    assert rest[0].startswith("Topor 0.4.0")
    assert ui_wx.summary_parts(_drc_report(tmp_path, n=0))[0] == ""
    assert ui_wx.summary_parts(FakeReport(tmp_path))[0] == ""  # no facts: no alert


def _panels(tmp_path):
    from topor import preview as pv
    png = tmp_path / "preview.png"
    wx.Image(80, 40).SaveFile(str(png), wx.BITMAP_TYPE_PNG)
    for k, p in pv.panel_paths(png).items():
        wx.Image(60, 120).SaveFile(str(p), wx.BITMAP_TYPE_PNG)
    pv.legend_path(png).write_text('[{"label": "F.Cu", "rgb": [200, 52, 52]}, {"label": "B.Cu", "rgb": [77, 127, 196]}]')
    return png


def test_preview_two_images_summary_hierarchy_and_findings(app, tmp_path):
    """Two board images under their own headings, each scaled to its pane; the Summary with the red DRC line and
    the key facts first (bold); the findings list of the run; the report path once (the field)."""
    png = _panels(tmp_path)
    f = Finding("DRC_NEW_VIOLATIONS", "warning", "KiCad's DRC finds 9 new violations", "Inspect → Design Rules Checker")
    d = ui_wx.PreviewDialog(png, _drc_report(tmp_path, findings=[FINDINGS[0], f]), "eprom")
    try:
        assert d.images["before"] is not None and d.images["after"] is not None
        assert d.bitmaps["before"] is not None and d.bitmaps["after"] is not None
        assert d.widget("before_heading") is not None and d.widget("after_heading") is not None
        d.SetSize(d.FromDIP(wx.Size(1400, 900)))
        d.Layout()
        d._render()
        for k in ("before", "after"):  # Fit: the image fills its pane (height-bound here: a tall board)
            assert d.bitmaps[k].GetBitmap().GetHeight() >= d.scrollers[k].GetClientSize().height - 2, k
        assert d.drc_alert is not None and d.drc_alert.GetLabel().startswith("⚠ DRC new 9")
        assert d.drc_alert.GetForegroundColour() == ui_wx.tone_colour("error")
        assert [k.GetLabel().split()[0] for k in d.key_facts] == ["Routed", "DRC", "Vias", "Length"]
        assert all(k.GetFont().GetWeight() == wx.FONTWEIGHT_BOLD for k in d.key_facts)
        assert "Report:" not in d.summary.GetValue() and str(tmp_path) == d.dir_field.GetValue()
        assert [x.code for x in d.findings] == ["DRC_NEW_VIOLATIONS", "DEFAULTS_USED"]
        assert d.list.GetItemCount() == 2 and "Inspect → Design Rules Checker" in d.cause.GetValue()
        assert d.GetDefaultItem() is d.ok_button and d.ok_button.GetFont().GetWeight() == wx.FONTWEIGHT_BOLD
        assert [w.GetToolTipText() for w in d.GetChildren() if w.GetName() == "legend_swatch"] == ["F.Cu", "B.Cu"]
    finally:
        d.Destroy()
    # an older report folder (only the composite preview.png): the composite in the first pane, no second pane
    (tmp_path / "preview-before.png").unlink()
    d = ui_wx.PreviewDialog(png, FakeReport(tmp_path), "eprom")
    try:
        assert d.images["before"] is not None and d.images["after"] is None and d.drc_alert is None
        assert d.bitmaps["after"] is None and not d.scrollers["after"].GetParent().IsShown()
    finally:
        d.Destroy()


def test_progress_waits_for_round_1_colours_the_verdict_and_never_changes_size(app):
    dlg = ui_wx.ProgressDialog("qmtech", 600.0, 4)
    try:
        size = dlg.GetSize()
        assert dlg.score_waiting.IsShown() and not dlg.scores_shown  # no dash rows before round 1
        assert dlg.score_waiting.GetLabel() == "Waiting for round 1…"
        dlg.update(Progress("route", 1, 10, 426, 3, 5.0, "a very long stage name that would widen the dialog " * 3))
        assert not dlg.scores_shown
        dlg.update(_scored(elapsed_s=36000.0, wall_s=36500.0, cpu_s=99999.0, vias=123456))
        assert dlg.scores_shown and not dlg.score_waiting.IsShown()
        assert dlg.score_verdict.GetForegroundColour() == ui_wx.tone_colour("warning")  # worse: amber
        dlg.update(_scored(last_vs_best="new best", last_decider="length", last_delta=-30.0))
        assert dlg.score_verdict.GetForegroundColour() == ui_wx.tone_colour("ok")  # better: green
        dlg.update(_scored(last_vs_best="same", last_decider=""))
        assert dlg.score_verdict.GetForegroundColour() == ui_wx.tone_colour("")
        dlg._grow_to_best()
        assert dlg.GetSize() == size, "numbers, stage names and verdicts never resize the dialog"
        for st in (dlg.elapsed_value, dlg.total_value, dlg.vias_value):
            assert st.GetSize().width >= st.GetTextExtent(st.GetLabel()).width, st.GetName()
        assert dlg.widget("cancel_hint").GetLabel() == "Accept keeps the best result so far. Cancel takes nothing."
        assert dlg.widget("cancel_hint").GetToolTipText() == ui_wx.WORDING["progress"]["cancel_hint_tip"]
    finally:
        dlg.Destroy()


def test_verdict_tone():
    assert ui_wx.verdict_tone(_scored()) == "warning"
    assert ui_wx.verdict_tone(_scored(last_vs_best="better", last_decider="open", last_delta=-1.0)) == "ok"
    assert ui_wx.verdict_tone(_scored(last_vs_best="new best", last_decider="vias", last_delta=-1.0)) == "ok"
    assert ui_wx.verdict_tone(_scored(last_vs_best="new best", last_decider="", last_delta=0.0)) == ""  # the first
    assert ui_wx.verdict_tone(_scored(last_vs_best="tie", last_decider="")) == ""
    assert ui_wx.verdict_tone(_scored(last_round=0)) == ""


def test_preflight_offers_use_the_current_default_for_pinned_lines(app):
    """User 2026-11-17 ("Show it in the dialog"): a pinned old default or a deprecated key in topor.toml is listed
    with a checkbox naming its key; ticking it puts the key into Decision.use_defaults (Route comments the line out),
    the state is kept per key, and other rows hide the checkbox."""
    fs = [Finding("CFG_DEPRECATED_KEY", "warning", "topor.toml (line 6) sets optimize.finish_when_stable = true, which "
                  "is deprecated since 2026-11-17 and ignored: ...", "fix", ("optimize.finish_when_stable",)),
          Finding("CFG_PINNED_OLD_DEFAULT", "warning", "topor.toml (line 3) pins run.timeout_s = 300, the old default",
                  "fix", ("run.timeout_s",)),
          Finding("VIA_NOT_SET_UP", "warning", "no via set up", "Board Setup")]
    p = ui_wx.PreflightDialog(fs, SUMMARY, Config.defaults())
    try:
        codes = [f.code for f in p.findings]
        i_dep, i_pin, i_via = (codes.index(c) for c in ("CFG_DEPRECATED_KEY", "CFG_PINNED_OLD_DEFAULT", "VIA_NOT_SET_UP"))
        p._show_finding(i_dep)
        assert p.use_default_cb.IsShown() and not p.accept_cb.IsShown()
        assert "optimize.finish_when_stable" in p.use_default_cb.GetLabel() and "&" in p.use_default_cb.GetLabel()
        p.use_default_cb.SetValue(True)
        p._on_use_default(None)
        p._show_finding(i_pin)
        assert p.use_default_cb.IsShown() and "run.timeout_s" in p.use_default_cb.GetLabel()
        assert not p.use_default_cb.GetValue(), "its own state, per key"
        p._show_finding(i_via)
        assert not p.use_default_cb.IsShown()
        assert p.decision(True).use_defaults == ("optimize.finish_when_stable",)
        p._show_finding(i_pin)
        p.use_default_cb.SetValue(True)
        p._on_use_default(None)
        assert set(p.decision(True).use_defaults) == {"optimize.finish_when_stable", "run.timeout_s"}
        assert p.decision(False) == Decision(False)
    finally:
        p.Destroy()


@pytest.mark.parametrize("ticked", [False, True])
@pytest.mark.parametrize("field", [False, True])
def test_use_the_current_default_also_reaches_the_progress_dialog(app, monkeypatch, ticked, field):
    """verify-sl3 F3: "Use the current default" for a pinned run.timeout_s / run.timeout_clock (or optimize.rounds /
    stable_window) makes the run use the default (run.apply_use_defaults); the progress dialog shows the same limit,
    clock and caps, not the pinned topor.toml values. Unticked: the pinned values, as the run uses them. A limit typed in
    the pre-flight field wins over both (run.py applies the dialog's overrides last)."""
    from topor.config import Config as _Config
    pinned = {("run", "timeout_s"): 300.0, ("run", "timeout_clock"): "cpu", ("optimize", "rounds"): 7,
              ("optimize", "stable_window"): 9}
    cfg = _Config.defaults().with_values(pinned)
    use = tuple(f"{s}.{k}" for s, k in pinned) if ticked else ()

    class Pre:
        def __init__(self, *a):
            pass

        def ShowModal(self):
            return wx.ID_OK

        def decision(self, proceed):
            return Decision(True, overrides={"timeout_s": 120.0} if field else {}, use_defaults=use)

        def Destroy(self):
            pass

    monkeypatch.setattr(ui_wx, "PreflightDialog", Pre)
    ui = _QuietUi()
    ui.preflight([], RunSummary(board="b"), cfg)
    want = _Config.defaults() if ticked else cfg
    ui.progress(Progress("route", 1, 0, 27, 0, 1.0))
    dlg = ui._progress
    assert dlg.timeout_s == (120.0 if field else float(want.get("run", "timeout_s")))
    assert dlg.timeout_clock == want.get("run", "timeout_clock")
    assert dlg.iteration_cap == want.get("optimize", "rounds") and dlg.stable_window == want.get("optimize", "stable_window")
    ui._close_progress()


def test_the_cause_pane_never_reads_full_stop_colon(app):
    """verify-sl3 F4 (seen in the real dialog): a message that ends its sentence with "." goes on with its items
    (CFG_DEPRECATED_KEY: "...stable_patience: optimize.finish_when_stable", not "stable_patience.:"); an
    ellipsis stays."""
    from topor.config import _deprecated
    dep = _deprecated("optimize", "finish_when_stable", True, 263)
    assert dep.message.endswith(".") and dep.items == ("optimize.finish_when_stable",)
    ell = Finding("VIA_NOT_SET_UP", "warning", "no via set up...", "Board Setup", ("a",))
    p = ui_wx.PreflightDialog([dep, ell], SUMMARY, Config.defaults())
    try:
        txt = p.cause_text(dep)
        assert ".:" not in txt and "stable_patience: optimize.finish_when_stable" in txt, txt
        assert "no via set up...: a" in p.cause_text(ell)
    finally:
        p.Destroy()


def test_grid_snap_tick_mapping_and_line(app):
    """grid-ui: tick <-> [drawing] grid_snap (prefer = ticked, off = unticked, fallback = half-ticked and kept)
    and the read-only grid line (grid.describe, the run's own lookup)."""
    from topor import grid
    assert [ui_wx.grid_tick_state(v) for v in ("off", "prefer", "fallback")] == \
        [wx.CHK_UNCHECKED, wx.CHK_CHECKED, wx.CHK_UNDETERMINED]
    for cur in ("off", "prefer", "fallback"):
        assert ui_wx.grid_snap_value(wx.CHK_CHECKED, cur) == "prefer"
        assert ui_wx.grid_snap_value(wx.CHK_UNCHECKED, cur) == "off"
    assert ui_wx.grid_snap_value(wx.CHK_UNDETERMINED, "fallback") == "fallback"
    assert ui_wx.grid_snap_value(wx.CHK_UNDETERMINED, "off") == "off"
    cfg = Config.defaults().with_values({("drawing", "grid_snap"): "fallback"})
    pcb = "(kicad_pcb (setup (grid_origin 75.927 45)))"
    s = RunSummary(board="b", grid_line=grid.describe(0.25, pcb, None))
    d = ui_wx.PreflightDialog([], s, cfg)
    try:
        assert d.grid_snap.Get3StateValue() == wx.CHK_UNDETERMINED
        assert "grid_snap" not in d.decision(True).overrides  # untouched fallback stays
        d.grid_snap.Set3StateValue(wx.CHK_UNCHECKED)
        assert d.decision(True).overrides["grid_snap"] == "off"
        d.grid_snap.Set3StateValue(wx.CHK_CHECKED)
        assert d.decision(True).overrides["grid_snap"] == "prefer"
        assert d.grid_line.GetLabel() == \
            "Grid: 0.25 mm from topor.toml; origin (75.927, 45) mm from the board"
    finally:
        d.Destroy()


def test_grid_describe(tmp_path):
    from topor import grid
    pcb = "(kicad_pcb (setup (grid_origin 1 2)))"
    assert grid.describe("auto", pcb, None).startswith("no grid found")
    (tmp_path / "pcbnew.json").write_text(json.dumps({"window": {"grid": {"sizes": [{"x": "0.5 mm", "y": "0.5 mm"}],
                                                                        "last_size": 0}}}))
    assert grid.describe("auto", pcb, tmp_path) == "0.5 mm from KiCad settings; origin (1, 2) mm from the board"
    assert grid.describe(0.1, pcb, tmp_path).startswith("0.1 mm from topor.toml")
    assert grid.resolve("auto", pcb, tmp_path)[0]["grid_track_nm"] == [500000, 500000]
