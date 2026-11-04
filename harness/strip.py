"""Router input preparation (plan item F): copy a project with every track, via and teardrop zone
removed, zones/planes and everything else kept. The source is never written.

    .venv/bin/python -m harness.strip <board|pcb> <out_dir> [--whole-project]

`<board>` is a harness.paths.BENCH_BOARDS key or a .kicad_pcb path. Writes
`<out_dir>/<name>-input/<pcb>` (the stripped router input) and `<out_dir>/<name>-hand/<pcb>` (an
untouched copy: the hand reference), then verifies the source tree is unchanged. `--whole-project`
copies the entire source directory (default: the pcb, .kicad_pro/.dru/.prl, fp-lib-table and the
project-local footprint libraries, as harness.copy does).

Teardrop zones (zone with `(attr (teardrop (type padvia|track_end)))`) are removed with the tracks:
they are generated copper that hangs off the tracks. Zone fills of the remaining zones stay as saved;
kicad-cli refills them on every DRC run. Complexity O(len(board text)).
"""
from __future__ import annotations

import argparse
import re
import shutil
from dataclasses import dataclass
from pathlib import Path

from harness.copy import ProjectCopy, assert_unchanged, copy_project
from toporlib import pcbfile

_TEARDROP_ZONE = re.compile(r"\(attr\s*\(teardrop\s*\(type\s+(?:padvia|track_end)\b")


@dataclass
class StripReport:
    tracks: int = 0
    arcs: int = 0
    vias: int = 0
    teardrop_zones: int = 0


def strip_text(text: str) -> tuple[str, StripReport]:
    """The board text without segments, arcs, vias and teardrop zones (all else byte-identical)."""
    rep = StripReport()
    spans = pcbfile.find_items(text, ("segment", "arc", "via", "zone"))
    drop = []
    for sp in spans:
        if sp.kind == "zone":
            if not _TEARDROP_ZONE.search(text, sp.start, sp.end):
                continue
            rep.teardrop_zones += 1
        elif sp.kind == "segment":
            rep.tracks += 1
        elif sp.kind == "arc":
            rep.arcs += 1
        else:
            rep.vias += 1
        drop.append(sp)
    for sp in sorted(drop, key=lambda s: -s.start):  # back to front keeps the offsets valid
        a, b = sp.start, sp.end
        while a > 0 and text[a - 1] in " \t":
            a -= 1
        if a > 0 and text[a - 1] == "\n":
            a -= 1
        text = text[:a] + text[b:]
    return text, rep


def _src(board: str) -> tuple[Path, str]:
    from harness.paths import BENCH_BOARDS
    if board in BENCH_BOARDS:
        return Path(BENCH_BOARDS[board]), board
    p = Path(board)
    if not p.is_file():
        raise SystemExit(f"not a benchmark board key or a .kicad_pcb file: {board}")
    return p, p.stem


def copy_whole(src_pcb: Path, dst_dir: Path) -> ProjectCopy:
    """Copy the whole project directory (hashes of the source taken first)."""
    from harness.copy import sha256_tree
    hashes = sha256_tree(src_pcb.parent)
    if dst_dir.exists():
        shutil.rmtree(dst_dir)
    shutil.copytree(src_pcb.parent, dst_dir, ignore=shutil.ignore_patterns(".git", "*-backups"))
    stem = src_pcb.stem
    d = lambda ext: (dst_dir / f"{stem}{ext}") if (dst_dir / f"{stem}{ext}").is_file() else None  # noqa: E731
    return ProjectCopy(pcb=dst_dir / src_pcb.name, pro=d(".kicad_pro"), dru=d(".kicad_dru"),
                       src_hashes=hashes)


def upgrade_copy(pcb: Path) -> bool:
    """`kicad-cli pcb upgrade` on a COPY whose file version is older than KiCad 10 (the plugin refuses old
    formats with PCB_FORMAT_OLD). True when it was upgraded."""
    import subprocess
    ver = pcbfile.file_version(Path(pcb).read_text())
    if ver is not None and ver >= pcbfile.KICAD10_VERSION:
        return False
    from toporlib.kicad_cli import kicad_cli
    subprocess.run([str(kicad_cli()), "pcb", "upgrade", str(pcb)], check=True, capture_output=True)
    return True


def prepare(board: str, out_dir: Path, *, whole_project: bool = False, name: str | None = None) -> tuple[ProjectCopy, ProjectCopy, StripReport]:
    """(hand copy, stripped input copy, report); the source is verified unchanged."""
    src, default_name = _src(board)
    name = name or default_name
    out_dir = Path(out_dir)
    cp = copy_whole if whole_project else copy_project
    hand = cp(src, out_dir / f"{name}-hand")
    inp = cp(src, out_dir / f"{name}-input")
    upgraded = upgrade_copy(hand.pcb)  # old-format boards: both copies, never the source
    if upgraded:
        upgrade_copy(inp.pcb)
    text, rep = strip_text(inp.pcb.read_text())
    inp.pcb.write_text(text)
    assert_unchanged(src.parent, hand.src_hashes)
    return hand, inp, rep


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("board")
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--whole-project", action="store_true")
    ap.add_argument("--name", default=None, help="directory/board name (default: key or pcb stem)")
    a = ap.parse_args(argv)
    hand, inp, r = prepare(a.board, a.out_dir, whole_project=a.whole_project, name=a.name)
    print(f"stripped {r.tracks} tracks, {r.arcs} arcs, {r.vias} vias, {r.teardrop_zones} teardrop zones")
    print(f"  input: {inp.pcb}\n  hand:  {hand.pcb}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
