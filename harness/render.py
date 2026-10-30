"""PNG renders of a board (zone fills stripped), captions and side-by-side comparisons."""
from __future__ import annotations

import subprocess
import tempfile
from pathlib import Path

import pcbnew

from harness.paths import kicad_cli

CAPTION_PX = 60
COMPARE_PAD_PX = 20


def strip_zone_fills(src_pcb: Path, dst_pcb: Path) -> Path:
    """Save a copy of the board with every zone unfilled; the source is never written."""
    src_pcb = Path(src_pcb)
    dst_pcb = Path(dst_pcb)
    if src_pcb.resolve() == dst_pcb.resolve():
        raise ValueError("strip_zone_fills must write to a different file than its source")
    board = pcbnew.LoadBoard(str(src_pcb))
    for zone in board.Zones():
        zone.UnFill()
    dst_pcb.parent.mkdir(parents=True, exist_ok=True)
    pcbnew.SaveBoard(str(dst_pcb), board)
    return dst_pcb


def render_png(pcb: Path, out_png: Path, *, height: int = 1100,
               layers: str = "F.Cu,B.Cu,Edge.Cuts") -> Path:
    """Board -> PNG via a zone-stripped copy, `kicad-cli pcb export svg`, then rsvg-convert."""
    out_png = Path(out_png)
    out_png.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="topor-render-") as tmp:
        stripped = strip_zone_fills(Path(pcb), Path(tmp) / "stripped.kicad_pcb")
        svg = Path(tmp) / "board.svg"
        cmd = [
            str(kicad_cli()), "pcb", "export", "svg",
            "-l", layers, "--mode-single", "--page-size-mode", "2", "--exclude-drawing-sheet",
            "-o", str(svg), str(stripped),
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode != 0 or not svg.is_file():
            raise RuntimeError(f"kicad-cli svg export failed (exit {proc.returncode}): "
                               f"{proc.stderr.strip()}")
        subprocess.run(["rsvg-convert", "-h", str(height), str(svg), "-o", str(out_png)],
                       check=True)
    return out_png


def caption(png: Path, text: str, out: Path, *, strip_px: int = CAPTION_PX) -> Path:
    """Add a white strip of `strip_px` above the image carrying `text` (height grows by strip_px)."""
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    # ImageMagick treats '%' as an escape in -annotate text.
    safe = text.replace("%", "%%")
    subprocess.run(
        ["convert", str(png), "-background", "white", "-gravity", "North",
         "-splice", f"0x{strip_px}", "-fill", "black", "-pointsize", "28",
         "-annotate", f"+0+{max(1, (strip_px - 28) // 2)}", safe, str(out)],
        check=True,
    )
    return out


def compose_compare(left_png: Path, right_png: Path, out_png: Path, *,
                    pad_px: int = COMPARE_PAD_PX) -> Path:
    """Left and right images side by side, each surrounded by `pad_px` of white."""
    out_png = Path(out_png)
    out_png.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["convert", "-background", "white",
         "(", str(left_png), "-bordercolor", "white", "-border", str(pad_px), ")",
         "(", str(right_png), "-bordercolor", "white", "-border", str(pad_px), ")",
         "+append", str(out_png)],
        check=True,
    )
    return out_png


def main(argv: list[str] | None = None) -> int:
    import argparse

    ap = argparse.ArgumentParser(description="Render a KiCad board to PNG (zone fills stripped).")
    ap.add_argument("pcb", type=Path)
    ap.add_argument("png", type=Path)
    ap.add_argument("--height", type=int, default=1100)
    args = ap.parse_args(argv)
    out = render_png(args.pcb, args.png, height=args.height)
    print(f"rendered {out} ({args.height}px high)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
