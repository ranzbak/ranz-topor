"""Topor application wiring (B10): engine handshake, KiCad IPC connection, UI, ``run``.

Usage
-----
KiCad runs ``plugin/topor_plugin.py`` (-> :func:`main`) as a separate Python process with ``KICAD_API_SOCKET`` and
``KICAD_API_TOKEN`` in the environment; no arguments = GUI mode (IPC board + wx dialogs).

Headless (no KiCad, no window; ``FakeBoard`` + ``ScriptedUi``; used by tests and for debugging)::

    python -m topor.app --headless <board.kicad_pcb> --model <model.json> [--accept] [--selection JSON]

``--model`` is a BoardModel JSON (harness ``export_model`` / IPC exporter output); ``--accept`` answers the
preview with Accept (default Reject); ``--selection`` is JSON ``{"pads": [["U1","1"]], "footprints": ["U1"],
"tracks": [uuid], "vias": [uuid], "area": [[x_nm, y_nm], ...]}``. ``topor.toml`` next to the board is used if
present, else the defaults. Reports go to ``$TOPOR_REPORT_ROOT`` (default ``~/.cache/kicad/10.0/topor``). Run
with ``PYTHONPATH`` containing the repo root and ``plugin/``.

Exit codes: 0 = accepted (also when a step after the commit failed: POST_COMMIT_STEP_FAILED) / rejected /
cancelled / aborted at pre-flight; 1 = error, shown in an error dialog in GUI mode: ENGINE_VERSION,
KICAD_API_CONNECT, REPORT_DIR_UNWRITABLE, INTERNAL (an exception, or a panic of the engine's routing thread, which
``RouteJob.wait`` raises as ``RuntimeError``). Every other ``BaseException`` (incl. PyO3 ``PanicException``) goes to stderr + the crash
log and gives exit 1 (31 section 6). Needs no pcbnew (M4-R1); kipy only behind ``topor.ipc``.

Complexity: O(1) glue around one ``run`` call.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import tempfile
import traceback
from datetime import datetime
from pathlib import Path

from .findings import Finding
from .ports import PadRef, Selection

ENGINE_FIX = "Restart the PCB Editor or use Recreate Plugin Environment."
_OK_STATUSES = ("accepted", "rejected", "cancelled", "aborted_preflight")


def report_root() -> Path:
    env = os.environ.get("TOPOR_REPORT_ROOT")
    if env:
        return Path(env)
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "kicad" / "10.0" / "topor"


def crash_log(text: str) -> Path | None:
    """Append `text` to ``<report root>/topor-crash.log``; None if it cannot be written."""
    try:
        root = report_root()
        root.mkdir(parents=True, exist_ok=True)
        p = root / "topor-crash.log"
        with p.open("a") as f:
            f.write(f"--- {datetime.now().isoformat(timespec='seconds')} ---\n{text}\n")
        return p
    except OSError:
        return None


def handshake(shipped_version: str | None, engine=None) -> Finding | None:
    """None if the installed engine matches `shipped_version` (None = no check), else an ENGINE_VERSION error."""
    if engine is None:
        import topor_engine as engine  # noqa: PLC0415
    got = str(engine.version())
    if shipped_version is not None and got != shipped_version:
        return Finding("ENGINE_VERSION", "error",
                       f"Engine version mismatch: installed {got}, plugin shipped with {shipped_version}.",
                       ENGINE_FIX, (got, shipped_version))
    return None


def selection_from_json(text: str | None) -> Selection:
    if not text:
        return Selection()
    d = json.loads(text)
    area = d.get("area")
    return Selection(pads=[PadRef(str(r), str(n)) for r, n in d.get("pads", [])],
                     footprints=[str(x) for x in d.get("footprints", [])],
                     tracks=[str(x) for x in d.get("tracks", [])], vias=[str(x) for x in d.get("vias", [])],
                     area=[(int(x), int(y)) for x, y in area] if area else None)


def _parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="topor", description="Topor curved-trace router (KiCad plugin)")
    ap.add_argument("--headless", metavar="PCB", help="run on a .kicad_pcb file without KiCad or a window")
    ap.add_argument("--model", metavar="JSON", help="BoardModel JSON (required with --headless)")
    ap.add_argument("--accept", action="store_true", help="headless: Accept the preview (default Reject)")
    ap.add_argument("--selection", metavar="JSON", help="headless: selection as JSON")
    return ap


def _exit_code(outcome) -> int:
    return 0 if outcome.status in _OK_STATUSES else 1


def run_headless(args, shipped_version: str | None, engine=None) -> int:
    from .config import Config
    from .fake import FakeBoard, ScriptedUi
    from .run import run

    bad = handshake(shipped_version, engine)
    if bad is not None:
        print(f"topor: {bad.message} {bad.fix}", file=sys.stderr)
        return 1
    pcb = Path(args.headless)
    model = json.loads(Path(args.model).read_text())
    board = FakeBoard(pcb, model, selection=selection_from_json(args.selection))
    ui = ScriptedUi(accept=bool(args.accept))
    cfg = None if (board.project_dir() / "topor.toml").exists() else Config.defaults()
    tmp = Path(tempfile.mkdtemp(prefix="topor-"))
    try:
        out = run(board, ui, cfg, engine, tmp_root=tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print(f"topor: {out.status} routed {out.routed}/{out.total}"
          + (f" report {out.report_dir}" if out.report_dir else ""))
    if out.error is not None:
        print(f"topor: {out.error.code}: {out.error.message} {out.error.fix}", file=sys.stderr)
    return _exit_code(out)


def _fatal(ui, f: Finding) -> int:
    """stderr + crash log + error dialog for an error before / outside ``run``; exit code 1."""
    print(f"topor: {f.code}: {f.message} {f.fix}", file=sys.stderr)
    crash_log(f"{f.code}: {f.message} {f.fix}")
    ui.error(f)
    return 1


def run_gui(shipped_version: str | None, engine=None, *, ui=None, connect=None) -> int:
    """GUI mode: engine handshake, KiCad connection (``IpcBoard.connect`` over ``KICAD_API_SOCKET`` /
    ``KICAD_API_TOKEN``), then ``run``. A failed handshake (ENGINE_VERSION) or connection (KICAD_API_CONNECT,
    ruling C32) shows an error dialog and gives exit 1. `ui` / `connect` are test seams (a recording UI instead of
    WxUi, no wx.App is created then; a function returning the BoardPort). UNVERIFIED without a running KiCad (GUI
    checklist)."""
    if ui is None:
        import wx  # noqa: PLC0415

        from .ui_wx import WxUi

        wx.App(False)  # KiCad runs the plugin as its own process: we own the wx.App
        ui = WxUi()
    # The splash first (user 2026-11-08): the imports, the KiCad connection, the board read and the engine
    # probe below take a few seconds; the first real window closes it. A UI without a splash skips it.
    splash = getattr(ui, "splash", None)
    if not callable(splash):
        def splash(*_a, **_kw):
            return None
    splash("Starting the router…", shipped_version)
    from . import messages
    from .ipc import IpcBoard
    from .run import run

    bad = handshake(shipped_version, engine)
    if bad is not None:
        return _fatal(ui, bad)
    if connect is None:
        def connect():
            return IpcBoard.connect(os.environ.get("KICAD_API_SOCKET"), os.environ.get("KICAD_API_TOKEN"))
    splash("Connecting to KiCad…")
    try:
        board = connect()
    except Exception as e:  # noqa: BLE001 - API off, KiCad still starting, no board open (kipy errors)
        return _fatal(ui, Finding("KICAD_API_CONNECT", "error", f"Cannot connect to KiCad: {type(e).__name__}: {e}",
                                  messages.fix_for("KICAD_API_CONNECT")))
    splash("Reading the board…")
    tmp = Path(tempfile.mkdtemp(prefix="topor-"))
    try:
        out = run(board, ui, None, engine, tmp_root=tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return _exit_code(out)


def main(argv=None, *, shipped_version: str | None = None, engine=None) -> int:
    """Entry point. `shipped_version` None skips the version check (``python -m topor.app``)."""
    args = _parser().parse_args(argv)  # bad usage: argparse exits 2
    try:
        if args.headless:
            if not args.model:
                print("topor: --headless needs --model <json>", file=sys.stderr)
                return 2
            return run_headless(args, shipped_version, engine)
        return run_gui(shipped_version, engine)
    except BaseException as e:  # noqa: BLE001 - incl. PyO3 PanicException (31 section 6)
        tb = traceback.format_exc()
        print(f"topor: fatal {type(e).__name__}: {e}\n{tb}", file=sys.stderr)
        crash_log(tb)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
