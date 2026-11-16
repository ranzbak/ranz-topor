"""The version string the plugin shows (splash): ``0.4.0 · built 2026-11-14 01:05 UTC · 7e320c1-dirty``.
tools/build.sh writes ``topor/_build_info.py`` (git-ignored: BUILD_DATE, BUILD_REV) into the installed plugin; when it
is missing (running from source, tests) the plain version is shown. Never raises."""
from __future__ import annotations

import importlib


def display_version(version: str | None, info=None) -> str | None:
    """`version` plus the build stamp. `info` is a module-like object with BUILD_DATE / BUILD_REV (default: the
    generated ``_build_info`` module if present). No build date: the version unchanged (None stays None)."""
    if info is None:
        try:
            info = importlib.import_module(__package__ + "._build_info")
        except Exception:  # missing (source tree) or unreadable: no stamp
            return version
    date = str(getattr(info, "BUILD_DATE", "") or "").strip()
    rev = str(getattr(info, "BUILD_REV", "") or "").strip()
    if not date:
        return version
    parts = [version] if version else []
    parts.append(f"built {date}")
    if rev:
        parts.append(rev)
    return " · ".join(parts)
