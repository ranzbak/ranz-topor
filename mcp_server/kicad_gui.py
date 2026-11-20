"""Real KiCad for the MCP tools: one bounded Xephyr session (harness.gui_e2e.E2E: isolated KiCad home, never the
user's KiCad, never bare pcbnew) plus the test-only helper harness/gui_helper/topor_gui_helper.py inside it, which
drives the DRC and Cleanup dialogs by widget label (no pixel coordinates).

Rules (lead 2026-11-19, after an agent "lost its way" improvising in Xephyr): one session at a time with an
explicit handle; a fixed sequence of steps, each with its own timeout and a check of its effect (window present,
label changed, file changed); a failed step raises GuiError (step name + screenshot) and the session is stopped;
nothing is retried by improvising. Everything we start (Xephyr, the pcbnew process group) is stopped by its own
pid: on error, on `stop()`, after `idle_s` without use, and at interpreter exit.

Complexity: a session start is one KiCad start (~20-40 s); every helper call is one UI round trip.
"""
from __future__ import annotations

import atexit
import json
import os
import re
import secrets
import shutil
import socket
import subprocess
import threading
import time
from pathlib import Path

from harness.gui_e2e import E2E, copy_board

ROOT = Path(__file__).resolve().parents[1]
HELPER_SRC = ROOT / "harness" / "gui_helper" / "topor_gui_helper.py"
DISPLAYS = range(20, 30)
IDLE_S = 600.0
DRC_TITLE = "Design Rules Checker"
# the DRC dialog's severity filter boxes (its "Show:" row) per kicad-cli JSON severity; the list shows only the
# ticked severities, so drc_spots unticks Warnings by default (fewer rows to walk)
DRC_SEVERITY_BOXES = {"error": "Errors", "warning": "Warnings"}
CLEANUP_TITLE = "Cleanup Tracks and Vias"
#: the Cleanup dialog's action boxes exactly as KiCad 10 labels them (dialog_cleanup_tracks_and_vias_base.cpp);
#: short names are accepted as aliases (harness.cleanup_check.OPTIONS / gui_e2e.CLEANUP_BOXES)
CLEANUP_LABELS = {
    "refill": "Refill zones before and after cleanup",
    "short": "Delete tracks connecting different nets",
    "vias": "Delete redundant vias",
    "dangling_vias": "Delete vias connected on only one layer",
    "merge": "Merge co-linear tracks",
    "unconnected": "Delete tracks unconnected at one end",
    "in_pads": "Delete tracks fully inside pads",
}
CLEANUP_FILTERS = ("Filter items by net:", "Filter items by net class:", "Filter items by layer:",
                   "Selected items only")
#: the DRC dialog: fixed place on the right of the 1920x1080 display, so the canvas left of it shows the marker
DRC_RECT = (1134, 40, 780, 1000)


class GuiError(RuntimeError):
    """A GUI step failed; the message names the step and the screenshot."""


#: what a GUI step can raise: our GuiError, a bounded X tool / IPC client call that timed out
#: (harness.gui_e2e X_TIMEOUT_S, ipc 600 s), an IPC client failure (RuntimeError), a missing tool (OSError)
STEP_ERRORS = (RuntimeError, subprocess.TimeoutExpired, OSError)


def describe(ex: BaseException) -> str:
    if isinstance(ex, subprocess.TimeoutExpired):
        cmd = ex.cmd if isinstance(ex.cmd, str) else " ".join(map(str, ex.cmd[:3]))
        return f"{cmd[:120]!r} did not finish within {ex.timeout:.0f} s"
    return str(ex) if isinstance(ex, GuiError) else f"{type(ex).__name__}: {ex}"


def free_display() -> str:
    for n in DISPLAYS:
        if not Path(f"/tmp/.X11-unix/X{n}").exists() and not Path(f"/tmp/.X{n}-lock").exists():
            return f":{n}"
    raise GuiError(f"no free X display in :{DISPLAYS.start}..:{DISPLAYS.stop - 1}")


class _E2E(E2E):
    """E2E whose KiCad environment carries the helper's socket + token (only this session's pcbnew sees them)."""

    def helper_paths(self) -> tuple:
        t = self.tmpdir()
        return t / "topor-gui.sock", self.home / "gui-helper.token"

    def env(self) -> dict:
        e = super().env()
        sock, tok = self.helper_paths()
        e.update(TOPOR_GUI_HELPER_SOCK=str(sock), TOPOR_GUI_HELPER_TOKEN_FILE=str(tok))
        return e


class GuiSession:
    """One Xephyr + pcbnew on a copy of a board. Start with :meth:`start`; stop with :meth:`stop` (idempotent)."""

    starts_total = 0  # pcbnew processes started by this server process (tests: one per drc_spots call at most)

    def __init__(self, board: Path, out: Path, *, display: str | None = None, idle_s: float = IDLE_S):
        self.board = Path(board)
        self.out = Path(out)
        self.display = display or free_display()
        self.idle_s = idle_s
        self.e = _E2E(self.out, self.display, None)
        self.main = None
        self.started = time.monotonic()
        self.last_use = time.monotonic()
        self.steps: list = []
        self._lock = threading.RLock()
        self.board_state = "as opened"  # changes when a tool edits the open board (cleanup)
        self.drc_state_of = None  # board state the last DRC run belongs to
        self.drc_rows: list = []
        self.drc_severities: tuple | None = None  # severity filter drc_rows was read under (None: not set by us)
        self.kicad_starts = 0
        self.zoom_value: float | None = None  # the preset asked for last (set_zoom)
        self._scale: tuple | None = None  # (mm per px X, Y) of the board view at that zoom (focus())

    # ---------------------------------------------------------------- lifecycle
    def start(self, timeout_s: float = 240.0) -> "GuiSession":
        e = self.e
        e.make_home()
        cfg = e.home / ".config/kicad/10.0"
        pj = cfg / "pcbnew.json"
        d = json.loads(pj.read_text()) if pj.is_file() else {}
        # the GUI DRC as kicad-cli runs it (--all-track-errors): KiCad 10 keeps the DRC dialog's gear-menu toggles in
        # pcbnew.json "DRC" (strings of _pcbnew.kiface 10.0.8: DRC.report_all_track_errors, DRC.crossprobe,
        # DRC.scroll_on_crossprobe; the KiCad 9 "drc_dialog" keys are gone). Zone refill = the dialog's checkbox,
        # checked in drc_run. Selecting a row must cross-probe and centre on the marker.
        d.pop("drc_dialog", None)
        d.setdefault("DRC", {}).update(report_all_track_errors=True, crossprobe=True, scroll_on_crossprobe=True)
        pj.write_text(json.dumps(d, indent=1))
        plugins = e.home / ".local/share/kicad/10.0/scripting/plugins"
        plugins.mkdir(parents=True, exist_ok=True)
        shutil.copy(HELPER_SRC, plugins / HELPER_SRC.name)
        sock, tok = e.helper_paths()
        self.token = secrets.token_hex(16)
        tok.write_text(self.token)
        os.chmod(tok, 0o600)
        if not e.start_x():
            raise GuiError(f"start: display {self.display} was already running (not ours); refusing to use it")
        try:
            e.start_kicad(self.board)
            self.kicad_starts += 1
            GuiSession.starts_total += 1
            t0 = time.monotonic()
            w = e.wait(f"{self.board.stem} — PCB Editor", min(180.0, timeout_s))
            if not w:
                self.fail("start: the PCB editor window did not open")
            while True:  # "not ready to reply" while the board loads
                try:
                    e.ipc("out = {'n': len(list(board.get_vias()))}")
                    break
                except RuntimeError as ex:
                    if time.monotonic() - t0 > timeout_s:
                        self.fail(f"start: KiCad API did not answer ({str(ex).strip().splitlines()[-1][:120]})")
                    time.sleep(2)
            while True:
                try:
                    self.call("ping", timeout=5)
                    break
                except GuiError as ex:
                    if time.monotonic() - t0 > timeout_s:
                        err = Path(str(sock) + ".err")
                        self.fail(f"start: the GUI helper does not answer ({ex}); "
                                  f"{err.read_text()[-400:] if err.is_file() else 'no helper error file'}")
                    time.sleep(1)
            self.main = e.wait(f"{self.board.stem} — PCB Editor", 10)[1]
            self.step("start", f"display {self.display}, pcbnew pid {self.pid()}")
        except BaseException:
            self.stop()
            raise
        return self

    def pid(self):
        return next((p.pid for n, p in self.e.procs if n == "pcbnew"), None)

    def alive(self) -> bool:
        p = next((p for n, p in self.e.procs if n == "pcbnew"), None)
        if p is None or p.poll() is not None:
            return False
        try:
            self.call("ping", timeout=5)
            return True
        except GuiError:
            return False

    def stop(self) -> None:
        with self._lock:
            if not self.e.procs:
                return
            self.e.stop(keep_x=False)  # SIGTERM / SIGKILL to the process groups WE started (stored Popen pids)
            self.e.procs.clear()
            for p in self.e.helper_paths():
                try:
                    p.unlink()
                except OSError:
                    pass

    # ---------------------------------------------------------------- steps
    def step(self, name: str, detail: str = "") -> None:
        self.steps.append({"t": round(time.monotonic() - self.started, 1), "step": name, "detail": detail})
        self.e.log(f"step {name}: {detail}")

    def fail(self, msg: str):
        shot = self.e.shot("fail")
        self.step("FAIL", msg)
        raise GuiError(f"{msg} (screenshot {shot})")

    def call(self, cmd: str, timeout: float = 30.0, **args):
        """One helper command; its error becomes a GuiError (the caller decides whether that ends the session)."""
        self.last_use = time.monotonic()
        sock, _ = self.e.helper_paths()
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
                s.settimeout(timeout)
                s.connect(str(sock))
                s.sendall((json.dumps({"token": self.token, "cmd": cmd, **args}) + "\n").encode())
                buf = b""
                while not buf.endswith(b"\n"):
                    chunk = s.recv(1 << 20)
                    if not chunk:
                        break
                    buf += chunk
        except OSError as ex:
            raise GuiError(f"helper {cmd}: {ex}") from None
        try:
            r = json.loads(buf)
        except ValueError:
            raise GuiError(f"helper {cmd}: no answer") from None
        if not r.get("ok"):
            raise GuiError(f"helper {cmd}: {r.get('error')}")
        return r.get("result")

    def must(self, cmd: str, step: str, **args):
        """`call`, a failure ends the step loudly (screenshot)."""
        try:
            return self.call(cmd, **args)
        except GuiError as ex:
            self.fail(f"{step}: {ex}")

    def wait_for(self, step: str, timeout: float, pred, every: float = 0.5, on_poll=None):
        """Poll `pred()` (truthy = done) until `timeout`; a timeout fails the step. `on_poll()` runs every poll."""
        t0 = time.monotonic()
        while True:
            if on_poll:
                on_poll()
            try:
                r = pred()
            except GuiError:
                r = None
            if r:
                return r
            if time.monotonic() - t0 > timeout:
                self.fail(f"{step}: not done within {timeout:.0f} s")
            time.sleep(every)

    def window(self, title: str):
        ws = [w for w in self.call("windows") if w["shown"] and title in w["title"]]
        return ws[0] if ws else None

    def xid(self, title: str):
        f = self.e.find(title)
        return f[1] if f else None

    def answer_confirmation(self) -> bool:
        """KiCad's "Zone fills are out-of-date. Refill?" (a native message box): Enter = its default, Refill."""
        c = self.e.find("Confirmation")
        if c:
            self.e.shot("confirmation", c[1])
            self.e.key(c[1], "Return")
            self.step("confirmation answered", c[0])
            return True
        return False

    # ---------------------------------------------------------------- DRC dialog
    def drc_open(self) -> None:
        if not self.window(DRC_TITLE):
            self.must("menu", "drc open", path=["Inspect", "Design Rules Checker"])
            self.wait_for("drc open: dialog shown", 30, lambda: self.window(DRC_TITLE))
        r = self.must("place", "drc place", title=DRC_TITLE, x=DRC_RECT[0], y=DRC_RECT[1], w=DRC_RECT[2], h=DRC_RECT[3])
        self.step("drc dialog placed", str(r))

    def drc_run(self, timeout_s: float = 600.0) -> dict:
        """Run DRC ONCE per board state: Run DRC pressed, the run seen (progress page / Run disabled / the status
        line changed, polled every 0.1 s for 15 s), then the results page with "Completed" in the status line.
        Returns the dialog state; the rows are kept in `self.drc_rows` for this board state."""
        st = self.must("drc_state", "drc state")
        if not st["boxes"].get("Refill all zones before performing DRC", True):
            self.fail("drc run: 'Refill all zones before performing DRC' is off (kicad-cli refills)")
        before = st.get("status", "")
        self.must("button", "drc run", title=DRC_TITLE, label="Run DRC")
        t0 = time.monotonic()
        seen = False
        while time.monotonic() - t0 < 15 and not seen:
            try:
                x = self.call("drc_state")
            except GuiError:
                x = None  # busy while the DRC runs
            seen = x is None or not x["run_enabled"] or x["results_page"] != 1 or x.get("status", "") != before
            time.sleep(0.1)
        if not seen:
            self.fail("drc run: the run was not observed within 15 s (no progress page, Run never disabled, status "
                      f"line unchanged: {before!r})")
        st = self.wait_for("drc run: results shown", timeout_s,
                           lambda: (lambda x: x if x["run_enabled"] and x["results_page"] == 1 and
                                    x.get("status", "").startswith("Completed") else None)(self.call("drc_state")),
                           every=0.5, on_poll=self.answer_confirmation)
        self.drc_rows = self.must("drc_rows", "drc rows", tab="violations")
        self.drc_state_of = self.board_state
        self.drc_severities = None
        self.step("drc ran", f"{', '.join(st['tabs'][:2])}; {st['status']}")
        return st

    def drc_filter(self, severities) -> list:
        """Tick exactly the severity boxes of `severities` (DRC_SEVERITY_BOXES keys) in the dialog's "Show:" row, as a
        user does (KiCad refilters the list), read the boxes back, then re-read the list into `self.drc_rows`."""
        want = {lab: sev in severities for sev, lab in DRC_SEVERITY_BOXES.items()}
        got = {b["label"]: b["value"] for b in self.must("set_checkboxes", "drc severity filter", title=DRC_TITLE,
                                                          values=want)}
        bad = {lab: got.get(lab) for lab, v in want.items() if got.get(lab) != v}
        if bad:
            self.fail(f"drc severity filter: boxes {bad} after setting {want}")
        self.drc_rows = self.must("drc_rows", "drc rows", tab="violations")
        self.drc_severities = tuple(severities)
        self.step("drc severity filter", f"{want}: {len(self.drc_rows)} rows listed")
        return self.drc_rows

    def activate(self, title: str) -> None:
        w = self.xid(title)
        if not w:
            self.fail(f"activate: no window {title!r}")
        self.e.x("xdotool", "windowactivate", "--sync", w)
        self.e.x("xdotool", "windowraise", w)

    def drc_walk(self, targets: list, settle_s: float = 1.0, max_resyncs: int = 3, busy_polls: int = 6,
                 busy_wait_s: float = 0.5):
        """Yield (k, selected_text, error) for each top-level DRC row index in `targets` (ascending), selecting it as
        a user does: one click on the list's first line, then Down / Up keys (the dialog lists every row expanded:
        headline, then its item lines). After every key the selection is read back; when it is not the expected
        line (the dialog shows a row with other item lines than read, e.g. a solder mask bridge), the position is
        relocated by that line's text (nearest such line) and the walk goes on; a line the model never read keeps
        the stepping going the same way until a known line shows (N2), an unchanged selection is waited for (KiCad
        busy), not counted as a relocation (N3). A row that cannot be reached
        (> max_resyncs relocations, key budget spent, a failed helper step) is yielded with its
        error and the next row starts again from a click on the first line: NEVER ends the walk (verify r4 F2).
        O(rows + lines) keys in a straight walk; per row at most 2 * lines + 10 keys."""
        lines = []
        for r in self.drc_rows:
            lines.append((r["k"], r["text"]))
            lines.extend((r["k"], t) for t in r["items"])
        head = {k: i for i, (k, t) in reversed(list(enumerate(lines))) if t == self.drc_rows[k]["text"]}

        def sel():
            return self.call("drc_selected", tab="violations").get("selected")

        def locate(got, near):
            cand = [i for i, (_, t) in enumerate(lines) if t == got]
            return min(cand, key=lambda i: abs(i - near)) if cand else None

        def first():
            """Click the first line; the position of the line that got selected (None: not a known line)."""
            self.activate(DRC_TITLE)
            x, y, w, h = self.must("drc_list_rect", "drc list")["rect"]
            self.e.click_abs(x + 80, y + 12)
            time.sleep(0.5)
            got = sel()
            if got != lines[0][1]:
                self.step("drc walk first line", f"a click on the first line selected {got!r}, not {lines[0][1]!r}")
                return locate(got, 0), got
            return 0, got

        pos = None
        for k in targets:
            want, err, resyncs = head[k], None, 0
            try:
                if pos is None:
                    pos, got = first()
                    if pos is None:
                        yield k, None, f"a click on the first line selected {got!r}, not a line of the list"
                        continue
                budget = 2 * len(lines) + 10
                lost, lost_dir = 0, 1  # keys since the selection was a line the model never read (N2)
                while pos != want and budget > 0:
                    step = lost_dir if lost else (1 if pos < want else -1)
                    self.activate(DRC_TITLE)
                    self.e.x("xdotool", "key", "Down" if step > 0 else "Up")
                    budget -= 1
                    time.sleep(0.08)
                    got = sel()
                    exp = lines[pos + step][1] if not lost and 0 <= pos + step < len(lines) else None
                    for _ in range(0 if lost else busy_polls):
                        if got != lines[pos][1] or got == exp:  # still the old line: KiCad is busy, wait, not a relocation (N3)
                            break
                        time.sleep(busy_wait_s)
                        got = sel()
                    if exp is not None and got == exp:
                        pos += step
                        continue
                    npos = locate(got, pos + step * (lost + 1))
                    if npos is None:  # a line the model never read: keep stepping the same way until a known one (N2)
                        lost, lost_dir = lost + 1, step
                        self.step("drc walk unknown line", f"row {k}: selection {got!r} is no known line ({lost} key(s) "
                                                           f"past line {pos})")
                        continue
                    lost = 0
                    resyncs += 1
                    self.step("drc walk resync", f"row {k}: after {'Down' if step > 0 else 'Up'} from line {pos} the "
                                                 f"selection is {got!r}, expected {exp!r}; relocated to {npos}")
                    if resyncs > max_resyncs:
                        err = (f"after {'Down' if step > 0 else 'Up'} from line {pos} the selection is {got!r}, expected "
                               f"{exp!r} ({resyncs} relocations)")
                        break
                    pos = npos
                if lost and not err:
                    err = f"the selection stayed on lines the model never read ({lost} keys)"
                if pos != want:
                    pos = None  # unknown: the next row starts from a click on the first line
                    yield k, None, err or f"line {want} not reached within the key budget"
                    continue
                time.sleep(settle_s)
                yield k, sel(), None
            except STEP_ERRORS as ex:  # a failed helper / X step: this row fails, the walk goes on
                pos = None
                yield k, None, describe(ex)
                if not self.alive():
                    return

    # ---------------------------------------------------------------- view transform
    def set_zoom(self, value: float) -> float:
        """The tool bar's zoom preset nearest `value` (cmd zoom); returns its number ("Zoom 20,00" -> 20.0)."""
        name = self.must("zoom", "zoom", value=float(value))["zoom"]
        m = re.search(r"([\d.,]+)\s*$", str(name))
        z = float(m.group(1).replace(",", ".")) if m else float(value)
        self.zoom_value, self._scale = float(value), None
        return z

    def _status_xy(self):
        for f in self.call("status"):
            m = re.match(r"^X\s*(-?[\d.,]+)\s+Y\s*(-?[\d.,]+)$", f.strip())
            if m:
                return float(m.group(1).replace(",", ".")), float(m.group(2).replace(",", "."))
        self.fail("view: no 'X .. Y ..' field in the status bar")

    def focus(self) -> dict:
        """Where KiCad focused after a DRC row was selected. Its cross-probe centres the selected row's items in
        the part of the canvas the DRC dialog does not cover (spot_px, screen px; for track items KiCad centres
        on the items' bounding box, not on the marker) without moving the pointer, and the status bar's X/Y only
        follows the pointer: the pointer is moved onto spot_px (no click) and X/Y read there until two reads agree
        (focus_mm, snapped to the finest grid), then, once per zoom, 300 / 200 px further to get the view scale.
        Returns {focus_mm, canvas, spot_px, mm_px (mm per px in X, Y), view_px (x0, y0, x1, y1 of the free part)}."""
        x0, y0, w, h = self.call("canvas_rect")
        free_w = max(1, min(x0 + w, DRC_RECT[0]) - x0)
        sx, sy = x0 + free_w // 2, y0 + h // 2

        def read(px, py):
            # two moves: the pointer may already sit on that pixel from the last row (no motion event, a stale X/Y)
            self.e.x("xdotool", "mousemove", str(px + 3), str(py + 3))
            time.sleep(0.15)
            self.e.x("xdotool", "mousemove", str(px), str(py))
            time.sleep(0.35)
            return self._status_xy()
        xy, prev = read(sx, sy), None
        for _ in range(3):  # the status bar may lag the view after a scroll: read until stable
            if xy == prev:
                break
            xy, prev = read(sx, sy), xy
        if self._scale is None:
            far = read(sx + 300, sy + 200)
            self._scale = (abs(far[0] - xy[0]) / 300 or None, abs(far[1] - xy[1]) / 200 or None)
        return {"focus_mm": list(xy), "canvas": [x0, y0, w, h], "spot_px": [sx, sy], "mm_px": list(self._scale),
                "view_px": [x0, y0, x0 + free_w, y0 + h]}

    def marker(self, tries: int = 5, wait_s: float = 0.2):
        """(x, y) mm of the DRC marker KiCad itself highlights (brightens) for the selected row, read through the
        helper's drc_marker command; None when it reports none or several after `tries` reads (the caller must then
        not claim a ring position)."""
        for i in range(tries):
            ms = self.call("drc_marker").get("markers") or []
            if len(ms) == 1:
                return float(ms[0]["x"]), float(ms[0]["y"])
            if i + 1 < tries:
                time.sleep(wait_s)
        return None

    def window_origin(self, win: str | None = None) -> tuple:
        """Screen position (x, y) of the top-left pixel of `xwd -id <win>` (default: the main window): the window's
        screenshot starts below the window manager's title bar, so screen px - this origin = screenshot px."""
        g = self.e.geom(win or self.main)
        return g[0], g[1]


# ------------------------------------------------------------------ the one session of this process

_SESSION: GuiSession | None = None
_LOCK = threading.RLock()
_TIMER: threading.Timer | None = None


def _arm_idle(s: GuiSession) -> None:
    global _TIMER
    if _TIMER is not None:
        _TIMER.cancel()

    def fire():
        with _LOCK:
            if _SESSION is s and time.monotonic() - s.last_use >= s.idle_s - 1:
                stop_session()
            elif _SESSION is s:
                _arm_idle(s)

    _TIMER = threading.Timer(max(1.0, s.idle_s - (time.monotonic() - s.last_use)), fire)
    _TIMER.daemon = True
    _TIMER.start()


def session_for(board: Path, out: Path, *, idle_s: float = IDLE_S, reuse: bool = True) -> GuiSession:
    """The live session on `board` when there is one (same board copy, pcbnew alive, helper answering), else the
    old one is stopped and a new one started. Caller holds `_LOCK` for the whole tool call."""
    global _SESSION
    s = _SESSION
    if s is not None and reuse and s.board == Path(board) and s.alive():
        s.last_use = time.monotonic()
        s.step("reused", str(board))
        _arm_idle(s)
        return s
    stop_session()
    s = GuiSession(board, out, idle_s=idle_s).start()
    _SESSION = s
    _arm_idle(s)
    return s


def stop_session() -> dict:
    global _SESSION, _TIMER
    with _LOCK:
        s, _SESSION = _SESSION, None
        if _TIMER is not None:
            _TIMER.cancel()
            _TIMER = None
        if s is None:
            return {"stopped": False}
        pid = s.pid()
        s.stop()
        return {"stopped": True, "display": s.display, "pcbnew_pid": pid, "board": str(s.board)}


def session_status() -> dict:
    with _LOCK:
        s = _SESSION
        if s is None:
            return {"session": None}
        return {"session": {"display": s.display, "board": str(s.board), "pcbnew_pid": s.pid(), "alive": s.alive(),
                            "idle_s": round(time.monotonic() - s.last_use, 1), "idle_limit_s": s.idle_s,
                            "steps": s.steps[-10:]}}


atexit.register(stop_session)


_PREV_HANDLERS: dict = {}  # signum -> the handler install_signal_stop replaced (callable or None)


def _on_signal(signum, frame):
    """SIGTERM / SIGHUP: stop our KiCad + Xephyr by pid (without _LOCK: a tool thread may hold it), then the
    previous handler, else exit 128 + signum (atexit does not run on a signal)."""
    s = _SESSION
    if s is not None:
        try:
            s.stop()
        except Exception:  # noqa: BLE001 - dying anyway; the pids are gone or going
            pass
    prev = _PREV_HANDLERS.get(signum)
    if callable(prev):
        prev(signum, frame)
    else:
        raise SystemExit(128 + signum)


def install_signal_stop() -> bool:
    """Install _on_signal for SIGTERM and SIGHUP (main thread only; the MCP server calls it at start)."""
    import signal  # noqa: PLC0415
    if threading.current_thread() is not threading.main_thread():
        return False
    for sig in (signal.SIGTERM, signal.SIGHUP):
        prev = signal.getsignal(sig)
        if prev is not _on_signal:
            _PREV_HANDLERS[sig] = prev if callable(prev) else None
            signal.signal(sig, _on_signal)
    return True


# ------------------------------------------------------------------ crops

def crop(src: Path, dst: Path, box: tuple, mark: tuple | None = None, r: int = 26) -> str:
    """`box` (x, y, w, h) of the PNG `src` -> `dst`; `mark` (x, y in src pixels) gets a yellow ring."""
    from PIL import Image, ImageDraw
    im = Image.open(src).convert("RGB")
    x, y, w, h = box
    x, y = max(0, x), max(0, y)
    c = im.crop((x, y, min(im.width, x + w), min(im.height, y + h)))
    if mark is not None:
        mx, my = mark[0] - x, mark[1] - y
        ImageDraw.Draw(c).ellipse((mx - r, my - r, mx + r, my + r), outline=(255, 230, 0), width=3)
    c.save(dst)
    return str(dst)


def norm_text(s: str) -> str:
    """A DRC text as both KiCad front ends print it: severity prefix off, decimal comma -> point, spaces folded."""
    s = re.sub(r"^(Error|Warning|Exclusion):\s*", "", (s or "").strip())
    s = re.sub(r"(\d),(\d)", r"\1.\2", s)
    return " ".join(s.split())


def gui_key(row: dict) -> tuple:
    return (norm_text(row["text"]), tuple(sorted(norm_text(i) for i in row["items"])))


def find_row(free: dict, key: tuple):
    """The GUI row for a kicad-cli record `key` (cli_key) from `free` ({gui_key: [rows]}, consumed): the same
    headline and the same items, else the same headline with the record's items a subset of the row's (the dialog
    lists more items for some types, e.g. the zone of a solder mask bridge). None when there is none."""
    rows = free.get(key)
    if rows:
        return rows.pop(0)
    for k, rows in free.items():
        if rows and k[0] == key[0] and set(key[1]) <= set(k[1]):
            return rows.pop(0)
    return None


def cli_key(v: dict) -> tuple:
    return (norm_text(v.get("text") or v.get("description", "")),
            tuple(sorted(norm_text(i.get("desc") or i.get("description", "")) for i in v.get("items") or [])))


__all__ = ["GuiSession", "GuiError", "session_for", "stop_session", "session_status", "crop", "norm_text",
           "gui_key", "cli_key", "find_row", "copy_board", "CLEANUP_LABELS", "CLEANUP_FILTERS", "CLEANUP_TITLE", "DRC_TITLE",
           "DRC_SEVERITY_BOXES"]
