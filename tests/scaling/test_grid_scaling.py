"""Grid query cost must not grow with the number of items at constant density (23 §3)."""

import pytest

pytestmark = pytest.mark.scaling


def test_grid_query_cost_constant(engine):
    probes = {n: engine.grid_probe(n, 1000, 1) for n in (1_000, 10_000, 100_000)}
    lo, hi = probes[1_000], probes[100_000]
    assert hi["candidates_per_query"] <= 4 * lo["candidates_per_query"], probes
    assert hi["seconds"] <= 20 * lo["seconds"], probes
