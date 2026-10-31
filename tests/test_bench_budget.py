"""M3-R8: options["wall_clock_s"] is the budget of the whole bench run (all DRC-loop rounds)."""
import time

from harness import bench
from tests.test_smoother import _thin_router


def _recording(seen, sleep_s):
    def router(model, options):
        seen.append(options.get("wall_clock_s"))
        time.sleep(sleep_s)
        return _thin_router(model, options)

    return router


def test_later_round_gets_remaining_budget(fixture_board, tmp_path):
    seen: list = []
    r = bench.run_bench("fixture", tmp_path / "o", router=_recording(seen, 0.3), rip="none",
                        options={"wall_clock_s": 60.0}, pcb_override=fixture_board.pcb)
    assert r["drc_loop"]["rounds"] == 2
    assert seen[0] == 60.0
    assert 59.0 < seen[1] <= 60.0 - 0.3
    assert not r["drc_loop"].get("budget_exhausted")


def test_exhausted_budget_skips_rerun_and_keeps_board(fixture_board, tmp_path):
    seen: list = []
    r = bench.run_bench("fixture", tmp_path / "o", router=_recording(seen, 0.3), rip="none",
                        options={"wall_clock_s": 1.0}, pcb_override=fixture_board.pcb)
    assert len(seen) == 1, seen  # remainder 0.7 s < 1 s: no second round
    assert r["drc_loop"]["rounds"] == 1
    assert r["drc_loop"]["budget_exhausted"] is True
    assert r["routed_items"] == 1  # the first round's board is kept
