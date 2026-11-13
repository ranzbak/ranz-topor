"""Frozen classic reference (S6 gate): crop runner and table writer for tools/classic_reference.sh.

    python -P harness/classic_ref.py crop <model.json> <name> <out.json>   # engine-only run of a model crop
    python -P harness/classic_ref.py table <results_dir> <out_prefix>      # *--metrics.json + crop-*.json -> .md/.json

The board rows come from `harness/route_board.py` (`<name>--metrics.json`); the crops are engine models
(no .kicad_pcb), so they carry routed / vias / detour / runtime from the engine report only: no KiCad DRC.
"""
from __future__ import annotations

import json
import sys
import time
from pathlib import Path

ORDER = ["qmtech", "qmtech-usb-crop", "esp-rust-board-usb-crop", "eprom-to-kernal", "wiccon-2023", "tanmatsu",
         "bornhack2026", "hh2027"]


def crop(model_json: Path, name: str, out: Path) -> dict:
    import topor_engine
    from toporlib.detour import detour_stats

    model = json.loads(Path(model_json).read_text())
    opts = json.loads(topor_engine.default_options())
    t0 = time.monotonic()
    res = json.loads(topor_engine.route(json.dumps(model), json.dumps(opts)))
    dt = time.monotonic() - t0
    items = res.get("items") or []
    det = detour_stats(res.get("connections"))
    m = {
        "board": name, "kind": "engine-crop", "engine": getattr(topor_engine, "__file__", "?"),
        "connections": res.get("connections_total"), "routed": res.get("routed"),
        "routed_pct": round(100.0 * res["routed"] / res["connections_total"], 2) if res.get("connections_total") else None,
        "unconnected": None, "routing_new": None,
        "vias": sum(1 for it in items if it.get("kind") == "via"),
        "detour": ({k: det[k] for k in ("median", "p90", "max", "count_gt_1_5", "n", "no_reference")} if det else None),
        "runtime_s": round(dt, 2), "router_s": round(dt, 2),
        "options": opts,
    }
    Path(out).write_text(json.dumps(m, indent=1))
    return m


def _fmt(v, nd=None):
    if v is None:
        return "n/a"
    return f"{v:.{nd}f}" if nd is not None and isinstance(v, float) else str(v)


def table(results: Path, prefix: Path) -> None:
    rows = {}
    for p in sorted(Path(results).rglob("*--metrics.json")):
        m = json.loads(p.read_text())
        rows[m["board"]] = m
    for p in sorted(Path(results).glob("crop-*.json")):
        m = json.loads(p.read_text())
        rows[m["board"]] = m
    names = [n for n in ORDER if n in rows] + sorted(n for n in rows if n not in ORDER)
    head = ("| board | connections | routed | routed % | DRC unconnected | hand unconnected | routing_new | vias | "
            "hand vias | detour median / p90 / max | runtime s | router s | status |")
    lines = [head, "|" + "---|" * 13]
    for n in names:
        m = rows[n]
        d = m.get("detour")
        dl = f"{d['median']} / {d['p90']} / {d['max']}" if d else "n/a"
        lines.append(
            f"| {n} | {_fmt(m['connections'])} | {_fmt(m['routed'])} | {_fmt(m['routed_pct'])} | "
            f"{_fmt(m['unconnected'])} | {_fmt(m.get('unconnected_hand'))} | {_fmt(m['routing_new'])} | "
            f"{_fmt(m['vias'])} | {_fmt(m.get('vias_hand'))} | {dl} | {_fmt(m['runtime_s'])} | "
            f"{_fmt(m.get('router_s'))} | {m.get('status', 'engine-only')} |")
    Path(f"{prefix}.md").write_text("\n".join(lines) + "\n")
    Path(f"{prefix}.json").write_text(json.dumps({n: rows[n] for n in names}, indent=1))
    print("\n".join(lines))


if __name__ == "__main__":
    a = sys.argv[1:]
    if a[:1] == ["crop"] and len(a) == 4:
        print(json.dumps(crop(Path(a[1]), a[2], Path(a[3])))[:400])
    elif a[:1] == ["table"] and len(a) == 3:
        table(Path(a[1]), Path(a[2]))
    else:
        sys.exit(__doc__)
