"""No KiCad process from harness/, mcp_server/ or tools/ may start with the user's environment (2026-11-19: even
`kicad-cli version` / `pcb upgrade` rewrote ~/.config/kicad/10.0/kicad_common.json). harness/kicad_env.py is the one
helper. Two guards: every KiCad launch of the known entry points runs under a patched subprocess.Popen that FAILS
on a non-isolated env, and a source scan fails on a KiCad subprocess call that bypasses the helper."""
from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest

from harness import kicad_env as K

ROOT = Path(__file__).resolve().parents[1]


class _Launched(Exception):
    """Raised by the guard instead of starting KiCad: the env was isolated."""


@pytest.fixture
def guard(monkeypatch, tmp_path):
    """subprocess.Popen that fails the test on a KiCad launch without the isolated env (and starts nothing)."""
    seen = []
    real = subprocess.Popen

    class Guard(real):
        def __init__(self, args, *a, **kw):
            if K.is_kicad(args if not kw.get("shell") else str(args)):
                env = kw.get("env")
                assert K.is_isolated(env), f"KiCad started with the user's environment: {args!r}"
                seen.append(args)
                raise _Launched(args)
            super().__init__(args, *a, **kw)
    monkeypatch.setattr(subprocess, "Popen", Guard)
    monkeypatch.setenv("TOPOR_KICAD_HOME", str(tmp_path / "kicad-home"))
    return seen


def _board(tmp_path, old=False):
    pcb = tmp_path / "b" / "x.kicad_pcb"
    pcb.parent.mkdir(parents=True, exist_ok=True)
    pcb.write_text(f"(kicad_pcb (version {20221018 if old else 20250114}) (generator \"pcbnew\"))\n")
    return pcb


def test_is_kicad_and_is_isolated():
    assert K.is_kicad(["/usr/bin/kicad-cli", "version"]) and K.is_kicad("kicad-cli pcb drc x | tee y")
    assert K.is_kicad(["dbus-run-session", "--", "sh", "-c", "metacity & exec pcbnew \"$1\"", "_", "b"])
    assert not K.is_kicad(["pgrep", "-x", "pcbnew"]) and not K.is_kicad("xwd -root | convert xwd:- a.png")
    assert not K.is_isolated(None) and not K.is_isolated(dict(__import__("os").environ))
    assert K.is_isolated(K.isolated_env(Path("/tmp/x-kicad-home")))


def test_every_kicad_entry_point_launches_isolated(guard, tmp_path, monkeypatch):
    from toporlib import kicad_cli
    import harness  # noqa: F401  (install(): toporlib's default env)
    assert kicad_cli.DEFAULT_ENV is not None and K.is_isolated(kicad_cli.DEFAULT_ENV)
    pcb = _board(tmp_path)
    out = tmp_path / "d.json"
    entries = {
        "kicad_env.run": lambda: K.run(["kicad-cli", "version"], capture_output=True),
        "kicad_env.run_drc": lambda: K.run_drc(pcb, out),
        "harness.drc.run_drc": lambda: __import__("harness.drc", fromlist=["x"]).run_drc(pcb, out),
        "toporlib.drc.run_drc (in-process plugin code)": lambda: __import__("toporlib.drc", fromlist=["x"]).run_drc(
            pcb, out),
        "strip.upgrade_copy": lambda: __import__("harness.strip", fromlist=["x"]).upgrade_copy(_board(tmp_path, True)),
        "render.render_png": lambda: __import__("harness.render", fromlist=["x"]).render_png(pcb, tmp_path / "a.png"),
        "mcp drc.kicad_cli_version": lambda: _mcp_version(monkeypatch, tmp_path),
        "mcp drc.run_kicad_cli": lambda: __import__("mcp_server.drc", fromlist=["x"]).run_kicad_cli(
            pcb, out, tmp_path / "h"),
        "mcp tools.route (route_board.sh)": lambda: _route(monkeypatch, tmp_path, pcb),
        "gui_e2e E2E.start_kicad (pcbnew)": lambda: _e2e(tmp_path).start_kicad(pcb),
    }
    for name, fn in entries.items():
        with pytest.raises(_Launched):
            fn()
    assert len(guard) == sum(1 for n, f in entries.items() if "route" not in n)  # route: own check


def _mcp_version(monkeypatch, tmp_path):
    from mcp_server import drc as D
    monkeypatch.setattr(D, "_VERSION", {})
    D.kicad_cli_version(tmp_path / "h")


def _route(monkeypatch, tmp_path, pcb):
    """tools.route runs bash route_board.sh: not a KiCad exe itself, so check its env as if it were one."""
    from mcp_server import tools
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    real_run = subprocess.run

    def run(cmd, *a, **kw):
        if "route_board.sh" in " ".join(map(str, cmd)):
            assert K.is_isolated(kw.get("env")), "route_board.sh (pcbnew + kicad-cli inside) with the user's env"
            raise _Launched(cmd)
        return real_run(cmd, *a, **kw)
    monkeypatch.setattr(subprocess, "run", run)
    tools.route(str(pcb), {"png": False})


def _e2e(tmp_path):
    from harness.gui_e2e import E2E
    e = E2E(tmp_path / "e2e", ":29", None)
    e.make_home()
    return e


SUBPROC = re.compile(r"subprocess\.(?:run|Popen|call|check_output|check_call)\(")
KICAD_WORD = re.compile(r"kicad-cli|kicad_cli\(\)|\bpcbnew\b(?![.\w])|\"kicad\"")


def test_no_kicad_subprocess_bypasses_the_helper():
    """Source scan of harness/, mcp_server/, tools/: a subprocess call naming kicad-cli / pcbnew must pass an env
    (the isolated one); toporlib's run_drc is imported via harness (kicad_env / harness.drc) only; shell job
    scripts set KICAD_CONFIG_HOME on the kicad-cli line."""
    bad = []
    for d in ("harness", "mcp_server", "tools"):
        for f in sorted((ROOT / d).glob("*.py")):
            if f.name.endswith(("_mut.py", "_mutate.py")) or f.name == "kicad_env.py":
                continue
            src = f.read_text()
            for m in SUBPROC.finditer(src):
                call = src[m.end():m.end() + 400].split("\n\n")[0]
                depth, end = 1, 0
                for i, ch in enumerate(call):
                    depth += (ch == "(") - (ch == ")")
                    if depth == 0:
                        end = i
                        break
                call = call[:end or len(call)]
                if KICAD_WORD.search(call.split(",")[0] + call[:200]) and "pgrep" not in call and "env=" not in call:
                    bad.append(f"{f.relative_to(ROOT)}:{src[:m.start()].count(chr(10)) + 1}")
            if re.search(r"from toporlib\.drc import[^\n]*\brun_drc\b", src):
                bad.append(f"{f.relative_to(ROOT)}: imports toporlib.drc.run_drc (use harness.kicad_env.run_drc)")
        for f in sorted((ROOT / d).glob("*.sh")):
            for n, line in enumerate(f.read_text().splitlines(), 1):
                code = line.split("#")[0]
                if re.search(r"(^|\s)kicad-cli\s", code) and "KICAD_CONFIG_HOME" not in code:
                    bad.append(f"{f.relative_to(ROOT)}:{n}")
    assert not bad, "KiCad started without harness.kicad_env:\n" + "\n".join(bad)
