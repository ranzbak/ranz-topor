"""End-to-end benchmark of one board: copy -> baseline DRC -> rip -> route -> write -> DRC -> images.

Source boards are never written: the procedure runs on a copy under `<out_dir>/<board>-work/`.
Full JSON and images stay on disk; `run_bench` prints a summary of at most 20 lines.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable, Literal

from harness import writer
from harness.copy import assert_unchanged, copy_project
from harness.drc import classify, run_drc, summarise
from harness.metrics import copper_metrics
from harness.model import export_model, load_board, write_model_json
from harness.paths import BENCH_BOARDS
from harness.render import caption, compose_compare, render_png
from harness.rip import record_widths
from harness.rules import resolve_rules

SUMMARY_MAX = 20
Router = Callable[[dict, dict], list[dict]]


def _source_pcb(board: str, pcb_override: Path | None) -> Path:
    if pcb_override is not None:
        src = Path(pcb_override)
    elif board in BENCH_BOARDS:
        src = BENCH_BOARDS[board]
    else:
        raise KeyError(f"unknown benchmark board {board!r}; known: {sorted(BENCH_BOARDS)}")
    if not src.is_file():
        raise RuntimeError(f"benchmark board not found: {src}")
    return src


ROOT = Path(__file__).resolve().parents[1]

# rip_tracks() runs in a child process: its board.Remove() calls leave pcbnew's SWIG state broken
# for the rest of the process (GetDesignSettings / GetSettingsManager stop working).
_RIP_CHILD = """
import json, sys
from dataclasses import asdict
from pathlib import Path
from harness import writer
from harness.copy import ProjectCopy
from harness.model import load_board
from harness.rip import rip_tracks
pcb, pro, mode, scope, rf = sys.argv[1:6]
cp = ProjectCopy(pcb=Path(pcb), pro=Path(pro) if pro else None, dru=None)
b = load_board(cp)
rep = rip_tracks(b, mode=mode, scope=json.loads(scope), rf_nets=set(json.loads(rf)))
writer.save(b, pcb)
print(json.dumps(asdict(rep)))
"""


def _rip_in_subprocess(cp, mode: str, scope: dict | None, rf_nets: set[str]) -> dict:
    """Rip the copy's tracks in a child process, which saves the result over cp.pcb."""
    args = [sys.executable, "-c", _RIP_CHILD, str(cp.pcb), str(cp.pro or ""), mode,
            json.dumps(scope), json.dumps(sorted(rf_nets))]
    proc = subprocess.run(args, capture_output=True, text=True, cwd=ROOT)
    if proc.returncode != 0:
        raise RuntimeError(f"rip step failed (exit {proc.returncode}): {proc.stderr.strip()[-2000:]}")
    return json.loads(proc.stdout.strip().splitlines()[-1])


def _summary(board: str, result: dict, routed_drc, base_drc, out_dir: Path) -> str:
    o, r, rm = result["orig_all"], result["routed_all"], result["removed"]
    wm = result["width_match"]
    head = [
        f"bench {board}: fp={result['footprints']} pads={result['pads']} "
        f"nets={result['nets_total']} rip={result['rip']}",
        f"  orig  seg={o['segments']} arc={o['arcs']} via={o['vias']} "
        f"track={o['track_mm']}mm arc={o['arc_mm']}mm",
        f"  removed tracks={rm['removed_tracks']} arcs={rm['removed_arcs']} "
        f"vias={rm['removed_vias']}; routed items={result['routed_items']}",
        f"  routed seg={r['segments']} arc={r['arcs']} via={r['vias']} "
        f"track={r['track_mm']}mm arc={r['arc_mm']}mm",
        f"  runtime {result['runtime_s']}s (router {result['router_s']}s, "
        f"threads={result['threads']})",
    ]
    drc = summarise(board, routed_drc, base_drc,
                    extra={"width_match": f"{wm['matched']}/{wm['compared']}"})
    lines = head + drc.splitlines() + [f"  outputs: {out_dir}"]
    return "\n".join(lines[:SUMMARY_MAX])


def run_bench(
    board: str,
    out_dir: Path,
    *,
    router: Router | None = None,
    rip: Literal["all", "none", "scoped"] = "all",
    options: dict | None = None,
    rf_nets: frozenset | set = frozenset(),
    threads: int = 4,
    pcb_override: Path | None = None,
) -> dict:
    """Run the benchmark procedure for one board; `router=None` routes nothing (M0)."""
    t_start = time.monotonic()
    options = dict(options or {})
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    src = _source_pcb(board, pcb_override)

    cp = copy_project(src, out_dir / f"{board}-work")
    base_drc = run_drc(cp.pcb, out_dir / f"{board}--drc-orig.json")

    b = load_board(cp)
    fps = list(b.GetFootprints())
    widths_before = record_widths(b)
    orig_all = copper_metrics(b)
    # Hand image is rendered from the untouched copy, before anything is saved over it.
    hand_raw = render_png(cp.pcb, cp.pcb.parent / f"{board}-hand-raw.png")

    rep = _rip_in_subprocess(cp, rip, options.get("scope"), set(rf_nets))
    removed = {k: rep[k] for k in ("removed_tracks", "removed_arcs", "removed_vias")}
    kept = {k: rep[k] for k in ("kept_locked_tracks", "kept_free_vias", "kept_locked_vias",
                                "kept_rf_tracks", "kept_rf_vias")}
    b = load_board(cp)  # fresh board holding the ripped copper the child process saved

    dru_text = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru_text, widths_before=widths_before)
    model = export_model(b, rules, name=board)
    write_model_json(model, out_dir / f"{board}--model.json")

    t_route = time.monotonic()
    items = router(model, {**options, "threads": threads}) if router is not None else []
    router_s = time.monotonic() - t_route
    writer.write_items(b, items)
    # Save over the working copy so the copied .kicad_pro/.kicad_dru still apply to the DRC run.
    writer.save(b, cp.pcb)
    routed_drc = run_drc(cp.pcb, out_dir / f"{board}--drc-routed.json")
    topor_raw = render_png(cp.pcb, cp.pcb.parent / f"{board}-topor-raw.png")

    routed_all = copper_metrics(b)
    routed_widths = record_widths(b)
    compared = sorted(n for n in widths_before if n in routed_widths)
    mismatched = [n for n in compared if routed_widths[n].median_nm != widths_before[n].median_nm]
    width_match = {"compared": len(compared), "matched": len(compared) - len(mismatched),
                   "mismatched": mismatched[:20]}

    hand_png = caption(hand_raw, f"{board}: hand", out_dir / f"{board}-hand.png")
    topor_png = caption(topor_raw, f"{board}: topor", out_dir / f"{board}-topor.png")
    compose_compare(hand_png, topor_png, out_dir / f"{board}-compare.png")

    result = {
        "board": board,
        "footprints": len(fps),
        "pads": sum(len(fp.Pads()) for fp in fps),
        "nets_total": len(model["nets"]),
        "orig_all": orig_all,
        "orig_drc": {"total": len(base_drc.violations), "violations": base_drc.by_type},
        "removed": removed,
        "kept": kept,
        "routed_all": routed_all,
        "routed_drc": classify(routed_drc, base_drc),
        "routed_items": len(items),
        "rip": rip,
        "threads": threads,
        "runtime_s": round(time.monotonic() - t_start, 3),
        "router_s": round(router_s, 3),
        "width_match": width_match,
    }
    (out_dir / f"{board}--metrics.json").write_text(json.dumps(result, indent=1))
    assert_unchanged(src.parent, cp.src_hashes)
    print(_summary(board, result, routed_drc, base_drc, out_dir))
    return result


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Benchmark one board (no router unless wired in).")
    ap.add_argument("board", help=f"one of {sorted(BENCH_BOARDS)}")
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--rip", choices=["all", "none", "scoped"], default="all")
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--pcb", type=Path, default=None,
                    help="use this board instead of BENCH_BOARDS[board] (synthetic fixtures)")
    args = ap.parse_args(argv)
    run_bench(args.board, args.out_dir, rip=args.rip, threads=args.threads, pcb_override=args.pcb)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
