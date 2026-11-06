"""K1 parity: the headless harness gives FakeBoard KiCad's real net classes (as the plugin reads them over IPC), so
both take net priorities from the same classes."""
import pytest

from harness.copy import copy_project
from harness.paths import board_pcb


def test_board_netclasses_are_kicads(tmp_path):
    try:
        src = board_pcb("eprom-to-kernal")
    except Exception as e:  # noqa: BLE001 - snapshot not present on this machine
        pytest.skip(f"no snapshot: {e}")
    from harness.route_board import board_netclasses
    pc = copy_project(src, tmp_path / "eprom")
    classes, of = board_netclasses(pc)
    # eprom-to-kernal: a "power" class next to Default (effective name "power,Default", 0.4 mm tracks)
    assert classes["power,Default"].track_width == 400_000
    assert "power,Default" in set(of.values()) and all(isinstance(v, str) and v for v in of.values())
