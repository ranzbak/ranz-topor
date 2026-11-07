"""Per-project run lock (plugin/topor/runlock.py): flock on a file in the topor cache folder."""
import json
import subprocess
import sys
import time
from pathlib import Path

import pytest

from topor import runlock
from topor.runlock import RunLockBusy, RunLockUnavailable, acquire, holder_text, lock_path

pytestmark = pytest.mark.skipif(runlock.fcntl is None, reason="no fcntl")


def test_second_take_fails_with_holder_info(tmp_path):
    proj = tmp_path / "proj"
    proj.mkdir()
    first = acquire(proj, "board", tmp_path / "root", report_dir=tmp_path / "rep")
    with pytest.raises(RunLockBusy) as ei:
        acquire(proj, "board", tmp_path / "root")
    h = ei.value.holder
    assert h["pid"] > 0 and h["board"] == "board" and h["report_dir"] == str(tmp_path / "rep") and h["started"]
    assert "process" in holder_text(h) and "started" in holder_text(h)
    first.release()
    acquire(proj, "board", tmp_path / "root").release()  # free again


def test_other_project_not_blocked_and_same_name_kept_apart(tmp_path):
    a, b = tmp_path / "a" / "p", tmp_path / "b" / "p"
    a.mkdir(parents=True)
    b.mkdir(parents=True)
    assert lock_path(a, "p", tmp_path) != lock_path(b, "p", tmp_path)
    with acquire(a, "p", tmp_path), acquire(b, "p", tmp_path):
        pass


def test_released_on_normal_end_and_on_exception(tmp_path):
    with acquire(tmp_path, "p", tmp_path / "r"):
        pass
    with pytest.raises(ValueError):
        with acquire(tmp_path, "p", tmp_path / "r"):
            raise ValueError("boom")
    acquire(tmp_path, "p", tmp_path / "r").release()


def test_stale_file_of_a_killed_process_does_not_block(tmp_path):
    root = tmp_path / "root"
    code = ("import sys,time; sys.path.insert(0, %r); from topor import runlock; "
            "l = runlock.acquire(%r, 'p', %r); print('held', flush=True); time.sleep(60)"
            % (str(Path(runlock.__file__).parent.parent), str(tmp_path), str(root)))
    pr = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.PIPE, text=True)
    try:
        assert pr.stdout.readline().strip() == "held"
        with pytest.raises(RunLockBusy) as ei:
            acquire(tmp_path, "p", root)
        assert ei.value.holder["pid"] == pr.pid
        pr.kill()
        pr.wait()
        assert lock_path(tmp_path, "p", root).read_text()  # the body is left behind: stale
        acquire(tmp_path, "p", root).release()
    finally:
        pr.kill()


def test_unwritable_cache_folder_is_a_clear_error(tmp_path):
    blocker = tmp_path / "file"
    blocker.write_text("x")
    with pytest.raises(RunLockUnavailable) as ei:
        acquire(tmp_path, "p", blocker / "sub")
    assert str(blocker) in str(ei.value) and ei.value.path.name.startswith("run-")


def test_no_fcntl_skips_the_lock(tmp_path, monkeypatch):
    monkeypatch.setattr(runlock, "fcntl", None)
    assert acquire(tmp_path, "p", tmp_path) is None


def test_release_clears_body_and_is_idempotent(tmp_path):
    lk = acquire(tmp_path, "p", tmp_path / "r")
    assert json.loads(lk.path.read_text())["pid"]
    lk.release()
    lk.release()
    assert lk.path.read_text() == "" and runlock.read_holder(lk.path) == {}
