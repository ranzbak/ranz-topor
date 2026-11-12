"""Headless route of one board through the plugin's real run flow, with metrics and a hand-vs-topor PNG.

    tools/route_board.sh <board|pcb> <out_dir> [--timeout S] [--clock wall|cpu] [--set section.key=value ...] [--whole-project]

What it does:
  1. `harness.strip.prepare`: copies the project twice under <out_dir>: `<name>-hand/` (untouched, the
     reference) and `<name>-input/` (tracks, vias and teardrop zones removed, zones kept).
  2. Builds the engine model of the stripped copy with pcbnew (rules/widths from the hand board, as
     harness.bench does) and runs `topor.run.run` on a `FakeBoard` + `ScriptedUi` (accepts the result):
     the same code path the KiCad plugin takes after the IPC reads, with the engine = `topor_engine`
     on sys.path.
  3. Writes `<out_dir>/<name>-topor/` (the routed copy), `<name>--metrics.json`, `<name>-compare.png`
     (hand left, topor right) and prints a summary.

Metrics (all measured on the routed copy by `kicad-cli pcb drc --refill-zones`, never from engine claims):
  routed / total connections (the run's own count), unconnected (DRC unconnected_items), routing_new
  (DRC violations of routing types that the stripped input does not have), vias, detour ratio (engine
  report, when the engine emits it), runtime, ladder warnings (when the engine report has them), and
  `cleanup_unstable`: what KiCad's Edit -> Cleanup Tracks & Vias would remove or change in topor's copper
  (`harness.cleanup_check` on the routed copy as saved, i.e. with the zone fills the plugin commits; details in
  `<name>--cleanup.json`).
Validity first (M0v, `harness.validity`): the engine-input rules of the run are compared with the rules the expected
answer assumes (a pattern's expected.json ``rules_assumed``, else the board's own files under ``[rules] fab_profile =
"board"``, the default; a rule fallback for a field the board lacks is reported); a difference makes the run INVALID (`<name>--validity.json`, metrics ``valid``, the summary's first line):
its faults are then not router faults.
`TOPOR_TREE` / `TOPOR_ENGINE_SITE` (see tools/route_board.sh) select the code tree and engine wheel, which is
how the frozen classic reference is run from a worktree of main @ 64b3979.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path


def _load_strip():
    """harness/strip.py by file path: the classic-reference tree (TOPOR_TREE) has no harness.strip."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("_topor_strip", Path(__file__).with_name("strip.py"))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    return mod


def _count_vias(pcb: Path) -> int:
    from toporlib import pcbfile
    return len(pcbfile.find_items(pcb.read_text(), ("via",)))


class _CapturingEngine:
    """Delegates to topor_engine; records every parsed route result (the last one is the final round)."""

    def __init__(self, engine):
        self._e = engine
        self.results: list = []

    def __getattr__(self, name):
        return getattr(self._e, name)

    def route_async(self, model, opts):
        job = self._e.route_async(model, opts)
        outer = self

        class _Job:
            def __getattr__(self, n):
                return getattr(job, n)

            def wait(self):
                s = job.wait()
                try:
                    outer.results.append(json.loads(s))
                except Exception:  # noqa: BLE001
                    pass
                return s

        return _Job()


def _ladder(result: dict | None):
    if not result:
        return None
    for k in ("ladder_warnings", "ladder"):
        if k in result:
            v = result[k]
            return len(v) if isinstance(v, (list, dict)) else v
    return None


def board_netclasses(pc) -> tuple:
    """(class name -> NetClassValues, net name -> effective class name) of the project copy `pc`, read with pcbnew:
    what the plugin's ``IpcBoard.netclasses`` returns for the same board. K1: without it FakeBoard reported every net
    in *Default*, so the harness took net priorities from the power-name guess (qmtech: GND, VIN5VPS21 tier 2)
    while the plugin used KiCad's classes (1V8, ADV_AVDD, USB_VBUS ...). O(nets)."""
    from harness.model import load_board
    from harness.rules import _swig_inputs
    from topor.ports import NetClassValues
    _, ncls, nc_of, _ = _swig_inputs(load_board(pc))
    return ({k: NetClassValues(v.name, v.clearance, v.track_width, v.via_diameter, v.via_drill) for k, v in ncls.items()},
            {n: c for n, c in nc_of.items() if n})


def route_board(board: str, out_dir: Path, *, overrides: dict | None = None, timeout_s: float | None = None,
                clock: str | None = None, whole_project: bool = False, png: bool = True, name: str | None = None,
                declared_layers: bool = False, expected: Path | None = None) -> dict:
    import topor_engine
    strip = _load_strip()
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.rules import resolve_rules
    from toporlib.detour import detour_stats
    from toporlib.drc import classify, run_drc
    from topor import run as run_mod
    from topor.config import Config
    from topor.fake import FakeBoard, ScriptedUi

    t_start = time.monotonic()
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    hand, inp, srep = strip.prepare(board, out_dir, whole_project=whole_project, name=name)
    name = Path(inp.pcb).parent.name.removesuffix("-input")

    hb = load_board(hand)
    widths = record_widths(hb)
    dru = inp.dru.read_text() if inp.dru is not None else None
    ib = load_board(inp)
    cfg = Config.defaults()
    ch = dict(overrides or {})
    if timeout_s is not None:
        ch[("run", "timeout_s")] = float(timeout_s)
    if clock is not None:
        ch[("run", "timeout_clock")] = clock
    if ch:
        cfg = cfg.with_values(ch)
    rules = resolve_rules(ib, dru_text=dru, widths_before=widths, fab_profile=str(cfg.get("rules", "fab_profile")))
    model = export_model(ib, rules, name=name, dru_text=dru)
    if not declared_layers:
        # Layer roles as the plugin resolves them (config > KiCad types > defaults by copper count): a
        # 4-layer board whose layers are all declared "signal" has planes in In1 / In2 (qmtech).
        from topor.layers import is_routed, resolve_layer_roles_ex
        roles, _, _ = resolve_layer_roles_ex([(l["name"], l["kind"]) for l in model["layers"]], {})
        for l in model["layers"]:
            if l["name"] in roles:
                l["routable"] = is_routed(roles[l["name"]])
    del hb, ib

    app = None
    try:  # the run flow renders its preview with wx; without a display it degrades to a warning
        import wx
        app = wx.App(False)
    except Exception:  # noqa: BLE001
        app = None

    os.environ.setdefault("TOPOR_REPORT_ROOT", str(out_dir / "reports"))
    eng = _CapturingEngine(topor_engine)
    fb = FakeBoard(inp.pcb, model, project_dir=inp.pcb.parent, netclasses=board_netclasses(inp))
    t_run = time.monotonic()
    outcome = run_mod.run(fb, ScriptedUi(accept=True), cfg, eng, tmp_root=out_dir / "work")
    run_s = time.monotonic() - t_run
    del app

    routed_dir = out_dir / f"{name}-topor"
    routed_pcb = fb.save_copy(routed_dir)
    created = list(outcome.commit.created_ids) if outcome.commit is not None else []

    base = outcome.base_drc or run_drc(inp.pcb, out_dir / f"{name}--drc-input.json")
    hand_drc = run_drc(hand.pcb, out_dir / f"{name}--drc-hand.json")
    routed_drc = run_drc(routed_pcb, out_dir / f"{name}--drc-routed.json")
    cls = classify(routed_drc, base, created, unconnected_baseline=hand_drc)

    res = eng.results[-1] if eng.results else None
    conns = (res or {}).get("connections")
    det = detour_stats(conns) if isinstance(conns, list) else None
    total, routed = int(outcome.total or 0), int(outcome.routed or 0)
    m = {
        "board": name,
        "status": outcome.status,
        "engine": getattr(topor_engine, "__file__", "?"),
        "engine_version": str(topor_engine.version()) if hasattr(topor_engine, "version") else "?",
        "stripped": {"tracks": srep.tracks, "arcs": srep.arcs, "vias": srep.vias,
                     "teardrop_zones": srep.teardrop_zones},
        "connections": total,
        "routed": routed,
        "routed_pct": round(100.0 * routed / total, 2) if total else None,
        "unconnected": len(routed_drc.unconnected),
        "unconnected_hand": len(hand_drc.unconnected),
        "unconnected_input": len(base.unconnected),
        "routing_new": cls["routing_new"],
        "drc_by_type_new": cls["by_type_new"],
        "dangling": {k: cls["incomplete"][k] for k in ("track_dangling", "via_dangling")},
        "vias": _count_vias(routed_pcb),
        "vias_hand": _count_vias(hand.pcb),
        "items_written": len(created),
        "detour": ({k: det[k] for k in ("median", "p90", "max", "count_gt_1_5", "n", "no_reference")}
                   if det else None),
        "ladder_warnings": _ladder(res),
        "stop_reason": outcome.stop_reason,
        "findings": [f"{f.code}: {f.severity}" for f in (outcome.findings or [])][:20],
        "runtime_s": round(time.monotonic() - t_start, 2),
        "run_flow_s": round(run_s, 2),
        "router_s": round(float(outcome.router_s or 0.0), 2),
        "router_cpu_s": round(float(outcome.router_cpu_s or 0.0), 2),
        "timeout_clock": cfg.get("run", "timeout_clock"),
        "timeout_s": cfg.get("run", "timeout_s"),
        "rayon_threads": os.environ.get("RAYON_NUM_THREADS"),
        "report_dir": str(outcome.report_dir) if outcome.report_dir else None,
    }
    # M1 compass (toporlib.compass): per violation class predicted vs drawn vs KiCad DRC, per connection detour and
    # vias planned vs drawn; full table in <name>--compass.json, flat columns in the metrics
    try:
        from toporlib import compass as compass_mod
        from toporlib.drc import new_routing as _new_routing
        cps = compass_mod.build(res or {}, _new_routing(routed_drc, base, created),
                                cls["incomplete"].get("unconnected_new"),
                                drc_radius_mm=float(cfg.get("output", "compass_join_mm")),
                                join_nets=bool(cfg.get("output", "compass_join_nets")))
        (out_dir / f"{name}--compass.json").write_text(json.dumps(cps, indent=1))
        m["compass"] = compass_mod.columns(cps)
        m["compass_line"] = compass_mod.summary_line(cps)
    except Exception as e:  # noqa: BLE001 - a metric, never a failed run
        m["compass"] = None
        m["compass_error"] = f"{type(e).__name__}: {e}"
    # KiCad's Cleanup Tracks & Vias replayed on the routed copy as the plugin hands it back (0 = nothing changes)
    try:
        from harness.cleanup_check import check as cleanup_check
        from harness.cleanup_check import summary as cleanup_summary
        cc = cleanup_check(routed_pcb, input_pcb=inp.pcb)
        (out_dir / f"{name}--cleanup.json").write_text(json.dumps(cc, indent=1))
        m["cleanup_unstable"] = cc["count"]
        m["cleanup_by_rule"] = cc["by_rule"]
        m["cleanup_per_option"] = {k: v["count"] for k, v in (cc.get("per_option") or {}).items()}
        ds = cc.get("dangling_split") or {}
        m["cleanup_dangling"] = {k: ds[k] for k in ("independent", "cascade", "cascade_after") if k in ds}
        m["cleanup_summary"] = cleanup_summary(cc)
    except Exception as e:  # noqa: BLE001 - a metric, never a failed run
        m["cleanup_unstable"] = None
        m["cleanup_error"] = f"{type(e).__name__}: {e}"
    if png:
        try:
            from harness.render import (board_bbox_nm, caption, compose_compare, draw_ratsnest, draw_violations,
                                        render_png, violation_marks_of)
            from toporlib.drc import new_routing, ratsnest_caption, ratsnest_marks
            # Blue lines: KiCad's DRC unconnected items after the refill (the open ratsnest).
            # Yellow rings: routing violations not in the input (user 2026-11-07), the hand board's own on the left.
            hv = violation_marks_of(hand.pcb, new_routing(hand_drc, base))
            hr = render_png(hand.pcb, out_dir / f"{name}-hand-raw.png")
            hr = draw_ratsnest(hr, hr, hand_drc.unconnected, bbox_nm=board_bbox_nm(hand.pcb))
            hr = draw_violations(hr, hr, hv, bbox_nm=board_bbox_nm(hand.pcb))
            h = caption(hr, f"{name}: {os.environ.get('TOPOR_HAND_LABEL', 'hand')} ({m['unconnected_hand']} unconnected, blue"
                            f"{ratsnest_caption(ratsnest_marks(hand_drc.unconnected))} · {len(hv)} violations, yellow)",
                        out_dir / f"{name}-hand.png")
            tv = violation_marks_of(routed_pcb, new_routing(routed_drc, base, created))
            tr = render_png(routed_pcb, out_dir / f"{name}-topor-raw.png")
            tr = draw_ratsnest(tr, tr, routed_drc.unconnected, bbox_nm=board_bbox_nm(routed_pcb))
            tr = draw_violations(tr, tr, tv, bbox_nm=board_bbox_nm(routed_pcb))
            t = caption(tr,
                        f"{name}: {os.environ.get('TOPOR_PNG_LABEL', 'topor')} ({routed}/{total} routed, {m['unconnected']} unconnected, blue"
                        f"{ratsnest_caption(ratsnest_marks(routed_drc.unconnected))} · {len(tv)} violations, yellow)",
                        out_dir / f"{name}-topor.png")
            compose_compare(h, t, out_dir / f"{name}-compare.png")
            m["png"] = str(out_dir / f"{name}-compare.png")
        except Exception as e:  # noqa: BLE001
            m["png"] = None
            m["png_error"] = f"{type(e).__name__}: {e}"
    m.update(_validity(board, hand, inp, cfg, outcome, out_dir, name, expected))
    (out_dir / f"{name}--metrics.json").write_text(json.dumps(m, indent=1))
    return m


def _validity(board: str, hand, inp, cfg, outcome, out_dir: Path, name: str, expected: Path | None) -> dict:
    """M0v validity gate (harness.validity): the engine-input rules of this run against the rules the expected
    answer assumes (`expected` / the pattern's expected.json ``rules_assumed``, else the board's own files).
    Writes `<name>--validity.json`; returns the metrics fields ``valid`` and ``validity``. Never fails the run."""
    from harness import validity as va
    try:
        exp = expected if expected is not None else va.find_expected(Path(board))
        decl = va.declared_of(exp)
        if outcome.model is None:
            return {"valid": None, "validity": {"error": "the run built no model"}}
        eff = va.model_rule_set(outcome.model, cfg)
        brd = va.board_rule_set(inp, hand, cfg)
        # a pattern's reference board (the hand copy) is its expected copper: it must use only what the router may
        v = va.judge(decl, eff, brd, cfg=cfg, expected_path=exp if decl is not None else None,
                     reference_text=Path(hand.pcb).read_text() if exp is not None else None)
        (out_dir / f"{name}--validity.json").write_text(json.dumps(
            {"verdict": v, "declared": decl, "board_files": brd, "effective": eff,
             "expected_json": str(exp) if exp else None}, indent=1))
        return {"valid": v["valid"], "validity": {"source": v["source"], "profile": v["profile"],
                                                  "invalid": [va.diff_text(d) for d in v["invalid"]],
                                                  "notes": v["notes"], "line": va.verdict_line(v)}}
    except Exception as e:  # noqa: BLE001 - a gate failure is reported, never a failed run
        return {"valid": None, "validity": {"error": f"{type(e).__name__}: {e}"}}


def summary(m: dict) -> str:
    d = m["detour"]
    dl = f"median {d['median']} p90 {d['p90']} max {d['max']} (n={d['n']})" if d else "n/a"
    va = m.get("validity") or {}
    return "\n".join([
        va.get("line") or f"VALIDITY: not checked ({va.get('error', 'no data')})",
        f"route_board {m['board']}: status {m['status']}, engine {m['engine_version']}",
        f"  routed {m['routed']}/{m['connections']} ({m['routed_pct']} %)",
        f"  DRC unconnected {m['unconnected']} (hand {m['unconnected_hand']}, input {m['unconnected_input']}), "
        f"routing_new {m['routing_new']}",
        f"  vias {m['vias']} (hand {m['vias_hand']}), detour {dl}, ladder warnings {m['ladder_warnings']}",
        f"  runtime {m['runtime_s']} s (run flow {m['run_flow_s']} s, router {m['router_s']} s, CPU {m['router_cpu_s']} s; limit {m['timeout_s']} {m['timeout_clock']})",
        f"  stop: {m['stop_reason']}; findings: {', '.join(m['findings']) or 'none'}",
        f"  cleanup-unstable items: {m.get('cleanup_unstable')} ({m.get('cleanup_by_rule') or m.get('cleanup_error') or 'none'})"
        + (f"; dangling removals {m['cleanup_dangling'].get('independent')} independent, "
           f"{m['cleanup_dangling'].get('cascade')} cascade after {m['cleanup_dangling'].get('cascade_after')}"
           if m.get("cleanup_dangling") else ""),
        f"  {m.get('compass_line') or 'compass: ' + str(m.get('compass_error', 'n/a'))}",
        f"  png: {m.get('png')}",
    ])


def _kv(s: str):
    k, _, v = s.partition("=")
    sec, _, key = k.partition(".")
    try:
        val = json.loads(v)
    except ValueError:
        val = v
    return (sec, key), val


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("board", help="BENCH_BOARDS key or .kicad_pcb path")
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--timeout", type=float, default=None, help="run.timeout_s (router time limit)")
    ap.add_argument("--clock", choices=("wall", "cpu"), default=None,
                    help="which clock --timeout counts: wall (default) or cpu (CPU seconds of router work, all "
                         "threads summed; comparable across machine load: use it for comparisons)")
    ap.add_argument("--set", action="append", default=[], metavar="SECTION.KEY=VALUE",
                    help="override a topor.toml setting (JSON value), repeatable")
    ap.add_argument("--whole-project", action="store_true", help="copy the entire project directory")
    ap.add_argument("--no-png", action="store_true")
    ap.add_argument("--declared-layers", action="store_true",
                    help="route every copper layer the board declares as signal (default: the plugin's layer roles)")
    ap.add_argument("--name", default=None, help="board name in file names (default: key or pcb stem)")
    ap.add_argument("--expected", type=Path, default=None,
                    help="expected.json whose rules_assumed the validity gate uses (default: the pattern's own, "
                         "next to or one level above the board; none: the board's own files)")
    a = ap.parse_args(argv)
    m = route_board(a.board, a.out_dir, overrides=dict(_kv(s) for s in a.set), timeout_s=a.timeout, clock=a.clock,
                    whole_project=a.whole_project, png=not a.no_png, name=a.name,
                    declared_layers=a.declared_layers, expected=a.expected)
    print(summary(m))
    return 0


if __name__ == "__main__":
    sys.exit(main())
