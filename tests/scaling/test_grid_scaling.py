"""Grid scaling placeholder: enabled in Task 6 once the PyO3 surface exposes the grid.

Skips cleanly while the `engine` fixture skips (wheel not built).
"""

import pytest

pytestmark = pytest.mark.scaling


def test_grid_query_cost_is_sublinear(engine):
    if not hasattr(engine, "Grid"):
        pytest.skip("engine.Grid not exposed yet (Task 6)")
    pytest.skip("scaling check lives in engine/tests/grid_props.rs until Task 6")
