"""Per-project run lock: the router cannot be started twice for the same KiCad project.

An OS file lock (``fcntl.flock``, exclusive, non-blocking) on ``run-<hash>.lock`` in the topor cache folder
(``<report root>/<project>/``, the root of the run reports: nothing is written into the project folder). The OS
releases the lock when the process ends, so a crashed run never leaves a stale lock. The JSON body (pid, host,
start time, board, report folder) is information for the message only; the flock decides, and a leftover file
without a held lock is reused silently. Without ``fcntl`` (not Linux) the lock is skipped.

Complexity: O(1) per call.
"""

from __future__ import annotations

import hashlib
import json
import os
import socket
from datetime import datetime
from pathlib import Path

try:  # not on Windows: the lock is then skipped (acquire returns None)
    import fcntl
except ImportError:  # pragma: no cover - Linux is the target
    fcntl = None


class RunLockBusy(Exception):
    """Another run holds the lock; `holder` is its JSON body (may be empty or partial)."""

    def __init__(self, path: Path, holder: dict):
        super().__init__(f"run lock {path} is held")
        self.path, self.holder = path, holder


class RunLockUnavailable(Exception):
    """The lock file could not be created or locked (unwritable cache folder, lock not supported)."""

    def __init__(self, path: Path, cause: BaseException):
        super().__init__(f"run lock {path} unavailable: {type(cause).__name__}: {cause}")
        self.path, self.cause = path, cause


def lock_path(project_dir: Path, board_name: str, root: Path) -> Path:
    """``<root>/<board name>/run-<first 12 hex of sha256(absolute project dir)>.lock``: the hash keeps two
    projects with the same name apart."""
    h = hashlib.sha256(str(Path(project_dir).resolve()).encode()).hexdigest()[:12]
    return Path(root) / board_name / f"run-{h}.lock"


class RunLock:
    """A held lock; ``release()`` (or leaving the ``with``) drops it. Idempotent."""

    def __init__(self, path: Path, fd: int):
        self.path, self._fd = path, fd

    def release(self) -> None:
        fd, self._fd = self._fd, None
        if fd is None:
            return
        try:
            os.ftruncate(fd, 0)  # the body of a finished run is not a holder
        except OSError:
            pass
        os.close(fd)  # closing drops the flock

    def __enter__(self) -> "RunLock":
        return self

    def __exit__(self, *exc) -> None:
        self.release()


def read_holder(path: Path) -> dict:
    """The JSON body of a lock file; {} when missing, empty or not JSON."""
    try:
        d = json.loads(Path(path).read_text())
        return d if isinstance(d, dict) else {}
    except (OSError, ValueError):
        return {}


def acquire(project_dir: Path, board_name: str, root: Path, *, report_dir: Path | None = None,
            now: datetime | None = None) -> RunLock | None:
    """Take the project's run lock. None when ``fcntl`` is missing (lock skipped). Raises RunLockBusy when
    another run holds it, RunLockUnavailable when the lock file cannot be created or locked."""
    if fcntl is None:
        return None
    path = lock_path(project_dir, board_name, root)
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o644)
    except OSError as e:
        raise RunLockUnavailable(path, e) from e
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(fd)
        raise RunLockBusy(path, read_holder(path)) from None
    except OSError as e:  # e.g. a file system without flock
        os.close(fd)
        raise RunLockUnavailable(path, e) from e
    body = {"pid": os.getpid(), "host": socket.gethostname(),
            "started": (now or datetime.now()).isoformat(timespec="seconds"), "board": board_name,
            "project_dir": str(Path(project_dir).resolve()), "report_dir": str(report_dir) if report_dir else None}
    try:
        os.ftruncate(fd, 0)
        os.pwrite(fd, json.dumps(body).encode(), 0)
    except OSError:
        pass  # information only; the lock is held
    return RunLock(path, fd)


def holder_text(holder: dict) -> str:
    """``started 20:31, process 12345`` from a holder body (missing parts left out)."""
    parts = []
    try:
        parts.append("started " + datetime.fromisoformat(holder["started"]).strftime("%H:%M"))
    except (KeyError, TypeError, ValueError):
        pass
    if holder.get("pid"):
        parts.append(f"process {holder['pid']}")
    return ", ".join(parts) or "no details"
