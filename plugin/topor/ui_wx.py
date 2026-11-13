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

import json
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
        # 0 errors and 0 warnings (user 2026-11-11: "No problems found" instead of a count of zeros)
        "no_problems": "No problems found",
        "no_problems_notes": "No problems found ({notes} notes)",
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
        "timeout_label": "Time limit",
        "timeout_unit": "s",
        "timeout_hint": "0 = no limit",
        "drawing_style_label": "Trace shape",
        "drawing_style_octilinear": "45° (octilinear)",
        "drawing_style_smooth": "Any angle (smooth)",
        "drawing_style_tip": "45°: straight and 45 degree segments, few bends, 90 degree bends only where needed; where "
                             "45 degrees does not fit a stretch goes any-angle at a price (warned per stretch). Any "
                             "angle: the taut, curved-looking drawing. Set in topor.toml as [drawing] style.",
        "timeout_clock_label": "Count",
        "timeout_clock_wall": "Wall time (what you wait)",
        "timeout_clock_cpu": "CPU time (all threads)",
        "timeout_clock_tip": "Wall: seconds on the clock. CPU: CPU seconds of router work summed over all threads; "
                             "comparable across machine load and thread count, so a 4-thread run uses it up about "
                             "4 times as fast. Set in topor.toml as [run] timeout_clock.",
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
        # after Cancel the engine finishes the stage it is in; {stage} = " (drawing the traces)" or "", {s} = seconds
        "phase_cancelling": "Cancel pressed: finishing the current stage{stage}… {s} s",
        "gauge_value": "{routed} / {total}",
        "iteration_label": "Round",
        "iteration_value": "{iteration} of up to {cap}",
        "routed_label": "Routed",
        "routed_value": "{routed} / {total}",
        "routed_pending": "– / {total}",
        "vias_label": "Vias",
        "elapsed_label": "Router time",
        "elapsed_value": "{elapsed} / {limit} · CPU {cpu}",
        "elapsed_cpu_value": "{elapsed} · CPU {cpu} / {limit}",
        "elapsed_nolimit": "{elapsed} · CPU {cpu} / no limit",
        "total_label": "Total time",
        # the time limit can be changed while the router runs (user 2026-11-07: "make the max run time editable
        # during the run"); in seconds on the run's clock (run.timeout_clock)
        "limit_label": "Time limit",
        "limit_unit_wall": "s wall time",
        "limit_unit_cpu": "s CPU time",
        "limit_set": "Set limit",
        "limit_add5": "+5 min",
        "limit_add15": "+15 min",
        "limit_hint": "0 = no limit. A limit below the time spent stops the run now, keeping the best result.",
        # one short line; the full text is its tooltip (user 2026-11-11)
        "cancel_hint": "Cancel keeps the best result so far.",
        "cancel_hint_tip": "Cancel keeps the best result so far. Once the routes are stable the button reads OK: the "
                           "same action, and the best result shown is exactly the board you get; waiting longer may "
                           "still find small gains. The time limit counts router time only and is an emergency stop; "
                           "KiCad's DRC and the preview come on top.",
        "score_waiting": "Waiting for round 1…",
        "cancel": "Cancel",
        # M-B0: iterations like TopoR ("Routes stable", Cancel -> OK, best of N runs)
        "ok": "OK",
        "phase_stable": "Routes stable",
        "title_stable": "Topor — Routing {board} — Routes stable",
        "phase_finishing": "OK pressed: finishing the current stage{stage}… {s} s",
        "round_value": "{round}",
        "gain_label": "Gain",
        "gain_value": "{gain} % / {window} rounds",
        "gain_pending": "–",
        "runs_label": "Runs",
        "runs_value": "{stable} of {runs} stable",
        # the score of the last round and of the best so far (user 2026-11-11: "It would help if I can see the
        # current score, and the best score in the UI"): the columns are the pick order ([optimize] pick), the line
        # below says how the last round compares with the best and on which criterion
        "score_head_run": "Run",
        "score_head_round": "Round",
        "score_head_errors": "Errors",
        "score_head_open": "Open",
        "score_head_vias": "Vias",
        "score_head_length": "Length",
        "score_last": "Last round",
        "score_best": "Best so far",
        "score_length": "{length} m",
        "score_pending": "–",
        "score_hint": "Scores compare in the pick order: fewest errors (open connections and connections with rule "
                      "violations), then fewest connections with a short, then fewest open, fewest vias, the shortest length ([optimize] pick). The best "
                      "so far is the board OK or Cancel keeps.",
        "vs_first": "The first result is the best so far.",
        "vs_new_best": "Last round is the new best: {delta}.",
        "vs_worse": "Last round is worse than the best: {delta}.",
        "vs_same": "Last round scores the same as the best; the earlier result stays.",
        "vs_tie": "Last round scores the same as the best and is the best now (ties go to the lower run).",
        "vs_better": "Last round is better ({delta}), but the variant table kept the best.",
        "delta_errors": "errors {d}",
        "delta_shorts": "connections with a short {d}",
        "delta_open": "open {d}",
        "delta_vias": "vias {d}",
        "delta_length": "length {d} m",
        "delta_shrinkages": "shrinkages {d}",
        "delta_score": "weighted score {d} mm",
        "delta_area": "area {d} mm²",
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
        # top line of the summary when KiCad's DRC finds new violations (red, bold)
        "drc_alert": "⚠ DRC new {n} ({types}) — see findings",
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
        "done_fills_committed": "Zones carry the fill KiCad's own filler made for the new copper (run on a copy), in "
                                "the same undo step as the routes: Edit → Undo reverts both.",
        "done_refilled": "Zones were refilled by topor — a second undo step: the first Edit → Undo reverts the refill, "
                         "the next one the routes.",
        "done_post_commit": "{message} {fix}",
        "done_times": "Router time {router}, CPU time {cpu} (all threads).",
        "done_drc_new": "⚠ {n} new DRC violations ({types}) — run Inspect → Design Rules Checker",
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
#: window icons: ``plugin/icons`` in the repo, ``org.topor.router/icons`` installed (tools/build.sh); both are the
#: parent of the ``topor`` package
ICON_DIR = Path(__file__).resolve().parents[1] / "icons"
ICON_SIZES = (24, 64)


def icon_files(dark: bool, icon_dir: Path = ICON_DIR) -> list:
    """The topor icons that exist for the theme (``topor-{light,dark}-{24,64}.png``), small first. O(1)."""
    theme = "dark" if dark else "light"
    return [p for p in (Path(icon_dir) / f"topor-{theme}-{n}.png" for n in ICON_SIZES) if p.is_file()]


def system_is_dark() -> bool:
    """The desktop uses a dark appearance (``wx.SystemSettings.GetAppearance``; False where wx cannot tell)."""
    try:
        return bool(wx.SystemSettings.GetAppearance().IsDark())
    except Exception:  # noqa: BLE001 - older wx: no appearance API
        return False


def window_icons(icon_dir: Path = ICON_DIR, dark: bool | None = None) -> "wx.IconBundle | None":
    """``wx.IconBundle`` of the topor icons for the system appearance (both sizes); None when no file loads. A
    missing or broken file is skipped silently (``wx.LogNull``): the dialog never depends on it. O(1)."""
    bundle = wx.IconBundle()
    with wx.LogNull():
        for f in icon_files(system_is_dark() if dark is None else dark, icon_dir):
            try:
                ic = wx.Icon(str(f), wx.BITMAP_TYPE_PNG)
            except Exception:  # noqa: BLE001
                continue
            if ic.IsOk():
                bundle.AddIcon(ic)
    return None if bundle.IsEmpty() else bundle


#: text colours by tone (user 2026-11-11: "problems visible at a glance"): (light theme, dark theme). Always next to
#: an icon or a word (never colour alone); each has a contrast >= 4.5 on GTK's default light / dark window colour.
TONE_RGB = {"error": ((192, 28, 40), (255, 123, 114)), "warning": ((154, 103, 0), (227, 179, 65)),
            "ok": ((26, 127, 55), (87, 199, 106))}


def window_is_dark() -> bool:
    """The window background (lists, text fields) is dark: its luminance, which follows the GTK theme even where
    ``GetAppearance`` cannot tell. O(1)."""
    c = wx.SystemSettings.GetColour(wx.SYS_COLOUR_WINDOW)
    return 0.2126 * c.Red() + 0.7152 * c.Green() + 0.0722 * c.Blue() < 128


def tone_colour(tone: str) -> "wx.Colour":
    """Text colour of a tone: ``error`` red, ``warning`` amber, ``ok`` green (light / dark pair of TONE_RGB),
    ``info`` the system's grey text, anything else the normal text colour. O(1)."""
    if tone in TONE_RGB:
        return wx.Colour(*TONE_RGB[tone][1 if window_is_dark() else 0])
    return wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT if tone == "info" else wx.SYS_COLOUR_WINDOWTEXT)


def drc_new(report) -> tuple[int, str]:
    """(new DRC violations, "clearance 9, short 1") of a run report (``facts["routing_new"]`` and
    ``drc_by_type_new``); (0, "") when unknown. O(types)."""
    f = getattr(report, "facts", None) or {}
    try:
        n = int(f.get("routing_new") or 0)
    except (TypeError, ValueError):
        n = 0
    by = f.get("drc_by_type_new") or {}
    types = ", ".join(f"{t} {c}" for t, c in sorted(by.items(), key=lambda kv: (-kv[1], kv[0])))
    return n, types


#: summary lines shown bold at the top of the preview's Summary (the facts to decide on), by their first word
SUMMARY_KEY_PREFIXES = ("Routed ", "DRC new ", "Vias ", "Length ")


def summary_parts(report) -> tuple[str, list, list]:
    """The preview Summary in its hierarchy: (alert line or "", key lines, other lines). The alert is the red line
    for new DRC violations; the key lines are routed, DRC new, vias and length (``SUMMARY_KEY_PREFIXES``, in that
    order); the rest keeps the report's order without the report path (the field below shows it). O(lines)."""
    lines = list(report.summary_lines())
    key = [ln for p in SUMMARY_KEY_PREFIXES for ln in lines if ln.startswith(p)]
    rest = [ln for ln in lines if ln not in key and not ln.startswith("Report: ")]
    n, types = drc_new(report)
    if not n:
        m = next((re.match(r"DRC new (\d+)", ln) for ln in key if ln.startswith("DRC new ")), None)
        n = int(m.group(1)) if m else 0
    tmpl = WORDING["preview"]["drc_alert"]
    alert = "" if not n else tmpl.format(n=n, types=types) if types else tmpl.replace(" ({types})", "").format(n=n)
    return alert, key, rest


#: the name on the start-up splash (user 2026-11-08)
SPLASH_NAME = "Ranz Topor"
#: the short notice GPL-3.0 ("How to Apply", interactive programs) asks for at start-up
SPLASH_NOTICE = ("Copyright (C) 2026 Paul Honig and contributors\n"
                 "Free software under GPL-3.0-only; comes with ABSOLUTELY NO WARRANTY")
#: logo size on the splash (``topor-{light,dark}-128.png``, tools/make_icons.py)
LOGO_SIZE = 128


def logo_file(dark: bool, icon_dir: Path = ICON_DIR) -> Path | None:
    """The splash logo for the theme: the 128 px icon, else the largest window icon that exists. O(1)."""
    big = Path(icon_dir) / f"topor-{'dark' if dark else 'light'}-{LOGO_SIZE}.png"
    if big.is_file():
        return big
    small = icon_files(dark, icon_dir)
    return small[-1] if small else None


class SplashWindow(wx.Frame):
    """Shown the moment the plugin starts (before the KiCad connection, the board read and the engine probe,
    which take a few seconds), so the user sees at once that the router launched: the logo, ``SPLASH_NAME``,
    the version and a status line. Borderless, centred, on top, not in the task bar; system colours only.
    ``WxUi`` closes it when the first real window (progress, pre-flight, a notice or an error) opens."""

    def __init__(self, version: str | None, status: str):
        super().__init__(None, title=SPLASH_NAME, name="splash",
                         style=wx.FRAME_NO_TASKBAR | wx.STAY_ON_TOP | wx.BORDER_SIMPLE)
        icons = window_icons()
        if icons is not None:
            self.SetIcons(icons)
        panel = wx.Panel(self)
        panel.SetBackgroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_WINDOW))
        fg = wx.SystemSettings.GetColour(wx.SYS_COLOUR_WINDOWTEXT)
        col = wx.BoxSizer(wx.VERTICAL)
        logo = logo_file(system_is_dark())
        if logo is not None:
            with wx.LogNull():
                img = wx.Image(str(logo), wx.BITMAP_TYPE_PNG)
            if img.IsOk():
                n = self.FromDIP(LOGO_SIZE)
                if img.GetWidth() != n:
                    img = img.Scale(n, n, wx.IMAGE_QUALITY_HIGH)
                col.Add(wx.StaticBitmap(panel, bitmap=wx.Bitmap(img), name="logo"), 0, wx.ALIGN_CENTER)
                col.AddSpacer(self.FromDIP(12))
        title = wx.StaticText(panel, label=SPLASH_NAME, name="title")
        title.SetFont(self.GetFont().Bold().Scaled(2.2))
        title.SetForegroundColour(fg)
        col.Add(title, 0, wx.ALIGN_CENTER)
        sub = wx.StaticText(panel, label="Topological router" + (f" {version}" if version else ""), name="subtitle")
        sub.SetForegroundColour(fg)
        col.Add(sub, 0, wx.ALIGN_CENTER | wx.TOP, self.FromDIP(4))
        col.AddSpacer(self.FromDIP(16))
        self.status = wx.StaticText(panel, label=status, name="status", style=wx.ST_NO_AUTORESIZE | wx.ALIGN_CENTER_HORIZONTAL)
        self.status.SetForegroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
        col.Add(self.status, 0, wx.EXPAND)
        col.AddSpacer(self.FromDIP(16))
        notice = wx.StaticText(panel, label=SPLASH_NOTICE, name="notice", style=wx.ALIGN_CENTER_HORIZONTAL)
        notice.SetFont(self.GetFont().Scaled(0.85))
        notice.SetForegroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
        col.Add(notice, 0, wx.ALIGN_CENTER)
        outer = wx.BoxSizer(wx.VERTICAL)
        outer.Add(col, 1, wx.EXPAND | wx.ALL, self.FromDIP(28))
        panel.SetSizer(outer)
        frame = wx.BoxSizer(wx.VERTICAL)
        frame.Add(panel, 1, wx.EXPAND)
        self.SetSizer(frame)
        self.SetMinClientSize(self.FromDIP(wx.Size(340, -1)))
        self.Fit()
        self.CentreOnScreen()

    def set_status(self, text: str) -> None:
        self.status.SetLabel(text)
        self.Layout()


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


def score_delta(decider: str, delta: float) -> str:
    """The deciding criterion and last minus best on it, in the units the rows show ("errors +2", "length
    -0.030 m"); "" for no decider. O(1)."""
    W = WORDING["progress"]
    if not decider:
        return ""
    if decider == "length":
        return W["delta_length"].format(d=f"{delta / 1000:+.3f}")
    if decider in ("score", "area"):
        return W[f"delta_{decider}"].format(d=f"{delta:+.1f}")
    key = f"delta_{decider}"
    return W[key].format(d=f"{delta:+.0f}") if key in W else f"{decider} {delta:+g}"


def score_rows(p: Progress) -> tuple[dict, str]:
    """Cells of the "Last round" / "Best so far" rows (``None`` = nothing yet: the rows keep "–") and the verdict
    line: how the last round compares with the best and on which criterion, in the pick order (the engine's
    ``Progress.last_vs_best`` / ``last_decider`` / ``last_delta``). O(1)."""
    W = WORDING["progress"]

    def cells(run, rnd, errors, open_, vias, length_mm):
        return {"run": str(run), "round": str(rnd), "errors": str(errors), "open": str(open_), "vias": str(vias),
                "length": W["score_length"].format(length=f"{length_mm / 1000:.3f}")}

    last = (cells(p.last_run, p.last_round, p.last_errors, p.last_open, p.last_vias, p.last_length_mm)
            if getattr(p, "last_round", 0) else None)
    best = (cells(p.best_run, p.best_round, p.best_errors, p.best_open, p.best_vias, p.best_length_mm)
            if getattr(p, "best_round", 0) else None)
    vs = getattr(p, "last_vs_best", "")
    delta = score_delta(getattr(p, "last_decider", ""), getattr(p, "last_delta", 0.0))
    if last is None or not vs:
        verdict = ""
    elif vs == "new best":
        verdict = W["vs_new_best"].format(delta=delta) if delta else W["vs_first"]
    elif vs == "worse":
        verdict = W["vs_worse"].format(delta=delta)
    elif vs == "better":
        verdict = W["vs_better"].format(delta=delta)
    elif vs == "tie":
        verdict = W["vs_tie"]
    else:
        verdict = W["vs_same"]
    return {"last": last, "best": best}, verdict


def verdict_tone(p: Progress) -> str:
    """Colour of the verdict line (user 2026-11-11): ``ok`` (green) when the last round beat the best (a new best
    with a margin, or better but not kept), ``warning`` (amber) when it is worse, "" otherwise (the first result,
    ties, the same score). O(1)."""
    vs = getattr(p, "last_vs_best", "")
    if not getattr(p, "last_round", 0):
        return ""
    if vs == "worse":
        return "warning"
    if vs == "better" or (vs == "new best" and getattr(p, "last_decider", "")):
        return "ok"
    return ""


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
        icons = window_icons()  # the topor icon in the title bar / task switcher (light or dark theme)
        if icons is not None:
            self.SetIcons(icons)
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
                   ok_id: int = wx.ID_OK, middle: "wx.Sizer | None" = None, primary: bool = False) -> wx.Sizer:
        """Bottom line. Each tuple = (name, wording key, mnemonic letter). Creation order = tab order:
        extras, help, cancel, ok. Esc = Cancel (or the single affirmative button), Enter = ok. Layout: extras (and
        Help) on the left, `middle` after them, then a stretch, then the ``wx.StdDialogButtonSizer`` (cancel + ok in
        the platform's order) on the right. `primary`: the ok button is the visually primary one (bold label; user
        2026-11-11, GTK draws no suggested-action colour for a wx button)."""
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
        if primary:
            okb.SetFont(okb.GetFont().Bold())
            okb.SetMinSize(wx.Size(max(self.FromDIP(88), okb.GetBestSize().width), -1))
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
            row.Add(middle, 0, wx.ALIGN_CENTER_VERTICAL | wx.LEFT | wx.RIGHT, self.row)
        # a stretch of its own: a sizer whose children are all hidden (the status without errors) takes no
        # stretch, which left Cancel / Route next to Help (live screenshot 2026-11-11)
        row.AddStretchSpacer(1)
        row.Add(std, 0, wx.EXPAND)
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
        # GTK knows the height of wrapped text and of the title bar only once the window exists: grow to the best
        # size then, else the last lines are cut off (seen live: the first-run notice lost "widths and via sizes
        # ..." at 200 px; the fitted pre-flight lost its buttons). Once at the first idle, once when shown.
        wx.CallAfter(self._grow_to_best)
        self.Bind(wx.EVT_SHOW, self._on_show_grow)

    def _on_show_grow(self, evt) -> None:
        evt.Skip()
        if evt.IsShown():
            wx.CallLater(50, self._grow_to_best)

    def _grow_to_best(self) -> None:
        """Grow the CLIENT area to what the sizer needs. Compared on client sizes: GTK learns the title bar's height
        only once the window is mapped and then takes it out of a size set before, so a dialog opened at its fitted
        size lost its button row (live screenshot 2026-11-11, pre-flight without an opening size)."""
        if not self:
            return
        self.Layout()
        sizer = self.GetSizer()
        need = sizer.CalcMin() if sizer is not None else self.GetBestSize()
        cur = self.GetClientSize()
        if need.width > cur.width or need.height > cur.height:
            self.SetClientSize(wx.Size(max(need.width, cur.width), max(need.height, cur.height)))
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


# --------------------------------------------------------------------------------------------- findings list
class _FindingsMixin:
    """The findings list of the pre-flight and the preview (one widget, user 2026-11-11): severity icon + word in
    column 1 and the row's text in the severity's colour (error red, warning amber, note grey; ``tone_colour``),
    sorted errors first, its height fitting the rows (at most ``LIST_ROWS_MAX`` before it scrolls), a heading that
    reads "No problems found" without errors and warnings, and the Cause / Fix pane of the selected row. The wording
    is the pre-flight's (``WORDING["preflight"]``). Construction O(#findings)."""
    LIST_ROWS_MAX = 8

    def build_findings(self, findings: list, max_rows: int | None = None) -> None:
        W = WORDING["preflight"]
        self.findings = sorted(findings, key=lambda f: (_SEV_ORDER[f.severity], f.code))
        self.n_err = sum(f.severity == "error" for f in self.findings)
        self.n_warn = sum(f.severity == "warning" for f in self.findings)
        self.n_info = sum(f.severity == "info" for f in self.findings)
        counts = W["findings_heading"].format(errors=self.n_err, warnings=self.n_warn, notes=self.n_info)
        ok = not self.n_err and not self.n_warn
        head = (W["no_problems_notes"].format(notes=self.n_info) if self.n_info else W["no_problems"]) if ok else counts
        self.findings_heading = self.text(self, "preflight", "findings_heading", bold=True, text=head)
        if ok:
            self.findings_heading.SetForegroundColour(tone_colour("ok"))

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
            self.list.SetItemTextColour(i, tone_colour(f.severity))
        rows = min(len(self.findings), max_rows or self.LIST_ROWS_MAX)
        self.list.SetMinSize(wx.Size(-1, self.list_height(max(1, rows))))
        self.list.SetToolTip(counts)
        self.list_grows = len(self.findings) > rows  # more rows than shown: the list takes the spare height

        self.cause = wx.TextCtrl(self, name="cause", style=wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_BESTWRAP)
        self.cause.SetToolTip(f"{W['cause_label']} / {W['fix_label']}")
        self.list.Show(bool(self.findings))
        self.cause.Show(bool(self.findings))
        self.list.Bind(wx.EVT_LIST_ITEM_SELECTED, self._on_select)
        self.list.Bind(wx.EVT_SIZE, self._on_list_size)

    def list_height(self, rows: int) -> int:
        """Pixel height of the list showing `rows` rows and its header: the row height of the generic list (the
        larger of the 16 DIP icon and the text, plus its line spacing), the header one text line with padding. O(1)."""
        ch = self.list.GetCharHeight()
        row = max(self.FromDIP(16), ch) + self.FromDIP(6)
        if self.findings:
            r = self.list.GetItemRect(0)
            if r.height > 0:
                row = r.height
        header = ch + self.FromDIP(14)
        return header + rows * row + self.FromDIP(6)

    def cause_text(self, f: Finding) -> str:
        W = WORDING["preflight"]
        items = [str(i) for i in f.items]
        txt = f.message
        if items:
            txt += ": " + ", ".join(items[:_ITEMS_PREFLIGHT])
            if len(items) > _ITEMS_PREFLIGHT:
                txt += ", " + W["items_more"].format(n=len(items) - _ITEMS_PREFLIGHT)
        return f"{W['cause_label']}  {txt}\n{W['fix_label']}    {f.fix}"

    def show_cause(self, idx) -> "Finding | None":
        """Fill the Cause / Fix pane for row `idx` (None / out of range: "No findings" or empty). O(items)."""
        if idx is None or not (0 <= idx < len(self.findings)):
            self.cause.SetValue(WORDING["preflight"]["no_findings"] if not self.findings else "")
            return None
        f = self.findings[idx]
        self.cause.SetValue(self.cause_text(f))
        return f

    def _on_select(self, evt) -> None:
        self._show_finding(evt.GetIndex())

    def _show_finding(self, idx) -> None:
        self.show_cause(idx)

    def _on_list_size(self, evt) -> None:
        evt.Skip()
        w = self.list.GetClientSize().width - self.list.GetColumnWidth(0) - self.list.GetColumnWidth(1)
        if w > self.FromDIP(200):
            self.list.SetColumnWidth(2, w)

    def select_first(self) -> None:
        if self.findings:
            self.list.Select(0)
            self.list.Focus(0)
        self._show_finding(0 if self.findings else None)


# --------------------------------------------------------------------------------------------- (2) pre-flight
class PreflightDialog(_FindingsMixin, _Base):
    _wk = "preflight"
    #: tab order from the note §2 (one Accept checkbox, relabelled per selected code)
    TAB_ORDER = ("findings", "cause", "accept", "scope_whole", "scope_between",
                 "scope_touching", "existing", "respect_locked", "drawing_style", "timeout", "timeout_clock",
                 "save_toml", "help", "cancel", "route")

    def __init__(self, findings: list, summary: RunSummary, cfg: Config):
        super().__init__(WORDING["preflight"]["title"].format(board=summary.board), resizable=True)
        self.cfg = cfg
        W = WORDING["preflight"]
        g = self.group

        self.summary = summary
        self.summary_label = self.text(self, "preflight", "summary_line", text=self._summary_text(summary.connections))
        self.root.Add(self.summary_label, 0, wx.BOTTOM, g)
        # findings list (icon + word + colour per row, height fitting the rows) and the Cause / Fix pane
        self.build_findings(findings)
        n_warn, n_info = self.n_warn, self.n_info
        self.root.Add(self.findings_heading, 0, wx.BOTTOM, self.row)
        self.root.Add(self.list, 1 if self.list_grows else 0, wx.EXPAND | wx.BOTTOM, self.row)
        self.cause.SetMinSize(wx.Size(-1, self.cause.GetCharHeight() * 4 + self.FromDIP(12)))
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
                                      extras=(("help", "help", "h", wx.ID_HELP),), middle=status_row, primary=True),
                      0, wx.EXPAND | wx.TOP, self.row)
        self.route = self.buttons["route"]
        self.route.Enable(self.n_err == 0)  # visible reason: self.status
        self.buttons["help"].Bind(wx.EVT_BUTTON, self._on_help)

        self.list.Bind(wx.EVT_LIST_ITEM_ACTIVATED, self._on_activate)  # Enter in the list = Route
        self.select_first()
        self.A11Y = {"findings": W["findings_heading"].format(errors=self.n_err, warnings=n_warn, notes=n_info),
                     "cause": f"{W['cause_label']} / {W['fix_label']}", "existing": W["existing_label"],
                     "drawing_style": W["drawing_style_label"], "timeout": W["timeout_label"],
                     "timeout_clock": W["timeout_clock_label"]}
        self.finish((640, 420))  # opens at its fitted size: the findings list is as tall as its rows
        (self.list if self.findings else self.route).SetFocus()

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
        grid.AddSpacer(0)
        grid.Add(self.respect_locked)

        # trace shape ([drawing] style, user 2026-11-07: "make it kicad accessible")
        self._style_keys = ("octilinear", "smooth")
        self.drawing_style = wx.Choice(self, name="drawing_style", choices=[
            W["drawing_style_octilinear"], W["drawing_style_smooth"]])
        style = cfg.get("drawing", "style")
        self.drawing_style.SetSelection(self._style_keys.index(style) if style in self._style_keys else 0)
        self.drawing_style.SetToolTip(W["drawing_style_tip"])
        grid.Add(self.text(self, "preflight", "drawing_style_label"), 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.drawing_style)

        # time limit (reached by Tab; spin controls carry a preceding label)
        self.timeout = wx.SpinCtrlDouble(self, name="timeout", min=0, max=86400, inc=10,
                                         initial=min(86400.0, float(cfg.get("run", "timeout_s"))))
        self.timeout.SetDigits(0)
        self.timeout.SetToolTip(W["timeout_label"])
        tr = wx.BoxSizer(wx.HORIZONTAL)
        tr.Add(self.timeout, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        tr.Add(self.text(self, "preflight", "timeout_unit"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.group)
        self._clock_keys = ("wall", "cpu")
        self.timeout_clock = wx.Choice(self, name="timeout_clock", choices=[
            W["timeout_clock_wall"], W["timeout_clock_cpu"]])
        self.timeout_clock.SetSelection(self._clock_keys.index(cfg.get("run", "timeout_clock")))
        self.timeout_clock.SetToolTip(W["timeout_clock_tip"])
        tr.Add(self.timeout_clock, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.text(self, "preflight", "timeout_label"), 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(tr)
        # the hint as grey text under the field (user 2026-11-11)
        hint_t = self.text(self, "preflight", "timeout_hint")
        hint_t.SetForegroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
        grid.AddSpacer(0)
        grid.Add(hint_t, 0)

        self.save_toml = wx.CheckBox(self, label=_mn(W["save_toml"], "s"), name="save_toml")
        grid.AddSpacer(0)
        grid.Add(self.save_toml)
        self.root.Add(grid, 0, wx.EXPAND | wx.BOTTOM, self.row)

    # -- behaviour
    def _show_finding(self, idx) -> None:
        f = self.show_cause(idx)
        code = f.code if f is not None else None
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

    def _on_activate(self, _evt) -> None:
        if self.route.IsEnabled():
            self.EndModal(wx.ID_OK)

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
        style = self._style_keys[self.drawing_style.GetSelection()]
        if style != cfg.get("drawing", "style"):
            ov["drawing_style"] = style
        put("timeout_s", float(self.timeout.GetValue()))
        put("timeout_clock", self._clock_keys[self.timeout_clock.GetSelection()])
        return Decision(True, accepted, ov, self.save_toml.GetValue())


# --------------------------------------------------------------------------------------------- (3) progress
class ProgressDialog(_Base):
    """Progress of the run (note §3). With ``limit_editable`` (the routing run after the pre-flight answer) the user
    can change the time limit while the router runs: +5 min / +15 min, or a new value in the field (seconds on the
    run's clock; 0 = no limit; below the time spent = stop now). A change is kept in ``limit_request`` until the run
    flow takes it (``WxUi.time_limit_request``); the run reports the limit it applies back in ``Progress.limit_s``."""
    _wk = "progress"
    TAB_ORDER = ("limit", "limit_set", "limit_add5", "limit_add15", "cancel")
    LIMIT_MAX_S = 86400.0  # schema range of run.timeout_s

    def __init__(self, board: str, timeout_s: float = 0.0, iteration_cap: int | None = None, stable_window: int = 4,
                 timeout_clock: str = "wall", limit_editable: bool = True):
        super().__init__(WORDING["progress"]["title"].format(board=board))
        self.board = board
        self.timeout_s = float(timeout_s or 0)
        self.timeout_clock = timeout_clock
        self.limit_editable = limit_editable
        self.limit_request: float | None = None  # a change the run flow has not taken yet
        self._spent = 0.0  # router time on the run's clock at the last update
        self._phase = ""
        self.iteration_cap = iteration_cap
        self._round, self._round_from_engine = (0, iteration_cap), False
        self.cancelled = False
        self.cancelled_at: float | None = None
        W = WORDING["progress"]

        # the phase never widens the dialog: a fixed width, a long stage name ends in "…" (full text: tooltip)
        self.phase = wx.StaticText(self, label="", name="phase",
                                   style=wx.ST_NO_AUTORESIZE | wx.ST_ELLIPSIZE_END)
        self.phase.SetFont(self.phase.GetFont().Bold())
        self.phase.SetMinSize(wx.Size(self.FromDIP(self.TEXT_WIDTH), -1))
        self.root.Add(self.phase, 0, wx.BOTTOM, self.group)
        gr = wx.BoxSizer(wx.HORIZONTAL)
        self.gauge = wx.Gauge(self, range=1, name="gauge", style=wx.GA_HORIZONTAL)
        self.gauge.SetMinSize(self.FromDIP(wx.Size(260, 16)))
        self.gauge_value = self._num("gauge_value", 80, "8888 / 8888")
        gr.Add(self.gauge, 1, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        gr.Add(self.gauge_value, 0, wx.ALIGN_CENTER_VERTICAL)
        self.root.Add(gr, 0, wx.EXPAND | wx.BOTTOM, self.group)

        # value widths from the widest text each can show (digits are tabular in the UI font): changing numbers
        # never move or widen anything (user 2026-11-11)
        grid = wx.FlexGridSizer(4, self.row, self.group)
        self.iteration_value = self._num("iteration_value", 110, "8888 of up to 8888")
        self.routed_value = self._num("routed_value", 90, "88888 / 88888")
        self.vias_value = self._num("vias_value", 110, "888888")
        self.elapsed_value = self._num("elapsed_value", 250, "88:88:88 / 88:88:88 · CPU 88:88:88",
                                       "88:88:88 · CPU 88:88:88 / no limit")
        self.total_value = self._num("total_value", 140, "88:88:88")
        self.gain_value = self._num("gain_now", 140, "-888.8 % / 88 rounds")
        self.runs_value = self._num("runs_now", 110, "888 of 888 stable")
        for lab, val in (("iteration_label", self.iteration_value), ("routed_label", self.routed_value),
                         ("vias_label", self.vias_value), ("elapsed_label", self.elapsed_value),
                         ("runs_label", self.runs_value), ("total_label", self.total_value),
                         ("gain_label", self.gain_value)):
            grid.Add(self.text(self, "progress", lab), 0)
            grid.Add(val, 0, wx.ALIGN_RIGHT)
        self.root.Add(grid, 0, wx.BOTTOM, self.group)
        # the score of the last round and of the best so far (what OK / Cancel keeps), over all runs
        self._build_score_rows()
        self.stable = False
        self.stable_window = stable_window
        self._build_limit_row()
        hint = self.text(self, "progress", "cancel_hint")  # one short line; the full text is its tooltip
        hint.SetToolTip(W["cancel_hint_tip"])
        self.root.Add(hint, 0, wx.BOTTOM, self.group)
        self.root.Add(self.button_row(("cancel", "cancel", "c"), ok_id=wx.ID_CANCEL), 0, wx.EXPAND)
        self.cancel_button = self.buttons["cancel"]
        self.cancel_button.Bind(wx.EVT_BUTTON, self._on_cancel)
        self.Bind(wx.EVT_CLOSE, self._on_cancel)
        self.Bind(wx.EVT_CHAR_HOOK, self._on_key)
        self.set_phase("")
        self.A11Y = {"limit": f"{W['limit_label']} ({W[self._unit_key]})"}
        self.finish((460, 300))
        self._sync_limit_controls()
        self.cancel_button.SetFocus()

    SCORE_COLS = (("run", 30, "888"), ("round", 40, "8888"), ("errors", 44, "88888"), ("open", 40, "88888"),
                  ("vias", 40, "888888"), ("length", 70, "888.888 m"))
    #: width of the phase and verdict lines (DIP); longer text ends in "…" / wraps inside it
    TEXT_WIDTH = 430

    def _build_score_rows(self) -> None:
        """Header + "Last round" / "Best so far" rows (run, round, errors, open, vias, length: the pick order) and
        the verdict line below. Fixed-width right-aligned cells: changing digits never shift the layout. Before the
        first round the table's place shows "Waiting for round 1…" (same height, so nothing moves when it comes)."""
        W = WORDING["progress"]
        grid = wx.FlexGridSizer(1 + len(self.SCORE_COLS), self.row // 2, self.group)
        grid.Add((0, 0))
        for col, _w, _s in self.SCORE_COLS:
            grid.Add(self.text(self, "progress", f"score_head_{col}", style=wx.ALIGN_RIGHT), 0, wx.ALIGN_RIGHT)
        self.score_cells: dict[str, dict[str, wx.StaticText]] = {}
        for row in ("last", "best"):
            name = self.text(self, "progress", f"score_{row}")
            name.SetToolTip(W["score_hint"])
            grid.Add(name, 0)
            self.score_cells[row] = {}
            for col, width, sample in self.SCORE_COLS:
                cell = self._num(f"{row}_{col}", width, sample)
                cell.SetLabel(W["score_pending"])
                self.score_cells[row][col] = cell
                grid.Add(cell, 0, wx.ALIGN_RIGHT)
        self.score_grid = grid
        self.score_waiting = self.text(self, "progress", "score_waiting")
        self.score_waiting.SetForegroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
        self.score_box = wx.BoxSizer(wx.VERTICAL)
        self.score_box.Add(grid, 0)
        self.score_box.Add(self.score_waiting, 0, wx.TOP, self.row)
        self.score_box.SetMinSize(grid.CalcMin())  # the slot keeps the table's size either way
        self.root.Add(self.score_box, 0, wx.BOTTOM, self.row)
        self._show_scores(False)
        # the verdict: a fixed box of two lines (a wrapped verdict never resizes the dialog)
        self.score_verdict = wx.StaticText(self, label=" ", name="score_verdict", style=wx.ST_NO_AUTORESIZE)
        self.score_verdict.SetMinSize(wx.Size(self.FromDIP(self.TEXT_WIDTH),
                                              2 * self.score_verdict.GetCharHeight() + self.FromDIP(2)))
        self.root.Add(self.score_verdict, 0, wx.BOTTOM, self.group)
        self._verdict = " "
        self._verdict_tone = ""

    def _show_scores(self, show: bool) -> None:
        self.scores_shown = show
        self.score_box.Show(self.score_grid, show, recursive=True)
        self.score_waiting.Show(not show)
        self.Layout()

    def _set_score(self, p: Progress) -> None:
        """Fill the score rows from the engine's progress (``score_rows``); the verdict in its tone's colour (green:
        the last round beat the best, amber: worse; ``verdict_tone``)."""
        rows, verdict = score_rows(p)
        if (rows["last"] is not None or rows["best"] is not None) != self.scores_shown:
            self._show_scores(not self.scores_shown)
        for row, cells in rows.items():
            if cells is None:
                continue
            for col, val in cells.items():
                self.score_cells[row][col].SetLabel(val)
        verdict = verdict or " "
        tone = verdict_tone(p)
        if verdict != self._verdict or tone != self._verdict_tone:
            self._verdict, self._verdict_tone = verdict, tone
            self.score_verdict.SetForegroundColour(tone_colour(tone))
            self.score_verdict.SetLabel(verdict)
            self.score_verdict.Wrap(self.FromDIP(self.TEXT_WIDTH))
            self.score_verdict.Refresh()

    @property
    def _unit_key(self) -> str:
        return "limit_unit_cpu" if self.timeout_clock == "cpu" else "limit_unit_wall"

    def _build_limit_row(self) -> None:
        """Time limit [field] s wall time [Set limit] [+5 min] [+15 min] / hint. Created in tab order."""
        W = WORDING["progress"]
        self.limit = wx.SpinCtrlDouble(self, name="limit", min=0, max=self.LIMIT_MAX_S, inc=60,
                                       initial=min(self.LIMIT_MAX_S, self.timeout_s), style=wx.SP_ARROW_KEYS
                                       | wx.TE_PROCESS_ENTER)
        self.limit.SetDigits(0)
        self.limit.Bind(wx.EVT_TEXT_ENTER, lambda _e: self.apply_field())
        self.limit_set = wx.Button(self, label=_mn(W["limit_set"], "l"), name="limit_set")
        self.limit_add5 = wx.Button(self, label=_mn(W["limit_add5"], "5"), name="limit_add5")
        self.limit_add15 = wx.Button(self, label=_mn(W["limit_add15"], "1"), name="limit_add15")
        self.limit_set.Bind(wx.EVT_BUTTON, lambda _e: self.apply_field())
        self.limit_add5.Bind(wx.EVT_BUTTON, lambda _e: self.add_minutes(5))
        self.limit_add15.Bind(wx.EVT_BUTTON, lambda _e: self.add_minutes(15))
        row = wx.BoxSizer(wx.HORIZONTAL)
        row.Add(self.text(self, "progress", "limit_label"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.group)
        row.Add(self.limit, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        row.Add(self.text(self, "progress", self._unit_key), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.group)
        for b in (self.limit_set, self.limit_add5, self.limit_add15):
            row.Add(b, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        self.root.Add(row, 0, wx.BOTTOM, self.row)
        self.limit_hint = self.text(self, "progress", "limit_hint", wrap_dip=420)
        self.limit_hint.SetForegroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
        self.root.Add(self.limit_hint, 0, wx.BOTTOM, self.group)

    # -- the time limit during the run
    def limit_open(self) -> bool:
        """The limit can still be changed: routing run, not cancelled, the limit not reached, not in the DRC /
        preview after the routing (the limit counts router time only)."""
        reached = 0 < self.timeout_s <= self._spent
        return (self.limit_editable and not self.cancelled and not reached
                and self._phase not in ("drc", "preview"))

    def _sync_limit_controls(self) -> None:
        open_ = self.limit_open()
        for w in (self.limit, self.limit_set):
            w.Enable(open_)
        for b in (self.limit_add5, self.limit_add15):  # nothing to add to "no limit"
            b.Enable(open_ and self.timeout_s > 0)

    def request_limit(self, seconds: float) -> float:
        """Ask for a new limit (seconds on the run's clock; <= 0 = no limit). A limit below the time spent becomes
        the time spent (stop at the next check, keeping the best result). Returns the value asked for. O(1)."""
        s = float(seconds)
        s = 0.0 if not math.isfinite(s) or s <= 0 else min(self.LIMIT_MAX_S, max(s, self._spent))
        self.timeout_s = s
        self.limit_request = s
        self.limit.SetValue(s)
        self._show_times()
        self._sync_limit_controls()
        return s

    def add_minutes(self, minutes: int) -> None:
        if self.limit_open() and self.timeout_s > 0:
            self.request_limit(self.timeout_s + 60.0 * minutes)

    def apply_field(self) -> None:
        if self.limit_open():
            self.request_limit(self.limit.GetValue())

    def _num(self, name: str, width_dip: int, *samples: str) -> wx.StaticText:
        """Right-aligned label of fixed width: at least `width_dip`, and as wide as the widest of `samples` (the
        longest text it can show, digits as 8) in the label's font. Changing digits never shift or widen the
        layout. O(len(samples))."""
        st = wx.StaticText(self, label="", style=wx.ALIGN_RIGHT | wx.ST_NO_AUTORESIZE, name=name)
        w = max([self.FromDIP(width_dip)] + [st.GetTextExtent(x).width + self.FromDIP(4) for x in samples])
        st.SetMinSize(wx.Size(w, -1))
        return st

    def set_phase(self, phase: str, stage: str = "") -> None:
        if self.cancelled:
            # The engine finishes the stage it is in after Cancel / OK: say so, with the seconds since the click; the
            # DRC and the preview of the result that follow keep their own labels.
            if phase in ("setup", "route", "done"):
                self._set_stopping_label(stage)
                return
            key = PHASE_KEYS.get(phase)
            self._phase_label(WORDING["progress"][key] if key else phase)
            return
        if self.stable and phase in ("setup", "route", "done"):  # M-B0: the user's cue that OK is fine now
            self._phase_label(WORDING["progress"]["phase_stable"])
            return
        if phase in ("setup", "route", "done") and stage:  # the topological core names its stage (K1)
            self._phase_label(WORDING["progress"]["phase_route_stage"].format(stage=stage))
            return
        key = PHASE_KEYS.get(phase)
        self._phase_label(WORDING["progress"][key] if key else phase)

    def _phase_label(self, text: str) -> None:
        if text != self.phase.GetLabel():
            self.phase.SetLabel(text)
            self.phase.SetToolTip(text)

    def _on_cancel(self, evt) -> None:
        if isinstance(evt, wx.CloseEvent) and evt.CanVeto():
            evt.Veto()
        self.cancel()

    def _on_key(self, evt) -> None:
        key = evt.GetKeyCode()
        enter = key in (wx.WXK_RETURN, wx.WXK_NUMPAD_ENTER)
        focus = wx.Window.FindFocus()
        if enter and focus is not None and (focus is self.limit or focus.GetParent() is self.limit):
            self.apply_field()  # Enter in the limit field sets the limit; it never cancels the run
        elif enter and focus in (self.limit_set, self.limit_add5, self.limit_add15):
            evt.Skip()  # the focused limit button acts
        elif key == wx.WXK_ESCAPE or enter:
            self.cancel()
        else:
            evt.Skip()

    def _set_stopping_label(self, stage: str = "") -> None:
        W = WORDING["progress"]
        s = int(time.monotonic() - (self.cancelled_at or time.monotonic()))
        self._phase_label(W["phase_finishing" if self.stable else "phase_cancelling"].format(stage=f" ({stage})" if stage else "", s=s))

    def cancel(self) -> None:
        if self.cancelled:
            return
        self.cancelled = True
        self.cancelled_at = time.monotonic()
        self._set_stopping_label()
        self.cancel_button.Disable()
        self._sync_limit_controls()
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
        self._set_score(p)
        if p.iteration == 0 and p.routed == 0:
            self.routed_value.SetLabel(W["routed_pending"].format(total=p.total))
        else:
            self.routed_value.SetLabel(W["routed_value"].format(routed=p.routed, total=p.total))
        self.vias_value.SetLabel(str(p.vias))
        self._phase = p.phase
        self._spent = float(getattr(p, "cpu_s", 0.0) if self.timeout_clock == "cpu" else p.elapsed_s)
        limit = getattr(p, "limit_s", None)
        if limit is not None and self.limit_request is None and float(limit) != self.timeout_s:
            # the limit the run applies (after a change the run took); the field follows unless being edited
            self.timeout_s = float(limit)
            if wx.Window.FindFocus() is not self.limit:
                self.limit.SetValue(min(self.LIMIT_MAX_S, self.timeout_s))
        self._last = p
        self._show_times()
        self._sync_limit_controls()
        self.total_value.SetLabel(_fmt_time(max(getattr(p, "wall_s", 0.0), p.elapsed_s)))

    def _show_times(self) -> None:
        p = getattr(self, "_last", None)
        if p is None:
            return
        W = WORDING["progress"]
        el = _fmt_time(p.elapsed_s)
        cpu = _fmt_time(getattr(p, "cpu_s", 0.0))
        if self.timeout_s <= 0:
            txt = W["elapsed_nolimit"].format(elapsed=el, cpu=cpu)
        else:
            key = "elapsed_cpu_value" if self.timeout_clock == "cpu" else "elapsed_value"
            txt = W[key].format(elapsed=el, cpu=cpu, limit=_fmt_time(self.timeout_s))
        self.elapsed_value.SetLabel(txt)


# --------------------------------------------------------------------------------------------- (4) preview
class PreviewDialog(_FindingsMixin, _Base):
    """Preview (note §4; user 2026-11-11): the current board and the result as two images, each under its own
    heading and scaled to its own pane (``preview.panel_paths``; an older report folder with only the composite
    ``preview.png`` shows that in the first pane), the Summary in its hierarchy (``summary_parts``: the red line for
    new DRC violations, the key facts bold, the rest smaller), the findings list of the run (the pre-flight's
    widget) and the report folder once (the field). The two panes scroll together."""
    _wk = "preview"
    TAB_ORDER = ("zoom", "image_before", "image_after", "summary", "findings", "cause", "report_dir",
                 "open_report_folder", "reject", "accept")
    FOCUS_ALIAS = {"frame_before": "image_before", "frame_after": "image_after"}
    #: findings rows shown before the list scrolls (the images keep the height)
    FINDINGS_ROWS = 5

    def __init__(self, png: Path, report, board: str = ""):
        super().__init__(WORDING["preview"]["title"].format(board=board), resizable=True)
        W = WORDING["preview"]
        self.report = report
        self.report_dir = Path(report.dir) if getattr(report, "dir", None) else None
        before, after = self._load_pair(png)
        self.images = {"before": before, "after": after}
        self.image = before  # the first pane's image (the composite for an older report folder)
        g, r = self.group, self.row

        # row: Zoom + legend
        top = wx.BoxSizer(wx.HORIZONTAL)
        top.Add(self.text(self, "preview", "zoom_label"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, r)
        self.zoom = wx.Choice(self, name="zoom", choices=[W[k] for k in _ZOOM_KEYS])
        self.zoom.SetSelection(0)
        self.zoom.SetToolTip(W["zoom_label"])
        top.Add(self.zoom, 0, wx.ALIGN_CENTER_VERTICAL)
        top.AddStretchSpacer()
        top.Add(self._legend_row(png), 0, wx.ALIGN_CENTER_VERTICAL)
        self.root.Add(top, 0, wx.EXPAND | wx.BOTTOM, r)

        # the two panes, each under its heading, and the Summary
        main = wx.BoxSizer(wx.HORIZONTAL)
        self.scrollers: dict[str, wx.ScrolledWindow] = {}
        self.bitmaps: dict[str, "wx.StaticBitmap | None"] = {}
        two = after is not None
        for key, head in (("before", "before_heading"), ("after", "after_heading")):
            col = wx.BoxSizer(wx.VERTICAL)
            col.Add(self.text(self, "preview", head, bold=True), 0, wx.BOTTOM, r)
            # 1 px SYS_COLOUR_GRAYTEXT outline so the image does not blur into the surface
            frame = wx.Panel(self, name=f"frame_{key}")
            frame.SetBackgroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_GRAYTEXT))
            sc = wx.ScrolledWindow(frame, name=f"image_{key}", style=wx.HSCROLL | wx.VSCROLL | wx.WANTS_CHARS)
            sc.SetCanFocus(True)
            sc.SetBackgroundColour(wx.SystemSettings.GetColour(wx.SYS_COLOUR_WINDOW))
            sc.SetScrollRate(self.FromDIP(10), self.FromDIP(10))
            fs = wx.BoxSizer(wx.VERTICAL)
            fs.Add(sc, 1, wx.EXPAND | wx.ALL, 1)
            frame.SetSizer(fs)
            ssz = wx.BoxSizer(wx.VERTICAL)
            if self.images[key] is None:
                self.bitmaps[key] = None
                if key == "before":
                    ssz.Add(self.text(sc, "preview", "no_image", wrap_dip=300), 1, wx.ALL, g)
            else:  # centred in its pane while smaller than it (Fit)
                self.bitmaps[key] = wx.StaticBitmap(sc, name=f"bitmap_{key}")
                ssz.AddStretchSpacer()
                ssz.Add(self.bitmaps[key], 0, wx.ALIGN_CENTER_HORIZONTAL)
                ssz.AddStretchSpacer()
            sc.SetSizer(ssz)
            col.Add(frame, 1, wx.EXPAND)
            self.scrollers[key] = sc
            main.Add(col, 1, wx.EXPAND | wx.RIGHT, g)
            if key == "after" and not two:  # one image (or none): the first pane takes the room
                main.Hide(col, recursive=True)
        self.scroller, self.bitmap = self.scrollers["before"], self.bitmaps["before"]  # first pane (tests, older code)

        right = wx.BoxSizer(wx.VERTICAL)
        right.Add(self.text(self, "preview", "summary_heading", bold=True), 0, wx.BOTTOM, r)
        alert, key_lines, rest = summary_parts(report)
        self.drc_alert = None
        if alert:  # new DRC violations: the first thing to read (red, bold)
            self.drc_alert = self.text(self, "preview", "drc_alert", text=alert, bold=True, wrap_dip=300)
            self.drc_alert.SetForegroundColour(tone_colour("error"))
            right.Add(self.drc_alert, 0, wx.BOTTOM, r)
        self.key_facts = []
        for ln in key_lines:  # routed, DRC new, vias, length: bold
            st = self.text(self, "preview", "key_fact", text=ln, bold=True, wrap_dip=300)
            self.key_facts.append(st)
            right.Add(st, 0, wx.BOTTOM, r // 2)
        self.summary = wx.TextCtrl(self, name="summary", value="\n".join(rest),
                                   style=wx.TE_MULTILINE | wx.TE_READONLY | wx.TE_BESTWRAP | wx.BORDER_NONE)
        self.summary.SetFont(self.summary.GetFont().Scaled(0.9))
        self.summary.SetBackgroundColour(self.GetBackgroundColour())
        self.summary.SetMinSize(self.FromDIP(wx.Size(300, 160)))
        self.summary.SetToolTip(W["summary_heading"])
        right.Add(self.summary, 1, wx.EXPAND | wx.TOP, r)
        main.Add(right, 0, wx.EXPAND)
        self.root.Add(main, 1, wx.EXPAND | wx.BOTTOM, g)

        # the run's findings: the pre-flight's list (left) and the Cause / Fix of the selected row (right)
        facts = getattr(report, "facts", None) or {}
        self.build_findings(list(facts.get("findings") or []), max_rows=self.FINDINGS_ROWS)
        self.root.Add(self.findings_heading, 0, wx.BOTTOM, r)
        fr = wx.BoxSizer(wx.HORIZONTAL)
        fr.Add(self.list, 3, wx.EXPAND | wx.RIGHT, r)
        fr.Add(self.cause, 2, wx.EXPAND)
        self.root.Add(fr, 0, wx.EXPAND | wx.BOTTOM, g)

        # report folder (shown once: here, not in the Summary)
        rd = wx.BoxSizer(wx.HORIZONTAL)
        rd.Add(self.text(self, "preview", "report_dir_label"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, r)
        # one line (selectable, copyable; a long path scrolls): the images keep the height
        self.dir_field = wx.TextCtrl(self, value=str(self.report_dir or ""), name="report_dir", style=wx.TE_READONLY)
        self.dir_field.SetFont(wx.Font(wx.FontInfo().Family(wx.FONTFAMILY_TELETYPE)))
        self.dir_field.SetToolTip(str(self.report_dir or ""))
        rd.Add(self.dir_field, 1, wx.ALIGN_CENTER_VERTICAL)
        self.root.Add(rd, 0, wx.EXPAND | wx.BOTTOM, g)
        self.root.Add(self.button_row(("accept", "accept", "a"), ("reject", "reject", "r"),
                                      extras=(("open_report_folder", "open_report_folder", "o"),), primary=True),
                      0, wx.EXPAND)
        self.open_btn = self.buttons["open_report_folder"]
        self.open_btn.Bind(wx.EVT_BUTTON, lambda _e: self.report_dir and open_path(self.report_dir))

        self.zoom.Bind(wx.EVT_CHOICE, lambda _e: self._render())
        for key, sc in self.scrollers.items():
            sc.Bind(wx.EVT_SIZE, self._on_size)
            other = self.scrollers["after" if key == "before" else "before"]
            sc.Bind(wx.EVT_SCROLLWIN, lambda e, a=sc, b=other: self._on_scroll(e, a, b))
            for w in (sc, self.bitmaps[key]):
                if w is not None:
                    w.Bind(wx.EVT_MOUSEWHEEL, self._on_wheel)
        self.select_first()

        disp = wx.Display(0).GetClientArea() if wx.Display.GetCount() else wx.Rect(0, 0, 1280, 800)
        self.A11Y = {"zoom": W["zoom_label"], "image_before": W["before_heading"], "image_after": W["after_heading"],
                     "summary": W["summary_heading"], "report_dir": W["report_dir_label"],
                     "findings": self.findings_heading.GetLabel(),
                     "cause": f"{WORDING['preflight']['cause_label']} / {WORDING['preflight']['fix_label']}"}
        self.finish((800, 560))
        mn = self.GetMinSize()
        self.SetSize(wx.Size(max(int(disp.width * 0.85), mn.width), max(int(disp.height * 0.85), mn.height)))
        self.CentreOnScreen()
        self._render()
        self.zoom.SetFocus()
        self.Bind(wx.EVT_CHAR_HOOK, self._on_char_hook)

    def _legend_row(self, png) -> wx.Sizer:
        """Legend: one swatch per entry of ``preview-legend.json`` (layer colours, the new-copper highlight, the
        open connections), else the fixed line with the highlight swatch. System colours apart from the swatches
        (note §0). O(entries)."""
        W = WORDING["preview"]
        row = wx.BoxSizer(wx.HORIZONTAL)
        entries = []
        try:
            from .preview import legend_path  # noqa: PLC0415
            lp = legend_path(png) if png is not None else None
            if lp is not None and lp.is_file():
                entries = [(str(e["label"]), e.get("rgb")) for e in json.loads(lp.read_text())]
        except Exception:  # noqa: BLE001 - an unreadable legend file: the fixed line
            entries = []
        if not entries:
            row.Add(self.text(self, "preview", "legend_label"), 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
            entries = [("", PREVIEW_HIGHLIGHT_RGB)]
        else:
            row.Add(self.text(self, "preview", "legend_head", text="Legend:"), 0,
                    wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row)
        self.swatch = None
        for label, rgb in entries:
            if rgb:
                sw = wx.Panel(self, name="legend_swatch", size=self.FromDIP(wx.Size(14, 14)))
                sw.SetBackgroundColour(wx.Colour(*rgb))
                sw.SetMinSize(self.FromDIP(wx.Size(14, 14)))
                sw.SetToolTip(label or W["legend_label"])
                self.swatch = self.swatch or sw
                row.Add(sw, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.row // 2)
            if label:
                row.Add(self.text(self, "preview", "legend_entry", text=label), 0,
                        wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, self.group)
        return row

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
            with wx.LogNull():
                img = wx.Image(str(png))
            return img if img.IsOk() else None
        except Exception:  # noqa: BLE001 - unreadable image -> "no_image" state
            return None

    @classmethod
    def _load_pair(cls, png) -> tuple:
        """(current board, result) images: the two separate PNGs next to `png` (``preview.panel_paths``), else
        (the composite `png`, None), else (None, None). O(pixels)."""
        if png is not None:
            from .preview import panel_paths  # noqa: PLC0415
            paths = panel_paths(png)
            b, a = cls._load(paths["before"]), cls._load(paths["after"])
            if b is not None and a is not None:
                return b, a
        return cls._load(png), None

    def zoom_factor(self):
        """None = Fit, else 1.0 / 2.0."""
        return (None, 1.0, 2.0)[self.zoom.GetSelection()]

    def _render(self) -> None:
        """Scale each image to its pane (Fit: the largest size that shows it whole) or to the zoom (100 % = the
        composite's pixel per DIP; the separate images are PANEL_HIRES x larger). O(pixels)."""
        from .preview import PANEL_HIRES  # noqa: PLC0415
        f = self.zoom_factor()
        hires = PANEL_HIRES if self.images["after"] is not None else 1
        for key, img in self.images.items():
            bmp, sc = self.bitmaps.get(key), self.scrollers[key]
            if img is None or bmp is None:
                continue
            iw, ih = img.GetSize()
            if f is None:
                cw, ch = sc.GetClientSize()
                s = min(max(cw, 1) / iw, max(ch, 1) / ih)
            else:
                s = f * self.FromDIP(1) / hires  # 100 % = one composite pixel per DIP
            w, h = max(1, int(iw * s)), max(1, int(ih * s))
            out = img if (w, h) == (iw, ih) else img.Scale(w, h, wx.IMAGE_QUALITY_HIGH)
            bmp.SetBitmap(wx.Bitmap(out))
            sc.SetVirtualSize(w, h)
            sc.Layout()

    def _on_size(self, evt) -> None:
        evt.Skip()
        if self.image is not None and self.zoom_factor() is None:
            wx.CallAfter(self._safe_render)

    def _safe_render(self) -> None:
        if self:  # not destroyed meanwhile
            self._render()

    def _on_scroll(self, evt, src, dst) -> None:
        """The other pane follows (same place on both boards). O(1)."""
        evt.Skip()
        wx.CallAfter(lambda: self and dst.Scroll(*src.GetViewStart()))

    def _on_wheel(self, evt) -> None:
        if evt.ControlDown():
            step = 1 if evt.GetWheelRotation() > 0 else -1
            sel = min(max(self.zoom.GetSelection() + step, 0), len(_ZOOM_KEYS) - 1)
            self.zoom.SetSelection(sel)
            self._render()
        else:
            evt.Skip()
            src = evt.GetEventObject()
            src = src if isinstance(src, wx.ScrolledWindow) else src.GetParent()
            for sc in self.scrollers.values():
                if sc is not src:
                    wx.CallAfter(lambda a=src, b=sc: self and b.Scroll(*a.GetViewStart()))


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
        # new DRC violations on the board after Accept: a warning icon and one line naming them (user 2026-11-11)
        n_drc, types = drc_new(report) if outcome == "accepted" else (0, "")
        warn = n_drc > 0
        icon = wx.StaticBitmap(self, bitmap=wx.ArtProvider.GetBitmap(
            wx.ART_WARNING if warn else wx.ART_INFORMATION, wx.ART_MESSAGE_BOX, self.FromDIP(wx.Size(32, 32))),
            name="icon")
        icon.SetToolTip("Warning" if warn else "Information")
        body = wx.BoxSizer(wx.VERTICAL)
        if warn:
            tmpl = W["done_drc_new"]
            line = tmpl.format(n=n_drc, types=types) if types else tmpl.replace(" ({types})", "").format(n=n_drc)
            self.drc_line = self.text(self, "report", "done_drc_new", text=line, wrap_dip=420, bold=True)
            self.drc_line.SetForegroundColour(tone_colour("warning"))
            body.Add(self.drc_line, 0, wx.BOTTOM, self.group)
        body.Add(self.text(self, "report", name, text=msg, wrap_dip=420), 0, wx.BOTTOM, self.group)
        facts0 = getattr(report, "facts", None) or {}
        if facts0.get("router_cpu_s") is not None and facts0.get("elapsed_s") is not None:
            body.Add(self.text(self, "report", "done_times", wrap_dip=420, text=W["done_times"].format(
                router=_fmt_time(facts0["elapsed_s"]), cpu=_fmt_time(facts0["router_cpu_s"]))), 0, wx.BOTTOM, self.group)
        if outcome == "accepted":
            facts = getattr(report, "facts", None) or {}
            if "zones_refilled" in facts:  # ruling C28
                rid = ("done_refilled" if facts["zones_refilled"] else
                       "done_fills_committed" if facts.get("zone_fills_committed") else "done_refill_hint")
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
        self._timeout_clock = str(Config.defaults().get("run", "timeout_clock"))
        self._iter_cap = int(Config.defaults().get("optimize", "rounds"))
        self._stable_window = int(Config.defaults().get("optimize", "stable_window"))
        self._last_progress: Progress | None = None
        self._limit_editable = False  # True once the pre-flight was answered with Route (the routing run)
        self._splash: SplashWindow | None = None

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

    def _close_splash(self) -> None:
        if self._splash is not None:
            self._splash.Destroy()
            self._splash = None

    def _close_progress(self) -> None:
        self._close_splash()
        if self._progress is not None:
            self._progress.Destroy()
            self._progress = None
        self._cancelled = False

    # -- UiPort
    def splash(self, status: str, version: str | None = None) -> None:
        """The start-up splash (``SplashWindow``): opened by the first call, later calls only change its status
        line. Painted at once (``Update`` + one event pump), so it shows while the start-up work blocks. O(1)."""
        if self._splash is None:
            self._splash = SplashWindow(version, status)
            self._present(self._splash)
        else:
            self._splash.set_status(status)
        self._splash.Update()
        self._pump()

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
            self._timeout_clock = str(d.overrides.get("timeout_clock", cfg.get("run", "timeout_clock")))
            self._limit_editable = True
        self._iter_cap = int(cfg.get("optimize", "rounds"))  # the topological core's whole-route rounds
        self._stable_window = int(cfg.get("optimize", "stable_window"))
        return d

    def progress(self, p: Progress) -> bool:
        self._last_progress = p
        if self._progress is None:
            self._close_splash()
            self._progress = ProgressDialog(self.ctx.board, self._timeout_s, self._iter_cap, self._stable_window,
                                           self._timeout_clock, limit_editable=self._limit_editable)
            self._present(self._progress)
        dlg = self._progress
        dlg.update(p)
        self._pump()
        if dlg.cancelled:
            self._cancelled = True
        return not self._cancelled

    def time_limit_request(self) -> float | None:
        """The time limit the user set in the progress dialog since the last call (seconds on the run's clock,
        0 = no limit), or None. The run flow polls it after every ``progress`` call. O(1)."""
        dlg = self._progress
        if dlg is None or dlg.limit_request is None:
            return None
        req, dlg.limit_request = dlg.limit_request, None
        return req

    def preview(self, png: Path, report) -> bool:
        return self._run_modal(PreviewDialog(png, report, self.ctx.board)) == wx.ID_OK

    def done(self, outcome: str, report, report_dir: Path) -> None:
        self._run_modal(DoneDialog(outcome, report, report_dir))

    def error(self, finding: Finding) -> None:
        self._run_modal(ErrorDialog(finding, self.ctx))
