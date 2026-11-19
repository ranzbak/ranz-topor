"""mcp-drc mutants (fix round 2): apply one at a time, run tests/mcp_server/test_mcp_drc.py + tests/test_kicad_env.py (never the real-config test), restore.

Usage: python tools/mcpdrc_mut.py   (from a clean tree; prints KILLED / SURVIVED per mutant)"""
import subprocess
import sys
from pathlib import Path

WT = Path(__file__).resolve().parents[1]
PY = sys.executable
MUTANTS = [
    ("vkey_by_text", "mcp_server/drc.py",
     'return (v.get("type"), tuple(sorted(i.get("uuid", "") for i in v.get("items") or [])))',
     'return (v.get("type"), v.get("description"))'),
    ("cache_key_no_project", "mcp_server/drc.py",
     'key = sha256(board + b"".join(project.get(e, b"") for e in PROJECT_EXTS) + ver.encode())[:16]',
     'key = sha256(board + ver.encode())[:16]'),
    ("match_ignores_focus", "mcp_server/tools.py",
     'match=bool(ok_sel) and (d_mm is None or d_mm <= FOCUS_TOL_MM))',
     'match=bool(ok_sel))'),
    ("version_user_env", "mcp_server/drc.py",
     'env=_cli_env(Path(home)), stdin=subprocess.DEVNULL).stdout.strip()',
     'stdin=subprocess.DEVNULL).stdout.strip()'),
    ("route_user_env", "mcp_server/tools.py",
     'env = dict(_cli_env(out_dir / "kicad-home", keep_display=True), TOPOR_REPORT_ROOT=str(out_dir / "reports"))',
     'env = dict(os.environ, TOPOR_REPORT_ROOT=str(out_dir / "reports"))'),
    ("no_pro_allowed", "mcp_server/drc.py",
     'if ".kicad_pro" not in project:', 'if False:'),
    ("sigterm_no_stop", "mcp_server/kicad_gui.py", '            s.stop()\n        except Exception:',
     '            pass\n        except Exception:'),
    ("strip_user_env", "harness/strip.py",
     'kicad_run([str(kicad_cli()), "pcb", "upgrade", str(pcb)], check=True, capture_output=True, timeout=600)',
     '__import__("subprocess").run([str(kicad_cli()), "pcb", "upgrade", str(pcb)], check=True, capture_output=True)'),
    ("no_default_env", "harness/kicad_env.py", "        kicad_cli.DEFAULT_ENV = isolated_env()", "        pass"),
    ("run_cli_ignores_default", "toporlib/kicad_cli.py", "env=env if env is not None else DEFAULT_ENV)", "env=env)"),
    ("helper_keeps_user_home", "harness/kicad_env.py", "    e.update(HOME=h, XDG_CONFIG_HOME", "    e.update(XDG_CONFIG_HOME"),
]
for name, f, a, b in MUTANTS:
    p = WT / f
    src = p.read_text()
    assert src.count(a) == 1, (name, src.count(a))
    p.write_text(src.replace(a, b))
    try:
        r = subprocess.run([PY, "-m", "pytest", "-q", "-x", "-p", "no:cacheprovider", "tests/mcp_server/test_mcp_drc.py", "tests/test_kicad_env.py", "-k", "not real_kicad_cli"],
                           cwd=WT, capture_output=True, text=True, env={"PYTHONPATH": ".:plugin", "PATH": "/usr/bin:/bin",
                                                                       "HOME": str(Path.home())}, timeout=600)
        print(name, "KILLED" if r.returncode else "SURVIVED", flush=True)
    finally:
        p.write_text(src)
sys.exit(0)
