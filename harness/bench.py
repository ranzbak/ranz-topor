"""End-to-end benchmark of one board: copy -> baseline DRC -> rip -> route -> write -> DRC -> images.

Source boards are never written: the procedure runs on a copy under `<out_dir>/<board>-work/`.
Full JSON and images stay on disk; `run_bench` prints a summary of at most 20 lines.
"""
from __future__ import annotations

import argparse
import json
import math
import shutil
import time
from dataclasses import asdict
from pathlib import Path
from typing import Callable, Literal

from harness import writer
from harness.copy import assert_unchanged, copy_project
from harness.drc import classify, new_routing, run_drc, summarise
from harness.metrics import copper_metrics
from harness.model import export_model, load_board, write_model_json
from harness.paths import BENCH_BOARDS
from harness.render import caption, compose_compare, render_png
from harness.rip import record_widths, rip_tracks
from harness.router import drc_spots
from harness.rules import resolve_rules
from harness.smoother import drc_pins

SUMMARY_MAX = 20
Router = Callable[[dict, dict], "list[dict] | dict"]


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

def _zone_reach(board) -> float:
    """Proximity radius (nm) for zone-fill neck violations: twice the largest (clearance + min
    thickness) of the copper pours (fill keeps that clearance, then drops slivers thinner than
    min thickness); 1 mm without pours."""
    r = [int(z.GetLocalClearance() or 0) + int(z.GetMinThickness()) for z in board.Zones()
         if not z.GetIsRuleArea() and not z.IsTeardropArea()]
    return 2.0 * max(r) if r else 1_000_000.0


def _prim_len(prim: dict) -> float:
    """Length (nm) of a seg or arc primitive of the wire JSON."""
    if prim.get("kind") == "arc":
        a, m, e = prim["start"], prim["mid"], prim["end"]
        d = 2.0 * (a[0] * (m[1] - e[1]) + m[0] * (e[1] - a[1]) + e[0] * (a[1] - m[1]))
        chord = math.dist(a, e)
        if abs(d) < 1e-9:
            return chord
        ux = ((a[0]**2 + a[1]**2) * (m[1] - e[1]) + (m[0]**2 + m[1]**2) * (e[1] - a[1])
              + (e[0]**2 + e[1]**2) * (a[1] - m[1])) / d
        uy = ((a[0]**2 + a[1]**2) * (e[0] - m[0]) + (m[0]**2 + m[1]**2) * (a[0] - e[0])
              + (e[0]**2 + e[1]**2) * (m[0] - a[0])) / d
        r = math.dist((ux, uy), a)
        ang = lambda p: math.atan2(p[1] - uy, p[0] - ux)
        sweep = (ang(e) - ang(a)) % (2 * math.pi)
        if (ang(m) - ang(a)) % (2 * math.pi) > sweep:  # mid not on the ccw side: go the other way
            sweep = 2 * math.pi - sweep
        return r * sweep
    return math.dist(prim["a"], prim["b"])


_DEFAULT_LAYER_NAMES = {0: "F.Cu", 2: "B.Cu", 31: "B.Cu"}


def _route_summary(report: dict | None, layers: list[dict] | None = None) -> dict | None:
    """Condensed topor `RouteResult` (None for other routers' reports). `layers` is the model's
    layer list (id -> name for the per-layer length)."""
    if not report or "connections" not in report:
        return None
    names = {int(l["id"]): l["name"] for l in layers or []} or _DEFAULT_LAYER_NAMES
    items = report.get("items") or []
    by_layer: dict[str, float] = {}
    for it in items:
        if it.get("kind") != "via" and it.get("prim"):
            n = names.get(int(it["layer"]), str(it["layer"]))
            by_layer[n] = by_layer.get(n, 0.0) + _prim_len(it["prim"])

    def mm(key):
        v = report.get(key)
        return None if v is None else round(v / 1e6, 3)

    return {
        "connections": report.get("connections_total", 0),
        "routed": report.get("routed", 0),
        "partial": report.get("partial", 0),
        "failed": report.get("failed", 0),
        "nets": report.get("nets_total", 0),
        "nets_routed": report.get("nets_routed", 0),
        "length_mm": mm("total_length_nm"),
        "outside_mm": mm("length_outside_pads_nm"),
        "vias": sum(1 for it in items if it.get("kind") == "via"),
        "length_by_layer_mm": {n: round(v / 1e6, 3) for n, v in sorted(by_layer.items())},
        "threads_used": report.get("threads_used"),
        "hand_outside_mm": None,
        "iterations": report.get("iterations"),
        "timed_out": report.get("timed_out"),
        "clinches": report.get("clinches"),
        "zero_slack_wires": [{"id": w.get("conn"), "net": w.get("name")}
                             for w in (report.get("zero_slack_wires") or [])],
        "zero_slack": report.get("zero_slack"),
        "via_reduction": report.get("via_reduction"),
    }


def _zero_slack_names(report: dict | None) -> list[str]:
    return [str(w.get("name")) for w in ((report or {}).get("zero_slack_wires") or [])]


def _smooth_summary(report: dict | None) -> dict | None:
    if not report or "connections" in report:
        return None
    nets = report.get("nets") or []
    wires = report.get("wires") or []

    def mm(key):
        v = report.get(key)
        return None if v is None else round(v / 1e6, 3)

    return {
        "nets_taut": sum(1 for n in nets if n.get("status") == "taut"),
        "nets": len(nets),
        "wires_taut": sum(1 for w in wires if w.get("status") == "taut"),
        "wires": len(wires),
        "length_mm": mm("total_length_nm"),
        "hand_mm": mm("hand_length_nm"),
        "outside_mm": mm("length_outside_pads_nm"),
        "hand_outside_mm": mm("hand_outside_pads_nm"),
        "iterations": report.get("iterations"),
        "timed_out": report.get("timed_out"),
        "wires_zero_slack": report.get("wires_zero_slack",
                                       sum(1 for w in wires if w.get("zero_slack"))),
        "fallback_reasons": [
            {"id": w.get("id"), "net": w.get("net"), "zero_slack": bool(w.get("zero_slack")),
             "reason": w.get("reason", "")}
            for w in wires if w.get("status") == "fallback"],
        "length_outside_pads_mm": mm("length_outside_pads_nm"),
        "hand_outside_pads_mm": mm("hand_outside_pads_nm"),
    }


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
    fl = result.get("floors") or []
    if fl:
        head.append("  floors: " + "; ".join(f"{f['name']} {f['design_mm']:g} -> {f['floor_mm']:g} mm"
                                             for f in fl))
    sm = result.get("smooth")
    if sm:
        head.append(f"  smooth: taut {sm['nets_taut']}/{sm['nets']} nets, "
                    f"{sm['wires_taut']}/{sm['wires']} wires, {sm['length_mm']} mm "
                    f"(hand {sm['hand_mm']}), outside pads {sm['outside_mm']} "
                    f"(hand {sm['hand_outside_mm']}), iters {sm['iterations']}, "
                    f"teardrops_removed {result['teardrops_removed']}")
        fb = sm["fallback_reasons"]
        head.append(f"  fallback: {len(fb)} wires (zero slack {sm['wires_zero_slack']})")
        head.extend(f"    wire {f['id']} net {f['net']}"
                    f"{' [zero slack]' if f['zero_slack'] else ''}: {f['reason']}" for f in fb)
    rt = result.get("route")
    if rt:
        cl = rt["clinches"]
        reg = cl.get("regular", 0) if isinstance(cl, dict) else 0
        reg = len(reg) if isinstance(reg, list) else reg
        u = cl.get("u", 0) if isinstance(cl, dict) else cl
        u = len(u) if isinstance(u, list) else u
        head.append(f"  route: routed {rt['routed']}/{rt['connections']} connections, "
                    f"{rt['nets_routed']}/{rt['nets']} nets, partial {rt['partial']}, "
                    f"failed {rt['failed']}, outside pads {rt['outside_mm']} "
                    f"(hand {rt['hand_outside_mm']}), iters {rt['iterations']}, "
                    f"clinches regular {reg} u {u}, vias {rt['vias']}, "
                    + " / ".join(f"{n} {v}" for n, v in rt['length_by_layer_mm'].items()) + " mm")
        zs = result.get("zero_slack") or {}
        names = [w["net"] for w in zs.get("wires") or []]
        head.append(f"  via reduction: {rt.get('via_reduction')}")
        head.append(f"  zero slack: {zs.get('status')}, {len(names)} wire(s)"
                    + (f" ({', '.join(names)})" if names else ""))
    partial = (result.get("router_report") or {}).get("partial") or 0
    if partial > 0:
        head.append(f"  partial={partial}")
    dl = result.get("drc_loop") or {}
    if dl.get("rounds", 1) > 1:
        head.append(f"  drc loop: {dl['rounds']} rounds, {len(dl.get('pinned') or {})} pinned"
                    + (f", {len(dl['spots'])} spots" if dl.get("spots") else ""))
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
    fab_floors = bool(options.pop("apply_fab_floors", True))  # harness-only, not an engine option
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

    dru_text = cp.dru.read_text() if cp.dru is not None else None
    rules = resolve_rules(b, dru_text=dru_text, widths_before=widths_before,
                          apply_fab_floors=fab_floors)
    pre_model = export_model(b, rules, name=board)  # the untouched hand board (hand stats)
    hand_stats = None
    try:
        import topor_engine
        hand_stats = json.loads(topor_engine.wire_stats(json.dumps(pre_model)))
    except ImportError:
        pass

    rep = asdict(rip_tracks(b, mode=rip, scope=options.get("scope"), rf_nets=set(rf_nets)))
    writer.save(b, cp.pcb)
    removed = {k: rep[k] for k in ("removed_tracks", "removed_arcs", "removed_vias", "removed_teardrops")}
    kept = {k: rep[k] for k in ("kept_locked_tracks", "kept_free_vias", "kept_locked_vias",
                                "kept_rf_tracks", "kept_rf_vias")}
    b = load_board(cp)  # fresh board holding the ripped copper just saved

    model = export_model(b, rules, name=board)
    write_model_json(model, out_dir / f"{board}--model.json")

    # DRC loop (plan T1.4, cap `drc_loop_cap`): KiCad DRC after a refill sees copper the engine
    # does not model (zone-fill necks). A dict router's wires named by, or (zone necks) next to,
    # a new routing violation are pinned to their hand copper and the routing is redone.
    loop_cap = int(options.get("drc_loop_cap", 5))
    reach = _zone_reach(b)
    pre_route = cp.pcb.with_name(cp.pcb.name + ".pre-route")
    shutil.copyfile(cp.pcb, pre_route)
    pinned: dict = dict(options.get("pinned") or {})
    spots: list = list(options.get("drc_spots") or [])
    # Zero-slack retry judge (R6): a topor result that used a zero-slack corridor is kept only
    # when KiCad DRC reports no new routing violation and nothing unconnected; otherwise the
    # board is restored and the router runs again with the retry off.
    zero_slack: dict = {"status": "not attempted", "wires": []}
    zs_off = options.get("zero_slack_retry") is False
    rounds, router_s = 0, 0.0
    while True:
        rounds += 1
        t_route = time.monotonic()
        opts = {**options, "threads": threads, **({"pinned": pinned} if pinned else {}),
                **({"drc_spots": spots} if spots else {}),
                **({"zero_slack_retry": False} if zs_off else {})}
        routed = router(model, opts) if router is not None else []
        router_s += time.monotonic() - t_route
        if isinstance(routed, dict):
            items = list(routed.get("items") or [])
            remove_uuids = list(routed.get("remove_uuids") or [])
            router_report = routed.get("report")
        else:
            items, remove_uuids, router_report = list(routed), [], None
        remove_uuids, write, unchanged = writer.skip_unchanged(model["kept"], remove_uuids, items)
        replaced, teardrops_removed = writer.remove_items_and_teardrops(b, remove_uuids)
        created = dict(zip(writer.write_items(b, write), write))
        # Save over the working copy so the copied .kicad_pro/.kicad_dru still apply to the DRC run.
        writer.save(b, cp.pcb)
        routed_drc = run_drc(cp.pcb, out_dir / f"{board}--drc-routed.json")
        if router_report is None or rounds > loop_cap:
            break
        if "connections" in router_report:  # topor: DRC spots for the offending nets
            zs_names = _zero_slack_names(router_report)
            if zs_names and not zs_off:
                wires = [{"id": w.get("conn"), "net": w.get("name")}
                         for w in router_report.get("zero_slack_wires") or []]
                bad = (classify(routed_drc, base_drc)["routing_new"] > 0
                       or len(routed_drc.unconnected) > 0)
                if bad:
                    zero_slack = {"status": "rejected by DRC", "wires": wires}
                    zs_off = True
                    shutil.copyfile(pre_route, cp.pcb)
                    b = load_board(cp)
                    continue
                zero_slack = {"status": "kept", "wires": wires}
            elif zero_slack["status"] == "not attempted" and not zs_off:
                zero_slack = {"status": str(router_report.get("zero_slack") or "not run"),
                              "wires": []}
            have = {(sp["layer"], tuple(sp["at"])) for sp in spots}
            fresh = []
            for sp in drc_spots(new_routing(routed_drc, base_drc), created, model, reach,
                                float(max(rules["min_clearance"],
                                          rules["default"].get("clearance", 0)))):
                if (sp["layer"], tuple(sp["at"])) not in have:
                    have.add((sp["layer"], tuple(sp["at"])))
                    fresh.append(sp)
            if not fresh:
                break
            spots.extend(fresh)
            shutil.copyfile(pre_route, cp.pcb)
            b = load_board(cp)
            continue
        names = {n["code"]: n["name"] for n in model["nets"]}
        more = {u: r for u, r in drc_pins(new_routing(routed_drc, base_drc), created,
                                          router_report.get("wires") or [], names, reach).items()
                if u not in pinned}
        if not more:
            break
        pinned.update(more)
        shutil.copyfile(pre_route, cp.pcb)
        b = load_board(cp)
    pre_route.unlink()
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

    route = _route_summary(router_report, model["layers"])
    if route is not None and hand_stats is not None:
        route["hand_outside_mm"] = round(hand_stats["hand_outside_pads_nm"] / 1e6, 3)
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
        "routed_items": len(write),
        "unchanged_items": unchanged,
        "replaced": replaced,
        "drc_loop": {"rounds": rounds, "pinned": pinned, "spots": spots},
        "zero_slack": zero_slack,
        "teardrops_removed": teardrops_removed,
        "router_report": router_report,
        "smooth": _smooth_summary(router_report),
        "route": route,
        "hand_stats": hand_stats,
        "floors": [{"name": f["name"], "design_mm": f["design"] / 1e6, "floor_mm": f["floor"] / 1e6}
                   for f in rules.get("floors_applied", [])],
        "rules_used": {k: rules[k] / 1e6 for k in ("edge_clearance", "min_clearance", "min_track_width")},
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
    ap.add_argument("--router", choices=["none", "smoother", "topor"], default="none")
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--pcb", type=Path, default=None,
                    help="use this board instead of BENCH_BOARDS[board] (synthetic fixtures)")
    args = ap.parse_args(argv)
    router = None
    if args.router == "smoother":
        from harness.smoother import smooth_router
        router = smooth_router
    elif args.router == "topor":
        from harness.router import route_router
        router = route_router
    run_bench(args.board, args.out_dir, router=router, rip=args.rip, threads=args.threads,
              pcb_override=args.pcb)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
