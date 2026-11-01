"""Progress PNG: hand vs smoothed topor routing with a numbers caption (user requirement)."""
from __future__ import annotations

import argparse
import datetime
import json
import tempfile
from pathlib import Path

from harness import bench
from harness.render import board_bbox_nm, compose_compare, highlight_points
from harness.render import caption as add_caption
from harness.router import route_router
from harness.smoother import smooth_router

ROOT = Path(__file__).resolve().parents[1]
DETOUR_THRESHOLD = 1.5
# Informational boards: the M2/M3 gate tests run them with these options.
_LONG_BOARD_OPTIONS = {"wall_clock_s": 60, "partial_routes": False}
# M4a gate options of dcsgonebadge (M3-R14, ruling A-C3): whole run <= 10 min, no partial routes.
_M4_GATE_OPTIONS = {"wall_clock_s": 600, "partial_routes": False}


def board_options(board: str, milestone: str = "m3") -> dict | None:
    """run_bench options for `board` (None = defaults); mirrors tests/gate/test_m3_gate.py
    (`milestone` m3: 60 s) and the M4a gate (`milestone` m4: 600 s) for dcsgonebadge."""
    if board != "dcsgonebadge":
        return None
    return dict(_M4_GATE_OPTIONS if milestone == "m4" else _LONG_BOARD_OPTIONS)


def merged_options(board: str, milestone: str, extra: dict | None) -> dict | None:
    """Board defaults with the `--options` JSON merged over them (None when both are empty)."""
    out = {**(board_options(board, milestone) or {}), **(extra or {})}
    return out or None


def progress_png_name(date: str, milestone: str, task: str, board: str | None = None) -> str:
    """`<date>-<milestone>-<task>[-<board>].png`; m1/m3 files keep their board-less names."""
    suffix = f"-{board}" if board and milestone not in ("m1", "m3") else ""
    return f"{date}-{milestone}-{task}{suffix}.png"


def detour_caption(stats: dict | None) -> str | None:
    """`detours > 1.5: N (max r) — <top 3 as name ratio>`; None without usable stats."""
    if not stats or not stats.get("n"):
        return None
    top = ", ".join(f"{r['name']} {r['ratio']:.1f}" for r in stats["top10"][:3])
    s = (f"detours > {DETOUR_THRESHOLD}: {stats['count_gt_1_5']} "
         f"(max {stats['max']:.1f}) \N{EM DASH} {top}")
    if stats.get("field") == "ref_len_nm":
        s += f" vs DRC-feasible reference (ghost fallbacks {stats.get('ghost_fallbacks', 0)}"
        short = stats.get("short_ref", 0)
        s += f", ghost < 2 mm {short})" if short else ")"
    return s


def _pre_unc(drc: dict) -> str:
    """` (pre N)` when N unconnected items are pre-existing (same pair in the ripped baseline)."""
    n = (drc.get("incomplete") or {}).get("unconnected_preexisting", 0)
    return f" (pre {n})" if n else ""


def _sha_suffix(result: dict) -> str:
    """`, pcb <sha12>` of the input pcb actually read (empty if the result carries none)."""
    sha = result.get("pcb_sha256")
    return f", pcb {sha[:12]}" if sha else ""


def m3_caption(task: str, result: dict, threads: int) -> str:
    rt = result.get("route") or {}
    r = result["routed_all"]
    drc = result["routed_drc"]
    zs = result.get("zero_slack") or {}
    by = rt.get("length_by_layer_mm") or {}
    line1 = (f"{task}: {rt.get('routed', 0)}/{rt.get('connections', 0)} connections, "
             f"{rt.get('nets_routed', 0)}/{rt.get('nets', 0)} nets, "
             f"partial {rt.get('partial', 0)}, failed {rt.get('failed', 0)}, "
             f"vias {rt.get('vias', 0)}, arcs {r['arcs']}, "
             f"zero slack {len(zs.get('wires') or [])}, iters {rt.get('iterations')}"
             f"{_sha_suffix(result)}")
    line2 = (f"DRC routing {drc['routing_new']} preexisting {drc.get('routing_preexisting', 0)}, "
             f"unconnected {drc['incomplete']['unconnected_items']}"
             f"{_pre_unc(drc)}, "
             f"outside pads {rt.get('outside_mm') or 0.0:.1f} vs hand "
             f"{(rt.get('hand_outside_mm') or 0.0):.1f} mm, "
             f"F.Cu {by.get('F.Cu', 0.0):.1f} / B.Cu {by.get('B.Cu', 0.0):.1f} mm, "
             f"router {result['router_s']:.2f} s on {rt.get('threads_used') or threads} threads")
    lines = [line1, line2]
    d = detour_caption(result.get("detour"))
    if d:
        lines.append(d)
    stop = (result.get("router_report") or {}).get("stop_reason")
    if stop:  # M4 engines report why the loop ended (always the last line)
        lines.append(f"stop_reason: {stop}")
    return "\n".join(lines)


def _highlight_detours(raw: Path, result: dict, pcb: Path) -> Path:
    """Ring both terminals of every routed connection with detour ratio > 1.5 on the PNG."""
    stats = result.get("detour")
    if not stats or not stats.get("n"):
        return raw
    pairs = []
    for c in (result.get("router_report") or {}).get("connections") or []:
        g = c.get(stats.get("field") or "ghost_len_nm")
        ln = c.get("length_nm")
        if c.get("status") == "routed" and g and g > 0 and ln and ln / g > DETOUR_THRESHOLD:
            pairs.append((c["from"]["at"], c["to"]["at"]))
    if not pairs:
        return raw
    return highlight_points(raw, raw.with_name(raw.stem + "-detours.png"), pairs,
                            bbox_nm=board_bbox_nm(pcb))


def render_progress(task: str, *, board: str = "eprom-to-kernal", date: str | None = None,
                    out_dir: Path = ROOT / "docs/benchmarks/progress",
                    pcb_override: Path | None = None, router: str = "smoother",
                    threads: int = 4, milestone: str | None = None,
                    options: dict | None = None) -> dict:
    """Render the hand-vs-topor PNG. `milestone` defaults to m4 for the topor router (m1 for the
    smoother); `options` are merged over the board's default run options."""
    if router == "topor":
        return _render_progress_m3(task, board=board, date=date, out_dir=out_dir,
                                   pcb_override=pcb_override, threads=threads,
                                   milestone=milestone or "m4", options=options)
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
                 f"fallback {wires_total - wires_taut} (zero slack {zs}), arcs {r['arcs']}"
                 f"{_sha_suffix(result)}")
        line2 = (f"DRC routing {drc['routing_new']} preexisting {drc.get('routing_preexisting', 0)}, "
                 f"unconnected {drc['incomplete']['unconnected_items']}"
                 f"{_pre_unc(drc)}, "
                 f"{length_mm:.1f} mm vs hand {hand_mm:.1f} mm, "
                 f"outside pads {sm.get('outside_mm') or 0.0:.1f} vs "
                 f"{sm.get('hand_outside_mm') or 0.0:.1f} mm")
        text = f"{line1}\n{line2}"
        topor_png = add_caption(tmp / f"{board}-work" / f"{board}-topor-raw.png", text,
                                tmp / "topor-progress.png")
        png = out_dir / progress_png_name(date, milestone or "m1", task)
        compose_compare(tmp / f"{board}-hand.png", topor_png, png)
    result["caption"] = text
    result["png"] = str(png)
    print(png)
    print(text)
    return result


def _render_progress_m3(task: str, *, board: str, date: str | None, out_dir: Path,
                        pcb_override: Path | None, threads: int = 4, milestone: str = "m4",
                        options: dict | None = None) -> dict:
    date = date or datetime.date.today().isoformat()
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="topor-progress-") as tmp:
        tmp = Path(tmp)
        result = bench.run_bench(board, tmp, router=route_router, rip="all", threads=threads,
                                 pcb_override=pcb_override,
                                 options=merged_options(board, milestone, options))
        text = m3_caption(task, result, threads)
        work = tmp / f"{board}-work"
        pcb = work / bench._source_pcb(board, pcb_override).name
        raw = _highlight_detours(work / f"{board}-topor-raw.png", result, pcb)
        topor_png = add_caption(raw, text, tmp / "topor-progress.png")
        png = out_dir / progress_png_name(date, milestone, task, board)
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
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--milestone", choices=["m3", "m4"], default="m4",
                    help="output name <date>-<milestone>-<task>[-<board>].png; m4 also selects "
                         "the M4 gate options of dcsgonebadge (600 s)")
    ap.add_argument("--options", default=None, metavar="JSON",
                    help="engine options JSON merged over the board defaults, "
                         "e.g. '{\"wall_clock_s\": 300}'")
    args = ap.parse_args(argv)
    extra = json.loads(args.options) if args.options else None
    render_progress(args.task, board=args.board, pcb_override=args.pcb, router=args.router,
                    threads=args.threads, milestone=args.milestone, options=extra)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
