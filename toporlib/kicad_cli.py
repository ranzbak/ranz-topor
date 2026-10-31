"""kicad-cli path resolution (Linux) and the Popen + poll runner. O(1) / O(run time / poll)."""
from __future__ import annotations

import os
import subprocess
import tempfile
import time
from pathlib import Path


def kicad_cli() -> Path:
    """`$KICAD_CLI` if set, else /usr/bin/kicad-cli."""
    return Path(os.environ.get("KICAD_CLI", "/usr/bin/kicad-cli"))


POLL_S = 0.25  # the progress callback runs at least this often while kicad-cli works


class KicadCliTimeout(RuntimeError):
    """kicad-cli ran longer than the timeout and was killed (plugin: ``KICAD_CLI_TIMEOUT``)."""

    def __init__(self, cmd, timeout_s: float):
        self.timeout_s = timeout_s
        self.cmd = list(cmd)
        super().__init__(f"kicad-cli did not finish within {timeout_s:g} s and was stopped: "
                         + " ".join(str(c) for c in cmd[1:4]))


class KicadCliCancelled(RuntimeError):
    """The progress callback returned False: kicad-cli was killed on the user's request."""


def run_cli(cmd, *, timeout_s: float | None = None, on_poll=None, poll_s: float = POLL_S):
    """Run `cmd` (kicad-cli) with ``Popen`` and a poll loop; returns ``subprocess.CompletedProcess`` (text
    stdout / stderr). `timeout_s` None or <= 0 = no timeout; on expiry the process is killed and
    :class:`KicadCliTimeout` raised. `on_poll()` (optional) is called at least every `poll_s`; a return
    value of ``False`` kills the process and raises :class:`KicadCliCancelled`. With `on_poll` None this is
    a blocking wait that still honours the timeout. Output goes to temp files, so a chatty child can never
    fill a pipe and stall. O(run time / poll_s) callbacks."""
    limit = float(timeout_s) if timeout_s and timeout_s > 0 else None
    with tempfile.TemporaryFile("w+") as so, tempfile.TemporaryFile("w+") as se:
        proc = subprocess.Popen([str(c) for c in cmd], stdin=subprocess.DEVNULL, stdout=so, stderr=se, text=True)
        t0 = time.monotonic()
        last = -1e9
        try:
            while True:
                try:
                    proc.wait(timeout=min(0.05, poll_s))
                    break
                except subprocess.TimeoutExpired:
                    pass
                now = time.monotonic()
                if limit is not None and now - t0 > limit:
                    raise KicadCliTimeout(cmd, limit)
                if on_poll is not None and now - last >= poll_s:
                    last = now
                    if on_poll() is False:
                        raise KicadCliCancelled("kicad-cli stopped on request")
        except BaseException:
            proc.kill()
            proc.wait()
            raise
        so.seek(0)
        se.seek(0)
        return subprocess.CompletedProcess(proc.args, proc.returncode, so.read(), se.read())
