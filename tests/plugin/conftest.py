import pathlib
import sys

import pytest

_PLUGIN = str(pathlib.Path(__file__).resolve().parents[2] / "plugin")
if _PLUGIN not in sys.path:
    sys.path.insert(0, _PLUGIN)

# S2a (first copper, two signal layers): the topological core routes again. These end-to-end tests
# need all 27 eprom / all wiccon connections routed; the core reaches 25/27 on eprom (existing
# hand vias kept on the board; two long connections end in a fine-pitch DIP channel the drawing
# cannot clear) and fewer on wiccon, so they stay XFAIL until the drawing improves (S2b / P4).
# The other routing tests of the old list pass and are no longer marked.
_NEEDS_ROUTER = {
    "test_app.py::test_headless_run_on_fixture_accepts",
    "test_run_flow.py::test_run_accept_single_commit",
    "test_run_flow.py::test_run_segments_geometry_has_no_arcs",
    "test_run_flow.py::test_run_preview_png_written_with_caption",
    "test_run_flow.py::test_run_wiccon_refill_gives_harness_connection_count",
    "test_run_flow.py::test_run_report_has_every_section",
}


def pytest_collection_modifyitems(config, items):
    for item in items:
        key = f"{item.path.name}::{getattr(item, 'originalname', item.name)}"
        if key in _NEEDS_ROUTER:
            item.add_marker(pytest.mark.xfail(reason="needs every connection routed; the S2a core routes 25/27 on eprom (see the comment above)", strict=False))
