"""kicad-cli path resolution (Linux) and the Popen + poll runner. O(1) / O(run time / poll)."""
from __future__ import annotations

import os
import signal
import subprocess
import tempfile
import time
from pathlib import Path


def kicad_cli() -> Path:
    """`$KICAD_CLI` if set, else /usr/bin/kicad-cli."""
    return Path(os.environ.get("KICAD_CLI", "/usr/bin/kicad-cli"))


#: environment for every kicad-cli run_cli starts when the caller passes none (None = this process's own). The plugin
#: inside the user's KiCad leaves it None; harness/kicad_env.py sets an isolated KiCad home for agents' test paths.
DEFAULT_ENV: dict | None = None

POLL_S = 0.25  # the progress callback runs at least this often while kicad-cli works

#: pause-button: what `on_poll` returns while the user has the run paused (the progress dialog's Pause): kicad-cli is
#: stopped (SIGSTOP, no CPU) until a poll returns anything else (SIGCONT); the paused time does not count toward the
#: timeout. False still kills the process (Cancel).
PAUSE = "pause"


class KicadCliTimeout(RuntimeError):
    """kicad-cli ran longer than the timeout and was killed (plugin: ``KICAD_CLI_TIMEOUT``)."""

    def __init__(self, cmd, timeout_s: float):
        self.timeout_s = timeout_s
        self.cmd = list(cmd)
        super().__init__(f"kicad-cli did not finish within {timeout_s:g} s and was stopped: "
                         + " ".join(str(c) for c in cmd[1:4]))


class KicadCliCancelled(RuntimeError):
    """The progress callback returned False: kicad-cli was killed on the user's request."""


def run_cli(cmd, *, timeout_s: float | None = None, on_poll=None, poll_s: float = POLL_S, env: dict | None = None):
    """Run `cmd` (kicad-cli) with ``Popen`` and a poll loop; returns ``subprocess.CompletedProcess`` (text
    stdout / stderr). `timeout_s` None or <= 0 = no timeout; on expiry the process is killed and
    :class:`KicadCliTimeout` raised. `on_poll()` (optional) is called at least every `poll_s`; a return
    value of ``False`` kills the process and raises :class:`KicadCliCancelled`. With `on_poll` None this is
    a blocking wait that still honours the timeout. Output goes to temp files, so a chatty child can never
    fill a pipe and stall. `env` None = :data:`DEFAULT_ENV`. pause-button: an `on_poll` that returns :data:`PAUSE`
    stops the process (SIGSTOP) until a later poll returns anything else (SIGCONT); the stopped time does not count
    toward `timeout_s`. O(run time / poll_s) callbacks."""
    limit = float(timeout_s) if timeout_s and timeout_s > 0 else None
    with tempfile.TemporaryFile("w+") as so, tempfile.TemporaryFile("w+") as se:
        proc = subprocess.Popen([str(c) for c in cmd], stdin=subprocess.DEVNULL, stdout=so, stderr=se, text=True,
                                env=env if env is not None else DEFAULT_ENV)
        t0 = time.monotonic()
        last = -1e9
        stopped_at, stopped_s = None, 0.0  # pause-button: SIGSTOP time, and the stopped seconds so far
        try:
            while True:
                try:
                    proc.wait(timeout=min(0.05, poll_s))
                    break
                except subprocess.TimeoutExpired:
                    pass
                now = time.monotonic()
                run_s = now - t0 - stopped_s - (now - stopped_at if stopped_at is not None else 0.0)
                if limit is not None and run_s > limit:
                    raise KicadCliTimeout(cmd, limit)
                if on_poll is not None and now - last >= poll_s:
                    last = now
                    r = on_poll()
                    if r is False:
                        raise KicadCliCancelled("kicad-cli stopped on request")
                    if r == PAUSE and stopped_at is None:
                        os.kill(proc.pid, signal.SIGSTOP)
                        stopped_at = now
                    elif r != PAUSE and stopped_at is not None:
                        os.kill(proc.pid, signal.SIGCONT)
                        stopped_s += now - stopped_at
                        stopped_at = None
        except BaseException:
            proc.kill()  # SIGKILL also ends a stopped process
            proc.wait()
            raise
        so.seek(0)
        se.seek(0)
        return subprocess.CompletedProcess(proc.args, proc.returncode, so.read(), se.read())
