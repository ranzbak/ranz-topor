"""wxPython dialogs of the Topor plugin: ``WxUi(UiPort)`` built verbatim from the reviewed wireframe note
``docs/progress/notes/m4-ui-wireframes.md`` (wording table = ``WORDING`` below).

Rules applied (note §0, ecc:make-interfaces-feel-better + ecc:accessibility adapted to wx):
native widgets only, ``wx.StdDialogButtonSizer`` (GTK button order), Enter = default / Esc = Cancel, mnemonics on
buttons / checkboxes / radios, sizes via ``FromDIP``, fixed-width right-aligned number labels (no layout shift),
system colours only (the sole fixed colour is the preview legend swatch), severity = icon + word, every
control carries ``SetName(id)`` (test hook and accessible name) and a tooltip where no label precedes it,
disabled Route states its reason, nothing is truncated silently.

Threading: everything runs on the wx main thread. ``progress`` shows a MODELESS dialog, pumps events and returns
False once Cancel was pressed; every other UiPort method is modal and closes the progress dialog first.

Complexity: construction O(#findings + #summary lines); ``progress`` O(1) per call; image scaling O(pixels) per
zoom change. No pcbnew (M4-R1).
"""

from __future__ import annotations

import math
import re
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path

import wx

from .config import Config
from .findings import Finding
from .ports import Decision, Progress, RunSummary

try:  # B5 owns the constants; the fallback is the value fixed in the note (§4)
    from .preview import PREVIEW_HIGHLIGHT_RGB  # type: ignore[attr-defined]
except ImportError:  # pragma: no cover - until B5 lands
    PREVIEW_HIGHLIGHT_RGB = (0, 139, 139)

# --------------------------------------------------------------------------------------------- wording
# Exact user-visible strings (mnemonics stripped) from the note's `## Wording` tables; `{..}` = runtime value.
# Rows owned by the run report (B8: md_*, sec_*, sum_*, outcome_*) are not UI strings and are not listed.
WORDING: dict[str, dict[str, str]] = {
    "first_run": {
        "title": "Topor — Settings file created",
        "intro": "Topor created a settings file for this project:",
        "toml_path": "{toml_path}",
        "explain": "It holds every option with its default and a comment. Design rules, widths and via sizes "
                   "still come from KiCad's Board Setup. Edit the file any time; it is read on each run.",
        "open_file": "Open file",
        "continue": "Continue",
    },
    "preflight": {
        "title": "Topor — Route {board}",
        "summary_line": "{connections} connections in scope · {nets} nets · {layers} copper layers · "
                        "engine {engine_version}",
        "findings_heading": "Findings — {errors} errors, {warnings} warnings, {notes} notes",
        "col_severity": "Severity",
        "col_code": "Code",
        "col_message": "Message",
        "sev_error": "Error",
        "sev_warn": "Warn",
        "sev_info": "Note",
        "cause_label": "Cause",
        "fix_label": "Fix",
        "no_findings": "No findings. The board setup covers everything Topor needs.",
        "items_more": "… and {n} more (all in the report)",
        "accept_pads": "Accept: skip these pads (saved to topor.toml ignore_pads)",
        "accept_drills": "Accept: add these drills to topor.toml allowed_drills_mm",
        "options_heading": "Options",
        "scope_label": "Scope",
        "scope_whole": "Whole board",
        "scope_between": "Between selected footprints",
        "scope_touching": "Touching selected footprints",
        "scope_hint": "(nothing selected in KiCad)",
        "existing_label": "Existing copper",
        "existing_fixed": "Keep as obstacles",
        "existing_smooth": "Keep and smooth",
        "existing_rip": "Rip up and re-route",
        "existing_tip": "Rip up keeps free and stitching vias as obstacles unless topor.toml sets "
                        "[run] rip_free_vias = true",
        "respect_locked": "Respect locked tracks and vias",
        "partial_routes": "Keep partial routes",
        "timeout_label": "Time limit",
        "timeout_unit": "s",
        "timeout_hint": "0 = no limit",
        "threads_label": "Threads",
        "threads_hint": "0 = all cores but one",
        "save_toml": "Save these options to topor.toml",
        "blocked_status": "Cannot route: {n} error(s) — see Fix above.",
        "help": "Help",
        "cancel": "Cancel",
        "route": "Route",
    },
    "progress": {
        "title": "Topor — Routing {board}",
        "phase_setup": "Preparing geometry",
        "phase_route": "Routing",
        "phase_route_stage": "Routing — {stage}",
        "phase_done": "Routing finished",
        "phase_preflight_drc": "Checking the board (KiCad DRC)",
        "phase_drc": "Verifying with KiCad DRC",
        "phase_preview": "Rendering the preview",
        "phase_cancelling": "Cancelling — keeping the best result so far…",
        "gauge_value": "{routed} / {total}",
        "iteration_label": "Round",
        "iteration_value": "{iteration} of up to {cap}",
        "routed_label": "Routed",
        "routed_value": "{routed} / {total}",
        "routed_pending": "– / {total}",
        "vias_label": "Vias",
        "elapsed_label": "Router time",
        "elapsed_value": "{elapsed} / {limit}",
        "elapsed_nolimit": "{elapsed} / no limit",
        "total_label": "Total time",
        "cancel_hint": "Cancel keeps the best result so far. Once the routes are stable the button reads OK: the "
                       "same action, and the best result shown is exactly the board you get; waiting longer may "
                       "still find small gains. The time limit counts router time only and is an emergency stop; "
                       "KiCad's DRC and the preview come on top.",
        "cancel": "Cancel",
        # M-B0: iterations like TopoR ("Routes stable", Cancel -> OK, best of N runs)
        "ok": "OK",
        "phase_stable": "Routes stable",
        "title_stable": "Topor — Routing {board} — Routes stable",
        "phase_finishing": "Finishing — keeping the stable result…",
        "round_value": "{round}",
        "gain_label": "Gain",
        "gain_value": "{gain} % / {window} rounds",
        "gain_pending": "–",
        "runs_label": "Runs",
        "runs_value": "{stable} of {runs} stable",
        "best_label": "Best so far: {routed} / {total} routed, {length} m, {vias} vias",
        "best_run": "(run {run})",
        "best_pending": "Best so far: –",
    },
    "preview": {
        "title": "Topor — Preview: {board}",
        "zoom_label": "Zoom",
        "zoom_fit": "Fit",
        "zoom_100": "100 %",
        "zoom_200": "200 %",
        "legend_label": "Legend: F.Cu · B.Cu · new copper (highlighted)",
        "before_heading": "Current board",
        "after_heading": "Result",
        "summary_heading": "Summary",
        "no_image": "Preview could not be rendered (see log.txt in the report folder)",
        "report_dir_label": "Report folder",
        "report_dir": "{report_dir}",
        "open_report_folder": "Open report folder",
        "reject": "Reject",
        "accept": "Accept",
    },
    "report": {
        "title": "Topor — Finished",
        "done_accepted": "Routed {routed}/{total} connections — applied to the board as one undo step "
                         "(Edit → Undo).",
        "done_accepted_cancelled": "Stopped early by Cancel: routed {routed}/{total} connections — applied to the "
                                   "board as one undo step (Edit → Undo).",
        "done_rejected": "Nothing was changed on the board.",
        "done_cancelled": "Cancelled — nothing was changed on the board.",
        "done_refill_hint": "Zones were not refilled: press B (Edit → Fill All Zones) to refill them — a separate "
                            "undo step.",
        "done_refilled": "Zones were refilled by topor — a second undo step: the first Edit → Undo reverts the refill, "
                         "the next one the routes.",
        "done_post_commit": "{message} {fix}",
        "report_dir_label": "Report folder",
        "report_dir": "{report_dir}",
        "open_report_folder": "Open report folder",
        "close": "Close",
    },
    "error": {
        "title": "Topor — Error",
        "code": "{code}",
        "message": "{message}",
        "fix_label": "Fix",
        "fix": "{fix}",
        "details_label": "Details",
        "details": "code: {code}\nmessage: {message}\nfix: {fix}\nitems: {items}\nlog: {log_path}\n"
                   "engine {engine_version} · plugin {plugin_version} · kipy {kipy_version} · "
                   "KiCad {kicad_version}\n{timestamp}",
        "items_more": "… and {n} more",
        "copy_details": "Copy details",
        "copied": "Copied",
        "open_report_folder": "Open report folder",
        "no_report_folder": "(no report folder yet)",
        "close": "Close",
    },
}

#: phase name (engine) -> WORDING["progress"] key
PHASE_KEYS = {p: f"phase_{p}" for p in ("setup", "route", "done", "preflight_drc", "drc", "preview")}

ACCEPT_CODES = {"PAD_OUTSIDE_BOARD": "accept_pads", "DRILL_OUTSIDE_FILTER": "accept_drills"}
HELP_REL = Path("docs") / "user" / "operate.md"
HELP_ANCHOR = "options"


def find_help_doc(module_file) -> Path | None:
    """operate.md for the Help button: the installed plugin layout first (``org.topor.router/topor/ui_wx.py`` ->
    ``org.topor.router/docs/user/``, tools/build.sh), then the repo layout (``plugin/topor/ui_wx.py`` ->
    ``<repo>/docs/user/``); None when neither exists. O(1)."""
    here = Path(module_file).resolve()
    for base in (here.parents[1], here.parents[2]):
        if (base / HELP_REL).is_file():
            return base / HELP_REL
    return None


HELP_DOC = find_help_doc(__file__)
_SEV_ORDER = {"error": 0, "warning": 1, "info": 2}
_SEV_WORD = {"error": "sev_error", "warning": "sev_warn", "info": "sev_info"}
_SEV_ART = {"error": wx.ART_ERROR, "warning": wx.ART_WARNING, "info": wx.ART_INFORMATION}
_ITEMS_PREFLIGHT = 10
_ITEMS_ERROR = 20
_COPIED_MS = 2000
_ZOOM_KEYS = ("zoom_fit", "zoom_100", "zoom_200")


# --------------------------------------------------------------------------------------------- helpers
def _mn(text: str, key: str) -> str:
    """Insert the mnemonic marker before the first `key` letter of `text`."""
    i = text.lower().index(key.lower())
    return text[:i] + "&" + text[i:]


def open_path(path) -> bool:
    """Open a file / folder with the desktop default application; never blocks. Falls back to xdg-open."""
    p = str(path)
    try:
        if wx.LaunchDefaultApplication(p):
            return True
    except Exception:  # noqa: BLE001 - fall through to xdg-open
        pass
    try:
        subprocess.Popen(["xdg-open", p], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, start_new_session=True)
        return True
    except OSError:
        return False


def _fmt_time(seconds: float) -> str:
    s = max(0, int(seconds))
    if s >= 3600:
        return f"{s // 3600}:{s % 3600 // 60:02d}:{s % 60:02d}"
    return f"{s // 60}:{s % 60:02d}"


@dataclass
class UiContext:
    """What the dialogs need beyond the UiPort arguments; B5 fills it via ``WxUi.set_context``."""
    board: str = ""
    report_dir: Path | None = None
    log_path: Path | None = None
    versions: dict = field(default_factory=dict)  # engine, plugin, kipy, kicad


class _Base(wx.Dialog):
    """Common dialog frame: parentless, centred, std button row, DIP sizes, system colours only."""

    def __init__(self, title: str, resizable: bool = False):
        style = wx.DEFAULT_DIALOG_STYLE | (wx.RESIZE_BORDER if resizable else 0)
        super().__init__(None, title=title, style=style)
        self.SetName("dialog")
        self.root = wx.BoxSizer(wx.VERTICAL)
        self.buttons: dict[str, wx.Button] = {}
        self.ids: dict[str, wx.Window] = {}  # wording id -> widget (filled by finish())
        self.A11Y: dict[str, str] = {}  # id -> accessible label for controls whose text is not their own

    # spacing constants (DIP): rows 6, groups 12, border 12
    @property
    def row(self) -> int:
        return self.FromDIP(6)

    @property
    def group(self) -> int:
        return self.FromDIP(12)

    def text(self, parent, key_dialog: str, key: str, name: str | None = None, text: str | None = None,
             style: int = 0, wrap_dip: int | None = None, bold: bool = False) -> wx.StaticText:
        st = wx.StaticText(parent, label=text if text is not None else WORDING[key_dialog][key], style=style)
        st.SetName(name or key)
        if bold:
            f = st.GetFont()
            f.SetWeight(wx.FONTWEIGHT_BOLD)
            st.SetFont(f)
        if wrap_dip:
            st.Wrap(self.FromDIP(wrap_dip))
        return st

    def mono_field(self, parent, name: str, value: str, width_dip: int = 520) -> wx.TextCtrl:
        """Read-only, selectable, copyable monospace field; wraps (never truncates)."""
        tc = wx.TextCtrl(parent, value=value, name=name,
                         style=wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_BESTWRAP | wx.TE_NO_VSCROLL)
        tc.SetFont(wx.Font(wx.FontInfo().Family(wx.FONTFAMILY_TELETYPE)))
        w = self.FromDIP(width_dip)
        lines = max(1, math.ceil((tc.GetTextExtent(value).width + self.FromDIP(16)) / max(1, w - 16)))
        tc.SetMinSize(wx.Size(w, tc.GetCharHeight() * lines + self.FromDIP(10)))
        tc.SetToolTip(value)
        return tc

    def button_row(self, ok: tuple[str, str, str], cancel: tuple[str, str, str] | None = None,
                   help_: tuple[str, str, str] | None = None, extras: tuple = (),
                   ok_id: int = wx.ID_OK, middle: "wx.Sizer | None" = None) -> wx.Sizer:
        """Bottom line. Each tuple = (name, wording key, mnemonic letter). Creation order = tab order:
        extras, help, cancel, ok. Esc = Cancel (or the single affirmative button), Enter = ok."""
        wk = self._wk

        def make(wid, spec):
            name, key, mn = spec
            b = wx.Button(self, wid, _mn(WORDING[wk][key], mn), name=name)
            # never narrower than the label (a fixed 88 DIP cut "Open report folder" to "pen report fold")
            b.SetMinSize(wx.Size(max(self.FromDIP(88), b.GetBestSize().width), -1))
            self.buttons[name] = b
            return b

        extra_btns = [make(e[3] if len(e) > 3 else wx.ID_ANY, e[:3]) for e in extras]
        hb = make(wx.ID_HELP, help_) if help_ else None
        cb = make(wx.ID_CANCEL, cancel) if cancel else None
        okb = make(ok_id, ok)
        okb.SetDefault()
        self.SetAffirmativeId(ok_id)
        self.SetEscapeId(wx.ID_CANCEL if cancel else ok_id)
        std = wx.StdDialogButtonSizer()
        std.AddButton(okb)
        if cb:
            std.AddButton(cb)
        if hb:
            std.AddButton(hb)
        std.Realize()
        row = wx.BoxSizer(wx.HORIZONTAL)
        for b in extra_btns:
            row.Add(b, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        if middle is not None:  # e.g. the "Cannot route" status between Help and Cancel
            row.Add(middle, 1, wx.ALIGN_CENTER_VERTICAL | wx.LEFT | wx.RIGHT, self.row)
            row.Add(std, 0, wx.EXPAND)
        else:
            row.Add(std, 1, wx.EXPAND)
        self.ok_button = okb
        return row

    def finish(self, min_dip: tuple[int, int], open_dip: tuple[int, int] | None = None) -> None:
        """Border, fit, honour the minimum size (DIP), centre on screen."""
        outer = wx.BoxSizer(wx.VERTICAL)
        outer.Add(self.root, 1, wx.EXPAND | wx.ALL, self.group)
        self.SetSizer(outer)
        self.Layout()
        self.Fit()
        mn = self.FromDIP(wx.Size(*min_dip))
        self.SetMinSize(mn)
        size = self.FromDIP(wx.Size(*open_dip)) if open_dip else self.GetSize()
        if wx.Display.GetCount():  # opening size is clamped to 90 % of the display client area
            area = wx.Display(0).GetClientArea()
            size = wx.Size(min(size.width, int(area.width * 0.9)), min(size.height, int(area.height * 0.9)))
        self.SetSize(wx.Size(max(size.width, mn.width), max(size.height, mn.height)))
        self._register()
        self.CentreOnScreen()
        # GTK knows the height of wrapped text only once the window exists: grow to the best size then, else the
        # last lines are cut off (seen live: the first-run notice lost "widths and via sizes ..." at 200 px).
        wx.CallAfter(self._grow_to_best)

    def _grow_to_best(self) -> None:
        if not self:
            return
        self.Layout()
        best, cur = self.GetBestSize(), self.GetSize()
        if best.width > cur.width or best.height > cur.height:
            self.SetSize(wx.Size(max(best.width, cur.width), max(best.height, cur.height)))
            self.Layout()

    def _register(self) -> None:
        """Collect every named descendant into ``ids``; controls that carry no text of their own get
        ``SetName(label)`` + tooltip (wx/GTK has no label-for relation, note §0)."""
        stack = list(self.GetChildren())
        while stack:
            c = stack.pop(0)
            self.ids.setdefault(c.GetName(), c)
            stack[0:0] = list(c.GetChildren())
        for ident, label in self.A11Y.items():
            w = self.ids[ident]
            w.SetName(label)
            w.SetToolTip(label)

    def widget(self, ident: str):
        return self.ids.get(ident)

    def child_ids(self) -> list:
        """Wording ids of the direct children in creation (= Tab) order."""
        rev = {id(w): i for i, w in self.ids.items()}
        alias = getattr(self, "FOCUS_ALIAS", {})  # a bordering panel stands for the focusable window inside
        return [alias.get(rev[id(c)], rev[id(c)]) for c in self.GetChildren() if id(c) in rev]


# --------------------------------------------------------------------------------------------- (1) first run
class FirstRunDialog(_Base):
    _wk = "first_run"
    #: tab order from the note §1
    TAB_ORDER = ("toml_path", "open_file", "continue")

    def __init__(self, toml_path: Path):
        super().__init__(WORDING["first_run"]["title"])
        self.toml_path = Path(toml_path)
        icon = wx.StaticBitmap(self, bitmap=wx.ArtProvider.GetBitmap(
            wx.ART_INFORMATION, wx.ART_MESSAGE_BOX, self.FromDIP(wx.Size(32, 32))), name="icon")
        icon.SetToolTip("Information")
        body = wx.BoxSizer(wx.VERTICAL)
        body.Add(self.text(self, "first_run", "intro", wrap_dip=440), 0, wx.BOTTOM, self.row)
        body.Add(self.mono_field(self, "toml_path", str(toml_path)), 0, wx.EXPAND | wx.BOTTOM, self.row)
        body.Add(self.text(self, "first_run", "explain", wrap_dip=440), 0)
        top = wx.BoxSizer(wx.HORIZONTAL)
        top.Add(icon, 0, wx.RIGHT, self.group)
        top.Add(body, 1, wx.EXPAND)
        self.root.Add(top, 1, wx.EXPAND | wx.BOTTOM, self.group)
        self.root.Add(self.button_row(("continue", "continue", "c"),
                                      extras=(("open_file", "open_file", "o"),)), 0, wx.EXPAND)
        self.buttons["open_file"].Bind(wx.EVT_BUTTON, lambda _e: open_path(self.toml_path))
        self.finish((440, 200))
        self.ok_button.SetFocus()


# --------------------------------------------------------------------------------------------- (2) pre-flight
class PreflightDialog(_Base):
    _wk = "preflight"
    #: tab order from the note §2 (one Accept checkbox, relabelled per selected code)
    TAB_ORDER = ("findings", "cause", "accept", "scope_whole", "scope_between",
                 "scope_touching", "existing", "respect_locked", "partial_routes", "timeout", "threads",
                 "save_toml", "help", "cancel", "route")

    def __init__(self, findings: list, summary: RunSummary, cfg: Config):
        super().__init__(WORDING["preflight"]["title"].format(board=summary.board), resizable=True)
        self.cfg = cfg
        self.findings = sorted(findings, key=lambda f: (_SEV_ORDER[f.severity], f.code))
        self.n_err = sum(f.severity == "error" for f in self.findings)
        n_warn = sum(f.severity == "warning" for f in self.findings)
        n_info = sum(f.severity == "info" for f in self.findings)
        W = WORDING["preflight"]
        g = self.group

        self.summary = summary
        self.summary_label = self.text(self, "preflight", "summary_line", text=self._summary_text(summary.connections))
        self.root.Add(self.summary_label, 0, wx.BOTTOM, g)
        self.root.Add(self.text(self, "preflight", "findings_heading", bold=True, text=W[
            "findings_heading"].format(errors=self.n_err, warnings=n_warn, notes=n_info)), 0, wx.BOTTOM, self.row)

        # findings list (report mode, icon + word in column 1)
        self.list = wx.ListCtrl(self, name="findings", style=wx.LC_REPORT | wx.LC_SINGLE_SEL | wx.BORDER_THEME)
        sz = self.FromDIP(16)
        il = wx.ImageList(sz, sz)
        self._icon = {}
        for sev, art in _SEV_ART.items():
            self._icon[sev] = il.Add(wx.ArtProvider.GetBitmap(art, wx.ART_LIST, wx.Size(sz, sz)))
        self.list.AssignImageList(il, wx.IMAGE_LIST_SMALL)
        self.list.InsertColumn(0, W["col_severity"], width=self.FromDIP(90))
        self.list.InsertColumn(1, W["col_code"], width=self.FromDIP(190))
        self.list.InsertColumn(2, W["col_message"], width=self.FromDIP(380))
        for i, f in enumerate(self.findings):
            self.list.InsertItem(i, W[_SEV_WORD[f.severity]], self._icon[f.severity])
            self.list.SetItem(i, 1, f.code)
            self.list.SetItem(i, 2, f.message)
        self.list.SetMinSize(self.FromDIP(wx.Size(-1, 150)))  # >= 5 rows (live test: 110 showed 1.5 rows)
        self.list.SetToolTip(W["findings_heading"].format(errors=self.n_err, warnings=n_warn, notes=n_info))
        self.root.Add(self.list, 1, wx.EXPAND | wx.BOTTOM, self.row)

        self.cause = wx.TextCtrl(self, name="cause",
                                 style=wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_BESTWRAP)
        self.cause.SetMinSize(wx.Size(-1, self.cause.GetCharHeight() * 4 + self.FromDIP(12)))
        self.cause.SetToolTip(f"{W['cause_label']} / {W['fix_label']}")
        self.root.Add(self.cause, 0, wx.EXPAND | wx.BOTTOM, self.row)

        # ONE Accept checkbox, relabelled per selected code; its state is kept per code
        self.accepted_state = {c: False for c in ACCEPT_CODES}
        self.accept_cb = wx.CheckBox(self, label=_mn(W["accept_pads"], "a"), name="accept")
        self.accept_cb.Hide()
        self.accept_cb.Bind(wx.EVT_CHECKBOX, self._on_accept)
        self.accept_code = None
        self.root.Add(self.accept_cb, 0, wx.BOTTOM, self.row)

        self.root.Add(self.text(self, "preflight", "options_heading", bold=True), 0, wx.TOP | wx.BOTTOM, self.row)
        self._build_options(summary, cfg)

        # blocked status + buttons
        sz16 = self.FromDIP(16)
        self.status_icon = wx.StaticBitmap(self, bitmap=wx.ArtProvider.GetBitmap(
            wx.ART_ERROR, wx.ART_LIST, wx.Size(sz16, sz16)), name="blocked_icon")
        self.status_icon.SetToolTip("Error")
        self.status = self.text(self, "preflight", "blocked_status",
                                text=W["blocked_status"].format(n=self.n_err))
        status_row = wx.BoxSizer(wx.HORIZONTAL)
        status_row.Add(self.status_icon, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        status_row.Add(self.status, 0, wx.ALIGN_CENTER_VERTICAL)
        self.status.Show(self.n_err > 0)
        self.status_icon.Show(self.n_err > 0)
        # bottom line: [Help] (x) Cannot route ... [Cancel] [Route]
        self.root.Add(self.button_row(("route", "route", "r"), ("cancel", "cancel", "c"),
                                      extras=(("help", "help", "h", wx.ID_HELP),), middle=status_row),
                      0, wx.EXPAND | wx.TOP, self.row)
        self.route = self.buttons["route"]
        self.route.Enable(self.n_err == 0)  # visible reason: self.status
        self.buttons["help"].Bind(wx.EVT_BUTTON, self._on_help)

        self.list.Bind(wx.EVT_LIST_ITEM_SELECTED, self._on_select)
        self.list.Bind(wx.EVT_LIST_ITEM_ACTIVATED, self._on_activate)  # Enter in the list = Route
        self.list.Bind(wx.EVT_SIZE, self._on_list_size)
        if self.findings:
            self.list.Select(0)
            self.list.Focus(0)
        self._show_finding(0 if self.findings else None)
        self.A11Y = {"findings": W["findings_heading"].format(errors=self.n_err, warnings=n_warn, notes=n_info),
                     "cause": f"{W['cause_label']} / {W['fix_label']}", "existing": W["existing_label"],
                     "timeout": W["timeout_label"], "threads": W["threads_label"]}
        self.finish((640, 560), (760, 780))
        self.list.SetFocus()

    def _build_options(self, summary: RunSummary, cfg: Config) -> None:
        W = WORDING["preflight"]
        grid = wx.FlexGridSizer(2, self.row, self.group)
        grid.AddGrowableCol(1, 1)

        # scope
        self.has_selection = bool(summary.has_selection)
        self.scope_whole = wx.RadioButton(self, label=_mn(W["scope_whole"], "w"), name="scope_whole",
                                          style=wx.RB_GROUP)
        self.scope_between = wx.RadioButton(self, label=_mn(W["scope_between"], "b"), name="scope_between")
        self.scope_touching = wx.RadioButton(self, label=_mn(W["scope_touching"], "t"), name="scope_touching")
        mode = cfg.get("run", "scope_mode")
        if self.has_selection:
            (self.scope_touching if mode == "touching" else self.scope_between).SetValue(True)
        else:
            self.scope_whole.SetValue(True)
            self.scope_between.Disable()
            self.scope_touching.Disable()
        hint = self.text(self, "preflight", "scope_hint")
        hint.SetForegroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
        hint.Show(not self.has_selection)
        rv = wx.BoxSizer(wx.VERTICAL)
        first = wx.BoxSizer(wx.HORIZONTAL)
        first.Add(self.scope_whole, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.group)
        first.Add(hint, 0, wx.ALIGN_CENTER_VERTICAL)
        rv.Add(first)
        rv.Add(self.scope_between, 0, wx.TOP, self.row // 2)
        rv.Add(self.scope_touching, 0, wx.TOP, self.row // 2)
        for rb in (self.scope_whole, self.scope_between, self.scope_touching):
            rb.Bind(wx.EVT_RADIOBUTTON, self._update_count)
        self._update_count()
        grid.Add(self.text(self, "preflight", "scope_label"), 0)
        grid.Add(rv)

        # existing copper
        self._existing_keys = ("fixed", "smooth", "rip")
        self.existing = wx.Choice(self, name="existing", choices=[
            W["existing_fixed"], W["existing_smooth"], W["existing_rip"]])
        self.existing.SetSelection(self._existing_keys.index(cfg.get("run", "existing_copper")))
        self.existing.SetToolTip(W["existing_tip"])
        grid.Add(self.text(self, "preflight", "existing_label"), 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.existing)

        self.respect_locked = wx.CheckBox(self, label=_mn(W["respect_locked"], "l"), name="respect_locked")
        self.respect_locked.SetValue(bool(cfg.get("run", "respect_locked")))
        self.partial_routes = wx.CheckBox(self, label=_mn(W["partial_routes"], "k"), name="partial_routes")
        self.partial_routes.SetValue(bool(cfg.get("run", "partial_routes")))
        grid.AddSpacer(0)
        grid.Add(self.respect_locked)
        grid.AddSpacer(0)
        grid.Add(self.partial_routes)

        # time limit / threads (reached by Tab; spin controls carry a preceding label)
        self.timeout = wx.SpinCtrlDouble(self, name="timeout", min=0, max=86400, inc=10,
                                         initial=min(86400.0, float(cfg.get("run", "timeout_s"))))
        self.timeout.SetDigits(0)
        self.timeout.SetToolTip(W["timeout_label"])
        tr = wx.BoxSizer(wx.HORIZONTAL)
        tr.Add(self.timeout, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        tr.Add(self.text(self, "preflight", "timeout_unit"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.group)
        tr.Add(self.text(self, "preflight", "timeout_hint"), 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.text(self, "preflight", "timeout_label"), 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(tr)
        self.threads = wx.SpinCtrl(self, name="threads", min=0, max=256, initial=int(cfg.get("run", "threads")))
        self.threads.SetToolTip(W["threads_label"])
        thr = wx.BoxSizer(wx.HORIZONTAL)
        thr.Add(self.threads, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.group)
        thr.Add(self.text(self, "preflight", "threads_hint"), 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.text(self, "preflight", "threads_label"), 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(thr)

        self.save_toml = wx.CheckBox(self, label=_mn(W["save_toml"], "s"), name="save_toml")
        grid.AddSpacer(0)
        grid.Add(self.save_toml)
        self.root.Add(grid, 0, wx.EXPAND | wx.BOTTOM, self.row)

    # -- behaviour
    def cause_text(self, f: Finding) -> str:
        W = WORDING["preflight"]
        items = [str(i) for i in f.items]
        txt = f.message
        if items:
            txt += ": " + ", ".join(items[:_ITEMS_PREFLIGHT])
            if len(items) > _ITEMS_PREFLIGHT:
                txt += ", " + W["items_more"].format(n=len(items) - _ITEMS_PREFLIGHT)
        return f"{W['cause_label']}  {txt}\n{W['fix_label']}    {f.fix}"

    def _show_finding(self, idx) -> None:
        if idx is None or not (0 <= idx < len(self.findings)):
            self.cause.SetValue(WORDING["preflight"]["no_findings"] if not self.findings else "")
            code = None
        else:
            f = self.findings[idx]
            self.cause.SetValue(self.cause_text(f))
            code = f.code
        self.accept_code = code if code in ACCEPT_CODES else None
        if self.accept_code:
            self.accept_cb.SetLabel(_mn(WORDING["preflight"][ACCEPT_CODES[code]], "a"))
            self.accept_cb.SetValue(self.accepted_state[code])
        self.accept_cb.Show(self.accept_code is not None)
        self.Layout()

    def _on_accept(self, _evt) -> None:
        if self.accept_code:
            self.accepted_state[self.accept_code] = self.accept_cb.GetValue()

    def _summary_text(self, connections: int) -> str:
        s = self.summary
        return WORDING["preflight"]["summary_line"].format(
            connections=connections, nets=s.nets, layers=s.layers, engine_version=s.engine_version)

    def scope_mode_selected(self) -> str:
        return "touching" if self.scope_touching.GetValue() else "between" if self.scope_between.GetValue() \
            else "whole"

    def _update_count(self, _evt=None) -> None:
        """The connection count follows the Scope radio (RunSummary.connections_by_scope)."""
        n = self.summary.connections_by_scope.get(self.scope_mode_selected(), self.summary.connections)
        self.summary_label.SetLabel(self._summary_text(n))

    def _on_select(self, evt) -> None:
        self._show_finding(evt.GetIndex())

    def _on_activate(self, _evt) -> None:
        if self.route.IsEnabled():
            self.EndModal(wx.ID_OK)

    def _on_list_size(self, evt) -> None:
        evt.Skip()
        w = self.list.GetClientSize().width - self.list.GetColumnWidth(0) - self.list.GetColumnWidth(1)
        if w > self.FromDIP(200):
            self.list.SetColumnWidth(2, w)

    def _on_help(self, _evt) -> None:
        if HELP_DOC is not None and HELP_DOC.exists():
            wx.LaunchDefaultBrowser(HELP_DOC.as_uri() + "#" + HELP_ANCHOR)

    def decision(self, proceed: bool) -> Decision:
        """The user's answer. Cancel/Esc -> nothing proceeds and nothing is saved (not even accepted codes)."""
        if not proceed:
            return Decision(False)
        present = {f.code for f in self.findings}
        accepted = tuple(c for c in ACCEPT_CODES if c in present and self.accepted_state[c])
        cfg = self.cfg
        ov: dict = {}

        def put(key, val):
            if val != cfg.get("run", key):
                ov[key] = val

        if self.has_selection:
            mode = "touching" if self.scope_touching.GetValue() else "between"
            put("scope_mode", mode)
            # whole board with a selection present: scope_mode stays a valid toml value, this flag carries it
            ov["scope_whole"] = self.scope_whole.GetValue()
        put("existing_copper", self._existing_keys[self.existing.GetSelection()])
        put("respect_locked", self.respect_locked.GetValue())
        put("partial_routes", self.partial_routes.GetValue())
        put("timeout_s", float(self.timeout.GetValue()))
        put("threads", int(self.threads.GetValue()))
        return Decision(True, accepted, ov, self.save_toml.GetValue())


# --------------------------------------------------------------------------------------------- (3) progress
class ProgressDialog(_Base):
    _wk = "progress"
    TAB_ORDER = ("cancel",)

    def __init__(self, board: str, timeout_s: float = 0.0, iteration_cap: int | None = None, stable_window: int = 4):
        super().__init__(WORDING["progress"]["title"].format(board=board))
        self.board = board
        self.timeout_s = float(timeout_s or 0)
        self.iteration_cap = iteration_cap
        self._round, self._round_from_engine = (0, iteration_cap), False
        self.cancelled = False
        W = WORDING["progress"]

        self.phase = self.text(self, "progress", "phase_route", name="phase", text="", bold=True)
        self.root.Add(self.phase, 0, wx.BOTTOM, self.group)
        gr = wx.BoxSizer(wx.HORIZONTAL)
        self.gauge = wx.Gauge(self, range=1, name="gauge", style=wx.GA_HORIZONTAL)
        self.gauge.SetMinSize(self.FromDIP(wx.Size(260, 16)))
        self.gauge_value = self._num("gauge_value", 80)
        gr.Add(self.gauge, 1, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        gr.Add(self.gauge_value, 0, wx.ALIGN_CENTER_VERTICAL)
        self.root.Add(gr, 0, wx.EXPAND | wx.BOTTOM, self.group)

        grid = wx.FlexGridSizer(4, self.row, self.group)
        self.iteration_value = self._num("iteration_value", 110)
        self.routed_value = self._num("routed_value", 90)
        self.vias_value = self._num("vias_value", 110)
        self.elapsed_value = self._num("elapsed_value", 140)
        self.total_value = self._num("total_value", 140)
        self.gain_value = self._num("gain_now", 140)
        self.runs_value = self._num("runs_now", 110)
        for lab, val in (("iteration_label", self.iteration_value), ("routed_label", self.routed_value),
                         ("vias_label", self.vias_value), ("elapsed_label", self.elapsed_value),
                         ("runs_label", self.runs_value), ("total_label", self.total_value),
                         ("gain_label", self.gain_value)):
            grid.Add(self.text(self, "progress", lab), 0)
            grid.Add(val, 0, wx.ALIGN_RIGHT)
        self.root.Add(grid, 0, wx.BOTTOM, self.group)
        # M-B0: the best result so far (what OK / Cancel keeps), over all runs
        self.best_value = self.text(self, "progress", "best_pending", name="best_value")
        self.root.Add(self.best_value, 0, wx.BOTTOM, self.group)
        self.stable = False
        self.stable_window = stable_window
        self.root.Add(self.text(self, "progress", "cancel_hint", wrap_dip=420), 0, wx.BOTTOM, self.group)
        self.root.Add(self.button_row(("cancel", "cancel", "c"), ok_id=wx.ID_CANCEL), 0, wx.EXPAND)
        self.cancel_button = self.buttons["cancel"]
        self.cancel_button.Bind(wx.EVT_BUTTON, self._on_cancel)
        self.Bind(wx.EVT_CLOSE, self._on_cancel)
        self.Bind(wx.EVT_CHAR_HOOK, self._on_key)
        self.set_phase("")
        self.finish((460, 300))
        self.cancel_button.SetFocus()

    def _num(self, name: str, width_dip: int) -> wx.StaticText:
        """Right-aligned label of fixed minimum width: digits changing never shifts the layout."""
        st = wx.StaticText(self, label="", style=wx.ALIGN_RIGHT | wx.ST_NO_AUTORESIZE, name=name)
        st.SetMinSize(wx.Size(self.FromDIP(width_dip), -1))
        return st

    def set_phase(self, phase: str, stage: str = "") -> None:
        if self.cancelled:
            return
        if self.stable and phase in ("setup", "route", "done"):  # M-B0: the user's cue that OK is fine now
            self.phase.SetLabel(WORDING["progress"]["phase_stable"])
            return
        if phase in ("setup", "route", "done") and stage:  # the topological core names its stage (K1)
            self.phase.SetLabel(WORDING["progress"]["phase_route_stage"].format(stage=stage))
            return
        key = PHASE_KEYS.get(phase)
        self.phase.SetLabel(WORDING["progress"][key] if key else phase)

    def _on_cancel(self, evt) -> None:
        if isinstance(evt, wx.CloseEvent) and evt.CanVeto():
            evt.Veto()
        self.cancel()

    def _on_key(self, evt) -> None:
        if evt.GetKeyCode() in (wx.WXK_ESCAPE, wx.WXK_RETURN, wx.WXK_NUMPAD_ENTER):
            self.cancel()
        else:
            evt.Skip()

    def cancel(self) -> None:
        if self.cancelled:
            return
        self.cancelled = True
        self.phase.SetLabel(WORDING["progress"]["phase_finishing" if self.stable else "phase_cancelling"])
        self.cancel_button.Disable()
        self.gauge.Pulse()

    def set_stable(self, stable: bool) -> None:
        """M-B0: once the routes are stable the Cancel button reads OK (same id, same action: stop and keep the
        best result so far, which is exactly the board the user gets). Stable never goes back within a run."""
        if stable and not self.stable:
            self.stable = True
            self.SetTitle(WORDING["progress"]["title_stable"].format(board=self.board))
            self.cancel_button.SetLabel(_mn(WORDING["progress"]["ok"], "o"))
            self.cancel_button.SetDefault()
            self.Layout()

    def update(self, p: Progress) -> None:
        W = WORDING["progress"]
        self.set_stable(bool(getattr(p, "stable", False)))
        self.set_phase(p.phase, getattr(p, "stage", ""))
        if self.cancelled or not (p.phase == "route" and p.total > 0):
            self.gauge.Pulse()
            self.gauge_value.SetLabel("")
        else:
            self.gauge.SetRange(p.total)
            self.gauge.SetValue(max(0, min(p.routed, p.total)))
            self.gauge_value.SetLabel(W["gauge_value"].format(routed=p.routed, total=p.total))
        if getattr(p, "rounds", 0):  # the engine names its round; the plugin's own phases keep the last one
            if p.round:
                self._round = (p.round, p.rounds)
        elif not self._round_from_engine:
            self._round = (p.iteration, self.iteration_cap)  # an engine without rounds: its iteration
        self._round_from_engine = self._round_from_engine or bool(getattr(p, "rounds", 0))
        rnd, cap = self._round
        if self._round_from_engine:  # M-B0: [optimize] rounds is only a safety cap; the run ends at "stable"
            self.iteration_value.SetLabel(W["round_value"].format(round=rnd))
        else:
            self.iteration_value.SetLabel(W["iteration_value"].format(iteration=rnd, cap=cap) if cap else str(rnd))
        g = getattr(p, "gain_pct", None)
        self.gain_value.SetLabel(W["gain_value"].format(gain=f"{g:.1f}", window=self.stable_window) if g is not None
                                 else W["gain_pending"])
        runs = int(getattr(p, "runs", 0) or 0)
        self.runs_value.SetLabel(W["runs_value"].format(stable=getattr(p, "runs_stable", 0), runs=runs) if runs
                                 else W["gain_pending"])
        if getattr(p, "best_total", 0):
            txt = W["best_label"].format(routed=p.best_routed, total=p.best_total, length=f"{p.best_length_mm / 1000:.2f}",
                                         vias=p.best_vias)
            if runs > 1 and p.best_run:
                txt += " " + W["best_run"].format(run=p.best_run)
            self.best_value.SetLabel(txt)
        if p.iteration == 0 and p.routed == 0:
            self.routed_value.SetLabel(W["routed_pending"].format(total=p.total))
        else:
            self.routed_value.SetLabel(W["routed_value"].format(routed=p.routed, total=p.total))
        self.vias_value.SetLabel(str(p.vias))
        el = _fmt_time(p.elapsed_s)
        self.elapsed_value.SetLabel(W["elapsed_value"].format(elapsed=el, limit=_fmt_time(self.timeout_s))
                                    if self.timeout_s > 0 else W["elapsed_nolimit"].format(elapsed=el))
        self.total_value.SetLabel(_fmt_time(max(getattr(p, "wall_s", 0.0), p.elapsed_s)))


# --------------------------------------------------------------------------------------------- (4) preview
class PreviewDialog(_Base):
    _wk = "preview"
    TAB_ORDER = ("zoom", "image", "summary", "report_dir", "open_report_folder", "reject", "accept")
    FOCUS_ALIAS = {"image_frame": "image"}

    def __init__(self, png: Path, report, board: str = ""):
        super().__init__(WORDING["preview"]["title"].format(board=board), resizable=True)
        W = WORDING["preview"]
        self.report = report
        self.report_dir = Path(report.dir) if getattr(report, "dir", None) else None
        self.image = self._load(png)
        g, r = self.group, self.row

        # row: Zoom + legend
        top = wx.BoxSizer(wx.HORIZONTAL)
        top.Add(self.text(self, "preview", "zoom_label"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, r)
        self.zoom = wx.Choice(self, name="zoom", choices=[W[k] for k in _ZOOM_KEYS])
        self.zoom.SetSelection(0)
        self.zoom.SetToolTip(W["zoom_label"])
        top.Add(self.zoom, 0, wx.ALIGN_CENTER_VERTICAL)
        top.AddStretchSpacer()
        top.Add(self.text(self, "preview", "legend_label"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, r)
        # the single fixed colour of the UI: the preview highlight (note §4)
        self.swatch = wx.Panel(self, name="legend_swatch", size=self.FromDIP(wx.Size(16, 16)))
        self.swatch.SetBackgroundColour(wx.Colour(*PREVIEW_HIGHLIGHT_RGB))
        self.swatch.SetMinSize(self.FromDIP(wx.Size(16, 16)))
        self.swatch.SetToolTip(W["legend_label"])
        top.Add(self.swatch, 0, wx.ALIGN_CENTER_VERTICAL)
        self.root.Add(top, 0, wx.EXPAND | wx.BOTTOM, r)

        # headings (screen readers get what the PNG caption shows) + image scroller + summary
        main = wx.BoxSizer(wx.HORIZONTAL)
        left = wx.BoxSizer(wx.VERTICAL)
        heads = wx.BoxSizer(wx.HORIZONTAL)
        heads.Add(self.text(self, "preview", "before_heading", bold=True, style=wx.ALIGN_CENTRE_HORIZONTAL), 1)
        heads.Add(self.text(self, "preview", "after_heading", bold=True, style=wx.ALIGN_CENTRE_HORIZONTAL), 1)
        left.Add(heads, 0, wx.EXPAND | wx.BOTTOM, r)
        # 1 px SYS_COLOUR_GRAYTEXT outline so the image does not blur into the surface
        frame = wx.Panel(self, name="image_frame")
        frame.SetBackgroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
        self.scroller = wx.ScrolledWindow(frame, name="image", style=wx.HSCROLL | wx.VSCROLL | wx.WANTS_CHARS)
        self.scroller.SetCanFocus(True)
        self.scroller.SetBackgroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_WINDOW))
        self.scroller.SetScrollRate(self.FromDIP(10), self.FromDIP(10))
        fs = wx.BoxSizer(wx.VERTICAL)
        fs.Add(self.scroller, 1, wx.EXPAND | wx.ALL, 1)
        frame.SetSizer(fs)
        ssz = wx.BoxSizer(wx.VERTICAL)
        if self.image is None:
            self.bitmap = None
            ssz.Add(self.text(self.scroller, "preview", "no_image", wrap_dip=420), 1, wx.ALL, g)
        else:
            self.bitmap = wx.StaticBitmap(self.scroller, name="bitmap")
            ssz.Add(self.bitmap, 0)
        self.scroller.SetSizer(ssz)
        left.Add(frame, 1, wx.EXPAND)
        main.Add(left, 1, wx.EXPAND | wx.RIGHT, g)

        right = wx.BoxSizer(wx.VERTICAL)
        right.Add(self.text(self, "preview", "summary_heading", bold=True), 0, wx.BOTTOM, r)
        self.summary = wx.TextCtrl(self, name="summary", value="\n".join(report.summary_lines()),
                                   style=wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_BESTWRAP)
        self.summary.SetMinSize(self.FromDIP(wx.Size(260, 200)))
        self.summary.SetToolTip(W["summary_heading"])
        right.Add(self.summary, 1, wx.EXPAND)
        main.Add(right, 0, wx.EXPAND)
        self.root.Add(main, 1, wx.EXPAND | wx.BOTTOM, g)

        # report folder
        rd = wx.BoxSizer(wx.HORIZONTAL)
        rd.Add(self.text(self, "preview", "report_dir_label"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, r)
        self.dir_field = self.mono_field(self, "report_dir", str(self.report_dir or ""), 600)
        rd.Add(self.dir_field, 1, wx.ALIGN_CENTER_VERTICAL)
        self.root.Add(rd, 0, wx.EXPAND | wx.BOTTOM, g)
        self.root.Add(self.button_row(("accept", "accept", "a"), ("reject", "reject", "r"),
                                      extras=(("open_report_folder", "open_report_folder", "o"),)),
                      0, wx.EXPAND)
        self.open_btn = self.buttons["open_report_folder"]
        self.open_btn.Bind(wx.EVT_BUTTON, lambda _e: self.report_dir and open_path(self.report_dir))

        self.zoom.Bind(wx.EVT_CHOICE, lambda _e: self._render())
        self.scroller.Bind(wx.EVT_SIZE, self._on_size)
        for w in (self.scroller, self.bitmap):
            if w is not None:
                w.Bind(wx.EVT_MOUSEWHEEL, self._on_wheel)

        disp = wx.Display(0).GetClientArea() if wx.Display.GetCount() else wx.Rect(0, 0, 1280, 800)
        self.A11Y = {"zoom": W["zoom_label"], "image": f"{W['before_heading']} / {W['after_heading']}",
                     "summary": W["summary_heading"], "report_dir": W["report_dir_label"]}
        self.finish((800, 520))
        mn = self.GetMinSize()
        self.SetSize(wx.Size(max(int(disp.width * 0.85), mn.width), max(int(disp.height * 0.85), mn.height)))
        self.CentreOnScreen()
        self._render()
        self.zoom.SetFocus()
        self.Bind(wx.EVT_CHAR_HOOK, self._on_char_hook)

    def _on_char_hook(self, evt) -> None:
        """Enter = Accept wherever the focus is (GTK: the focused Zoom choice would open its list instead, and the
        image / summary panes swallow the key), except on a button, which activates itself. Found in the live test
        (tools/gui_e2e.sh): Enter did nothing while the Zoom choice had the focus."""
        if evt.GetKeyCode() in (wx.WXK_RETURN, wx.WXK_NUMPAD_ENTER) and not evt.HasAnyModifiers() \
                and not isinstance(wx.Window.FindFocus(), wx.Button):
            self.EndModal(wx.ID_OK)
            return
        evt.Skip()

    @staticmethod
    def _load(png) -> "wx.Image | None":
        try:
            if png is None or not Path(png).is_file():
                return None
            img = wx.Image(str(png))
            return img if img.IsOk() else None
        except Exception:  # noqa: BLE001 - unreadable image -> "no_image" state
            return None

    def zoom_factor(self):
        """None = Fit, else 1.0 / 2.0."""
        return (None, 1.0, 2.0)[self.zoom.GetSelection()]

    def _render(self) -> None:
        if self.image is None or self.bitmap is None:
            return
        f = self.zoom_factor()
        iw, ih = self.image.GetSize()
        if f is None:
            cw, ch = self.scroller.GetClientSize()
            s = min(max(cw, 1) / iw, max(ch, 1) / ih)
        else:
            s = f * self.FromDIP(1)  # 100 % = one PNG pixel per DIP (x2 physical at 200 % scaling)
        w, h = max(1, int(iw * s)), max(1, int(ih * s))
        img = self.image if (w, h) == (iw, ih) else self.image.Scale(w, h, wx.IMAGE_QUALITY_HIGH)
        self.bitmap.SetBitmap(wx.Bitmap(img))
        self.scroller.SetVirtualSize(w, h)
        self.scroller.Layout()

    def _on_size(self, evt) -> None:
        evt.Skip()
        if self.image is not None and self.zoom_factor() is None:
            wx.CallAfter(self._safe_render)

    def _safe_render(self) -> None:
        if self:  # not destroyed meanwhile
            self._render()

    def _on_wheel(self, evt) -> None:
        if evt.ControlDown():
            step = 1 if evt.GetWheelRotation() > 0 else -1
            sel = min(max(self.zoom.GetSelection() + step, 0), len(_ZOOM_KEYS) - 1)
            self.zoom.SetSelection(sel)
            self._render()
        else:
            evt.Skip()


# --------------------------------------------------------------------------------------------- (5) done
class DoneDialog(_Base):
    _wk = "report"
    TAB_ORDER = ("report_dir", "open_report_folder", "close")

    def __init__(self, outcome: str, report, report_dir: Path):
        super().__init__(WORDING["report"]["title"])
        W = WORDING["report"]
        self.report_dir = Path(report_dir)
        routed, total = self._counts(report)
        if outcome == "accepted":
            name = ("done_accepted_cancelled" if getattr(report, "stop_reason", None) == "cancel"
                    else "done_accepted")
            msg = (W[name].format(routed=routed, total=total) if routed is not None
                   else "Applied to the board as one undo step (Edit → Undo).")
        elif outcome == "cancelled":
            msg, name = W["done_cancelled"], "done_cancelled"
        else:
            msg, name = W["done_rejected"], "done_rejected"
        icon = wx.StaticBitmap(self, bitmap=wx.ArtProvider.GetBitmap(
            wx.ART_INFORMATION, wx.ART_MESSAGE_BOX, self.FromDIP(wx.Size(32, 32))), name="icon")
        icon.SetToolTip("Information")
        body = wx.BoxSizer(wx.VERTICAL)
        body.Add(self.text(self, "report", name, text=msg, wrap_dip=420), 0, wx.BOTTOM, self.group)
        if outcome == "accepted":
            facts = getattr(report, "facts", None) or {}
            if "zones_refilled" in facts:  # ruling C28
                rid = "done_refilled" if facts["zones_refilled"] else "done_refill_hint"
                body.Add(self.text(self, "report", rid, wrap_dip=420), 0, wx.BOTTOM, self.group)
            for f in facts.get("post_commit_failed") or ():  # ruling C30: committed, then a step failed
                body.Add(self.text(self, "report", "done_post_commit", wrap_dip=420,
                                   text=W["done_post_commit"].format(message=f.message, fix=f.fix)),
                         0, wx.BOTTOM, self.group)
        body.Add(self.text(self, "report", "report_dir_label"), 0, wx.BOTTOM, self.row)
        body.Add(self.mono_field(self, "report_dir", str(self.report_dir)), 0, wx.EXPAND)
        top = wx.BoxSizer(wx.HORIZONTAL)
        top.Add(icon, 0, wx.RIGHT, self.group)
        top.Add(body, 1, wx.EXPAND)
        self.root.Add(top, 1, wx.EXPAND | wx.BOTTOM, self.group)
        self.root.Add(self.button_row(("close", "close", "c"),
                                      extras=(("open_report_folder", "open_report_folder", "o"),)), 0, wx.EXPAND)
        self.buttons["open_report_folder"].Bind(wx.EVT_BUTTON, lambda _e: open_path(self.report_dir))
        self.A11Y = {"report_dir": W["report_dir_label"]}
        self.finish((440, 220))
        self.ok_button.SetFocus()

    @staticmethod
    def _counts(report):
        """(routed, total) from the report's summary block ("Routed 27/27 connections ..."), else (None, None)."""
        try:
            for line in report.summary_lines():
                m = re.match(r"Routed (\d+)/(\d+)", line)
                if m:
                    return int(m.group(1)), int(m.group(2))
        except Exception:  # noqa: BLE001
            pass
        return None, None


# --------------------------------------------------------------------------------------------- (6) error
class ErrorDialog(_Base):
    _wk = "error"
    TAB_ORDER = ("details", "copy_details", "open_report_folder", "close")

    def __init__(self, finding: Finding, ctx: UiContext | None = None):
        super().__init__(WORDING["error"]["title"], resizable=True)
        ctx = ctx or UiContext()
        W = WORDING["error"]
        self.finding = finding
        self.ctx = ctx
        self.details_text = self._details(finding, ctx)

        icon = wx.StaticBitmap(self, bitmap=wx.ArtProvider.GetBitmap(
            wx.ART_ERROR, wx.ART_LIST, self.FromDIP(wx.Size(16, 16))), name="icon")
        icon.SetToolTip("Error")
        head = self.text(self, "error", "code", text=finding.code, bold=True)
        hr = wx.BoxSizer(wx.HORIZONTAL)
        hr.Add(icon, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        hr.Add(head, 0, wx.ALIGN_CENTER_VERTICAL)
        self.root.Add(hr, 0, wx.BOTTOM, self.row)
        self.root.Add(self.text(self, "error", "message", text=finding.message, wrap_dip=480), 0, wx.BOTTOM,
                      self.group)
        self.root.Add(self.text(self, "error", "fix_label", bold=True), 0, wx.BOTTOM, self.row)
        self.root.Add(self.text(self, "error", "fix", text=finding.fix, wrap_dip=480), 0, wx.BOTTOM, self.group)
        self.root.Add(self.text(self, "error", "details_label", bold=True), 0, wx.BOTTOM, self.row)
        self.details = wx.TextCtrl(self, name="details", value=self.details_text,
                                   style=wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_DONTWRAP | wx.HSCROLL)
        self.details.SetFont(wx.Font(wx.FontInfo().Family(wx.FONTFAMILY_TELETYPE)))
        self.details.SetMinSize(wx.Size(self.FromDIP(480), self.details.GetCharHeight() * 7 + self.FromDIP(12)))
        self.details.SetToolTip(W["details_label"])
        self.root.Add(self.details, 1, wx.EXPAND | wx.BOTTOM, self.row)

        self.pending = self.text(self, "error", "no_report_folder")
        self.pending.SetForegroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
        self.root.Add(self.pending, 0, wx.BOTTOM, self.row)
        self.root.Add(self.button_row(("close", "close", "c"), extras=(
            ("copy_details", "copy_details", "d"), ("open_report_folder", "open_report_folder", "o"))),
            0, wx.EXPAND)
        self.copy_btn = self.buttons["copy_details"]
        self.open_btn = self.buttons["open_report_folder"]
        exists = bool(ctx.report_dir and Path(ctx.report_dir).exists())
        self.open_btn.Enable(exists)
        self.pending.Show(not exists)
        self.copy_btn.Bind(wx.EVT_BUTTON, self._on_copy)
        self.open_btn.Bind(wx.EVT_BUTTON, lambda _e: ctx.report_dir and open_path(ctx.report_dir))
        self.A11Y = {"details": W["details_label"]}
        self.finish((520, 320))
        self.ok_button.SetFocus()

    @staticmethod
    def _details(f: Finding, ctx: UiContext) -> str:
        W = WORDING["error"]
        items = [str(i) for i in f.items]
        shown = ", ".join(items[:_ITEMS_ERROR])
        if len(items) > _ITEMS_ERROR:
            shown += ", " + W["items_more"].format(n=len(items) - _ITEMS_ERROR)
        v = ctx.versions

        def ver(k):
            return str(v.get(k, "unknown"))

        return W["details"].format(
            code=f.code, message=f.message, fix=f.fix, items=shown or "none",
            log_path=ctx.log_path or "none", engine_version=ver("engine"), plugin_version=ver("plugin"),
            kipy_version=ver("kipy"), kicad_version=ver("kicad"),
            timestamp=time.strftime("%Y-%m-%d %H:%M:%S"))

    def copy_details(self) -> bool:
        """Put the details on the clipboard; the label flip to 'Copied' for 2 s is the only feedback."""
        ok = False
        if wx.TheClipboard.Open():
            try:
                ok = wx.TheClipboard.SetData(wx.TextDataObject(self.details_text))
            finally:
                wx.TheClipboard.Close()
        if ok:
            self.copy_btn.SetLabel(WORDING["error"]["copied"])
            wx.CallLater(_COPIED_MS, self._restore_copy_label)
        return ok

    def _on_copy(self, _evt) -> None:
        self.copy_details()

    def _restore_copy_label(self) -> None:
        if self:  # dialog may be gone already
            self.copy_btn.SetLabel(_mn(WORDING["error"]["copy_details"], "d"))


# --------------------------------------------------------------------------------------------- the port
class WxUi:
    """``UiPort`` on wx. See the module docstring for the threading contract."""

    def __init__(self):
        self._app = wx.GetApp() or wx.App(False)
        self.ctx = UiContext()
        self._progress: ProgressDialog | None = None
        self._cancelled = False
        self._timeout_s = float(Config.defaults().get("run", "timeout_s"))
        self._iter_cap = int(Config.defaults().get("optimize", "rounds"))
        self._stable_window = int(Config.defaults().get("optimize", "stable_window"))
        self._last_progress: Progress | None = None

    #: M-B0: the progress dialog lets the user end the run (OK at "Routes stable"); the run flow then does not
    #: stop the engine at stable by itself.
    waits_for_ok = True

    def set_context(self, board: str | None = None, report_dir=None, log_path=None, versions=None) -> None:
        """B5: board name (progress / preview titles), report folder, log.txt and version strings for dialog 6."""
        if board is not None:
            self.ctx.board = board
        if report_dir is not None:
            self.ctx.report_dir = Path(report_dir)
        if log_path is not None:
            self.ctx.log_path = Path(log_path)
        if versions is not None:
            self.ctx.versions = dict(versions)

    # -- plumbing (overridable by tests so no window is ever shown)
    def _present(self, dlg: wx.Dialog) -> None:
        dlg.Show()

    def _pump(self) -> None:
        wx.GetApp().Yield(True)

    def _run_modal(self, dlg: wx.Dialog) -> int:
        self._close_progress()
        try:
            return dlg.ShowModal()
        finally:
            dlg.Destroy()

    def _close_progress(self) -> None:
        if self._progress is not None:
            self._progress.Destroy()
            self._progress = None
        self._cancelled = False

    # -- UiPort
    def first_run_notice(self, toml_path: Path) -> None:
        self._run_modal(FirstRunDialog(toml_path))

    def preflight(self, findings: list, summary: RunSummary, cfg: Config) -> Decision:
        if summary.board:
            self.ctx.board = summary.board
        self._close_progress()
        dlg = PreflightDialog(findings, summary, cfg)
        try:
            rc = dlg.ShowModal()
            d = dlg.decision(rc == wx.ID_OK)
        finally:
            dlg.Destroy()
        if d.proceed:  # progress needs the effective limit / cap
            self._timeout_s = float(d.overrides.get("timeout_s", cfg.get("run", "timeout_s")))
        self._iter_cap = int(cfg.get("optimize", "rounds"))  # the topological core's whole-route rounds
        self._stable_window = int(cfg.get("optimize", "stable_window"))
        return d

    def progress(self, p: Progress) -> bool:
        self._last_progress = p
        if self._progress is None:
            self._progress = ProgressDialog(self.ctx.board, self._timeout_s, self._iter_cap, self._stable_window)
            self._present(self._progress)
        dlg = self._progress
        dlg.update(p)
        self._pump()
        if dlg.cancelled:
            self._cancelled = True
        return not self._cancelled

    def preview(self, png: Path, report) -> bool:
        return self._run_modal(PreviewDialog(png, report, self.ctx.board)) == wx.ID_OK

    def done(self, outcome: str, report, report_dir: Path) -> None:
        self._run_modal(DoneDialog(outcome, report, report_dir))

    def error(self, finding: Finding) -> None:
        self._run_modal(ErrorDialog(finding, self.ctx))
