"""Create immutable board snapshots: scratch/boards/<board>/<sha12>/ (pcb/pro/dru/prl + MANIFEST).

This is the ONLY code that reads ~/work/kicad/projects, and it only reads there.
Usage: .venv/bin/python -m harness.snapshot [store_dir] [board ...]
"""
from __future__ import annotations

import datetime
import hashlib
import json
import shutil
import sys
from pathlib import Path

from harness.paths import BOARD_SNAPSHOTS, SNAPSHOT_ROOT, SOURCE_BOARDS

SUFFIXES = (".kicad_pcb", ".kicad_pro", ".kicad_dru", ".kicad_prl")


def _sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def snapshot_board(name: str, store: Path) -> tuple[str, dict]:
    pcb = SOURCE_BOARDS[name]
    h = _sha(pcb)
    dst = store / name / h[:12]
    if not dst.exists():
        dst.mkdir(parents=True)
        for f in pcb.parent.iterdir():
            if f.is_file() and f.suffix in SUFFIXES and not f.name.startswith("_autosave"):
                shutil.copy2(f, dst / f.name)
    for f in dst.iterdir():
        if f.name == "MANIFEST.json":
            continue
        if _sha(f) != _sha(pcb.parent / f.name):
            raise RuntimeError(f"snapshot file differs from source: {f}")
    m = {"board": name, "source": str(pcb), "pcb_sha256": h,
         "source_mtime": datetime.datetime.fromtimestamp(pcb.stat().st_mtime).isoformat(timespec="seconds"),
         "files": sorted(f.name for f in dst.iterdir() if f.name != "MANIFEST.json")}
    (dst / "MANIFEST.json").write_text(json.dumps(m, indent=1))
    return h[:12], m


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    store = Path(args.pop(0)) if args and Path(args[0]).is_dir() and args[0] not in SOURCE_BOARDS else SNAPSHOT_ROOT
    names = args or list(SOURCE_BOARDS)
    for name in names:
        sha12, m = snapshot_board(name, store)
        pin = BOARD_SNAPSHOTS.get(name)
        note = "" if pin == sha12 else f"  (pin in harness/paths.py is {pin}: update it)"
        print(f'{name:16} {sha12}  {m["source_mtime"]}  {len(m["files"])} files{note}')
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
