#!/usr/bin/env python3
"""draw-absorb Goal 3 RCA (read-only): why no over-full cut predicted the model gaps of a run.

    python3 tools/draw_absorb_gap_rca.py <dump dir with realise-L*.json> [<result.json>] [--json]

The dump comes from a run with TOPO_ERBS_DUMP=<dir> (erbs.rs `realise`: per layer the ERBS of every stretch,
its violations and, since draw-absorb A3, every cut that carries a stretch with its slack). For every model gap
(a non-predicted violation between two stretches: `predicted` = an over-full cut carries both) it reports:

- the cuts that carry BOTH stretches, the nearest one to the gap and its slack (>= 0: the cut test said they fit);
- the nearest cut carrying only one of them (the other passes that cut's ends elsewhere);
- whether the gap lies at a terminal of either band (within 2 pitches of an end: pad exit / via end);
- the class: `no_common_cut` (no cut carries both within 1 mm), `common_cut_fits` (a cut carries both with
  slack >= 0 within 1 mm: the cut count holds, the ERBS geometry between the cuts does not), `at_terminal`.

With result.json, the gap pairs the joint commit kept (route.erbs.gap_kept_list, by wire) are marked.
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path


def seg_dist(p, a, b):
    ax, ay = a
    bx, by = b
    px, py = p
    dx, dy = bx - ax, by - ay
    l2 = dx * dx + dy * dy
    t = 0.0 if l2 <= 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / l2))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def main(argv: list[str]) -> int:
    as_json = "--json" in argv
    args = [a for a in argv if a != "--json"]
    d = Path(args[0])
    kept_pairs: set[tuple[int, int]] = set()
    if len(args) > 1:
        r = json.loads(Path(args[1]).read_text())
        for g in ((r.get("route") or r).get("erbs") or {}).get("gap_kept_list", []):
            kept_pairs.add(tuple(sorted((g["wire"], g["other_wire"]))))
    rows = []
    for f in sorted(d.glob("realise-L*.json")):
        j = json.loads(f.read_text())
        es = {e["stretch"]: e for e in j["erbs"]}
        cuts = j.get("cuts", [])
        for v in j["violations"]:
            if v["predicted"] or not v["other"].startswith("Stretch("):
                continue
            a, b = v["stretch"], int(v["other"][len("Stretch("):-1])
            if a not in es or b not in es:
                continue
            ea, eb = es[a], es[b]
            at = v["at"]
            both = [c for c in cuts if a in c["st"] and b in c["st"]]
            near_both = min(((seg_dist(at, c["p"], c["q"]), c) for c in both), default=(math.inf, None), key=lambda x: x[0])
            one = [c for c in cuts if (a in c["st"]) != (b in c["st"])]
            near_one = min(((seg_dist(at, c["p"], c["q"]), c) for c in one), default=(math.inf, None), key=lambda x: x[0])
            pitch = 0.5 * (ea["w"] + eb["w"]) + max(ea["s"], eb["s"])
            ends = [ea["path"][0], ea["path"][-1], eb["path"][0], eb["path"][-1]]
            dterm = min(math.hypot(at[0] - p[0], at[1] - p[1]) for p in ends)
            if dterm < 2 * pitch:
                cls = "at_terminal"
            elif near_both[1] is not None and near_both[0] < 1.0:
                cls = "common_cut_fits" if near_both[1]["slack"] >= 0 else "common_cut_overfull"
            else:
                cls = "no_common_cut"
            pair = tuple(sorted((ea["wire"], eb["wire"])))
            rows.append({
                "layer": j["layer"], "stretches": [a, b], "wires": list(pair), "nets": [ea["net"], eb["net"]], "at": at,
                "mm": v["mm"], "pitch": pitch, "class": cls, "kept": pair in kept_pairs,
                "common_cuts": len(both),
                "near_common": None if near_both[1] is None else {"d": round(near_both[0], 3), "slack": near_both[1]["slack"], "n": len(near_both[1]["st"])},
                "near_one": None if near_one[1] is None else {"d": round(near_one[0], 3), "slack": near_one[1]["slack"]},
                "d_terminal": round(dterm, 3),
            })
    if as_json:
        print(json.dumps(rows, indent=1))
        return 0
    from collections import Counter
    c_all = Counter(r["class"] for r in rows)
    c_kept = Counter(r["class"] for r in rows if r["kept"])
    print(f"model gaps between stretches: {len(rows)}; by class {dict(c_all)}; of the joint commit's gap pairs: {dict(c_kept)} ({len(kept_pairs)} pairs)")
    for r in rows:
        print(f"L{r['layer']} wires {r['wires']} nets {r['nets']} at ({r['at'][0]:.3f},{-r['at'][1]:.3f}) {r['mm']:.3f} mm pitch {r['pitch']:.3f} {r['class']}"
              f"{' KEPT' if r['kept'] else ''} common {r['common_cuts']} near_common {r['near_common']} near_one {r['near_one']} d_term {r['d_terminal']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
