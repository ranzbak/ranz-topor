import hashlib
import json

import pytest

from harness import paths


def test_missing_snapshot_raises_actionable(monkeypatch):
    monkeypatch.setitem(paths.BOARD_SNAPSHOTS, "dcsgonebadge", "000000000000")
    with pytest.raises(RuntimeError) as e:
        paths.board_pcb("dcsgonebadge")
    msg = str(e.value)
    assert "dcsgonebadge" in msg and "000000000000" in msg
    assert "scratch/boards/dcsgonebadge/000000000000/dcsgonebadge.kicad_pcb" in msg
    assert ".venv/bin/python -m harness.snapshot" in msg and "harness/paths.py" in msg
    with pytest.raises(RuntimeError, match="harness.snapshot"):
        paths.BENCH_BOARDS["dcsgonebadge"]  # lazy: fails on use only
    assert "dcsgonebadge" in paths.BENCH_BOARDS  # membership never resolves


def test_snapshot_sha_mismatch_and_missing_manifest(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "SNAPSHOT_ROOT", tmp_path)
    name = paths.SOURCE_BOARDS["wiccon-2023"].name
    d = tmp_path / "wiccon-2023" / "abcdef012345"
    d.mkdir(parents=True)
    (d / name).write_text("board")
    monkeypatch.setitem(paths.BOARD_SNAPSHOTS, "wiccon-2023", "abcdef012345")
    with pytest.raises(RuntimeError, match="MANIFEST.json missing"):
        paths.board_pcb("wiccon-2023")
    (d / "MANIFEST.json").write_text(json.dumps({"pcb_sha256": "abcdef012345" + "0" * 52}))
    with pytest.raises(RuntimeError, match="does not match"):
        paths.board_pcb("wiccon-2023")
    h = hashlib.sha256(b"board").hexdigest()
    d2 = tmp_path / "wiccon-2023" / h[:12]
    d2.mkdir()
    (d2 / name).write_text("board")
    (d2 / "MANIFEST.json").write_text(json.dumps({"pcb_sha256": h}))
    monkeypatch.setitem(paths.BOARD_SNAPSHOTS, "wiccon-2023", h[:12])
    assert paths.board_pcb("wiccon-2023") == d2 / name


def test_pins_resolve_for_all_boards():
    for b in paths.SOURCE_BOARDS:
        assert paths.board_pcb(b).is_file()
