"""last-preview cost probe (docs/dev/design/last-preview.md): the router's wall time with the progress dialog's board
image on vs off, and the copper of both (must be the same bits).

    python -m harness.preview_cost esp-rust-board OUT_DIR [--reps 3] [--interval 2.0]

The board is stripped and exported as `harness.route_board` does; the engine options are the plugin's
(`run.build_options` of the defaults, headless stop at "Routes stable"). Each rep routes twice, off then on
(alternating order per rep, so a drift of the machine hits both). "on" does what the plugin's poll loop does
(`run.sketch_poll` at POLL_S): enable sketches, ask for one every `--interval` s, fetch the shown one on a new
seq and build its SVG (`preview.live_svg`); only the wx rasterising of the dialog is left out. Wall = route_async
to done (poll-quantised); the medians use the engine's runtime_s. Writes OUT_DIR/preview-cost.json. Timing numbers only from an exclusive ccx43 run.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import sys
import time
from pathlib import Path


def board_model(board: str, out_dir: Path):
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.route_board import _load_strip
    from harness.rules import resolve_rules
    from topor.config import Config
    from topor.layers import is_routed, resolve_layer_roles_ex

    strip = _load_strip()
    hand, inp, _ = strip.prepare(board, out_dir)
    name = Path(inp.pcb).parent.name.removesuffix("-input")
    widths = record_widths(load_board(hand))
    dru = inp.dru.read_text() if inp.dru is not None else None
    ib = load_board(inp)
    cfg = Config.defaults()
    rules = resolve_rules(ib, dru_text=dru, widths_before=widths, fab_profile=str(cfg.get("rules", "fab_profile")))
    model = export_model(ib, rules, name=name, dru_text=dru)
    roles, _, _ = resolve_layer_roles_ex([(l["name"], l["kind"]) for l in model["layers"]], {})
    for l in model["layers"]:
        if l["name"] in roles:
            l["routable"] = is_routed(roles[l["name"]])
    return cfg, model


def route_once(engine, mj: str, oj: str, base, sketches: bool, interval: float) -> dict:
    from topor import preview
    from topor.run import POLL_S

    t0 = time.monotonic()
    job = engine.route_async(mj, oj)
    if sketches:
        job.enable_sketches()
    last_req, seen, built, svg_s = -1e9, -1, 0, 0.0
    while not job.done():
        now = time.monotonic()
        p = json.loads(job.progress())
        if sketches:
            if now - last_req >= interval:
                job.request_sketch()
                last_req = now
            if p.get("sketch_seq", 0) != seen:
                seen = p.get("sketch_seq", 0)
                s = job.sketch("last")
                if s:
                    t1 = time.monotonic()
                    preview.live_svg(base, json.loads(s)["items"])
                    svg_s += time.monotonic() - t1
                    built += 1
        time.sleep(POLL_S)
    wall = time.monotonic() - t0
    r = json.loads(job.wait())
    key = json.dumps([r.get("items"), r.get("connections")], sort_keys=True)
    return {"sketches": sketches, "wall_s": round(wall, 3), "runtime_s": round(float(r.get("runtime_s") or 0.0), 3), "copper_sha": hashlib.sha256(key.encode()).hexdigest()[:16],
            "items": len(r.get("items") or []), "images": built, "svg_s": round(svg_s, 3),
            "sketch_seq": json.loads(job.progress()).get("sketch_seq", 0)}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("board")
    ap.add_argument("out", type=Path)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--interval", type=float, default=2.0)
    a = ap.parse_args(argv)
    import topor_engine
    from topor import preview
    from topor.run import build_options

    a.out.mkdir(parents=True, exist_ok=True)
    cfg, model = board_model(a.board, a.out)
    opts = build_options(cfg, topor_engine, model=model)
    opts.update({"wall_clock_s": 600.0, "stop_when_stable": True})
    mj, oj = json.dumps(model), json.dumps(opts)
    base = preview.live_base(model)
    rows = []
    for rep in range(a.reps):
        for on in ((False, True) if rep % 2 == 0 else (True, False)):
            rows.append({"rep": rep, **route_once(topor_engine, mj, oj, base, on, a.interval)})
            print(json.dumps(rows[-1]), flush=True)
    # the engine's own runtime_s (route start to result, not quantised by the poll interval)
    off = [r["runtime_s"] for r in rows if not r["sketches"]]
    on = [r["runtime_s"] for r in rows if r["sketches"]]
    out = {"board": a.board, "reps": a.reps, "interval_s": a.interval, "runs": opts.get("runs"), "rows": rows,
           "off_median_s": statistics.median(off), "on_median_s": statistics.median(on),
           "delta_pct": round(100.0 * (statistics.median(on) - statistics.median(off)) / statistics.median(off), 2),
           "copper_same": len({r["copper_sha"] for r in rows}) == 1}
    (a.out / "preview-cost.json").write_text(json.dumps(out, indent=1))
    print(json.dumps({k: v for k, v in out.items() if k != "rows"}))
    return 0 if out["copper_same"] else 1


if __name__ == "__main__":
    sys.exit(main())
