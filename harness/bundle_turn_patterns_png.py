"""PNG + summary of the PROPOSED bundle-turn patterns (harness/bundle_turn_patterns.py): expected | one panel per run cfg,
same view and scale; clearance violations ringed red (allowed ones orange). Run on the VM:

    python -m harness.bundle_turn_patterns_png RUNS OUT [name ...]
    python -m harness.bundle_turn_patterns_png --expected OUT      (the expected panels only; no runs)
RUNS/<cfg>/<name>/<name>/s0/<name>-topor/<name>.kicad_pcb, one panel per cfg dir; RUNS/labels.json {cfg: label} optional.
'matches expected' = no via, no clearance violation but the allowed ones, every wire within 2 % of its reference length
and with no more bends than the reference."""
from __future__ import annotations

import glob
import json
import math
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import textwrap  # noqa: E402

from matplotlib.patches import Circle, Rectangle  # noqa: E402

from harness.bundle_turn_patterns import PATTERNS, STATUS, allowed, violations  # noqa: E402
from harness.fx_survivors_png import _panel  # noqa: E402


def _bends(segs, net) -> int:
    """Direction changes along `net`'s segments (chained greedily from the first end that touches only one segment)."""
    rest = [(p[i], p[i + 1]) for _, n, _, p in segs if n == net for i in range(len(p) - 1) if math.dist(p[i], p[i + 1]) > 1e-4]
    if not rest:
        return 0
    ends = [q for s in rest for q in s]
    start = next((q for q in ends if sum(math.dist(q, r) < 1e-3 for r in ends) == 1), ends[0])
    out, cur = [], start
    while rest:
        k = min(range(len(rest)), key=lambda j: min(math.dist(cur, rest[j][0]), math.dist(cur, rest[j][1])))
        a, b = rest.pop(k)
        if math.dist(cur, b) < math.dist(cur, a):
            a, b = b, a
        out.append(math.atan2(b[1] - a[1], b[0] - a[0]))
        cur = b
    return sum(1 for u, v in zip(out, out[1:]) if abs((v - u + math.pi) % (2 * math.pi) - math.pi) > 1e-3)


def reach(pat, segs, vias) -> dict:
    e = pat.extra["bt"]
    vs = violations(pat, segs)
    lens = {n: round(sum(math.dist(a, b) for _, m, _, p in segs if m == n for a, b in zip(p, p[1:])), 3) for n in pat.nets}
    bends = {n: _bends(segs, n) for n in pat.nets}
    bad = [v for v in vs if not allowed(pat, v)]
    ok = (len(vias) == e["vias"] and not bad and all(lens[n] <= 1.02 * e["ref_len"][n] for n in pat.nets)
          and all(bends[n] <= e["bends"][n] for n in pat.nets))
    return {"vias": len(vias), "len": lens, "ref_len": e["ref_len"], "bends": bends, "ref_bends": e["bends"],
            "violations": len(bad), "allowed_violations": len(vs) - len(bad), "detail": bad[:8], "matches_expected": ok}


def _ring(ax, pat, segs):
    for v in violations(pat, segs):
        ax.add_patch(Circle(v["at"], 0.25, fc="none", ec="#ff8c00" if allowed(pat, v) else "#d62728", lw=1.6, zorder=6))


def _hole(ax, pat):  # _panel draws round pads as squares
    for f in pat.fps:
        for p in f.pads:
            if p.shape == "circle" and not p.net:
                w = p.size[0] * 1.08
                ax.add_patch(Rectangle((p.pos[0] - w / 2, p.pos[1] - w / 2), w, w, fc="#fbfbf6", ec="none", zorder=2.4))
                ax.add_patch(Circle(p.pos, p.size[0] / 2, fc="#d0d0d0", ec="k", lw=0.6, zorder=2.5))
                if p.drill:
                    ax.add_patch(Circle(p.pos, p.drill / 2, fc="white", ec="#555555", lw=0.5, zorder=2.6))


def main(argv: list[str] | None = None) -> int:
    a = list(sys.argv[1:] if argv is None else argv)
    only_exp = a[:1] == ["--expected"]
    if only_exp:
        runs, out, names = None, Path(a[1]), a[2:] or list(PATTERNS)
    else:
        runs, out, names = Path(a[0]), Path(a[1]), a[2:] or list(PATTERNS)
    out.mkdir(parents=True, exist_ok=True)
    try:
        labels = json.load(open(runs / "labels.json")) if runs else {}
    except (OSError, ValueError):
        labels = {}
    table = {}
    for name in names:
        pat = PATTERNS[name]()
        W, H = pat.size
        pat.keepouts = [k for k in pat.keepouts if k.layers != ["B.Cu"]]  # B.Cu closed: said in the title
        rows = {}
        for pcb in sorted(glob.glob(str(runs / "*" / name / name / "s0" / f"{name}-topor" / f"{name}.kicad_pcb"))) if runs else []:
            from harness.fx_survivors_png import read_copper

            segs, vias = read_copper(Path(pcb))
            r = reach(pat, segs, vias)
            met = glob.glob(str(Path(pcb).parents[1] / "*--metrics.json"))
            if met:
                m = json.load(open(met[0]))
                r["drc_new"], r["unconnected"] = m.get("routing_new"), m.get("unconnected")
            rows[Path(pcb).parents[4].name] = (r, segs, vias)
        table[name] = {c: r for c, (r, _, _) in rows.items()}
        view = (0, W, 0, H)
        n = 1 + len(rows)
        fig, ax = plt.subplots(1, n, figsize=(7 * n, 7 * H / W + 1.6))
        ax = list(ax) if n > 1 else [ax]
        ref = [(t.layer, t.net, t.width, t.pts) for t in pat.ref_tracks]
        e = pat.extra["bt"]
        _panel(ax[0], pat, ref, [], f"expected: bends {e['bends']}, pitch {e['pitch']} mm", view)
        _hole(ax[0], pat)
        _ring(ax[0], pat, ref)
        for k, (c, (r, segs, vias)) in enumerate(sorted(rows.items()), start=1):
            _panel(ax[k], pat, segs, vias, f"{labels.get(c, c)}: violations {r['violations']} (+{r['allowed_violations']} allowed), bends "
                                         f"{r['bends']}, DRC new {r.get('drc_new', '?')}; matches expected: {'yes' if r['matches_expected'] else 'no'}", view)
            _hole(ax[k], pat)
            _ring(ax[k], pat, segs)
        fig.suptitle(textwrap.fill(f"{name}: {pat.title}", 80 * n) + "\n" + textwrap.fill(f"[{STATUS.get(name, 'PROPOSED (awaiting approval)')}]  B.Cu closed. Default 0.25 / 0.2; "
                     "grey pads = unconnected; hatched = keepout; red ring = clearance violation, orange = the allowed one", 80 * n), fontsize=9)
        fig.tight_layout()
        fig.savefig(out / f"{name}.png", dpi=110)
        plt.close(fig)
        print(name, json.dumps(table[name]))
    (out / "summary.json").write_text(json.dumps(table, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
