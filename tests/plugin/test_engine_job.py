"""Gate review B-1 / ruling C31: the REAL ``topor_engine.RouteJob`` after a panic of its routing thread.

``_route_async_panicking`` is the engine's test hook (a RouteJob whose thread panics; not reachable from
topor.toml or the engine Options). ``done()`` must become true and ``wait()`` must raise a Python exception that
carries the panic message, so the run flow can never poll a dead job for ever.
"""
import json
import time

import pytest

topor_engine = pytest.importorskip("topor_engine")


def _wait_done(job, limit_s=10.0) -> bool:
    t0 = time.monotonic()
    while time.monotonic() - t0 < limit_s:
        if job.done():
            return True
        time.sleep(0.01)
    return job.done()


def test_real_route_job_panic_sets_done_and_wait_raises():
    job = topor_engine._route_async_panicking("boom 42 from the test hook")
    assert _wait_done(job), "RouteJob.done() stayed False after the routing thread panicked"
    with pytest.raises(RuntimeError, match="routing thread panicked: boom 42 from the test hook"):
        job.wait()
    with pytest.raises(RuntimeError, match="boom 42"):  # idempotent
        job.wait()
    json.loads(job.progress())  # progress still answers
    job.cancel()  # and cancel is harmless


def test_real_route_job_normal_run_still_done_and_returns_json():
    model = {"units": "nm", "board": "empty", "layers": [{"id": 0, "name": "F.Cu"}, {"id": 2, "name": "B.Cu"}],
             "nets": [], "pads": [], "vias": [], "kept": [], "copper_shapes": [], "holes": [], "outline": [],
             "keepouts": [], "net_ties": [], "pours": [], "dangling_ends": [], "rules": {}}
    job = topor_engine.route_async(json.dumps(model), topor_engine.default_options())
    assert _wait_done(job, 60.0)
    res = json.loads(job.wait())
    assert "items" in res and res == json.loads(job.wait())
