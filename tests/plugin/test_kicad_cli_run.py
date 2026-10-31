"""MINOR 10: kicad-cli runs via Popen + poll (timeout, progress callback, cancel) and the run flow's use of it;
MINOR 11: the board fingerprint (FakeBoard + the kipy helper). Fake `kicad-cli` = a shell script; no KiCad."""
import stat
import time
from types import SimpleNamespace as NS

import pytest

from toporlib.kicad_cli import KicadCliCancelled, KicadCliTimeout, run_cli


@pytest.fixture
def slow_cli(tmp_path):
    p = tmp_path / "slow-cli"
    p.write_text("#!/bin/sh\necho out; echo err >&2\nsleep 5\n")
    p.chmod(p.stat().st_mode | stat.S_IEXEC)
    return p


def test_run_cli_blocking_returns_output_and_code(tmp_path):
    p = tmp_path / "cli"
    p.write_text("#!/bin/sh\nyes x | head -c 300000\necho oops >&2\nexit 5\n")  # more than a pipe buffer
    p.chmod(p.stat().st_mode | stat.S_IEXEC)
    r = run_cli([str(p)])
    assert r.returncode == 5 and len(r.stdout) == 300000 and r.stderr.strip() == "oops"


def test_run_cli_timeout_kills(slow_cli):
    t0 = time.monotonic()
    with pytest.raises(KicadCliTimeout) as e:
        run_cli([str(slow_cli)], timeout_s=0.4)
    assert time.monotonic() - t0 < 2.0 and e.value.timeout_s == 0.4


def test_run_cli_polls_at_least_every_quarter_second(slow_cli):
    ticks = []
    with pytest.raises(KicadCliTimeout):
        run_cli([str(slow_cli)], timeout_s=1.3, on_poll=lambda: ticks.append(time.monotonic()))
    assert len(ticks) >= 4
    assert max(b - a for a, b in zip(ticks, ticks[1:])) < 0.35


def test_run_cli_cancel_kills(slow_cli):
    t0 = time.monotonic()
    n = []
    with pytest.raises(KicadCliCancelled):
        run_cli([str(slow_cli)], timeout_s=60, on_poll=lambda: n.append(1) or len(n) < 2)
    assert time.monotonic() - t0 < 2.0


def test_run_cli_no_timeout_when_zero(tmp_path):
    p = tmp_path / "cli"
    p.write_text("#!/bin/sh\nsleep 0.5\n")
    p.chmod(p.stat().st_mode | stat.S_IEXEC)
    assert run_cli([str(p)], timeout_s=0).returncode == 0


def test_schema_timeout_key():
    from topor import schema
    s = schema.SETTINGS_BY_KEY[("run", "kicad_cli_timeout_s")]
    assert s.default == 120 and s.unit == "s" and s.range == (0, 86400)
    assert s.engine_field is None and s.plugin_field == "run.kicad_cli_timeout_s" and s.doc


# ---------------------------------------------------------------- fingerprint helper (kipy attribute shapes)
def _pt(x, y):
    return NS(x=x, y=y)


def _track(i, a, b):
    return NS(id=NS(value=i), start=_pt(*a), end=_pt(*b), layer=3)


def _fp(ref, at, deg):
    return NS(reference_field=NS(text=NS(value=ref)), position=_pt(*at), orientation=NS(degrees=deg))


def test_board_fingerprint_changes_on_edit_and_ignores_order():
    from topor.ipc import board_fingerprint
    t = [_track("a", (0, 0), (5, 0)), _track("b", (1, 1), (2, 2))]
    v = [NS(id=NS(value="v"), position=_pt(9, 9))]
    f = [_fp("U1", (10, 10), 0.0), _fp("R1", (3, 3), 90.0)]
    base = board_fingerprint(t, v, f)
    assert board_fingerprint(t[::-1], v, f[::-1]) == base
    assert board_fingerprint(t[:1], v, f) != base  # deleted track
    assert board_fingerprint(t, v, [_fp("U1", (10, 11), 0.0), f[1]]) != base  # moved part
    assert board_fingerprint(t, v, [_fp("U1", (10, 10), 90.0), f[1]]) != base  # rotated part
    assert board_fingerprint(t, [], f) != base  # deleted via
