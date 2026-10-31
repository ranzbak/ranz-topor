"""Progress PNG: hand vs smoothed topor routing with a numbers caption (user requirement)."""
from __future__ import annotations

import argparse
import datetime
import tempfile
from pathlib import Path

from harness import bench
from harness.render import caption as add_caption
from harness.render import compose_compare
from harness.router import route_router
from harness.smoother import smooth_router

ROOT = Path(__file__).resolve().parents[1]


def render_progress(task: str, *, board: str = "eprom-to-kernal", date: str | None = None,
                    out_dir: Path = ROOT / "docs/benchmarks/progress",
                    pcb_override: Path | None = None, router: str = "smoother") -> dict:
    if router == "topor":
        return _render_progress_m2(task, board=board, date=date, out_dir=out_dir,
                                   pcb_override=pcb_override)
    date = date or datetime.date.today().isoformat()
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="topor-progress-") as tmp:
        tmp = Path(tmp)
        result = bench.run_bench(board, tmp, router=smooth_router, rip="none",
                                 pcb_override=pcb_override)
        sm = result.get("smooth") or {}
        rep = result.get("router_report") or {}
        nets_total = rep.get("nets_total", sm.get("nets", 0))
        nets_taut = rep.get("nets_taut", sm.get("nets_taut", 0))
        wires_total = rep.get("wires_total", sm.get("wires", 0))
        wires_taut = rep.get("wires_taut", sm.get("wires_taut", 0))
        r = result["routed_all"]
        length_mm = r["track_mm"] + r["arc_mm"]
        hand_mm = result["orig_all"]["track_mm"] + result["orig_all"]["arc_mm"]  # board under test
        drc = result["routed_drc"]
        zs = rep.get("wires_zero_slack", sm.get("wires_zero_slack", 0))
        line1 = (f"{task}: {nets_taut}/{nets_total} nets, {wires_taut}/{wires_total} wires, "
                 f"fallback {wires_total - wires_taut} (zero slack {zs}), arcs {r['arcs']}")
        line2 = (f"DRC routing {drc['routing_new']}, "
                 f"unconnected {drc['incomplete']['unconnected_items']}, "
                 f"{length_mm:.1f} mm vs hand {hand_mm:.1f} mm, "
                 f"outside pads {sm.get('outside_mm') or 0.0:.1f} vs "
                 f"{sm.get('hand_outside_mm') or 0.0:.1f} mm")
        text = f"{line1}\n{line2}"
        topor_png = add_caption(tmp / f"{board}-work" / f"{board}-topor-raw.png", text,
                                tmp / "topor-progress.png")
        png = out_dir / f"{date}-m1-{task}.png"
        compose_compare(tmp / f"{board}-hand.png", topor_png, png)
    result["caption"] = text
    result["png"] = str(png)
    print(png)
    print(text)
    return result


def _render_progress_m2(task: str, *, board: str, date: str | None, out_dir: Path,
                        pcb_override: Path | None) -> dict:
    date = date or datetime.date.today().isoformat()
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="topor-progress-") as tmp:
        tmp = Path(tmp)
        result = bench.run_bench(board, tmp, router=route_router, rip="all",
                                 pcb_override=pcb_override)
        rt = result.get("route") or {}
        r, o = result["routed_all"], result["orig_all"]
        drc = result["routed_drc"]
        line1 = (f"{task}: {rt.get('routed', 0)}/{rt.get('connections', 0)} connections, "
                 f"{rt.get('nets_routed', 0)}/{rt.get('nets', 0)} nets, "
                 f"partial {rt.get('partial', 0)}, failed {rt.get('failed', 0)}, "
                 f"arcs {r['arcs']}")
        line2 = (f"DRC routing {drc['routing_new']}, "
                 f"unconnected {drc['incomplete']['unconnected_items']}, "
                 f"outside pads {rt.get('outside_mm') or 0.0:.1f} vs hand "
                 f"{(rt.get('hand_outside_mm') or 0.0):.1f} mm, "
                 f"KiCad {r['track_mm'] + r['arc_mm']:.1f} vs hand "
                 f"{o['track_mm'] + o['arc_mm']:.1f} mm, router {result['router_s']:.2f} s")
        text = f"{line1}\n{line2}"
        topor_png = add_caption(tmp / f"{board}-work" / f"{board}-topor-raw.png", text,
                                tmp / "topor-progress.png")
        png = out_dir / f"{date}-m2-{task}.png"
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
    ap.add_argument("--pcb", type=Path, default=None,
                    help="use this board instead of BENCH_BOARDS[board]")
    ap.add_argument("--router", choices=["smoother", "topor"], default="smoother")
    args = ap.parse_args(argv)
    render_progress(args.task, board=args.board, pcb_override=args.pcb, router=args.router)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
