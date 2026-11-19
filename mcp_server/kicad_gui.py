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

    # ---------------------------------------------------------------- lifecycle
    def start(self, timeout_s: float = 240.0) -> "GuiSession":
        e = self.e
        e.make_home()
        cfg = e.home / ".config/kicad/10.0"
        pj = cfg / "pcbnew.json"
        d = json.loads(pj.read_text()) if pj.is_file() else {}
        # the GUI DRC as kicad-cli runs it: zones refilled, every track error (drc_dialog keys of pcbnew.json)
        d.setdefault("drc_dialog", {}).update(refill_zones=True, test_all_track_errors=True)
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
        st = self.must("drc_state", "drc state")
        if not st["boxes"].get("Refill all zones before performing DRC", True):
            self.fail("drc run: 'Refill all zones before performing DRC' is off (kicad-cli refills)")
        self.must("button", "drc run", title=DRC_TITLE, label="Run DRC")
        time.sleep(1.0)
        st = self.wait_for("drc run: results shown", timeout_s,
                           lambda: (lambda s: s if s["run_enabled"] and s["results_page"] == 1 and
                                    any(t.startswith("Violations (") for t in s["tabs"]) else None)(self.call("drc_state")),
                           every=1.0, on_poll=self.answer_confirmation)
        self.step("drc ran", ", ".join(st["tabs"][:2]))
        return st

    # ---------------------------------------------------------------- view transform
    def cursor_mm(self, sx: int, sy: int) -> tuple:
        """Board position (mm) under screen point (sx, sy): pointer warped there, KiCad's status bar read."""
        self.e.x("xdotool", "mousemove", str(sx), str(sy))
        time.sleep(0.35)
        for f in self.call("status"):
            m = re.match(r"^X\s*(-?[\d.,]+)\s+Y\s*(-?[\d.,]+)$", f.strip())
            if m:
                return float(m.group(1).replace(",", ".")), float(m.group(2).replace(",", "."))
        self.fail("view: no 'X .. Y ..' field in the status bar")

    def view(self) -> dict:
        """Screen -> board transform of the canvas: two pointer positions 600 px apart (grid = finest)."""
        x0, y0, w, h = self.call("canvas_rect")
        cx, cy = x0 + w // 2, y0 + h // 2
        a = self.cursor_mm(cx - 300, cy - 300)
        b = self.cursor_mm(cx + 300, cy + 300)
        self.e.x("xdotool", "mousemove", "1700", "1065")  # off the canvas: no crosshair / hover in the pictures
        sx, sy = (b[0] - a[0]) / 600.0, (b[1] - a[1]) / 600.0
        if abs(sx) < 1e-9 or abs(sy) < 1e-9:
            self.fail(f"view: degenerate transform {a} -> {b}")
        return {"canvas": [x0, y0, w, h], "mm_per_px": (sx, sy), "origin": (a[0] - sx * (cx - 300), a[1] - sy * (cy - 300))}

    @staticmethod
    def to_screen(v: dict, x_mm: float, y_mm: float) -> tuple:
        return (round((x_mm - v["origin"][0]) / v["mm_per_px"][0]), round((y_mm - v["origin"][1]) / v["mm_per_px"][1]))


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
           "gui_key", "cli_key", "find_row", "copy_board", "CLEANUP_LABELS", "CLEANUP_FILTERS", "CLEANUP_TITLE", "DRC_TITLE"]
