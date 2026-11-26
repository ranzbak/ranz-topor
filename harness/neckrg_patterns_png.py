"""PNG + summary of the PROPOSED neck-regrow patterns (harness/neckrg_patterns.py): expected | one panel per run cfg, same
view and scale; thin copper (below the net class width) outlined, its stretches counted. Run on the VM:

    python -m harness.neckrg_patterns_png RUNS OUT [name ...]
RUNS/<cfg>/<name>/<name>/s0/<name>-topor/<name>.kicad_pcb, one panel per cfg dir; RUNS/labels.json {cfg: label} optional.
'matches expected' = no via, length within 5 %, exactly the expected number of necked stretches along PWR's path from P1
to P2, no width rise after a drop except at the far end's own neck (none for neck_both_ends: whole wire)."""
from __future__ import annotations

import glob
import json
import math
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

from harness.dvn_patterns_png import _join  # noqa: E402
from harness.fx_survivors_png import _panel, read_copper  # noqa: E402
from harness.neckrg_patterns import PATTERNS, STATUS, W_POW  # noqa: E402


def _chain(segs, start):
    """PWR's segments ordered along the path from `start` (greedy end matching): [(width, a, b)]."""
    rest = [(w, p[i], p[i + 1]) for _, net, w, p in segs if net == "PWR" for i in range(len(p) - 1)]
    out, cur = [], start
    while rest:
        k = min(range(len(rest)), key=lambda j: min(math.dist(cur, rest[j][1]), math.dist(cur, rest[j][2])))
        w, a, b = rest.pop(k)
        if math.dist(cur, b) < math.dist(cur, a):
            a, b = b, a
        out.append((w, a, b))
        cur = b
    return out


def _reach(pat, segs, vias) -> dict:
    e = pat.extra["nrg"]
    ln = sum(math.dist(a, b) for _, _, _, p in segs for a, b in zip(p, p[1:]))
    p1 = next(p.pos for f in pat.fps if f.ref == "P1" for p in f.pads)
    ch = _chain(segs, p1)
    thin = [w < W_POW - 1e-4 for w, _, _ in ch]
    stretches = sum(1 for i, t in enumerate(thin) if t and (i == 0 or not thin[i - 1]))
    widths = [round(w, 3) for w, _, _ in ch]
    out = {"vias": len(vias), "len": round(ln, 3), "ref_len": e["ref_len"], "neck_stretches": stretches, "widths_along": widths,
           "necked_len": round(sum(math.dist(a, b) for (w, a, b), t in zip(ch, thin) if t), 3)}
    ok = len(vias) == e["vias"] and ln <= 1.05 * e["ref_len"] and stretches == e["neck_stretches"]
    if e.get("whole_wire_neck"):
        ok = ok and all(thin)
    out["matches_expected"] = ok
    return out


def main(argv: list[str] | None = None) -> int:
    a = list(sys.argv[1:] if argv is None else argv)
    runs, out = Path(a[0]), Path(a[1])
    names = a[2:] or list(PATTERNS)
    out.mkdir(parents=True, exist_ok=True)
    try:
        labels = json.load(open(runs / "labels.json"))
    except (OSError, ValueError):
        labels = {}
    table = {}
    for name in names:
        pat = PATTERNS[name]()
        W, H = pat.size
        pat.keepouts = [k for k in pat.keepouts if k.layers != ["B.Cu"]]  # B.Cu closed: said in the title
        rows = {}
        for pcb in sorted(glob.glob(str(runs / "*" / name / name / "s0" / f"{name}-topor" / f"{name}.kicad_pcb"))):
            segs, vias = read_copper(Path(pcb))
            r = _reach(pat, segs, vias)
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
        e = pat.extra["nrg"]
        _panel(ax[0], pat, _join([(t.layer, t.net, t.width, t.pts) for t in pat.ref_tracks]), [], f"expected: {e['neck_stretches']} necked stretch, {e['neck_width']} mm", view)
        for k, (c, (r, segs, vias)) in enumerate(sorted(rows.items()), start=1):
            _panel(ax[k], pat, segs, vias, f"{labels.get(c, c)}: necked stretches {r['neck_stretches']}, DRC new {r.get('drc_new', '?')}, "
                                         f"unconnected {r.get('unconnected', '?')}; matches expected: {'yes' if r['matches_expected'] else 'no'}", view)
        fig.suptitle(f"{name}: {pat.title}\n[{STATUS.get(name, 'PROPOSED (awaiting approval)')}]  B.Cu closed. PWR 0.5 mm POWER class, "
                     "neck floor 0.2; grey pads = unconnected walls; thin copper = below 0.5 mm", fontsize=11)
        fig.tight_layout()
        fig.savefig(out / f"{name}.png", dpi=110)
        plt.close(fig)
        print(name, json.dumps(table[name]))
    (out / "summary.json").write_text(json.dumps(table, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
