"""Filesystem locations used by the harness (Linux only)."""
import hashlib
import json
from collections.abc import MutableMapping
from pathlib import Path

MM = 1_000_000  # nm per mm

PROJECTS = Path.home() / "work/kicad/projects"  # only harness/snapshot.py reads below this

ROOT = Path(__file__).resolve().parents[1]
SNAPSHOT_ROOT = ROOT / "scratch/boards"

# Source locations (live user projects). ONLY harness.snapshot reads these.
SOURCE_BOARDS = {
    "eprom-to-kernal": PROJECTS / "eprom-to-kernal/eprom-to-kernal/eprom-to-kernal.kicad_pcb",
    "wiccon-2023": PROJECTS / "Wiccon/wiccon-main-badge/2023.kicad_pcb",
    "dcsgonebadge": PROJECTS / "badge.team/dcsgonebadge-0.8.5/dcsgonebadge.kicad_pcb",
    "bornhack2026": PROJECTS
    / "badge.team/bornhack2026/bornhack2026-hardware/bornhack2026-hardware.kicad_pcb",
    "nametag-sao": PROJECTS
    / "Wiccon/WICCON2024-hardware/nametag_sao_holder/WICCON-2024-SAO-Nametag.kicad_pcb",
    "wedding-gift": PROJECTS
    / "wedding-gift-sprite/kicad/wedding-gift-jeroen-and-mingming.kicad_pcb",
}

# Pinned immutable snapshots (sha256[:12] of the pcb): scratch/boards/<board>/<sha12>/.
BOARD_SNAPSHOTS = {
    "eprom-to-kernal": "bfaf705c0f25",
    "wiccon-2023": "2d3063e70ed5",
    "dcsgonebadge": "3e7e66f862a7",
    "bornhack2026": "21c5f478f1af",
    "nametag-sao": "ca9b0acc5ff0",
    "wedding-gift": "683d3088a14f",
}


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def board_pcb(board: str) -> Path:
    """The pinned snapshot pcb of `board`, verified against its MANIFEST and the pin."""
    if board not in BOARD_SNAPSHOTS:
        raise KeyError(f"unknown benchmark board {board!r}; known: {sorted(BOARD_SNAPSHOTS)}")
    sha12 = BOARD_SNAPSHOTS[board]
    d = SNAPSHOT_ROOT / board / sha12
    pcb = d / SOURCE_BOARDS[board].name

    def fail(why: str) -> "NoReturn":
        raise RuntimeError(
            f"snapshot of board {board!r} unusable ({why}); expected {pcb} (sha12 {sha12}). "
            "Run `.venv/bin/python -m harness.snapshot` (creates the snapshot from "
            "~/work/kicad/projects) and/or update the pin in harness/paths.py BOARD_SNAPSHOTS.")

    manifest = d / "MANIFEST.json"
    if not d.is_dir():
        fail("snapshot directory missing")
    if not pcb.is_file():
        fail("pcb missing")
    if not manifest.is_file():
        fail("MANIFEST.json missing")
    want = json.loads(manifest.read_text()).get("pcb_sha256", "")
    got = _sha256(pcb)
    if got != want or not got.startswith(sha12):
        fail(f"pcb sha256 {got[:12]} does not match MANIFEST {want[:12]} / pin {sha12}")
    return pcb


class _BenchBoards(MutableMapping):
    """board -> snapshot pcb path, resolved (and verified) lazily on access.

    Writable so tests can `monkeypatch.setitem(BENCH_BOARDS, "fixture", path)`.
    """

    def __init__(self):
        self._over: dict[str, Path] = {}

    def __getitem__(self, board):
        if board in self._over:
            return self._over[board]
        if board not in BOARD_SNAPSHOTS:
            raise KeyError(board)
        return board_pcb(board)

    def __setitem__(self, board, path):
        self._over[board] = Path(path)

    def __delitem__(self, board):
        del self._over[board]

    def __contains__(self, board):
        return board in self._over or board in BOARD_SNAPSHOTS

    def __iter__(self):
        yield from dict.fromkeys([*BOARD_SNAPSHOTS, *self._over])

    def __len__(self):
        return len(set(BOARD_SNAPSHOTS) | set(self._over))


BENCH_BOARDS = _BenchBoards()


from toporlib.kicad_cli import kicad_cli  # noqa: E402,F401  (re-export)


def plugin_dir(identifier: str = "org.topor.router") -> Path:
    return Path.home() / ".local/share/kicad/10.0/plugins" / identifier


def venv_dir(identifier: str = "org.topor.router") -> Path:
    return Path.home() / ".cache/kicad/10.0/python-environments" / identifier
