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


FP_LIB = Path("/usr/share/kicad/footprints/Resistor_SMD.pretty")  # host KiCad libraries: optional (CI has none)


def _clean_env(fake_home: Path, kicad_home: Path) -> dict:
    import os
    e = {k: v for k, v in os.environ.items() if not k.startswith(("XDG_", "KICAD_"))}
    e.update(HOME=str(fake_home), TOPOR_KICAD_HOME=str(kicad_home), PYTHONPATH=f"{ROOT}:{ROOT / 'plugin'}")
    return e


def _files(d: Path) -> list:
    return sorted(str(p.relative_to(d)) for p in d.rglob("*"))


@pytest.mark.skipif(__import__("importlib").util.find_spec("pcbnew") is None, reason="needs KiCad's pcbnew module")
def test_in_process_pcbnew_through_harness_stays_off_the_user_home(tmp_path):
    """`import harness` then SWIG pcbnew (LoadBoard, SaveBoard, FootprintLoad) in a process whose HOME stands for
    the user's: nothing appears under that HOME (without harness pcbnew creates ~/.config/kicad/10.0), the
    KiCad dirs land in the isolated home; the real ~/.config/kicad is byte- and mtime-identical."""
    import sys
    from tests.mcp_server.test_mcp_drc import _tree_state  # noqa: PLC0415
    user = Path.home() / ".config" / "kicad"
    before = _tree_state(user)
    fake, iso = tmp_path / "user-home", tmp_path / "kicad-home"
    fake.mkdir()
    pcb = tmp_path / "b.kicad_pcb"
    pcb.write_text('(kicad_pcb (version 20241229) (generator "pcbnew") (generator_version "9.0"))\n')
    prog = ("import harness, pcbnew\n"
            f"b = pcbnew.LoadBoard({str(pcb)!r}); pcbnew.SaveBoard({str(tmp_path / 'o.kicad_pcb')!r}, b)\n"
            + ("pcbnew.FootprintLoad(%r, 'R_0603_1608Metric')\n" % str(FP_LIB) if FP_LIB.is_dir() else ""))
    r = subprocess.run([sys.executable, "-c", prog], env=_clean_env(fake, iso), capture_output=True, text=True,
                       timeout=120, cwd=tmp_path)
    assert r.returncode == 0, r.stderr[-2000:]
    assert _files(fake) == [] and (iso / ".config/kicad/10.0").is_dir()
    assert "WARNING" not in r.stderr
    assert _tree_state(user) == before


def test_route_board_script_isolates_before_anything_else(tmp_path):
    """harness/route_board.py runs as a script (tools/route_board.sh): main() isolates first."""
    import sys
    fake, iso = tmp_path / "user-home", tmp_path / "kicad-home"
    fake.mkdir()
    r = subprocess.run([sys.executable, "-P", str(ROOT / "harness/route_board.py"), "--help"],
                       env=_clean_env(fake, iso), capture_output=True, text=True, timeout=120, cwd=tmp_path)
    assert r.returncode == 0, r.stderr[-1500:]
    assert (iso / ".config/kicad/10.0").is_dir() and _files(fake) == []
    assert (iso / ".config/kicad/10.0/fp-lib-table").is_file() == (K.TEMPLATE / "fp-lib-table").is_file()


def test_harness_import_sets_the_kicad_location_variables():
    import os
    for k in K.PROCESS_VARS:
        assert not Path(os.environ[k]).resolve().is_relative_to(Path.home().resolve() / ".config"), k


def test_the_pytest_session_itself_is_isolated_by_conftest(request):
    """tests/conftest.py imports harness (whole session: toporlib kicad-cli + in-process pcbnew isolated)."""
    top = [m for m in request.config.pluginmanager.get_plugins()
           if Path(getattr(m, "__file__", "") or "").resolve() == ROOT / "tests" / "conftest.py"]
    assert len(top) == 1 and getattr(top[0], "harness", None) is __import__("harness")


@pytest.mark.skipif(__import__("importlib").util.find_spec("pcbnew") is None, reason="needs KiCad's pcbnew module")
def test_pcbnew_imported_before_harness_is_a_loud_warning(tmp_path):
    import sys
    fake, iso = tmp_path / "user-home", tmp_path / "kicad-home"
    fake.mkdir()
    r = subprocess.run([sys.executable, "-c", "import pcbnew, harness"], env=_clean_env(fake, iso),
                       capture_output=True, text=True, timeout=120, cwd=tmp_path)
    assert r.returncode == 0 and "WARNING pcbnew was imported before harness" in r.stderr, r.stderr[-1000:]


def test_e2e_session_env_uses_its_own_home_not_the_shared_one(tmp_path):
    """gui_e2e.E2E.env(): KiCad's location variables of an `import harness` process must not leak into the GUI
    session (KICAD_DOCUMENTS_HOME moved the scripting plugin folder: helper never loaded, verify r3 V1)."""
    import os
    from harness.gui_e2e import E2E
    assert all(k in os.environ for k in K.PROCESS_VARS)  # set by conftest's import harness
    e = E2E(tmp_path / "s", ":29", None).env()
    assert e["KICAD_CONFIG_HOME"].startswith(str(tmp_path / "s"))
    assert not {"KICAD_CACHE_HOME", "KICAD_DOCUMENTS_HOME"} & set(e)
    assert K.is_isolated(e)
    iso = K.isolated_env(tmp_path / "h")
    assert all(iso.get(k, str(tmp_path / "h")).startswith(str(tmp_path / "h")) for k in K.PROCESS_VARS)
