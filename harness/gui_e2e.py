"""Live end-to-end test of the plugin in a real KiCad 10, on a nested X display, without touching the user's KiCad.

    tools/gui_e2e.sh <board|pcb> <out_dir> [--display :6] [--wheel WHEEL] [--cancel-after S] [--keep]
                     [--harness METRICS_JSON] [--toml SECTION.KEY=VALUE ...] [--from-hand] [--name N]
                     [--cleanup] [--existing] [--cleanup-opts all|user|pcbnew-json|BOX,BOX]

What it does (every step leaves a screenshot in <out_dir>/shots/ and a line in <out_dir>/e2e.log):
  1. A SEPARATE KiCad home <out_dir>/home: HOME, XDG_{CONFIG,DATA,CACHE,STATE}_HOME, KICAD_CONFIG_HOME, TMPDIR (so
     the API socket is <home>/tmp/kicad/api.sock) and a private D-Bus session all point there. Config files that
     suppress KiCad's first-start wizard and enable the KiCad API are written; the plugin is installed into it
     with tools/build.sh (XDG_DATA_HOME / XDG_CACHE_HOME of the test home; TOPOR_WHEEL = --wheel). The user's
     ~/.config/kicad, ~/.local/share/kicad, ~/.cache/kicad and /tmp/kicad are never written.
  2. `harness.strip` copies the board: <out_dir>/boards/<name>-input/ (tracks and vias removed: what is routed)
     and <name>-hand/ (the reference). The source is only read. A copy of the input is kept as
     <name>-input.orig.kicad_pcb (the routed file is saved over the input copy). `--from-hand`: KiCad opens a copy
     of the hand board (<name>-live/) instead and topor rips everything (existing_copper = "rip", rip_free_vias,
     respect_locked = false): the input the headless harness models (track widths from the existing copper).
  3. Xephyr on --display (started if not running), metacity, `pcbnew <input copy>`; waits until KiCad created the
     plugin environment (first start: pip from PyPI) and the board is loaded.
  4. Clicks the Topor button (found in the toolbar by its icon), then: first-run notice -> Continue (only without
     a topor.toml), pre-flight -> Route, progress (screenshots; with --cancel-after S presses Cancel (Esc: stop at once, nothing
     taken, the board must be unchanged) after S seconds; with --accept-after S clicks Accept (stop and keep the best,
     the preview follows) after S seconds; with --ok-at-stable clicks Accept when the dialog says "Routes stable" and checks the
     board is the best result shown), preview -> Accept,
     (no finished window: Accept or Reject just closes the dialog).
  5. Over the KiCad API (an external kipy client in the plugin environment): counts the tracks / vias after
     Accept, presses Ctrl+Z (one undo step, two with run.refill_after_accept, must remove them all), Ctrl+Shift+Z (back), saves the board.
  6. `harness.board_metrics` judges the saved board with kicad-cli DRC (unconnected, routing_new, vias) and
     renders <name>-gui-compare.png (hand left, KiCad result right, open ratsnest blue); with --harness the
     numbers are compared with a `tools/route_board.sh` metrics JSON of the same board.
  7. `--cleanup`: Tools -> Cleanup Tracks & Vias in the real dialog on the saved board (see :func:`cleanup_step`):
     Build Changes, screenshot of the change list, Update PCB; removed items by uuid over the API, the board
     saved, kicad-cli DRC before / after, and harness.cleanup_check's replay of the same options on the board
     from before must remove exactly the same items. `--existing`: `board` is an already routed .kicad_pcb;
     KiCad opens a copy of it (no strip, no plugin install, no routing) and only this step runs.
  8. Stops KiCad and (unless it was running before or --keep) Xephyr.

The test home's TMPDIR is a short symlink /tmp/topor-e2e-<hash> -> <home>/tmp when <home>/tmp/kicad/api.sock
would not fit a unix socket path (KiCad's API then never answers).

Writes <out_dir>/<name>-e2e.json (checks, metrics, comparison). Exit 0 when every check passed.
Complexity: one KiCad session; wall time = KiCad start + plugin run + 3 kicad-cli DRC runs.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
X_TIMEOUT_S = 30.0  # one xdotool / wmctrl / xwd call; a hung X tool must not hang the run
REPLAY_TIMEOUT_S = 600.0  # harness.cleanup_check replay in its own pcbnew
#: time-limit-fix r2: room after the optimizing time limit for the final drawing (not counted: qmtech about 50-360 s),
#: KiCad's DRC and the preview export, before the harness gives up waiting for the preview
DRAW_MARGIN_S = 900.0
ICON = ROOT / "plugin" / "icons" / "topor-light-24.png"
PLUGIN_ID = "org.topor.router"
# Net name -> the effective netclass KiCad's API returns for it (get_netclass_for_nets): name, type, constituents and
# the board values (nm; None = unset), written to <out_dir>/netclasses-ipc.json before the plugin runs.
NETCLASS_DUMP = (
    "nets = list(board.get_nets())\n"
    "for n, nc in board.get_netclass_for_nets(nets).items():\n"
    "    out[n] = {'name': nc.name, 'type': int(nc.type), 'constituents': nc.constituents,"
    " 'clearance': nc.clearance, 'track_width': nc.track_width, 'via_diameter': nc.via_diameter,"
    " 'via_drill': nc.via_drill}\n")


class E2E:
    def __init__(self, out: Path, display: str, cleanup_opts: dict | None = None):
        self.out = out
        self.display = display
        self.cleanup_opts = cleanup_opts  # the Cleanup dialog's options (CLEANUP_BOXES keys) when it is tested
        self.home = out / "home"
        self.shots = out / "shots"
        self.shots.mkdir(parents=True, exist_ok=True)
        self.logf = (out / "e2e.log").open("a")
        self.checks: list = []
        self.procs: list = []
        self.n = 0
        self._drc_refill_off = False

    # ---------------------------------------------------------------- plumbing
    def log(self, msg: str) -> None:
        line = f"[{time.strftime('%H:%M:%S')}] {msg}"
        print(line, flush=True)
        self.logf.write(line + "\n")
        self.logf.flush()

    def check(self, name: str, ok: bool, detail: str = "") -> bool:
        self.checks.append({"check": name, "ok": bool(ok), "detail": detail})
        self.log(f"{'PASS' if ok else 'FAIL'} {name}{': ' + detail if detail else ''}")
        return ok

    def tmpdir(self) -> Path:
        """<home>/tmp, or a short symlink to it: KiCad's API socket <TMPDIR>/kicad/api.sock must fit a unix socket
        path (108 bytes; with a longer one the API never answers: "Address invalid")."""
        t = self.home / "tmp"
        if len(str(t / "kicad" / "api.sock")) < 100:
            return t
        import hashlib
        link = Path("/tmp") / f"topor-e2e-{hashlib.sha1(str(t).encode()).hexdigest()[:8]}"
        if not link.is_symlink():
            link.symlink_to(t)
        return link

    def env(self) -> dict:
        h = str(self.home)
        e = dict(os.environ)
        e.update(HOME=h, XDG_CONFIG_HOME=f"{h}/.config", XDG_DATA_HOME=f"{h}/.local/share",
                 XDG_CACHE_HOME=f"{h}/.cache", XDG_STATE_HOME=f"{h}/.local/state", TMPDIR=str(self.tmpdir()),
                 XDG_RUNTIME_DIR=f"{h}/run", KICAD_CONFIG_HOME=f"{h}/.config/kicad", DISPLAY=self.display,
                 GDK_BACKEND="x11", NO_AT_BRIDGE="1", RAYON_NUM_THREADS=e.get("RAYON_NUM_THREADS", "4"))
        # KiCad's location variables an `import harness` process carries (harness/kicad_env.PROCESS_VARS) point at
        # the shared scratch home: KiCad must use THIS session home (KICAD_DOCUMENTS_HOME moves the scripting
        # plugin folder: the GUI helper / plugin never loaded, verify r3 V1)
        for k in ("DBUS_SESSION_BUS_ADDRESS", "KICAD_API_SOCKET", "KICAD_API_TOKEN", "TOPOR_REPORT_ROOT",
                  "KICAD_CACHE_HOME", "KICAD_DOCUMENTS_HOME"):
            e.pop(k, None)
        return e

    def x(self, *args, check=False, timeout: float = X_TIMEOUT_S) -> str:
        """One X tool call (xdotool, wmctrl, ...) on this display, bounded: a hang raises subprocess.TimeoutExpired."""
        r = subprocess.run(list(args), capture_output=True, text=True, env={**os.environ, "DISPLAY": self.display},
                           timeout=timeout, stdin=subprocess.DEVNULL)
        if check and r.returncode:
            raise RuntimeError(f"{args}: {r.stderr}")
        return r.stdout

    def windows(self) -> dict:
        """title -> window id of the visible top-level windows (wmctrl)."""
        out = {}
        for ln in self.x("wmctrl", "-l").splitlines():
            parts = ln.split(None, 3)
            if len(parts) == 4:
                out[parts[3]] = parts[0]
        return out

    def find(self, prefix: str):
        for t, w in self.windows().items():
            if t.startswith(prefix) or prefix in t:
                return t, w
        return None

    def wait(self, prefix: str, timeout: float, *, every: float = 0.5, on_tick=None):
        t0 = time.monotonic()
        while time.monotonic() - t0 < timeout:
            f = self.find(prefix)
            if f:
                return f
            if on_tick:
                on_tick(time.monotonic() - t0)
            time.sleep(every)
        return None

    def wait_gone(self, prefix: str, timeout: float) -> bool:
        t0 = time.monotonic()
        while time.monotonic() - t0 < timeout:
            if not self.find(prefix):
                return True
            time.sleep(0.3)
        return False

    def shot(self, tag: str, win: str | None = None) -> Path:
        self.n += 1
        p = self.shots / f"{self.n:02d}-{tag}.png"
        cmd = f"xwd -id {win}" if win else "xwd -root"
        subprocess.run(f"{cmd} -display {self.display} | convert xwd:- {p}", shell=True,
                       capture_output=True, timeout=X_TIMEOUT_S)
        return p

    def geom(self, win: str) -> tuple:
        info = self.x("xwininfo", "-id", win)
        g = {}
        for ln in info.splitlines():
            ln = ln.strip()
            for k in ("Absolute upper-left X", "Absolute upper-left Y", "Width", "Height"):
                if ln.startswith(k + ":"):
                    g[k] = int(ln.split(":")[1])
        return g["Absolute upper-left X"], g["Absolute upper-left Y"], g["Width"], g["Height"]

    def click_abs(self, x: int, y: int) -> None:
        """Slow click (dialogs ignore fast synthetic clicks)."""
        self.x("xdotool", "mousemove", str(x), str(y))
        time.sleep(0.3)
        self.x("xdotool", "mousedown", "1")
        time.sleep(0.15)
        self.x("xdotool", "mouseup", "1")

    def click_rel(self, win: str, rx: int, ry: int) -> None:
        """Click at a point given relative to the window's bottom-right corner when negative."""
        x0, y0, w, h = self.geom(win)
        self.click_abs(x0 + (rx if rx >= 0 else w + rx), y0 + (ry if ry >= 0 else h + ry))

    def click_accept(self, win: str) -> None:
        """Click the progress dialog's Accept button (bottom right: Cancel and Accept sit side by side; Enter presses
        neither, and the Alt+A mnemonic is not delivered reliably through Xephyr)."""
        _, _, w, h = self.geom(win)
        self.click_rel(win, w - 68, h - 30)

    def click_pause(self, win: str) -> None:
        """pause-button: click the progress dialog's Pause / Resume button (left of Cancel, which is left of Accept:
        three buttons of at least 88 DIP side by side at the bottom right)."""
        _, _, w, h = self.geom(win)
        self.click_rel(win, w - 272, h - 30)

    def plugin_pid(self):
        """The plugin's Python process (KiCad starts it from the test home's plugin environment), or None."""
        env = self.home / ".cache/kicad/10.0/python-environments/org.topor.router/bin/python"
        r = subprocess.run(["pgrep", "-f", f"^{env}"], capture_output=True, text=True, timeout=X_TIMEOUT_S)
        pids = r.stdout.split()
        return int(pids[0]) if pids else None

    def key(self, win: str, *keys: str) -> None:
        self.x("xdotool", "windowactivate", "--sync", win)
        time.sleep(0.3)
        for k in keys:
            self.x("xdotool", "key", "--window", win, k)
            time.sleep(0.2)

    # ---------------------------------------------------------------- setup
    def make_home(self) -> None:
        cfg = self.home / ".config" / "kicad" / "10.0"
        cfg.mkdir(parents=True, exist_ok=True)
        for d in ("tmp", "run", ".local/share", ".cache"):
            (self.home / d).mkdir(parents=True, exist_ok=True)
        os.chmod(self.home / "run", 0o700)
        tpl = Path("/usr/share/kicad/template")
        for n in ("fp-lib-table", "sym-lib-table"):
            if not (cfg / n).exists() and (tpl / n).is_file():
                shutil.copy(tpl / n, cfg / n)
        if not (cfg / "design-block-lib-table").exists():
            uri = tpl / "design-block-lib-table"
            body = (f'\t(lib (name "KiCad") (type "Table") (uri "{uri}") (options "") (descr "KiCad Default Libraries"))\n'
                    if uri.is_file() else "")
            (cfg / "design-block-lib-table").write_text(f"(design_block_lib_table\n\t(version 7)\n{body})\n")
        kc = cfg / "kicad_common.json"
        d = json.loads(kc.read_text()) if kc.is_file() else {}
        d.setdefault("api", {}).update(enable_server=True, interpreter_path=shutil.which("python3") or "/usr/bin/python3")
        d.setdefault("do_not_show_again", {}).update(data_collection_prompt=True, update_check_prompt=True)
        if self.cleanup_opts is not None:
            # the Cleanup dialog's check boxes: KiCad 10 keeps them in DIALOG_SHIM's control state, keyed by the
            # dialog title (kicad_common.json dialog.controls; pcbnew.json "cleanup" is the unused KiCad 9 store)
            d.setdefault("dialog", {}).setdefault("controls", {})[CLEANUP_TITLE] = cleanup_control_state(
                self.cleanup_opts)
        kc.write_text(json.dumps(d, indent=1))
        if self.cleanup_opts is not None:
            hk = cfg / "user.hotkeys"  # a hotkey for Tools -> Cleanup Tracks & Vias (none by default)
            lines = [ln for ln in (hk.read_text().splitlines() if hk.is_file() else [])
                     if not ln.startswith(CLEANUP_ACTION + "\t")]
            hk.write_text("\n".join([*lines, f"{CLEANUP_ACTION}\t{CLEANUP_HOTKEY_KICAD}\t"]) + "\n")
        self.log(f"test KiCad home {self.home}")

    def install(self, wheel: str | None) -> None:
        e = {**os.environ, "XDG_DATA_HOME": str(self.home / ".local/share"), "XDG_CACHE_HOME": str(self.home / ".cache")}
        e.pop("TOPOR_KICAD_HOME", None)
        e.pop("KICAD_DOCUMENTS_HOME", None)
        if wheel:
            e["TOPOR_WHEEL"] = wheel
        r = subprocess.run([str(ROOT / "tools" / "build.sh")], cwd=ROOT, env=e, capture_output=True, text=True)
        self.log((r.stdout + r.stderr).strip().splitlines()[-1] if (r.stdout + r.stderr).strip() else "build.sh")
        if r.returncode:
            raise RuntimeError(f"tools/build.sh failed:\n{r.stderr[-2000:]}")
        envpy = self.env_python()
        if envpy.exists():  # same version, new build: KiCad keeps the old environment, so reinstall the wheel
            whl = next((self.plugin_dir() / "wheels").glob("topor_engine-*.whl"))
            subprocess.run([str(envpy), "-m", "pip", "install", "-q", "--force-reinstall", "--no-deps", str(whl)],
                           check=True, capture_output=True)
            self.log(f"engine wheel reinstalled into the test plugin environment ({whl.name})")

    def plugin_dir(self) -> Path:
        return self.home / ".local/share/kicad/10.0/plugins" / PLUGIN_ID

    def env_dir(self) -> Path:
        return self.home / ".cache/kicad/10.0/python-environments" / PLUGIN_ID

    def env_python(self) -> Path:
        return self.env_dir() / "bin" / "python"

    def start_x(self) -> bool:
        """Start Xephyr unless the display exists. True when started here."""
        sock = Path("/tmp/.X11-unix") / f"X{self.display.lstrip(':')}"
        if sock.exists():
            self.log(f"display {self.display} already running")
            return False
        p = subprocess.Popen(["Xephyr", self.display, "-ac", "-screen", "1920x1080", "-nolisten", "tcp",
                              "-no-host-grab", "-title", f"KiCad test ({self.display})"],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        self.procs.append(("Xephyr", p))
        for _ in range(50):
            if sock.exists():
                break
            time.sleep(0.1)
        self.log(f"Xephyr {self.display} started (pid {p.pid})")
        return True

    def start_kicad(self, pcb: Path) -> None:
        script = ('metacity --replace >"$HOME/metacity.log" 2>&1 & sleep 1; exec pcbnew "$1"')
        p = subprocess.Popen(["dbus-run-session", "--", "sh", "-c", script, "_", str(pcb)], env=self.env(),
                             stdout=(self.out / "pcbnew.log").open("w"), stderr=subprocess.STDOUT,
                             start_new_session=True)
        self.procs.append(("pcbnew", p))
        self.log(f"pcbnew {pcb} (pid {p.pid})")

    def stop(self, keep_x: bool) -> None:
        for name, p in reversed(self.procs):
            if name == "Xephyr" and keep_x:
                continue
            try:
                os.killpg(p.pid, signal.SIGTERM)
                p.wait(timeout=10)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                try:
                    os.killpg(p.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            self.log(f"stopped {name}")
        link = self.tmpdir()
        if link.is_symlink() and any(n == "pcbnew" for n, _ in self.procs):
            link.unlink()  # the short TMPDIR link (the files stay in <home>/tmp)

    # ---------------------------------------------------------------- KiCad
    def env_ready(self) -> bool:
        sp = list(self.env_dir().glob("lib/python3*/site-packages"))
        return bool(sp) and (sp[0] / "topor_engine").is_dir() and (sp[0] / "kipy").is_dir()

    def ipc(self, code: str) -> dict:
        """Run `code` (defines `out` dict, has `board`) in the plugin environment as an external API client."""
        sock = self.tmpdir() / "kicad" / "api.sock"
        prog = ("import json\nfrom kipy import KiCad\nk = KiCad(socket_path='ipc://" + str(sock) + "', client_name='e2e')\n"
                "board = k.get_board()\nout = {}\n" + code + "\nprint(json.dumps(out))\n")
        # the plugin environment's kipy; without a plugin install (--existing) the harness venv's kipy
        py = self.env_python() if self.env_python().exists() else Path(sys.executable)
        r = subprocess.run([str(py), "-c", prog], capture_output=True, text=True, timeout=600,
                           env={k: v for k, v in self.env().items() if k != "PYTHONPATH"})
        if r.returncode:
            raise RuntimeError(f"IPC client failed: {r.stderr[-1500:]}")
        return json.loads(r.stdout.strip().splitlines()[-1])

    def ipc_retry(self, code: str, tries: int = 60, *, connect: bool = False) -> dict:
        """:meth:`ipc`, retried while KiCad answers "busy" (a modal dialog or a running tool); with `connect` also
        while the API server is not up yet."""
        for i in range(tries):
            try:
                return self.ipc(code)
            except RuntimeError as ex:
                txt = str(ex).lower()
                if not ("busy" in txt or (connect and "failed to connect" in txt)) or i == tries - 1:
                    raise
                time.sleep(1)

    def snapshot(self) -> dict:
        """uuid -> {kind, net, layer, g (geometry, nm), w} of every track, arc and via of the open board (API)."""
        return self.ipc_retry(
            "for t in board.get_tracks():\n"
            "    arc = type(t).__name__ == 'ArcTrack'\n"
            "    g = [t.start.x, t.start.y, *( [t.mid.x, t.mid.y] if arc else []), t.end.x, t.end.y]\n"
            "    out[t.id.value] = {'kind': 'arc' if arc else 'segment', 'net': t.net.name, 'layer': int(t.layer),"
            " 'g': g, 'w': t.width}\n"
            "for v in board.get_vias():\n"
            "    out[v.id.value] = {'kind': 'via', 'net': v.net.name, 'layer': -1,"
            " 'g': [v.position.x, v.position.y], 'w': 0}\n")

    def pcbnew_pid(self):
        for name, p in self.procs:
            if name == "pcbnew":
                r = subprocess.run(["pgrep", "-g", str(p.pid), "-x", "pcbnew"], capture_output=True, text=True,
                                   timeout=X_TIMEOUT_S)
                pids = r.stdout.split()
                return int(pids[0]) if pids else None
        return None

    def cpu_ticks(self, pid) -> int:
        try:
            f = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
            return int(f[11]) + int(f[12])  # utime + stime (fields 14, 15)
        except (OSError, IndexError, ValueError):
            return 0

    def grab(self, win: str):
        """The window's pixels as an RGB array (numpy), after raising it (a window under another one gives stale
        pixels: KiCad raises its main frame over a dialog when a progress window closes)."""
        import io

        import numpy as np
        from PIL import Image
        self.x("xdotool", "windowraise", win)
        time.sleep(0.3)
        r = subprocess.run(f"xwd -id {win} -display {self.display} -silent | convert xwd:- png:-", shell=True,
                           capture_output=True, timeout=X_TIMEOUT_S)
        if r.returncode or not r.stdout:
            return None
        return np.asarray(Image.open(io.BytesIO(r.stdout)).convert("RGB"), dtype=np.int16)

    def board_image(self, win: str):
        """last-preview: the progress dialog's board image as pixels: the bounding box of its ground colour
        (plugin/topor/preview.py LIVE_BOARD_RGB), None when the window shows none (fewer than 4000 such pixels)."""
        import numpy as np

        sys.path.insert(0, str(ROOT / "plugin"))
        from topor.preview import LIVE_BOARD_RGB
        px = self.grab(win)
        if px is None:
            return None
        mask = (np.abs(px - np.array(LIVE_BOARD_RGB, dtype=np.int16)).max(axis=2) <= 3)
        if mask.sum() < 4000:
            return None
        ys, xs = np.nonzero(mask)
        return px[ys.min():ys.max() + 1, xs.min():xs.max() + 1]

    def settle(self, win: str, timeout: float, *, done, quiet: float = 3.0, on_other=None) -> bool:
        """Wait until `done(pixels)` of the window `win` holds and pcbnew uses < 5 % of a core, both for `quiet`
        seconds (the whole image is not compared: a blinking text caret changes it). `on_other()` is called every poll (answers dialogs that pop up; True when one
        is up, which restarts the quiet time). False on timeout."""
        pid = self.pcbnew_pid()
        hz = os.sysconf("SC_CLK_TCK")
        t0 = time.monotonic()
        last_ticks, calm_since = self.cpu_ticks(pid), None
        while time.monotonic() - t0 < timeout:
            time.sleep(1.0)
            if on_other and on_other():
                calm_since = None
                continue
            img = self.grab(win)
            ticks = self.cpu_ticks(pid)
            busy = (ticks - last_ticks) / hz > 0.05
            last_ticks = ticks
            if busy or not done(img):
                calm_since = None
                continue
            calm_since = calm_since or time.monotonic()
            if time.monotonic() - calm_since >= quiet:
                return True
        return False

    def gui_drc(self, main: str, tag: str) -> Path:
        """KiCad's own Inspect -> Design Rules Checker on the live board WITHOUT its zone refill (the checkbox is
        switched off), so it judges the pours the board came back with; returns the cropped PNG of the result tabs
        ("Violations (N)", "Unconnected Items (N)"), also kept in shots/. O(one DRC)."""
        x0, y0, _w, _h = self.geom(main)
        dlg = None
        for _try in range(4):  # the menu click is lost now and then while KiCad is busy: retry
            self.x("xdotool", "windowactivate", "--sync", main)
            self.click_abs(x0 + 251, y0 + 14)
            time.sleep(1)
            self.click_abs(x0 + 318, y0 + 96)
            dlg = self.wait("Design Rules Checker", 10)
            if dlg:
                break
            self.x("xdotool", "key", "Escape")
            time.sleep(2)
        if not dlg:
            raise RuntimeError("DRC dialog did not open")
        time.sleep(1)
        dx, dy, _dw, _dh = self.geom(dlg[1])
        img = self.tmpdir() / "drc-dialog.png"
        subprocess.run(f"xwd -id {dlg[1]} -display {self.display} | convert xwd:- {img}", shell=True,
                       timeout=X_TIMEOUT_S)
        if not self._drc_refill_off:  # KiCad's default is checked; it remembers the setting afterwards
            self.click_abs(dx + 28, dy + 26)
            self._drc_refill_off = True
            time.sleep(0.5)
        self.click_abs(dx + 628, dy + 473)
        time.sleep(1)
        for _ in range(120):
            time.sleep(1)
            subprocess.run(f"xwd -id {dlg[1]} -display {self.display} | convert xwd:- {img}", shell=True,
                       timeout=X_TIMEOUT_S)
            if self._dlg_text(img):
                break
        self.n += 1
        txt_shot = self.shots / f"{self.n:02d}-drc-{tag}.png"
        shutil.copy(img, txt_shot)
        crop = self.shots / f"drc-{tag}-tabs.png"
        # only the two count labels (not the focus frame around the first tab): "Violations (N)", "Unconnected Items (N)"
        subprocess.run(["convert", "(", str(img), "-crop", "88x14+32+60", "+repage", ")", "(", str(img), "-crop",
                        "150x14+145+60", "+repage", ")", "+append", str(crop)])
        self.click_abs(dx + 225, dy + 473)  # Delete All Markers: they would change the canvas pixels
        time.sleep(1)
        self.click_abs(dx + 528, dy + 473)  # Close
        self.wait_gone("Design Rules Checker", 10)
        return crop

    def _dlg_text(self, img: Path) -> str:
        """"Completed" when the status line below the buttons shows text (it is blank before the run)."""
        r = subprocess.run(["convert", str(img), "-crop", "200x20+10+498", "+repage", "-colorspace", "Gray",
                            "-format", "%[fx:standard_deviation]", "info:"], capture_output=True, text=True).stdout
        try:
            return "Completed" if float(r) > 0.02 else ""
        except ValueError:
            return ""

    def zone_fills(self) -> dict:
        """uuid -> {net, layers: {layer: [polygons, outer area mm2 - hole area mm2]}} of every zone's FILLED polygons
        of the open board (API); what the user sees as the pour."""
        return self.ipc_retry(
            "def area(o):\n"
            "    pts = [(n.point.x_nm, n.point.y_nm) for n in o.nodes]\n"
            "    return abs(sum(a[0]*b[1]-b[0]*a[1] for a, b in zip(pts, pts[1:] + pts[:1]))) / 2e12\n"
            "for z in board.get_zones():\n"
            "    ls = {}\n"
            "    for fp in z._proto.filled_polygons:\n"
            "        n = a = 0\n"
            "        for p in fp.shapes.polygons:\n"
            "            n += 1; a += area(p.outline) - sum(area(h) for h in p.holes)\n"
            "        ls[int(fp.layer)] = [n, round(a, 3)]\n"
            "    out[z.id.value] = {'net': z.net.name, 'layers': ls}\n")

    def counts(self, tries: int = 30) -> dict:
        """Tracks / vias of the open board; retried while KiCad answers "busy" (a modal dialog or tool). """
        for i in range(tries):
            try:
                return self.ipc("out = {'tracks': len(list(board.get_tracks())), 'vias': len(list(board.get_vias()))}")
            except RuntimeError as ex:
                if "busy" not in str(ex) or i == tries - 1:
                    raise
                if i == 0:
                    self.shot("kicad-busy")
                time.sleep(1)

    def toolbar_button(self, win: str):
        """Centre of the Topor icon in the main window's top toolbar (template match), absolute coords."""
        import numpy as np
        from PIL import Image
        x0, y0, w, h = self.geom(win)
        p = self.shot("toolbar", win)
        img = np.asarray(Image.open(p).convert("RGB"), dtype=np.int32)[:140]
        ic = Image.open(ICON).convert("RGBA")
        from numpy.lib.stride_tricks import sliding_window_view
        best = None
        for size in (24, 22, 20, 26, 28, 32):
            t = np.asarray(ic.resize((size, size)), dtype=np.int32)
            mask = t[:, :, 3] > 127
            tc = t[:, :, :3][mask]
            H, W = img.shape[:2]
            for y in range(0, H - size):
                rows = sliding_window_view(img[y:y + size], (size, size, 3))[0, :, 0]  # (W-size+1, s, s, 3)
                d = np.abs(rows[:, mask] - tc).mean(axis=(1, 2))
                x = int(d.argmin())
                if best is None or d[x] < best[0]:
                    best = (float(d[x]), x + size // 2, y + size // 2, size)
            if best is not None and best[0] < 20:
                break
        if best is None or best[0] > 40:
            return None
        return x0 + best[1], y0 + best[2], best[0]


# ---------------------------------------------------------------- Tools -> Cleanup Tracks & Vias (the real dialog)
CLEANUP_TITLE = "Cleanup Tracks and Vias"
CLEANUP_ACTION = "pcbnew.GlobalEdit.cleanupTracksAndVias"
CLEANUP_HOTKEY_KICAD, CLEANUP_HOTKEY_X = "Ctrl+Shift+F9", "ctrl+shift+F9"  # unused by KiCad 10's defaults
#: the dialog's "Actions" check boxes in creation order (dialog_cleanup_tracks_and_vias_base.cpp), which is the
#: index of their DIALOG_SHIM control-state keys "wxStaticBox_0wxCheckBox_<i>"; names as harness.cleanup_check.OPTIONS
CLEANUP_BOXES = ("refill", "short", "vias", "dangling_vias", "merge", "unconnected", "in_pads")
#: the KiCad 9 pcbnew.json "cleanup" keys of the same boxes (only for --cleanup-opts pcbnew-json)
_CLEANUP_PCBNEW_JSON = {"refill": "cleanup_refill_zones", "short": "cleanup_short_circuits", "vias": "cleanup_vias",
                        "dangling_vias": "delete_dangling_vias", "merge": "merge_segments",
                        "unconnected": "cleanup_unconnected", "in_pads": "cleanup_tracks_in_pad"}
USER_KICAD_COMMON = Path.home() / ".config/kicad/10.0/kicad_common.json"  # only read
USER_PCBNEW_JSON = Path.home() / ".config/kicad/10.0/pcbnew.json"  # only read


def cleanup_options(spec: str) -> dict:
    """The Cleanup dialog options for `--cleanup-opts`: ``all`` (every box on: the replay's ALL_ON with the refill),
    ``user`` (the boxes as the user last left them in their own KiCad: kicad_common.json dialog.controls, read
    only), ``pcbnew-json`` (the KiCad 9 pcbnew.json "cleanup" section, read only) or a comma list of the box names
    that are on (``refill,short,merge``)."""
    if spec == "all":
        return {k: True for k in CLEANUP_BOXES}
    if spec == "user":
        st = json.loads(USER_KICAD_COMMON.read_text()).get("dialog", {}).get("controls", {}).get(CLEANUP_TITLE, {})
        if not st:
            raise SystemExit(f"no {CLEANUP_TITLE!r} control state in {USER_KICAD_COMMON}")
        return {k: bool(st.get(f"wxStaticBox_0wxCheckBox_{i}", False)) for i, k in enumerate(CLEANUP_BOXES)}
    if spec == "pcbnew-json":
        c = json.loads(USER_PCBNEW_JSON.read_text()).get("cleanup", {})
        return {k: bool(c.get(v, False)) for k, v in _CLEANUP_PCBNEW_JSON.items()}
    on = {s.strip() for s in spec.split(",") if s.strip()}
    bad = on - set(CLEANUP_BOXES)
    if bad:
        raise SystemExit(f"--cleanup-opts: unknown {sorted(bad)}; known {CLEANUP_BOXES}")
    return {k: k in on for k in CLEANUP_BOXES}


def cleanup_control_state(opts: dict) -> dict:
    """DIALOG_SHIM control state of the Cleanup dialog: the action boxes from `opts`, every filter off, a large
    window so the change list shows many rows."""
    st = {f"wxStaticBox_0wxCheckBox_{i}": bool(opts[k]) for i, k in enumerate(CLEANUP_BOXES)}
    st.update({f"wxStaticBox_1wxCheckBox_{i}": False for i in range(4)})  # net / net class / layer / selected only
    st["__geometry"] = {"x": 40, "y": 40, "w": 1100, "h": 980}
    return st


def copy_board(pcb: Path, dst: Path, stem: str | None = None) -> Path:
    """`pcb` with its project / rules / local library tables copied into `dst` (as `stem` when given). O(files)."""
    dst.mkdir(parents=True, exist_ok=True)
    stem = stem or pcb.stem
    for ext in (".kicad_pcb", ".kicad_pro", ".kicad_prl", ".kicad_dru"):
        src = pcb.with_suffix(ext)
        if src.is_file():
            shutil.copy(src, dst / f"{stem}{ext}")
    for n in ("fp-lib-table", "sym-lib-table"):
        if (pcb.parent / n).is_file():
            shutil.copy(pcb.parent / n, dst / n)
    return dst / f"{stem}.kicad_pcb"


def replay(pcb: Path, opts: dict, out_json: Path, env: dict) -> dict:
    """harness.cleanup_check's replay of the same cleanup on `pcb` (own process: pcbnew, KiCad home of `env`), as
    {uuid: item}."""
    prog = ("import json, sys\nfrom pathlib import Path\nfrom harness.cleanup_check import cleanup_items\n"
            "o = json.loads(sys.argv[2])\nrefill = o.pop('refill')\n"
            "items = cleanup_items(Path(sys.argv[1]), refill=refill, opts=o)\n"
            "Path(sys.argv[3]).write_text(json.dumps(items, indent=1))\n")
    env = {**{k: v for k, v in env.items() if k != "DISPLAY"}, "PYTHONPATH": f"{ROOT}:{ROOT / 'plugin'}"}
    r = subprocess.run([sys.executable, "-c", prog, str(pcb), json.dumps(opts), str(out_json)], capture_output=True,
                       text=True, env=env, cwd=ROOT, timeout=REPLAY_TIMEOUT_S)
    if r.returncode:
        raise RuntimeError(f"cleanup_check replay failed: {r.stderr[-1500:]}")
    return {i["uuid"]: i for i in json.loads(out_json.read_text())}


def _by(xs, key) -> dict:
    out: dict = {}
    for x in xs:
        out[key(x)] = out.get(key(x), 0) + 1
    return dict(sorted(out.items()))


def cleanup_step(e: E2E, main: str, pcb: Path, name: str, opts: dict) -> dict:
    """Tools -> Cleanup Tracks & Vias on the open, saved board `pcb` (window `main`), with the options `opts`:
    dialog -> Build Changes (the dry run; "Zone fills are out-of-date" -> Refill) -> screenshot of the change
    list -> Update PCB. Removed / changed items are found by uuid over the API (before vs after), the board is
    saved, kicad-cli DRC runs on copies from before and after, and harness.cleanup_check replays the same options on
    the copy from before: the replay must remove exactly the items the dialog removed. Returns the result dict
    (also checks on `e`). O(two DRC runs + one replay + KiCad's cleanup)."""
    from harness.kicad_env import run_drc
    cdir = e.out / "cleanup"
    if cdir.exists():
        shutil.rmtree(cdir)
    before_pcb = copy_board(pcb, cdir / "before")
    res: dict = {"board": str(pcb), "options": opts}
    snap0 = e.snapshot()
    res["before"] = _by(snap0.values(), lambda v: v["kind"])
    e.log(f"cleanup: board before {res['before']}")
    e.x("xdotool", "windowactivate", "--sync", main)
    x0, y0, w, h = e.geom(main)
    e.x("xdotool", "mousemove", str(x0 + w // 2), str(y0 + h // 2))  # the canvas takes the hotkey
    time.sleep(0.5)
    e.key(main, CLEANUP_HOTKEY_X)
    dlg = e.wait(CLEANUP_TITLE, 30)
    if not dlg:
        e.shot("no-cleanup-dialog")
        e.check("Cleanup Tracks & Vias dialog opens (hotkey)", False)
        return res
    e.check("Cleanup Tracks & Vias dialog opens (hotkey)", True, dlg[0])
    time.sleep(1.5)
    e.x("xdotool", "windowraise", dlg[1])
    time.sleep(0.5)
    res["shot_dialog"] = str(e.shot("cleanup-dialog", dlg[1]))
    answered: list = []

    def other() -> bool:
        """Answers the zone-fill confirmation ("Zone fills are out-of-date. Refill?") with Refill (Enter)."""
        c = e.find("Confirmation")
        if c:
            time.sleep(0.5)
            answered.append(str(e.shot("zone-refill-question", c[1])))
            e.key(c[1], "Return")
            time.sleep(1)
            return True
        return any(e.find(t) for t in ("Check Zones", "Fill All Zones", "Fill Zone"))

    # The dialog's own state, from its pixels (text extents, so focus / backdrop shading does not matter): the
    # "Changes to be applied:" page is back (the run shows the shorter "Progress:") and the OK button no longer
    # reads "Build Changes" ("Update PCB" is narrower, so the Cancel label left of it moves right). Enter is not
    # used: with the focus in the change list it activates a row, not the OK button.
    import numpy as np

    def extent(img, rows, cols):
        m = img[rows, cols].min(axis=2) < 215
        c = np.where(m.any(axis=0))[0]
        return (int(c.min()), int(c.max())) if len(c) else None

    def label_right(img):  # right end of the text above the list
        x = extent(img, slice(255, 276), slice(0, 250))
        return x[1] if x else 0

    def buttons_left(img):  # left end of the button texts ([Cancel] [OK])
        H, W = img.shape[:2]
        x = extent(img, slice(H - 30, H - 14), slice(W - 250, W - 25))
        return x[0] if x else None

    ref = e.grab(dlg[1])
    ref_buttons = buttons_left(ref)

    def dry_run_done(img) -> bool:
        return img is not None and img.shape == ref.shape and label_right(img) > 120 \
            and buttons_left(img) != ref_buttons

    t0 = time.monotonic()
    e.click_rel(dlg[1], -55, -22)  # Build Changes (the dry run)
    time.sleep(2)
    settled = e.settle(dlg[1], 1800, done=dry_run_done, on_other=other)
    res["dry_run_s"] = round(time.monotonic() - t0, 1)
    res["zone_refill_question"] = answered
    e.check("Build Changes: the dry run ends (change list shown)", settled, f"{res['dry_run_s']} s")
    e.x("xdotool", "windowraise", dlg[1])
    time.sleep(0.5)
    res["shot_changes"] = str(e.shot("cleanup-changes", dlg[1]))
    t1 = time.monotonic()
    e.click_rel(dlg[1], -55, -22)  # Update PCB
    gone = False
    while time.monotonic() - t1 < 1800:
        other()
        if not e.find(CLEANUP_TITLE):
            gone = True
            break
        time.sleep(1)
    res["update_s"] = round(time.monotonic() - t1, 1)
    e.check("Update PCB applies the cleanup and closes the dialog", gone, f"{res['update_s']} s")
    if not gone:
        e.shot("cleanup-stuck")
        return res
    time.sleep(2)
    e.x("xdotool", "windowactivate", "--sync", main)
    res["shot_board"] = str(e.shot("board-after-cleanup", main))
    snap1 = e.snapshot()
    removed = {u: snap0[u] for u in snap0 if u not in snap1}
    added = {u: snap1[u] for u in snap1 if u not in snap0}
    changed = {u: snap1[u] for u in snap1 if u in snap0 and snap1[u] != snap0[u]}
    res["after"] = _by(snap1.values(), lambda v: v["kind"])
    res["removed"] = _by(removed.values(), lambda v: v["kind"])
    res["added"], res["changed"] = len(added), len(changed)
    e.log(f"cleanup: removed {res['removed']}, added {len(added)}, changed {len(changed)}; after {res['after']}")
    e.ipc_retry("board.save()")
    time.sleep(1)
    after_pcb = copy_board(pcb, cdir / "after")
    # kicad-cli DRC (zones refilled) of the board before and after the cleanup
    d0 = run_drc(before_pcb, cdir / "drc-before.json")
    d1 = run_drc(after_pcb, cdir / "drc-after.json")
    res["drc"] = {"before": {"unconnected": len(d0.unconnected), "by_type": d0.by_type},
                  "after": {"unconnected": len(d1.unconnected), "by_type": d1.by_type}}
    e.log(f"cleanup: DRC unconnected {len(d0.unconnected)} -> {len(d1.unconnected)}; "
          f"violations {sum(d0.by_type.values())} -> {sum(d1.by_type.values())}")
    # the replay of the same options on the same board
    rp = replay(before_pcb, dict(opts), cdir / "replay-items.json", e.env())
    rule_of = {u: i["rule"] for u, i in rp.items()}
    real_by_rule = _by(removed, lambda u: rule_of.get(u, f"not in replay ({removed[u]['kind']})"))
    res["real_by_rule"] = real_by_rule
    res["replay_by_rule"] = _by(rp.values(), lambda i: i["rule"])
    only_real = sorted(set(removed) - set(rp))
    only_replay = sorted(set(rp) - set(removed))
    merged_into = {i.get("other") for i in rp.values() if i["rule"] == "merge_collinear"}
    res["only_dialog"] = [{"uuid": u, **removed[u]} for u in only_real]
    res["only_replay"] = [rp[u] for u in only_replay]
    res["changed_not_merge_target"] = sorted(set(changed) - merged_into)
    e.log(f"cleanup: dialog removed {len(removed)} {real_by_rule}; replay {len(rp)} {res['replay_by_rule']}")
    e.check("the replay (harness.cleanup_check) removes exactly what the dialog removed",
            not only_real and not only_replay and not added,
            f"dialog {len(removed)}, replay {len(rp)}, only dialog {len(only_real)}, only replay {len(only_replay)}, "
            f"added {len(added)}")
    (cdir / "cleanup.json").write_text(json.dumps(res, indent=1))
    return res


def _kv(s: str):
    k, _, v = s.partition("=")
    sec, _, key = k.partition(".")
    try:
        val = json.loads(v)
    except ValueError:
        val = v
    return sec, key, val


def run_existing(a, e: E2E) -> int:
    """--existing: open a copy of a routed board (no routing, no plugin install) and run only the cleanup step."""
    src = Path(a.board).resolve()
    name = a.name or src.stem
    live = copy_board(src, e.out / "boards" / f"{name}-existing")
    e.log(f"existing routed board {src} -> copy {live}")
    e.make_home()
    started_x = e.start_x()
    result: dict = {"board": name, "source": str(src), "input": str(live), "mode": "existing", "checks": e.checks}
    ok = False
    try:
        e.start_kicad(live)
        main = e.wait(f"{live.stem} — PCB Editor", 180)
        e.check("KiCad PCB editor opened the board", bool(main), main[0] if main else "")
        if not main:
            e.shot("no-editor")
            return 1
        time.sleep(3)
        try:  # the board is loaded and the API server is up
            e.ipc_retry("out = {'n': len(list(board.get_vias()))}", tries=120, connect=True)
        except RuntimeError as ex:
            e.shot("no-api")
            e.check("KiCad API answers", False, str(ex).strip().splitlines()[-1])
            return 1
        main = e.wait(f"{live.stem} — PCB Editor", 10)
        e.x("xdotool", "windowactivate", "--sync", main[1])
        time.sleep(1)
        e.shot("editor", main[1])
        result["cleanup"] = cleanup_step(e, main[1], live, name, e.cleanup_opts)
        ok = all(c["ok"] for c in e.checks)
        return 0 if ok else 1
    finally:
        result["ok"] = ok
        (e.out / f"{name}-e2e.json").write_text(json.dumps(result, indent=1, default=str))
        if not a.keep:
            e.stop(keep_x=not started_x)
        e.log(f"result {'PASS' if ok else 'FAIL'}: {e.out / (name + '-e2e.json')}")


def run(a) -> int:
    out = Path(a.out_dir).resolve()
    out.mkdir(parents=True, exist_ok=True)
    opts = cleanup_options(a.cleanup_opts) if (a.cleanup or a.existing) else None
    e = E2E(out, a.display, opts)
    sys.path[:0] = [str(ROOT), str(ROOT / "plugin")]
    if opts is not None:
        e.log(f"cleanup options ({a.cleanup_opts}): {opts}")
    if a.existing:
        return run_existing(a, e)
    from harness import strip
    name = a.name or (Path(a.board).stem if a.board.endswith(".kicad_pcb") else a.board)
    boards = out / "boards"
    for d in (boards / f"{name}-input", boards / f"{name}-hand"):
        if d.exists():
            shutil.rmtree(d)
    hand, inp, rep = strip.prepare(a.board, boards, name=name)
    orig_input = boards / f"{name}-input.orig.kicad_pcb"
    shutil.copy(inp.pcb, orig_input)
    e.log(f"input {inp.pcb}: stripped {rep.tracks} tracks, {rep.arcs} arcs, {rep.vias} vias")
    live = inp.pcb  # the board KiCad opens and topor routes
    if a.from_hand:
        # the real-use path matching the harness: a hand-routed copy, Rip up and re-route (track widths from the
        # existing copper, as route_board takes them from the hand board), everything ripped as strip does
        d = boards / f"{name}-live"
        if d.exists():
            shutil.rmtree(d)
        shutil.copytree(hand.pcb.parent, d)
        live = d / hand.pcb.name
        a.toml = ["run.existing_copper=\"rip\"", "run.rip_free_vias=true", "run.respect_locked=false", *a.toml]
    if a.toml:
        from topor.config import Config, render_toml
        cfg = Config.defaults().with_values({(s, k): v for s, k, v in map(_kv, a.toml)})
        (live.parent / "topor.toml").write_text(render_toml(cfg))
        e.log(f"topor.toml written with {a.toml}")
    # the settings the run really uses: the board's own topor.toml (an old one may pin refill_after_accept = false),
    # else the defaults plus --toml; the file is read before the plugin may create one
    from topor.config import Config as _Config, load_config as _load_config
    if (live.parent / "topor.toml").is_file():
        eff_cfg, _ = _load_config(live.parent)
    else:
        eff_cfg = _Config.defaults()
    e.log(f"effective settings from {'the board topor.toml' if (live.parent / 'topor.toml').is_file() else 'defaults'}: "
          f"refill_after_accept={eff_cfg.get('run', 'refill_after_accept')} commit_fills={eff_cfg.get('run', 'commit_fills')}")
    e.make_home()
    e.install(a.wheel)
    started_x = e.start_x()
    result: dict = {"board": name, "input": str(live), "hand": str(hand.pcb), "from_hand": bool(a.from_hand),
                    "checks": e.checks}
    ok = False
    try:
        e.start_kicad(live)
        stem = live.stem
        main = e.wait(f"{stem} — PCB Editor", 120)
        e.check("KiCad PCB editor opened the board", bool(main), main[0] if main else "")
        if not main:
            e.shot("no-editor")
            return 1
        t0 = time.monotonic()
        while not e.env_ready() and time.monotonic() - t0 < 300:
            time.sleep(2)
        time.sleep(5)  # KiCad marks the environment ready after pip returns
        e.check("plugin environment created by KiCad", e.env_ready(), str(e.env_dir()))
        try:  # what KiCad's API reports as each net's effective netclass (the plugin's rule input)
            ncs = e.ipc_retry(NETCLASS_DUMP, tries=60, connect=True)
            (e.out / "netclasses-ipc.json").write_text(json.dumps(ncs, indent=1, sort_keys=True))
            e.log(f"netclasses over the API: {len(ncs)} nets -> {e.out / 'netclasses-ipc.json'}")
        except RuntimeError as ex:
            e.log(f"netclass dump failed: {str(ex).strip().splitlines()[-1]}")
        main = e.wait(f"{stem} — PCB Editor", 10)
        e.x("xdotool", "windowactivate", "--sync", main[1])
        time.sleep(1)
        e.shot("editor", main[1])
        btn = e.toolbar_button(main[1])
        e.check("Topor button in the toolbar", btn is not None, f"at {btn[:2]} (match {btn[2]:.1f})" if btn else "")
        if not btn:
            return 1
        e.click_abs(btn[0], btn[1])
        first = e.wait("Topor — Settings file created", 5 if a.toml else 60)
        if not a.toml:
            e.check("first-run notice (topor.toml created)", bool(first) and (live.parent / "topor.toml").is_file())
        if first:
            e.shot("first-run", first[1])
            e.key(first[1], "Return")
        pre = e.wait("Topor — Route", 180)
        e.check("pre-flight dialog", bool(pre), pre[0] if pre else "")
        if not pre:
            e.shot("no-preflight")
            err = e.find("Topor — Error")
            if err:
                e.shot("error", err[1])
            return 1
        time.sleep(0.5)
        e.shot("preflight", pre[1])
        e.key(pre[1], "Return")
        fills_before = e.zone_fills()
        counts_run_start = e.counts()
        editor_titles = lambda: sorted(t for t in e.windows() if t.endswith("PCB Editor"))  # "*name — PCB Editor" = modified
        title_run_start = editor_titles()
        text_run_start = live.read_bytes()
        e.log(f"editor title before the run: {title_run_start} (dirty flag already set here = not ours)")
        e.log(f"zones before the run: {len(fills_before)}")
        t_route = time.monotonic()
        state = {"last": -10.0, "cancelled": False, "img_t": -10.0, "imgs": []}

        def tick(dt):
            prog = e.find("Topor — Routing")
            if prog and dt - state["last"] >= 10:
                state["last"] = dt
                e.shot(f"progress-{int(dt):04d}s", prog[1])
            if prog and a.preview_check and not state["cancelled"] and dt - state["img_t"] >= 2.5:
                # last-preview: the progress dialog's board image (its ground is preview.LIVE_BOARD_RGB): one crop
                # every 2.5 s; the image must change between rounds
                state["img_t"] = dt
                crop = e.board_image(prog[1])
                if crop is not None:
                    state["imgs"].append((round(dt, 1), hash(crop.tobytes())))
                    if len(state["imgs"]) <= 2 or len({h for _, h in state["imgs"]}) == 2:
                        e.shot(f"progress-image-{int(dt):04d}s", prog[1])
            if (prog and a.resize_progress_at is not None and not state.get("resized")
                    and dt >= a.resize_progress_at and not state["cancelled"]):
                # wide progress dialog: shot of the first size, then resize the window and shot again
                state["resized"] = True
                e.shot("progress-before-resize", prog[1])
                e.x("xdotool", "windowsize", prog[1], *a.resize_progress_to.split("x"))
                time.sleep(1.5)
                e.shot("progress-after-resize", prog[1])
                e.log(f"progress dialog resized to {a.resize_progress_to} at {dt:.1f} s")
            if prog and a.ok_at_stable and not state["cancelled"] and "Routes stable" in prog[0]:
                # M-B0: the dialog says "Routes stable"; click Accept (Enter presses nothing)
                time.sleep(1.0)  # let the label repaint for the screenshot
                e.shot("routes-stable", prog[1])
                e.click_accept(prog[1])
                state["cancelled"] = True
                state["cancel_t"] = dt
                state["ok"] = True
                time.sleep(0.3)
                e.shot("accept-pressed", prog[1])
                e.log(f"Routes stable at {dt:.1f} s: Accept pressed")
            if prog and a.pause_at is not None and not state["cancelled"] and not state.get("paused") \
                    and not state.get("resumed") and dt >= a.pause_at:
                # pause-button: running -> Pause -> the plugin uses no CPU, the dialog says paused -> Resume
                e.shot("pause-running", prog[1])
                e.click_pause(prog[1])
                state["paused"], state["pause_t"] = True, dt
                time.sleep(2.0)  # the engine waits at its next stop test within about a second
                pid = e.plugin_pid()
                c0 = e.cpu_ticks(pid) if pid else 0
                time.sleep(2.0)
                cores = ((e.cpu_ticks(pid) - c0) / 100.0 / 2.0) if pid else None
                state["paused_cores"] = cores
                e.shot("pause-paused", prog[1])
                e.log(f"Pause clicked at {dt:.1f} s; plugin CPU while paused: {cores} cores (pid {pid})")
            if prog and state.get("paused") and dt >= state["pause_t"] + a.resume_after:
                e.shot("pause-before-resume", prog[1])
                e.click_pause(prog[1])
                state["paused"], state["resumed"] = False, dt
                time.sleep(3.0)
                e.shot("pause-resumed", prog[1])
                e.log(f"Resume clicked at {dt:.1f} s")
            if prog and a.accept_after is not None and not state["cancelled"] and dt >= a.accept_after \
                    and not state.get("paused"):
                e.shot("before-accept", prog[1])
                e.click_accept(prog[1])
                state["cancelled"] = True
                state["cancel_t"] = dt
                time.sleep(0.5)
                e.shot("accepting", prog[1])
                e.log(f"Accept pressed at {dt:.1f} s")
            if prog and a.cancel_after is not None and not state["cancelled"] and dt >= a.cancel_after:
                e.shot("before-cancel", prog[1])
                e.key(prog[1], "Escape")
                state["cancelled"] = True
                state["discard"] = True
                state["cancel_t"] = dt
                time.sleep(0.5)
                e.shot("cancelling", prog[1])
                e.log(f"Cancel pressed at {dt:.1f} s")

        nxt = None
        while time.monotonic() - t_route < a.max_wait:
            tick(time.monotonic() - t_route)
            for t in ("Topor — Preview", "Topor — Finished", "Topor — Error"):
                f = e.find(t)
                if f:
                    nxt = f
                    break
            if nxt:
                break
            if state.get("discard") and not e.find("Topor — Routing"):
                break  # Cancel: the dialog closed and no result window follows
            time.sleep(0.5)
        result["route_wall_s"] = round(time.monotonic() - t_route, 1)
        if a.pause_at is not None:
            rds = sorted((e.home / ".cache/kicad/10.0/topor").glob(f"{live.stem}/*/log.txt"))
            lg = rds[-1].read_text() if rds else ""
            cores = state.get("paused_cores")
            e.check("Pause: the plugin used about no CPU while paused", cores is not None and cores < 0.1,
                    f"{cores} cores")
            e.check("Pause / Resume reached the engine (log.txt)", "Pause pressed at" in lg and "Resume pressed" in lg,
                    str(rds[-1]) if rds else "no log.txt")
            result["paused_cores"] = cores
        if a.preview_check:
            hashes = [h for _, h in state["imgs"]]
            result["progress_images"] = state["imgs"]
            e.check("progress dialog shows a board image", bool(hashes), f"{len(hashes)} crops")
            e.check("the board image changes between rounds", len(set(hashes)) >= 2,
                    f"{len(set(hashes))} different images in {len(hashes)} crops")
        if a.ok_at_stable:
            e.check("progress dialog showed Routes stable (Accept pressed there)", bool(state.get("ok")))
        if state.get("discard"):
            # Cancel: stop at once, take nothing: no result window, no error, the board exactly as before the run
            result["cancel_reaction_s"] = round(time.monotonic() - t_route - state["cancel_t"], 1)
            time.sleep(2)
            stray = [t for t in e.windows() if t.startswith(("Topor — Preview", "Topor — Finished", "Topor — Error"))]
            e.check("Cancel closes the progress dialog at once, no preview / error window", nxt is None and not stray,
                    f"{result['cancel_reaction_s']} s after Cancel; {stray}")
            e.shot("after-cancel")
            e.check("Cancel leaves the copper unchanged", e.counts() == counts_run_start, f"{counts_run_start} -> {e.counts()}")
            # no dirty flag and no undo entry of ours: the editor title is what it was before the run (a '*' only if it
            # was there before), the plugin committed nothing, refilled nothing, ran no action, the file on disk is untouched
            e.check("Cancel: editor title (dirty flag) unchanged by the run", editor_titles() == title_run_start,
                    f"{title_run_start} -> {editor_titles()}")
            e.check("Cancel: board file on disk untouched", live.read_bytes() == text_run_start)
            rds = sorted((e.home / ".cache/kicad/10.0/topor").glob(f"{live.stem}/*/log.txt"))
            lg = rds[-1].read_text() if rds else ""
            e.check("Cancel: the run log shows no commit / refill / cleanup (no undo entry)",
                    bool(lg) and "committed:" not in lg and "zone refill" not in lg and "cancelled by the user" in lg,
                    str(rds[-1]) if rds else "no log.txt")
            ok = all(c["ok"] for c in e.checks)
            return 0 if ok else 1
        if state["cancelled"]:
            result["cancel_reaction_s"] = round(time.monotonic() - t_route - state["cancel_t"], 1)
            e.check("Accept ends the routing (best result or nothing, no error)", bool(nxt) and not nxt[0].startswith("Topor — Error"), f"{result['cancel_reaction_s']} s after Accept")
        if not nxt:
            e.check("preview after routing", False, f"nothing after {a.max_wait} s")
            e.shot("stuck")
            return 1
        time.sleep(1.0)
        e.shot(nxt[0].split(":")[0].replace("Topor — ", "").lower().replace(" ", "-"), nxt[1])
        if not nxt[0].startswith("Topor — Preview"):
            e.check("preview after routing", False, nxt[0])
            e.key(nxt[1], "Return")
            return 1
        e.check("preview after routing", True, f"{result['route_wall_s']} s after Route")
        before = e.counts()
        fills0 = fills_before
        if a.reject:  # Esc = Reject: the dialog closes, no finished window, the board is untouched
            e.key(nxt[1], "Escape")
            e.check("Reject closes the preview", e.wait_gone("Topor — Preview", 30))
            time.sleep(4)  # a finished window would show up within this
            stray = [t for t in e.windows() if t.startswith(("Topor — Finished", "Topor — Error"))]
            e.check("Reject: no 'Topor — Finished' / error window", not stray, str(stray))
            e.shot("after-reject")
            e.check("Reject leaves the copper unchanged", e.counts() == before, f"{before} -> {e.counts()}")
            ok = all(c["ok"] for c in e.checks)
            return 0 if ok else 1
        e.key(nxt[1], "Return")  # Enter = Accept
        e.check("Accept closes the preview", e.wait_gone("Topor — Preview", 60))
        time.sleep(4)  # no finished window (user 2026-11-19): nothing "Topor — ..." may appear after the commit
        stray = [t for t in e.windows() if t.startswith(("Topor — Finished", "Topor — Error"))]
        e.check("Accept: no 'Topor — Finished' / error window", not stray, str(stray))
        after = e.counts()
        e.check("Accept changed the copper", after != before, f"{before} -> {after}")
        refill_on = bool(eff_cfg.get("run", "refill_after_accept"))
        commit_on = bool(eff_cfg.get("run", "commit_fills"))
        fills1 = e.zone_fills()
        e.check("zones present", bool(fills1), f"{len(fills1)} zones")
        stale = [u for u in fills1 if fills1[u]["layers"] == fills0.get(u, {}).get("layers")]
        e.log(f"zones with the same fill as before the run: {len(stale)}/{len(fills1)}")
        if refill_on or commit_on:
            e.check("zone fills changed by Accept (not the pre-run pours)", len(stale) < len(fills1),
                    f"{len(stale)}/{len(fills1)} zones unchanged")
        main = e.wait(f"{stem} — PCB Editor", 30)
        if not main:
            e.shot("no-editor-after-accept")
            e.check("PCB editor window after Accept", False, str(sorted(e.windows())))
            return 1
        e.x("xdotool", "windowactivate", "--sync", main[1])
        e.x("xdotool", "mousemove", "1000", "600")
        canvas_a = e.shot("canvas-after-accept", main[1])
        drc_a = e.gui_drc(main[1], "after-accept")  # KiCad's DRC on the pours as Accept left them (no refill)
        e.x("xdotool", "windowactivate", "--sync", main[1])
        e.x("xdotool", "mousemove", "1000", "600")
        steps = 2 if refill_on else 1  # the zone refill is its own undo step (run.refill_after_accept)
        for i_ in range(steps):
            e.key(main[1], "ctrl+z")
            time.sleep(2)
            if steps == 2 and i_ == 0:  # the refill is its own undo step: after one Ctrl+Z the routes are still there
                mid = e.counts()
                e.check("the 1st Ctrl+Z undoes only the refill (routes still on the board)", mid == after,
                        f"{mid} vs {after}")
        undone = e.counts()
        e.check(f"{steps} Ctrl+Z remove everything Topor added ({steps} undo step{'s' if steps > 1 else ''})",
                undone == before, f"{undone}")
        for _ in range(steps):
            e.key(main[1], "ctrl+y")
            time.sleep(2)
        fills2 = e.zone_fills()
        e.check("redo brings the same zone fills back", fills2 == fills1)
        e.shot("after-redo", main[1])
        redone = e.counts()
        e.check("Ctrl+Y (Edit → Redo) brings it back", redone == after, f"{redone}")
        e.shot("board-after-accept", main[1])
        # the pours the user sees now must be what KiCad's own filler makes (= what B would give)
        if True:  # without refill and commit_fills this FAILS: that is the stale-pours symptom
            e.ipc_retry("board.refill_zones(max_poll_seconds=600)", tries=120)
            ref = e.zone_fills()
            bad = []
            for u, z in fills2.items():
                for l, (n, a_) in z["layers"].items():
                    r_ = ref.get(u, {"layers": {}})["layers"].get(l) or [0, 0.0]
                    if abs(a_ - r_[1]) > max(0.02, 0.005 * r_[1]):
                        bad.append(f"{z['net']} layer {l}: {a_} mm2 vs refill {r_[1]} mm2")
            result["fills_vs_refill"] = bad
            e.check("pours after Accept = pours after a fresh refill (B)", not bad, "; ".join(bad[:4]))
            time.sleep(2)
            canvas_b = e.shot("canvas-after-refill", main[1])
            crop = ["-crop", "1300x850+345+120", "+repage"]
            diff = subprocess.run(["compare", "-metric", "AE", "-fuzz", "8%", "(", str(canvas_a), *crop, ")", "(",
                                   str(canvas_b), *crop, ")", "null:"], capture_output=True, text=True).stderr.split()
            npx = int(float(diff[0])) if diff else -1
            result["canvas_diff_px"] = npx
            e.check("the canvas shows the pours Accept left = the pours after a fresh refill (pixel diff)",
                    0 <= npx < 6000, f"{npx} px of {1300 * 850} differ; {canvas_a} vs {canvas_b}")
            drc_b = e.gui_drc(main[1], "after-refill")
            same = subprocess.run(["compare", "-metric", "AE", str(drc_a), str(drc_b), "null:"],
                                  capture_output=True, text=True).stderr.strip().split()[0:1] == ["0"]
            e.check("KiCad's own DRC counts after Accept = after a fresh refill (tab labels pixel-identical)", same,
                    f"{drc_a} vs {drc_b}")
        e.ipc("board.save()")
        time.sleep(1)
        rds = sorted((e.home / ".cache/kicad/10.0/topor").glob(f"{stem}/*/result.json"))
        report_dir = rds[-1].parent if rds else None
        from harness.board_metrics import board_metrics
        m = board_metrics(live, orig_input, hand.pcb, work=out / f"{name}-metrics-work", report_dir=report_dir,
                          png=out / f"{name}-gui-compare.png", name=name)
        result["metrics"] = m
        if a.preview_check and report_dir is not None:
            log_txt = (report_dir / "log.txt").read_text() if (report_dir / "log.txt").is_file() else ""
            shown = [ln.split("progress image: ", 1)[1] for ln in log_txt.splitlines() if "progress image: last" in ln]
            rounds = {tuple(x.split(", ")[1:3]) for x in shown}
            result["progress_image_lines"] = shown[:50]
            e.check("the run flow handed over last-round images of different rounds (log.txt)", len(rounds) >= 2,
                    f"{len(shown)} images, {len(rounds)} distinct run/round")
        if a.ok_at_stable and report_dir is not None:
            # the board is the best result the dialog showed at Accept (run.py logs it), and an Accept at stable is not a cancel
            import re as _re
            log_txt = (report_dir / "log.txt").read_text() if (report_dir / "log.txt").is_file() else ""
            mo = _re.search(r"OK at Routes stable: best so far (\d+)/(\d+) routed", log_txt)
            res = json.loads(rds[-1].read_text())
            # "Routes stable" waits for a pick (user 2026-11-17): Best so far is never empty at the OK; with the TopoR
            # loop it is the loops' predicted pick (every planned connection is drawn: never drop), so routed matches
            e.check("Accept keeps the best result shown (stop: stable, not cancelled)",
                    bool(mo) and int(mo.group(2)) > 0 and int(mo.group(1)) == res.get("routed")
                    and res.get("stop_reason") == "stable" and not res.get("cancelled"),
                    f"shown {mo.group(0) if mo else None}; result routed {res.get('routed')}, stop {res.get('stop_reason')}, "
                    f"cancelled {res.get('cancelled')}")
        e.check("result accepted, routing_new 0", m.get("status") == "accepted" and m["routing_new"] == 0,
                f"routed {m.get('routed')}/{m.get('connections')}, unconnected {m['unconnected']} "
                f"(hand {m['unconnected_hand']}), routing_new {m['routing_new']}, vias {m['vias']}")
        if a.harness:
            h = json.loads(Path(a.harness).read_text())
            keys = ("routed", "connections", "unconnected", "routing_new", "vias")
            cmp = {k: [m.get(k), h.get(k)] for k in keys}
            result["vs_harness"] = cmp
            e.check("same numbers as tools/route_board.sh", all(x == y for x, y in cmp.values()),
                    ", ".join(f"{k} {x} vs {y}" for k, (x, y) in cmp.items()))
        if a.cleanup:
            main = e.wait(f"{stem} — PCB Editor", 30)
            if not main:
                e.shot("no-editor-before-cleanup")
                e.check("PCB editor window before the cleanup", False, str(sorted(e.windows())))
                return 1
            result["cleanup"] = cleanup_step(e, main[1], live, name, opts)
        ok = all(c["ok"] for c in e.checks)
        return 0 if ok else 1
    finally:
        result["ok"] = ok
        (out / f"{name}-e2e.json").write_text(json.dumps(result, indent=1, default=str))
        if not a.keep:
            e.stop(keep_x=not started_x)
        e.log(f"result {'PASS' if ok else 'FAIL'}: {out / (name + '-e2e.json')}")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("board", help="BENCH_BOARDS key or .kicad_pcb path (only read)")
    ap.add_argument("out_dir")
    ap.add_argument("--display", default=":6")
    ap.add_argument("--wheel", default=None, help="prebuilt topor_engine wheel (default: tools/build.sh builds one)")
    ap.add_argument("--cancel-after", type=float, default=None,
                    help="press Cancel (Esc) this many seconds after Route: the run ends at once, nothing is taken, the board is unchanged")
    ap.add_argument("--accept-after", type=float, default=None,
                    help="click Accept this many seconds after Route: stop, keep the best, the preview follows")
    ap.add_argument("--pause-at", type=float, default=None, metavar="S",
                    help="pause-button: click Pause S seconds into the routing, check the plugin's CPU, then Resume")
    ap.add_argument("--resume-after", type=float, default=10.0, metavar="S", help="seconds paused (with --pause-at)")
    ap.add_argument("--reject", action="store_true", help="press Esc (Reject) in the preview and check the board is untouched")
    ap.add_argument("--ok-at-stable", action="store_true",
                    help="press Accept as soon as the progress dialog says Routes stable (M-B0) and check the result")
    ap.add_argument("--max-wait", type=float, default=None,
                    help="longest wait for the preview after Route (s); default max(1800, run.timeout_s from --toml + "
                         f"{DRAW_MARGIN_S:g}): the limit counts optimizing time only, the final drawing, KiCad's DRC and "
                         "the preview come after it (time-limit-fix r2)")
    ap.add_argument("--preview-check", action="store_true",
                    help="last-preview: check the progress dialog's board image (shown, changes between rounds; log)")
    ap.add_argument("--resize-progress-at", type=float, default=None, metavar="S",
                    help="resize the progress dialog this many seconds after Route (screenshots before / after)")
    ap.add_argument("--resize-progress-to", default="1500x800", metavar="WxH")
    ap.add_argument("--harness", default=None, help="route_board metrics JSON of the same board to compare with")
    ap.add_argument("--from-hand", action="store_true",
                    help="open a copy of the hand-routed board and Rip up and re-route (default: the stripped copy)")
    ap.add_argument("--toml", action="append", default=[], metavar="SECTION.KEY=VALUE",
                    help="write a topor.toml with this setting before the run (no first-run notice then)")
    ap.add_argument("--cleanup", action="store_true",
                    help="after the routed board is saved: Tools -> Cleanup Tracks & Vias in the real dialog, DRC "
                         "before / after, compared with the harness.cleanup_check replay")
    ap.add_argument("--existing", action="store_true",
                    help="`board` is an already routed .kicad_pcb: open a copy of it (no routing, no plugin install) "
                         "and run only the cleanup step")
    ap.add_argument("--cleanup-opts", default="all", metavar="all|user|pcbnew-json|BOX,BOX",
                    help="the dialog's check boxes: all (default, the replay's all-on with refill), user (as left in "
                         "your own KiCad, read only), pcbnew-json (KiCad 9 store) or the boxes that are on, of "
                         + ",".join(CLEANUP_BOXES))
    ap.add_argument("--keep", action="store_true", help="leave KiCad and Xephyr running")
    ap.add_argument("--name", default=None)
    a = ap.parse_args(argv)
    if a.max_wait is None:
        a.max_wait = default_max_wait(a.toml)
    return run(a)


def default_max_wait(toml: list) -> float:
    """time-limit-fix r2: the wait for the preview after Route: at least 1800 s, and run.timeout_s (from --toml;
    default 600) plus DRAW_MARGIN_S, since the limit counts optimizing time only and the final drawing, KiCad's DRC
    and the preview export follow it. O(len(toml))."""
    limit = 600.0
    for kv in toml or []:
        k, _, v = kv.partition("=")
        if k.strip() == "run.timeout_s":
            try:
                limit = float(v.strip().strip('"'))
            except ValueError:
                pass
    return max(1800.0, limit + DRAW_MARGIN_S) if limit > 0 else max(1800.0, 4 * 3600.0)


if __name__ == "__main__":
    sys.exit(main())
