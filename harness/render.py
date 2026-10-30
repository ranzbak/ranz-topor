"""PNG renders of a board (zone fills stripped), captions and side-by-side comparisons."""
from __future__ import annotations

import subprocess
import tempfile
from pathlib import Path

import pcbnew

from harness.paths import kicad_cli

CAPTION_PX = 60
CAPTION_FONT_PT = 28
CAPTION_LINE_PX = 36  # line pitch for multi-line captions
CAPTION_PAD_PX = 16
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


def _png_width(png: Path) -> int:
    out = subprocess.run(["identify", "-format", "%w", str(png)], check=True,
                         capture_output=True, text=True).stdout
    return int(out.strip())


def _text_width(text: str, pointsize: int) -> int:
    out = subprocess.run(["convert", "-pointsize", str(pointsize), f"label:{text}",
                          "-format", "%w", "info:"], check=True,
                         capture_output=True, text=True).stdout
    return int(out.strip())


def caption(png: Path, text: str, out: Path, *, strip_px: int = CAPTION_PX) -> Path:
    """Add a white strip above the image carrying `text`, one line per newline-separated line.

    The strip is at least `strip_px` high and grows with the line count. The font is 28pt, shrunk
    only if the widest line would not fit the image width. Single-line captions are unchanged.
    """
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    lines = text.split("\n")
    strip = max(strip_px,
                CAPTION_FONT_PT + (len(lines) - 1) * CAPTION_LINE_PX + 2 * CAPTION_PAD_PX)
    pointsize = CAPTION_FONT_PT
    avail = _png_width(png) - 2 * CAPTION_PAD_PX
    width = _text_width(text.replace("%", "%%"), pointsize)
    if width > avail:
        pointsize = max(8, pointsize * avail // width)
    top = max(1, (strip - (pointsize + (len(lines) - 1) * CAPTION_LINE_PX)) // 2)
    # Each line is annotated at an explicit y so the line pitch is fixed.
    cmd = ["convert", str(png), "-background", "white", "-gravity", "North",
           "-splice", f"0x{strip}", "-fill", "black", "-pointsize", str(pointsize)]
    for i, line in enumerate(lines):
        # ImageMagick treats '%' as an escape in -annotate text.
        cmd += ["-annotate", f"+0+{top + i * CAPTION_LINE_PX}", line.replace("%", "%%")]
    subprocess.run(cmd + [str(out)], check=True)
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
