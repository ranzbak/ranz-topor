"""stitch-vias measurement of a routed board FILE after KiCad's own zone refill (kicad-cli DRC --refill-zones
--save-board on a copy): the unconnected items (as a set of item pairs), the isolated-copper items, the DRC
violation types (all severities), the via count and the filled pour area per (net, layer).

    python -m tools.stitchvias_measure <routed.kicad_pcb> --out <m.json> [--work <dir>]

O(one DRC run + len(board text)).
"""
from __future__ import annotations

import argparse
import json
import re
import shutil
from collections import Counter
from pathlib import Path


def _area(pts) -> float:
    s = 0.0
    for (x0, y0), (x1, y1) in zip(pts, pts[1:] + pts[:1]):
        s += x0 * y1 - x1 * y0
    return abs(s) / 2.0


def fill_areas(text: str) -> dict:
    """{"NET LAYER": mm²} of the filled polygons of every zone (fractured rings: shoelace = area)."""
    from toporlib import pcbfile
    fills = pcbfile.parse_zone_fills(text)
    nets: dict = {}
    for m in re.finditer(r"\(zone\b(.{0,400}?)\(uuid \"([^\"]+)\"", text, re.S):
        nm = re.search(r"\(net_name \"([^\"]*)\"\)", m.group(1)) or re.search(r"\(net \"([^\"]*)\"\)", m.group(1))
        nets[m.group(2)] = nm.group(1) if nm else "?"
    out: Counter = Counter()
    for u, polys in fills.items():
        for p in polys:
            out[f"{nets.get(u, '?')} {p['layer']}"] += _area(p["pts"]) / 1e12
    return {k: round(v, 2) for k, v in sorted(out.items())}


def measure(pcb: Path, work: Path) -> dict:
    from harness.kicad_env import run_drc
    from toporlib import pcbfile
    work.mkdir(parents=True, exist_ok=True)
    for ext in (".kicad_pcb", ".kicad_pro", ".kicad_dru"):
        src = pcb.with_suffix(ext)
        if src.is_file():
            shutil.copy(src, work / src.name)
    cp = work / pcb.name
    js = work / f"{pcb.stem}--drc-refill.json"
    run_drc(cp, js, save_board=True)
    d = json.loads(js.read_text())
    unc = sorted(" | ".join(sorted(f"{i['description']}" for i in u["items"])) for u in d.get("unconnected_items", []))
    viol = d.get("violations", [])
    types = Counter(f"{v.get('type')}:{v.get('severity')}" for v in viol)
    iso = sorted(" | ".join(i["description"] for i in v["items"]) for v in viol if v.get("type") == "isolated_copper")
    text = cp.read_text()
    vias = len(pcbfile.find_items(text, ("via",)))
    return {"board": str(pcb), "unconnected": unc, "n_unconnected": len(unc), "isolated_copper": iso,
            "types": dict(sorted(types.items())), "vias": vias, "fill_mm2": fill_areas(text)}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pcb", type=Path)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--work", type=Path)
    a = ap.parse_args(argv)
    m = measure(a.pcb, a.work or a.out.parent / (a.out.stem + "-work"))
    a.out.write_text(json.dumps(m, indent=1))
    print(json.dumps({k: m[k] for k in ("n_unconnected", "vias")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
