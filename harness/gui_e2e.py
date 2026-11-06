"""Live end-to-end test of the plugin in a real KiCad 10, on a nested X display, without touching the user's KiCad.

    tools/gui_e2e.sh <board|pcb> <out_dir> [--display :6] [--wheel WHEEL] [--cancel-after S] [--keep]
                     [--harness METRICS_JSON] [--toml SECTION.KEY=VALUE ...] [--from-hand] [--name N]

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
     a topor.toml), pre-flight -> Route, progress (screenshots; with --cancel-after S presses Cancel after S seconds), preview -> Accept,
     finished -> Close.
  5. Over the KiCad API (an external kipy client in the plugin environment): counts the tracks / vias after
     Accept, presses Ctrl+Z (one undo step must remove them all), Ctrl+Shift+Z (back), saves the board.
  6. `harness.board_metrics` judges the saved board with kicad-cli DRC (unconnected, routing_new, vias) and
     renders <name>-gui-compare.png (hand left, KiCad result right, open ratsnest blue); with --harness the
     numbers are compared with a `tools/route_board.sh` metrics JSON of the same board.
  7. Stops KiCad and (unless it was running before or --keep) Xephyr.

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
ICON = ROOT / "plugin" / "icons" / "topor-light-24.png"
PLUGIN_ID = "org.topor.router"


class E2E:
    def __init__(self, out: Path, display: str):
        self.out = out
        self.display = display
        self.home = out / "home"
        self.shots = out / "shots"
        self.shots.mkdir(parents=True, exist_ok=True)
        self.logf = (out / "e2e.log").open("a")
        self.checks: list = []
        self.procs: list = []
        self.n = 0

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

    def env(self) -> dict:
        h = str(self.home)
        e = dict(os.environ)
        e.update(HOME=h, XDG_CONFIG_HOME=f"{h}/.config", XDG_DATA_HOME=f"{h}/.local/share",
                 XDG_CACHE_HOME=f"{h}/.cache", XDG_STATE_HOME=f"{h}/.local/state", TMPDIR=f"{h}/tmp",
                 XDG_RUNTIME_DIR=f"{h}/run", KICAD_CONFIG_HOME=f"{h}/.config/kicad", DISPLAY=self.display,
                 GDK_BACKEND="x11", NO_AT_BRIDGE="1", RAYON_NUM_THREADS=e.get("RAYON_NUM_THREADS", "4"))
        for k in ("DBUS_SESSION_BUS_ADDRESS", "KICAD_API_SOCKET", "KICAD_API_TOKEN", "TOPOR_REPORT_ROOT"):
            e.pop(k, None)
        return e

    def x(self, *args, check=False) -> str:
        r = subprocess.run(list(args), capture_output=True, text=True, env={**os.environ, "DISPLAY": self.display})
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
                       capture_output=True)
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
        kc.write_text(json.dumps(d, indent=1))
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

    # ---------------------------------------------------------------- KiCad
    def env_ready(self) -> bool:
        sp = list(self.env_dir().glob("lib/python3*/site-packages"))
        return bool(sp) and (sp[0] / "topor_engine").is_dir() and (sp[0] / "kipy").is_dir()

    def ipc(self, code: str) -> dict:
        """Run `code` (defines `out` dict, has `board`) in the plugin environment as an external API client."""
        sock = self.home / "tmp" / "kicad" / "api.sock"
        prog = ("import json\nfrom kipy import KiCad\nk = KiCad(socket_path='ipc://" + str(sock) + "', client_name='e2e')\n"
                "board = k.get_board()\nout = {}\n" + code + "\nprint(json.dumps(out))\n")
        r = subprocess.run([str(self.env_python()), "-c", prog], capture_output=True, text=True, timeout=120,
                           env={k: v for k, v in self.env().items() if k != "PYTHONPATH"})
        if r.returncode:
            raise RuntimeError(f"IPC client failed: {r.stderr[-1500:]}")
        return json.loads(r.stdout.strip().splitlines()[-1])

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


def _kv(s: str):
    k, _, v = s.partition("=")
    sec, _, key = k.partition(".")
    try:
        val = json.loads(v)
    except ValueError:
        val = v
    return sec, key, val


def run(a) -> int:
    out = Path(a.out_dir).resolve()
    out.mkdir(parents=True, exist_ok=True)
    e = E2E(out, a.display)
    sys.path[:0] = [str(ROOT), str(ROOT / "plugin")]
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
        t_route = time.monotonic()
        state = {"last": -10.0, "cancelled": False}

        def tick(dt):
            prog = e.find("Topor — Routing")
            if prog and dt - state["last"] >= 10:
                state["last"] = dt
                e.shot(f"progress-{int(dt):04d}s", prog[1])
            if prog and a.cancel_after is not None and not state["cancelled"] and dt >= a.cancel_after:
                e.shot("before-cancel", prog[1])
                e.key(prog[1], "Escape")
                state["cancelled"] = True
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
            time.sleep(0.5)
        result["route_wall_s"] = round(time.monotonic() - t_route, 1)
        if state["cancelled"]:
            result["cancel_reaction_s"] = round(time.monotonic() - t_route - state["cancel_t"], 1)
            e.check("Cancel ends the routing (best result or nothing, no error)", bool(nxt) and not nxt[0].startswith("Topor — Error"), f"{result['cancel_reaction_s']} s after Cancel")
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
        e.key(nxt[1], "Return")  # Enter = Accept
        fin = e.wait("Topor — Finished", 60)
        e.check("Enter in the preview = Accept -> finished notice", bool(fin))
        if not fin:
            prev = e.find("Topor — Preview")
            if prev:  # fall back to the Accept mnemonic so the rest of the test still runs
                e.key(prev[1], "alt+a")
                fin = e.wait("Topor — Finished", 60)
        if not fin:
            return 1
        e.shot("finished", fin[1])
        e.key(fin[1], "Return")
        e.wait_gone("Topor — Finished", 10)
        after = e.counts()
        e.check("Accept changed the copper", after != before, f"{before} -> {after}")
        main = e.find(f"{stem} — PCB Editor")
        e.x("xdotool", "windowactivate", "--sync", main[1])
        e.x("xdotool", "mousemove", "1000", "600")
        e.key(main[1], "ctrl+z")
        time.sleep(2)
        undone = e.counts()
        e.check("one Ctrl+Z removes everything Topor added (one undo step)", undone == before, f"{undone}")
        e.key(main[1], "ctrl+y")
        time.sleep(2)
        e.shot("after-redo", main[1])
        redone = e.counts()
        e.check("Ctrl+Y (Edit → Redo) brings it back", redone == after, f"{redone}")
        e.shot("board-after-accept", main[1])
        e.ipc("board.save()")
        time.sleep(1)
        rds = sorted((e.home / ".cache/kicad/10.0/topor").glob(f"{stem}/*/result.json"))
        report_dir = rds[-1].parent if rds else None
        from harness.board_metrics import board_metrics
        m = board_metrics(live, orig_input, hand.pcb, work=out / f"{name}-metrics-work", report_dir=report_dir,
                          png=out / f"{name}-gui-compare.png", name=name)
        result["metrics"] = m
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
    ap.add_argument("--cancel-after", type=float, default=None, help="press Cancel this many seconds after Route")
    ap.add_argument("--max-wait", type=float, default=1800, help="longest wait for the preview after Route (s)")
    ap.add_argument("--harness", default=None, help="route_board metrics JSON of the same board to compare with")
    ap.add_argument("--from-hand", action="store_true",
                    help="open a copy of the hand-routed board and Rip up and re-route (default: the stripped copy)")
    ap.add_argument("--toml", action="append", default=[], metavar="SECTION.KEY=VALUE",
                    help="write a topor.toml with this setting before the run (no first-run notice then)")
    ap.add_argument("--keep", action="store_true", help="leave KiCad and Xephyr running")
    ap.add_argument("--name", default=None)
    return run(ap.parse_args(argv))


if __name__ == "__main__":
    sys.exit(main())
