"""PNG + summary of the PROPOSED TW6 patterns (harness/tw6_patterns.py): expected | topor (one run per cfg), same view and
scale; B.Cu rule areas hatched, the spots without via room marked x. Run on the VM:

    python -m harness.tw6_patterns_png RUNS OUT [name ...]
RUNS/<cfg>/<name>/<name>/s0/<name>-topor/<name>.kicad_pcb (harness.score_runs layout), one panel per cfg dir."""
from __future__ import annotations

import glob
import json
import math
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.patches import Polygon  # noqa: E402

from harness.fx_survivors_png import _panel, read_copper  # noqa: E402
from harness.tw6_patterns import PATTERNS, STATUS  # noqa: E402


def _marks(ax, pat):
    for k in pat.keepouts:
        ax.add_patch(Polygon(k.poly, closed=True, fc="#eeeeee", ec="#777777", hatch="xx", lw=0.6, alpha=0.6, zorder=1))
    for x, y in pat.extra["tw6"].get("no_site_marks", []):
        ax.plot([x], [y], marker="x", ms=11, mew=2.2, color="#d62728", zorder=7)


def _join(segs):
    """Joins polylines of one net and layer that meet end to start (the kept hand track and the new copper that continues
    it are one track on the board), so the panel shows no round cap at the joint."""
    out = []
    for layer, net, w, pts in segs:
        prev = out[-1] if out else None
        if prev and prev[0] == layer and prev[1] == net and prev[2] == w and math.dist(prev[3][-1], pts[0]) < 1e-6:
            out[-1] = (layer, net, w, list(prev[3]) + list(pts[1:]))
        else:
            out.append((layer, net, w, list(pts)))
    return out


def _reach(pat, segs, vias) -> dict:
    """'matches': the routed copper has the expected via count, a length within 5 % of the expected and, for lead_via_end,
    its via on y 6 inside an x range with via room (the expected end state; DRC is shown separately)."""
    e = pat.extra["tw6"]
    ln = sum(sum(math.dist(a, b) for a, b in zip(p, p[1:])) for _, _, _, p in segs)
    ok = len(vias) == e["vias"] and ln <= 1.05 * e["ref_len"]
    if pat.name == "lead_via_end" and vias:
        x, y = vias[0][0], vias[0][1]
        ok = ok and abs(y - 6.0) < 0.05 and any(a - 1e-3 <= x <= b + 1e-3 for a, b in e["via_ok_x"])
    return {"vias": len(vias), "via_at": [[round(v[0], 3), round(v[1], 3)] for v in vias], "len": round(ln, 3),
            "ref_len": e["ref_len"], "matches_expected": ok}


def main(argv: list[str] | None = None) -> int:
    a = list(sys.argv[1:] if argv is None else argv)
    runs, out = Path(a[0]), Path(a[1])
    names = a[2:] or list(PATTERNS)
    out.mkdir(parents=True, exist_ok=True)
    table = {}
    for name in names:
        pat = PATTERNS[name]()
        pcbs = sorted(glob.glob(str(runs / "*" / name / name / "s0" / f"{name}-topor" / f"{name}.kicad_pcb")))
        rows = {}
        for pcb in pcbs:
            segs, vias = read_copper(Path(pcb))
            met = glob.glob(str(Path(pcb).parents[1] / "*--metrics.json"))
            r = _reach(pat, segs, vias)
            if met:
                m = json.load(open(met[0]))
                r["drc_new"], r["unconnected"] = m.get("routing_new"), m.get("unconnected")
            rows[Path(pcb).parents[4].name] = (r, segs, vias)
        table[name] = {c: r for c, (r, _, _) in rows.items()}
        W, H = pat.size
        view = (0, W, 0, H)
        ref_segs = _join([(t.layer, t.net, t.width, t.pts) for t in pat.kept_tracks + pat.ref_tracks])
        ref_vias = [(v.pos[0], v.pos[1], v.dia, v.net) for v in pat.ref_vias]
        n = 1 + len(rows)
        fig, ax = plt.subplots(1, n, figsize=(7 * n, 7 * H / W + 1.4))
        ax = list(ax) if n > 1 else [ax]
        _panel(ax[0], pat, ref_segs, ref_vias, "expected", view)
        _marks(ax[0], pat)
        for k, (c, (r, segs, vias)) in enumerate(sorted(rows.items()), start=1):
            _panel(ax[k], pat, segs, vias, f"topor main, cfg {c}: DRC new {r.get('drc_new', '?')}, unconnected "
                                         f"{r.get('unconnected', '?')}; matches expected: {'yes' if r['matches_expected'] else 'no'}", view)
            _marks(ax[k], pat)
        fig.suptitle(f"{name}: {pat.title}\n[{STATUS}]  x = no via room; hatched = B.Cu rule area; 'matches expected' = same via count, length within 5 %, via where it fits", fontsize=11)
        fig.tight_layout()
        fig.savefig(out / f"{name}.png", dpi=110)
        plt.close(fig)
        print(name, json.dumps(table[name]))
    (out / "summary.json").write_text(json.dumps(table, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
