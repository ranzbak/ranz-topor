"""Headless route of one board through the plugin's real run flow, with metrics and a hand-vs-topor PNG.

    tools/route_board.sh <board|pcb> <out_dir> [--timeout S] [--set section.key=value ...] [--whole-project]

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
  report, when the engine emits it), runtime, ladder warnings (when the engine report has them).
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


def route_board(board: str, out_dir: Path, *, overrides: dict | None = None, timeout_s: float | None = None,
                whole_project: bool = False, png: bool = True, name: str | None = None) -> dict:
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
    rules = resolve_rules(ib, dru_text=dru, widths_before=widths)
    model = export_model(ib, rules, name=name, dru_text=dru)
    del hb, ib

    cfg = Config.defaults()
    ch = dict(overrides or {})
    if timeout_s is not None:
        ch[("run", "timeout_s")] = float(timeout_s)
    if ch:
        cfg = cfg.with_values(ch)

    app = None
    try:  # the run flow renders its preview with wx; without a display it degrades to a warning
        import wx
        app = wx.App(False)
    except Exception:  # noqa: BLE001
        app = None

    os.environ.setdefault("TOPOR_REPORT_ROOT", str(out_dir / "reports"))
    eng = _CapturingEngine(topor_engine)
    fb = FakeBoard(inp.pcb, model, project_dir=inp.pcb.parent)
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
        "report_dir": str(outcome.report_dir) if outcome.report_dir else None,
    }
    if png:
        try:
            from harness.render import caption, compose_compare, render_png
            h = caption(render_png(hand.pcb, out_dir / f"{name}-hand-raw.png"), f"{name}: hand",
                        out_dir / f"{name}-hand.png")
            t = caption(render_png(routed_pcb, out_dir / f"{name}-topor-raw.png"),
                        f"{name}: {os.environ.get('TOPOR_PNG_LABEL', 'topor')} ({routed}/{total} routed, {m['unconnected']} unconnected)",
                        out_dir / f"{name}-topor.png")
            compose_compare(h, t, out_dir / f"{name}-compare.png")
            m["png"] = str(out_dir / f"{name}-compare.png")
        except Exception as e:  # noqa: BLE001
            m["png"] = None
            m["png_error"] = f"{type(e).__name__}: {e}"
    (out_dir / f"{name}--metrics.json").write_text(json.dumps(m, indent=1))
    return m


def summary(m: dict) -> str:
    d = m["detour"]
    dl = f"median {d['median']} p90 {d['p90']} max {d['max']} (n={d['n']})" if d else "n/a"
    return "\n".join([
        f"route_board {m['board']}: status {m['status']}, engine {m['engine_version']}",
        f"  routed {m['routed']}/{m['connections']} ({m['routed_pct']} %)",
        f"  DRC unconnected {m['unconnected']} (hand {m['unconnected_hand']}, input {m['unconnected_input']}), "
        f"routing_new {m['routing_new']}",
        f"  vias {m['vias']} (hand {m['vias_hand']}), detour {dl}, ladder warnings {m['ladder_warnings']}",
        f"  runtime {m['runtime_s']} s (run flow {m['run_flow_s']} s, router {m['router_s']} s)",
        f"  stop: {m['stop_reason']}; findings: {', '.join(m['findings']) or 'none'}",
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
    ap.add_argument("--set", action="append", default=[], metavar="SECTION.KEY=VALUE",
                    help="override a topor.toml setting (JSON value), repeatable")
    ap.add_argument("--whole-project", action="store_true", help="copy the entire project directory")
    ap.add_argument("--no-png", action="store_true")
    ap.add_argument("--name", default=None, help="board name in file names (default: key or pcb stem)")
    a = ap.parse_args(argv)
    m = route_board(a.board, a.out_dir, overrides=dict(_kv(s) for s in a.set), timeout_s=a.timeout,
                    whole_project=a.whole_project, png=not a.no_png, name=a.name)
    print(summary(m))
    return 0


if __name__ == "__main__":
    sys.exit(main())
