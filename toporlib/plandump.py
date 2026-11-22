"""The plan of the drawn row (thick-layer F4, ``[output] plan_report``): read, write and summarise ``plan.json.gz``.

The engine keeps the plan it handed to the drawing of the row it drew in its result (``route.plan``, compact JSON,
format ``topor-plan`` version 1, ``engine/src/topo/plandump.rs``); the plugin writes it gzipped as ``plan.json.gz``
into the run's report folder and leaves it out of ``result.json``. So a user's own run can be traced plan vs drawing
without a re-route (docs/user/troubleshoot.md, docs/dev/debug.md).

Format (KiCad mm, y down; layers are signal layer indices into ``layers``):
  row       label (TopoR variant-table row, e.g. "run 14 cycle 9 (gen 8)"; null with loop = cycles), roles, round,
            seed (of the run that drew it), loop
  layers    signal layer names by index
  predicted / predicted_violations / foreign   the compass's predicted items of this plan, by class
  conns     per connection id: planned vias, taut_mm, necks, predicted {class: n}
  wires     per wire: wire, net, net_name, conn, w / s (width, clearance mm), a / b, taut (points), stretches
            [[layer, points]], events, layers_now (per triangle traversal: layer bit mask), passes (k, edge, a, b, len,
            room per layer, need, n wires on the edge, pos = this wire's place in the edge's order, at), crossings
            (k = traversal, other wire / other_k / other_net, layers / other_layers at the crossing), vias (iv =
            traversal interval, k, at, site, deficit mm = static room missing for the via disc), nosite

CLI: ``python3 -m toporlib.plandump <report folder | plan.json.gz> [--conn ID ...] [--net NAME ...]``: the row and one
block per matching wire (layers in order, vias with site, crossings in order with layers, predicted items).

Complexity: write O(plan size) (gzip); load O(size); summary O(wires of the selection x their crossings).
"""

from __future__ import annotations

import argparse
import gzip
import json
import sys
from pathlib import Path

FILE = "plan.json.gz"
FORMAT = "topor-plan"


def write(dir: Path, plan: str | dict) -> Path:
    """Write ``plan`` (the engine's JSON text, or a parsed dict) gzipped to ``<dir>/plan.json.gz``. O(size)."""
    text = plan if isinstance(plan, str) else json.dumps(plan, separators=(",", ":"))
    path = Path(dir) / FILE
    with gzip.open(path, "wt", encoding="utf-8", compresslevel=6) as f:
        f.write(text)
    return path


def load(path: Path | str) -> dict:
    """The plan of a report folder (its ``plan.json.gz``) or of a ``plan.json.gz`` / plain JSON file. Raises
    FileNotFoundError / ValueError (not a topor plan). O(size)."""
    p = Path(path)
    if p.is_dir():
        p = p / FILE
    raw = p.read_bytes()
    text = gzip.decompress(raw).decode("utf-8") if raw[:2] == b"\x1f\x8b" else raw.decode("utf-8")
    d = json.loads(text)
    if not isinstance(d, dict) or d.get("format") != FORMAT:
        raise ValueError(f"{p}: not a topor plan (format {d.get('format') if isinstance(d, dict) else None!r})")
    return d


def _layer(plan: dict, bits) -> str:
    names = plan.get("layers") or []
    return "+".join(names[i] if 0 <= i < len(names) else f"L{i}" for i in bits) or "?"


def _mask(plan: dict, m: int) -> str:
    return _layer(plan, [i for i in range(64) if m >> i & 1])


def row_line(plan: dict) -> str:
    """One line: which row this plan is (label, roles, round, seed, loop). O(1)."""
    r = plan.get("row") or {}
    label = r.get("label") or f"round {r.get('round')}"
    roles = f" [{', '.join(r['roles'])}]" if r.get("roles") else ""
    return (f"plan of {label}{roles}, round {r.get('round')}, seed {r.get('seed')}, loop {r.get('loop')}; "
            f"{len(plan.get('wires') or [])} wires, predicted violations {plan.get('predicted_violations')}")


def wire_lines(plan: dict, w: dict) -> list:
    """The plan of one wire as text: layers along it, vias, crossings in order with layers, predicted items. O(its
    traversals + crossings)."""
    out = [f"wire {w['wire']} net {w.get('net_name') or w['net']} conn {w.get('conn')}: width {w['w']:.3f} mm, "
           f"clearance {w['s']:.3f} mm, {w['a']} -> {w['b']}"]
    seq = []
    for m in w.get("layers_now") or []:
        name = _mask(plan, m)
        if not seq or seq[-1] != name:
            seq.append(name)
    out.append(f"  layers along it: {' | '.join(seq)}")
    for v in w.get("vias") or []:
        out.append(f"  via at {v['at']} (traversal {v['k']} in {v['iv']}): "
                   + ("site" if v.get("site") else "NO SITE") + (f", static room short {v['deficit']:.3f} mm" if v.get("deficit") else ""))
    xs = sorted(w.get("crossings") or [], key=lambda x: x["k"])
    if xs:
        out.append("  crossings in order (this wire's layer / the other's): "
                   + ", ".join(f"net {x['other_net']} {_layer(plan, x['layers'])}/{_layer(plan, x['other_layers'])}" for x in xs))
    c = (plan.get("conns") or {}).get(str(w.get("conn")))
    if c:
        pred = ", ".join(f"{k} {n}" for k, n in sorted((c.get("predicted") or {}).items())) or "none"
        out.append(f"  connection: planned vias {c.get('vias')}, taut {c.get('taut_mm')} mm, necks {c.get('necks')}, predicted: {pred}")
    return out


def select(plan: dict, conns=(), nets=()) -> list:
    """The wires of the given connection ids or net names (all wires when both are empty). O(wires)."""
    conns, nets = {int(c) for c in conns}, set(nets)
    return [w for w in plan.get("wires") or []
            if (not conns and not nets) or w.get("conn") in conns or w.get("net_name") in nets]


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="python3 -m toporlib.plandump", description=__doc__.split("\n\n")[0])
    ap.add_argument("plan", help="report folder or plan.json.gz")
    ap.add_argument("--conn", type=int, action="append", default=[], help="connection id (result.json route.connections[].id)")
    ap.add_argument("--net", action="append", default=[], help="net name")
    a = ap.parse_args(argv)
    plan = load(a.plan)
    print(row_line(plan))
    for w in select(plan, a.conn, a.net):
        print("\n".join(wire_lines(plan, w)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
