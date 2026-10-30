"""Progress PNG: hand vs smoothed topor routing with a numbers caption (user requirement)."""
from __future__ import annotations

import argparse
import datetime
import tempfile
from pathlib import Path

from harness import bench
from harness.render import caption as add_caption
from harness.render import compose_compare
from harness.smoother import smooth_router

ROOT = Path(__file__).resolve().parents[1]
HAND_MM = "189.9"


def render_progress(task: str, *, board: str = "eprom-to-kernal", date: str | None = None,
                    out_dir: Path = ROOT / "docs/benchmarks/progress") -> dict:
    date = date or datetime.date.today().isoformat()
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="topor-progress-") as tmp:
        tmp = Path(tmp)
        result = bench.run_bench(board, tmp, router=smooth_router, rip="none")
        sm = result.get("smooth") or {}
        r = result["routed_all"]
        length_mm = r["track_mm"] + r["arc_mm"]
        text = (f"{task}: {sm.get('nets_taut', 0)}/{sm.get('nets', 0)} nets, "
                f"{sm.get('wires_taut', 0)}/{sm.get('wires', 0)} wires, "
                f"DRC routing {result['routed_drc']['routing_new']}, "
                f"{length_mm:.1f} mm vs {HAND_MM} mm")
        topor_png = add_caption(tmp / f"{board}-work" / f"{board}-topor-raw.png", text,
                                tmp / "topor-progress.png")
        png = out_dir / f"{date}-m1-{task}.png"
        compose_compare(tmp / f"{board}-hand.png", topor_png, png)
    result["caption"] = text
    result["png"] = str(png)
    print(png)
    print(text)
    return result


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Render the M1 progress PNG for eprom-to-kernal.")
    ap.add_argument("--task", required=True)
    ap.add_argument("--board", default="eprom-to-kernal")
    args = ap.parse_args(argv)
    render_progress(args.task, board=args.board)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
