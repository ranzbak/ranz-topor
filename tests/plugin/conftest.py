import pathlib
import sys

import pytest

_PLUGIN = str(pathlib.Path(__file__).resolve().parents[2] / "plugin")
if _PLUGIN not in sys.path:
    sys.path.insert(0, _PLUGIN)

# Green field (2026-11-03): the routing loop was removed; route() reports every connection as
# failed until the new core exists. These end-to-end tests need routed copper; they show up as
# XPASS once the new core routes again, then drop them from this list.
_NEEDS_ROUTER = {
    "test_app.py::test_headless_run_on_fixture_accepts",
    "test_run_flow.py::test_run_accept_single_commit",
    "test_run_flow.py::test_run_cancel_keeps_best_result_and_previews",
    "test_run_flow.py::test_run_segments_geometry_has_no_arcs",
    "test_run_flow.py::test_run_partial_routes_group",
    "test_run_flow.py::test_run_failure_after_commit_stays_accepted",
    "test_run_flow.py::test_run_preview_png_written_with_caption",
    "test_run_flow.py::test_report_written_before_preview",
    "test_run_flow.py::test_run_wiccon_refill_gives_harness_connection_count",
    "test_run_flow.py::test_run_report_has_every_section",
    "test_run_flow_guards.py::test_run_refuses_to_commit_when_board_changed",
    "test_run_flow_guards.py::test_run_refuses_to_commit_when_a_track_was_deleted",
    "test_run_flow_guards.py::test_run_commits_when_board_unchanged",
    "test_scope.py::test_scope_between_vs_touching",
}


def pytest_collection_modifyitems(config, items):
    for item in items:
        key = f"{item.path.name}::{getattr(item, 'originalname', item.name)}"
        if key in _NEEDS_ROUTER:
            item.add_marker(pytest.mark.xfail(reason="needs the routing engine (green field)", strict=False))
