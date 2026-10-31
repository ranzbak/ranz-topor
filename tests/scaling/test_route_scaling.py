"""Router runtime growth stays within 2 x (m log m) from 10 to 100 to 1000 short connections."""
import json
import time

import pytest

from tests.scaling.synth import synth_model

pytestmark = pytest.mark.scaling


def _best_of_3(engine, n):
    model = json.dumps(synth_model(n, 1))
    best, res = None, None
    for _ in range(3):
        t = time.perf_counter()
        out = engine.route(model, json.dumps({"wall_clock_s": 120}))
        dt = time.perf_counter() - t
        if best is None or dt < best:
            best, res = dt, json.loads(out)
    return best, res


def test_route_scaling_m_log_m(engine):
    t, res = {}, {}
    for n in (10, 100, 1000):
        t[n], res[n] = _best_of_3(engine, n)
    info = {n: dict(t=round(t[n], 4), routed=res[n]["routed"], iters=res[n]["iterations"],
                    timed_out=res[n]["timed_out"]) for n in t}
    print(f"SCALING {info}")
    assert res[10]["routed"] == 10 and res[100]["routed"] == 100, info
    assert res[1000]["routed"] >= 0.95 * 1000, info
    for n in t:
        assert res[n]["iterations"] <= 50 and res[n]["timed_out"] is False, info
    t10, t100, t1000 = (max(t[n], 0.001) for n in (10, 100, 1000))
    print(f"SCALING ratios t100/t10 {t100 / t10:.2f} (<= 40), t1000/t100 {t1000 / t100:.2f} (<= 30)")
    assert t1000 / t100 <= 30.0, info
    assert t100 / t10 <= 40.0, info
