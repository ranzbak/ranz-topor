import hashlib
import json

import pytest

from harness import paths


def test_missing_snapshot_raises_actionable(monkeypatch):
    monkeypatch.setitem(paths.BOARD_SNAPSHOTS, "esp-rust-board", "000000000000")
    with pytest.raises(RuntimeError) as e:
        paths.board_pcb("esp-rust-board")
    msg = str(e.value)
    assert "esp-rust-board" in msg and "000000000000" in msg
    assert "scratch/boards/esp-rust-board/000000000000/esp-rust-board.kicad_pcb" in msg
    assert ".venv/bin/python -m harness.snapshot" in msg and "harness/paths.py" in msg
    with pytest.raises(RuntimeError, match="harness.snapshot"):
        paths.BENCH_BOARDS["esp-rust-board"]  # lazy: fails on use only
    assert "esp-rust-board" in paths.BENCH_BOARDS  # membership never resolves


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
        if b in paths.PRIVATE_BOARDS and b not in paths.BOARD_SNAPSHOTS:
            continue  # a local private board not pinned yet (scratch/private-boards.toml)
        assert paths.board_pcb(b).is_file()


def test_private_boards_file_merges_into_the_maps(monkeypatch, tmp_path):
    """scratch/private-boards.toml (git-ignored) adds boards that must not be named in git."""
    monkeypatch.setattr(paths, "SOURCE_BOARDS", dict(paths.SOURCE_BOARDS))
    monkeypatch.setattr(paths, "BOARD_SNAPSHOTS", dict(paths.BOARD_SNAPSHOTS))
    monkeypatch.setattr(paths, "LONG_BOARDS", set(paths.LONG_BOARDS))
    monkeypatch.setattr(paths, "PRIVATE_BOARDS", set())
    assert paths.load_private_boards(tmp_path / "absent.toml") == {}
    f = tmp_path / "private-boards.toml"
    f.write_text('[priv-a]\npath = "vendor/a/a.kicad_pcb"\npin = "0123456789ab"\nlong = true\n'
                 '[priv-b]\npath = "/abs/b.kicad_pcb"\n')
    got = paths.load_private_boards(f)
    assert got == {"priv-a": {"path": paths.PROJECTS / "vendor/a/a.kicad_pcb", "pin": "0123456789ab", "long": True},
                   "priv-b": {"path": paths.Path("/abs/b.kicad_pcb"), "pin": None, "long": False}}
    paths.merge_private_boards(got)
    assert paths.SOURCE_BOARDS["priv-a"] == paths.PROJECTS / "vendor/a/a.kicad_pcb"
    assert paths.BOARD_SNAPSHOTS["priv-a"] == "0123456789ab" and "priv-b" not in paths.BOARD_SNAPSHOTS
    assert paths.LONG_BOARDS >= {"priv-a", "esp-rust-board"} and "priv-b" not in paths.LONG_BOARDS
    assert paths.PRIVATE_BOARDS == {"priv-a", "priv-b"}
    with pytest.raises(ValueError, match="clashes"):
        paths.merge_private_boards({"wiccon-2023": {"path": paths.Path("/x.kicad_pcb"), "pin": None, "long": False}})
    f.write_text('[priv-c]\npin = "0123456789ab"\n')
    with pytest.raises(ValueError, match="needs a `path`"):
        paths.load_private_boards(f)


def test_snapshot_existing_pin_survives_source_edit(monkeypatch, tmp_path):
    from harness import snapshot

    src = tmp_path / "src"
    src.mkdir()
    pcb = src / "b.kicad_pcb"
    pcb.write_text("board v1")
    (src / "b.kicad_pro").write_text("pro v1")
    store = tmp_path / "store"
    monkeypatch.setitem(snapshot.SOURCE_BOARDS, "tmpboard", pcb)
    sha12, m = snapshot.snapshot_board("tmpboard", store)
    d = store / "tmpboard" / sha12
    assert sha12 == hashlib.sha256(b"board v1").hexdigest()[:12]
    # a sidecar edit in the source must not break (or change) the existing immutable snapshot
    (src / "b.kicad_pro").write_text("pro v2")
    (src / "extra.kicad_dru").write_text("new rules file")
    sha12b, m2 = snapshot.snapshot_board("tmpboard", store)
    assert sha12b == sha12 and m2 == m
    assert (d / "b.kicad_pro").read_text() == "pro v1"
    assert not (d / "extra.kicad_dru").exists()
    # a removed sidecar does not break it either
    (src / "b.kicad_pro").unlink()
    assert snapshot.snapshot_board("tmpboard", store)[0] == sha12
    # an edited pcb is a new hash: a new directory, the old one untouched
    pcb.write_text("board v2")
    sha12c, _ = snapshot.snapshot_board("tmpboard", store)
    assert sha12c != sha12 and (store / "tmpboard" / sha12c / "b.kicad_pcb").read_text() == "board v2"
    assert (d / "b.kicad_pcb").read_text() == "board v1"
    # a tampered snapshot pcb is still detected
    (d / "b.kicad_pcb").write_text("tampered")
    pcb.write_text("board v1")
    with pytest.raises(RuntimeError, match="does not match"):
        snapshot.snapshot_board("tmpboard", store)
