"""PNG + summary of the PROPOSED draw-via-neck patterns (harness/dvn_patterns.py): expected | topor (one run per cfg), same view and
scale; F.Cu/B.Cu rule areas dotted, the jammed via site marked x, the row's tip dashed; thin copper (necked) outlined red. Run on the VM:

    python -m harness.dvn_patterns_png RUNS OUT [name ...]
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
from harness.dvn_patterns import PATTERNS, STATUS  # noqa: E402


def _marks(ax, pat):
    for k in pat.keepouts:
        ax.add_patch(Polygon(k.poly, closed=True, fc="#eeeeee", ec="#777777", hatch="xx", lw=0.6, alpha=0.6, zorder=1))
    e = pat.extra["dvn"]
    for x, y in e.get("jam_marks", []):
        ax.plot([x], [y], marker="x", ms=11, mew=2.2, color="#d62728", zorder=7)
    if "row_tip_y" in e:
        ax.axhline(e["row_tip_y"], color="#1f77b4", lw=0.6, ls=(0, (4, 3)), zorder=6)


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
    """'matches': the expected via count, length within 5 %, and per pattern: via_jam_shift no copper thinner than the
    net class (no neck); the neck patterns the neck ending within 0.1 mm of the expected y; pad_axis_entry the copper of
    SIG within 0.02 mm of the pad axis from the pad centre to 0.1 mm past the tip (an axis exit, no side leg)."""
    e = pat.extra["dvn"]
    ln = sum(sum(math.dist(a, b) for a, b in zip(p, p[1:])) for _, _, _, p in segs)
    ok = len(vias) == e["vias"] and ln <= 1.05 * e["ref_len"]
    out = {"vias": len(vias), "via_at": [[round(v[0], 3), round(v[1], 3)] for v in vias], "len": round(ln, 3), "ref_len": e["ref_len"]}
    nominal = {"PWR": 0.5}
    thin = [(net, w, p) for _, net, w, p in segs if w < nominal.get(net, 0.25) - 1e-4]
    out["thin"] = [[net, round(w, 3), [[round(x, 3), round(y, 3)] for x, y in p]] for net, w, p in thin]
    if pat.name in ("via_jam_shift", "via_interval_sliver"):
        ok = ok and not thin
    if "neck_to_y" in e:
        ys = [y for net, _, p in thin if net == "PWR" for _, y in p]
        out["neck_to_y"] = round(max(ys), 3) if ys else None
        ok = ok and out["neck_to_y"] is not None and abs(out["neck_to_y"] - e["neck_to_y"]) <= 0.1
    if pat.name == "pad_axis_entry":
        x0, y0 = e["turn_at"]
        near = [pt for _, net, _, p in segs if net == "SIG" for pt in p if pt[1] <= e["row_tip_y"] + 0.1]
        out["side_leg"] = any(abs(x - x0) > 0.02 for x, _ in near)
        ok = ok and not out["side_leg"]
    out["matches_expected"] = ok
    return out


def main(argv: list[str] | None = None) -> int:
    a = list(sys.argv[1:] if argv is None else argv)
    runs, out = Path(a[0]), Path(a[1])
    names = a[2:] or list(PATTERNS)
    out.mkdir(parents=True, exist_ok=True)
    table = {}
    for name in names:
        pat = PATTERNS[name]()
        W0, H0 = pat.size
        # a rule area over the whole board (B.Cu closed) is said in the title, not hatched over everything
        full = [k for k in pat.keepouts if Polygon(k.poly).get_extents().width * Polygon(k.poly).get_extents().height >= 0.9 * W0 * H0]
        pat.keepouts = [k for k in pat.keepouts if k not in full]
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
            _panel(ax[k], pat, segs, vias, f"today (tw-on c7f8bc8f, defaults): DRC new {r.get('drc_new', '?')}, unconnected "
                                         f"{r.get('unconnected', '?')}; matches expected: {'yes' if r['matches_expected'] else 'no'}", view)
            _marks(ax[k], pat)
        fig.suptitle(f"{name}: {pat.title}\n[{STATUS}]{'  B.Cu closed.' if full else ''}  x = jammed via site (today on qmtech Q2); dotted = rule area; dashed = row tip; 'matches expected' = see expected.json", fontsize=11)
        fig.tight_layout()
        fig.savefig(out / f"{name}.png", dpi=110)
        plt.close(fig)
        print(name, json.dumps(table[name]))
    (out / "summary.json").write_text(json.dumps(table, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
