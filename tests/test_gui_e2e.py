"""Live end-to-end test of the plugin in a real KiCad 10 (K1): tools/gui_e2e.sh on a copy of wiccon-2023, in a
separate KiCad home on a nested X display (Xephyr). Deselected by default (marker ``gui``); run with

    TOPOR_E2E_WHEEL=<topor_engine wheel> .venv/bin/pytest tests/test_gui_e2e.py -m gui -s

Needs Xephyr, metacity, xdotool, wmctrl, xwd, ImageMagick, KiCad 10 and network on the first run (KiCad's pip).
Without TOPOR_E2E_WHEEL tools/build.sh builds the engine wheel first. Every step is a PASS / FAIL line in
<tmp>/e2e.log; the test asserts all of them passed."""
import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]

pytestmark = pytest.mark.gui


@pytest.mark.skipif(not all(shutil.which(t) for t in ("Xephyr", "xdotool", "wmctrl", "xwd", "pcbnew", "metacity")),
                    reason="needs Xephyr, xdotool, wmctrl, xwd, metacity and KiCad")
def test_wiccon_live_route_accept_undo(tmp_path):
    cmd = [str(ROOT / "tools" / "gui_e2e.sh"), "wiccon-2023", str(tmp_path), "--from-hand",
           "--display", os.environ.get("TOPOR_E2E_DISPLAY", ":7")]
    if os.environ.get("TOPOR_E2E_WHEEL"):
        cmd += ["--wheel", os.environ["TOPOR_E2E_WHEEL"]]
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
    res = json.loads((tmp_path / "wiccon-2023-e2e.json").read_text())
    failed = [c for c in res["checks"] if not c["ok"]]
    assert r.returncode == 0 and not failed, (failed, r.stdout[-3000:])
    m = res["metrics"]
    assert m["status"] == "accepted" and m["routing_new"] == 0 and m["routed"] == m["connections"]
