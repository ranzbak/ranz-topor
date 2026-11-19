"""PNG + seed table of the PROPOSED D8 patterns (harness/tw5_patterns.py): expected | topor (first seed), same view and scale,
and per seed whether the routed copper reaches the expected end state (vias, layer per net, length within 5 %). Run on the VM:

    python -m harness.tw5_patterns_png BOARDS RUNS OUT [name ...]
BOARDS = the tw5_patterns output dir; RUNS/<cfg>/d-<name>/<name>-topor/<name>.kicad_pcb for every cfg dir (one seed each)."""
from __future__ import annotations

import glob
import json
import math
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

from harness.fx_survivors_png import _panel, read_copper  # noqa: E402
from harness.tw5_patterns import PATTERNS, STATUS  # noqa: E402


def _reach(pat, segs, vias) -> dict:
    """The routed copper against the expected end state: vias, the layer set per net, the length."""
    ref_layers = {}
    for t in pat.ref_tracks:
        ref_layers.setdefault(t.net, set()).add(t.layer)
    got_layers = {}
    for layer, net, _, pts in segs:
        got_layers.setdefault(net, set()).add(layer)
    ln = sum(sum(math.dist(a, b) for a, b in zip(p, p[1:])) for _, _, _, p in segs)
    ref = pat.extra["tw5"]["ref_len"]
    same = {n: got_layers.get(n) == ref_layers[n] for n in ref_layers}
    alt = pat.name == "oscillating_cross" and len(vias) == 2  # either wire may dive
    ok = len(vias) == pat.extra["tw5"]["vias"] and (all(same.values()) or alt) and ln <= 1.05 * ref
    return {"vias": len(vias), "len": round(ln, 2), "ref_len": ref, "layers_as_expected": same, "reached": ok}


def main(argv: list[str] | None = None) -> int:
    a = list(sys.argv[1:] if argv is None else argv)
    boards, runs, out = Path(a[0]), Path(a[1]), Path(a[2])
    names = a[3:] or list(PATTERNS)
    out.mkdir(parents=True, exist_ok=True)
    table = {}
    for name in names:
        pat = PATTERNS[name]()
        rows = {}
        for pcb in sorted(glob.glob(str(runs / "*" / f"d-{name}" / f"{name}-topor" / f"{name}.kicad_pcb"))):
            cfg = Path(pcb).parents[2].name
            segs, vias = read_copper(Path(pcb))
            rows[cfg] = _reach(pat, segs, vias)
        table[name] = {"seeds": len(rows), "reached": sum(r["reached"] for r in rows.values()), "runs": rows}
        W, H = pat.size
        view = (0, W, 0, H)
        ref_segs = [(t.layer, t.net, t.width, t.pts) for t in pat.ref_tracks]
        ref_vias = [(v.pos[0], v.pos[1], v.dia, v.net) for v in pat.ref_vias]
        fig, ax = plt.subplots(1, 2, figsize=(14, 7 * H / W + 1.2))
        _panel(ax[0], pat, ref_segs, ref_vias, f"expected ({pat.extra['tw5']['ref_len']} mm, {len(ref_vias)} vias)", view)
        first = next(iter(sorted(rows)), None)
        if first:
            pcb = runs / first / f"d-{name}" / f"{name}-topor" / f"{name}.kicad_pcb"
            segs, vias = read_copper(pcb)
            r = rows[first]
            _panel(ax[1], pat, segs, vias, f"topor {first}: {r['len']} mm, {r['vias']} vias; reached in "
                                           f"{table[name]['reached']} of {len(rows)} seeds", view)
        fig.suptitle(f"{name}: {pat.title}   [{STATUS}]   fig. {pat.extra['tw5']['figure']}", fontsize=12)
        fig.tight_layout()
        fig.savefig(out / f"{name}.png", dpi=110)
        plt.close(fig)
        print(name, table[name]["reached"], "of", len(rows))
    (out / "summary.json").write_text(json.dumps(table, indent=1, default=list) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
