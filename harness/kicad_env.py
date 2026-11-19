"""One isolated KiCad home for every KiCad process an agent's test path starts (kicad-cli, pcbnew, kicad).

Even `kicad-cli version` rewrites ~/.config/kicad/10.0/kicad_common.json, and `kicad-cli pcb upgrade` / `drc` /
`export` do the same: run in the user's environment they change the user's real KiCad config (seen 2026-11-19).
So harness/, mcp_server/ and tools/ start KiCad only with :func:`isolated_env` (HOME, XDG_*, KICAD_CONFIG_HOME in a
scratch home with KiCad's default global library tables), via :func:`run` / :func:`run_drc` or an explicit
``env=isolated_env(...)``. Importing ``harness`` calls :func:`install`, which makes it the default of
``toporlib.kicad_cli.run_cli`` (every toporlib DRC, also plugin code the harness runs in-process). The plugin
inside the user's own KiCad does not import harness and keeps its environment.
tests/test_kicad_env.py fails on any KiCad call from these trees without it. O(1) per call (+ one copy per home).
"""
from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

#: KiCad executables (basename) that must never start with the user's environment
KICAD_EXES = ("kicad-cli", "pcbnew", "kicad", "eeschema", "kicad-python")
#: the variables that place KiCad's config / data / cache / state
ISOLATED_VARS = ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME", "KICAD_CONFIG_HOME")
TEMPLATE = Path("/usr/share/kicad/template")


def default_home() -> Path:
    """$TOPOR_KICAD_HOME, else <tmp>/topor-kicad-home-<uid> (shared by this user's agent runs; never ~)."""
    return Path(os.environ.get("TOPOR_KICAD_HOME") or Path(tempfile.gettempdir()) / f"topor-kicad-home-{os.getuid()}")


def isolated_env(home: Path | None = None, *, keep_display: bool = False, base: dict | None = None) -> dict:
    """`base` (default os.environ) with HOME / XDG_* / KICAD_CONFIG_HOME in `home` (default :func:`default_home`),
    KiCad's default global fp/sym library tables copied there once (else every footprint is a lib_footprint_issues),
    no KICAD_API_* (never talk to the user's running KiCad), DISPLAY dropped unless `keep_display`."""
    home = Path(home or default_home())
    cfg = home / ".config" / "kicad" / "10.0"
    cfg.mkdir(parents=True, exist_ok=True)
    for n in ("fp-lib-table", "sym-lib-table"):
        if not (cfg / n).exists() and (TEMPLATE / n).is_file():
            shutil.copy(TEMPLATE / n, cfg / n)
    h = str(home)
    e = {k: v for k, v in (os.environ if base is None else base).items() if not k.startswith(("KICAD_API", "XDG_"))}
    e.update(HOME=h, XDG_CONFIG_HOME=f"{h}/.config", XDG_DATA_HOME=f"{h}/.local/share", XDG_CACHE_HOME=f"{h}/.cache",
             XDG_STATE_HOME=f"{h}/.local/state", KICAD_CONFIG_HOME=f"{h}/.config/kicad")
    if not keep_display:
        e.pop("DISPLAY", None)
    return e


def is_isolated(env: dict | None) -> bool:
    """True when `env` puts every ISOLATED_VARS outside the user's real home config (and sets them all)."""
    if env is None:
        return False
    real = Path(os.path.expanduser("~")).resolve()
    try:
        homes = [Path(env[k]).resolve() for k in ISOLATED_VARS]
    except KeyError:
        return False
    return homes[0] != real and not any(h == real / ".config" or h.is_relative_to(real / ".config") or
                                        h.is_relative_to(real / ".local") or h.is_relative_to(real / ".cache")
                                        for h in homes)


_LAUNCHERS = ("sh", "bash", "dbus-run-session", "env", "nice", "timeout", "xvfb-run")


def is_kicad(cmd) -> bool:
    """`cmd` (argv list or shell string) starts a KiCad executable: argv[0] is one, or a launcher (sh -c, env,
    dbus-run-session, ...) whose arguments name one. pgrep / xdotool naming "pcbnew" do not count."""
    if isinstance(cmd, (str, bytes)):
        words = str(cmd).replace("|", " ").replace(";", " ").replace("&", " ").split()
        return bool(words) and (Path(words[0]).name in KICAD_EXES or (Path(words[0]).name in _LAUNCHERS and
                                any(Path(w).name in KICAD_EXES for w in words[1:])))
    argv = [str(c) for c in cmd] if cmd else []
    if not argv:
        return False
    if Path(argv[0]).name in KICAD_EXES:
        return True
    return Path(argv[0]).name in _LAUNCHERS and any(Path(w).name in KICAD_EXES
                                                    for a in argv[1:] for w in a.replace(";", " ").split())


def run(cmd, *, env: dict | None = None, home: Path | None = None, **kw) -> subprocess.CompletedProcess:
    """subprocess.run of a KiCad command with :func:`isolated_env` (unless an isolated `env` is given)."""
    if env is None:
        env = isolated_env(home)
    elif not is_isolated(env):
        raise ValueError(f"KiCad command {cmd!r} with a non-isolated env")
    kw.setdefault("stdin", subprocess.DEVNULL)
    return subprocess.run(cmd, env=env, **kw)


def run_drc(pcb, out_json, **kw):
    """toporlib.drc.run_drc with :func:`isolated_env` (an explicit isolated ``env=`` wins)."""
    from toporlib.drc import run_drc as _run_drc
    kw.setdefault("env", isolated_env())
    return _run_drc(pcb, out_json, **kw)


def install() -> None:
    """Make :func:`isolated_env` the default env of toporlib.kicad_cli.run_cli in this process."""
    from toporlib import kicad_cli
    if kicad_cli.DEFAULT_ENV is None:
        kicad_cli.DEFAULT_ENV = isolated_env()
