"""Read a TopoR Lite 5.0 session DSN (File > Export > Specctra) of a via pattern, add its copper to a copy of the problem
board, run KiCad DRC, plot expected | TopoR, write result.json.  usage: python -m harness.via_patterns_topor <name> <session.dsn> <outdir>
[variant text]  (black-box only: nothing here looks inside TopoR)."""
from __future__ import annotations

import json
import re
import shutil
import sys
from pathlib import Path

import pcbnew

from harness import via_patterns as vp
from harness.fit_patterns import Track, Via

U = 100000.0  # session resolution: MM 100000


def parse_session(text: str):
    tracks, vias = [], []
    lib = {m.group(1): float(m.group(2)) / U for m in re.finditer(r'\(padstack "?(\w+)"?\s*\(shape \(circ \S+ (\d+)', text)}
    nsec = text.split("(network_out")[-1]
    for blk in re.split(r'\(Net ', nsec)[1:]:
        net = blk.split()[0].strip('"')
        body = blk
        for w in re.finditer(r'\(wire \(path "?(\S+?)"? (\d+)\s+([-\d\s]+)\)\)', body):
            v = [int(a) / U for a in w.group(3).split()]
            pts = [(v[i], -v[i + 1]) for i in range(0, len(v) - 1, 2)]
            tracks.append(Track(net, w.group(1), int(w.group(2)) / U, pts))
        for w in re.finditer(r'\(via "?(\w+)"? (-?\d+) (-?\d+)', body):
            d = lib.get(w.group(1), 0.8)
            vias.append(Via(net, (int(w.group(2)) / U, -int(w.group(3)) / U), d, 0.25 if abs(d - 0.45) < 1e-6 else d / 2))
    return tracks, vias


def main(argv):
    name, sess, out = argv[0], Path(argv[1]), Path(argv[2])
    variant = argv[3] if len(argv) > 3 else ""
    out.mkdir(parents=True, exist_ok=True)
    pat = vp.build(name)
    tracks, vias = parse_session(sess.read_text())
    tracks = [t for t in tracks if t.net in pat.nets]
    vias = [v for v in vias if v.net in pat.nets]
    sol = vp.Sol("TopoR Lite 5.0 result", tracks, vias)
    shutil.copy(sess, out / "topor_session.dsn")
    pcb = vp.write_pat(vp._View(pat, sol), out / "kicad" / f"{name}_topor.kicad_pcb", copper=True)
    drc = vp.drc_of(pcb)
    rep = vp.clearance_report(pat, sol)
    nets = sorted({n for n, _, _ in pat.connections})
    res = {"pattern": name, "variant_table": variant, "vias": [{"net": v.net, "pos": vp.rp(v.pos), "dia": v.dia} for v in vias],
           "per_net_length_mm": {n: vp.r4(vp.tl(sol, n)) for n in nets}, "total_mm": vp.r4(sum(vp.tl(sol, n) for n in nets)),
           "price": vp.price(sol, nets), "kicad_drc": drc, "shapely_min_clearance": rep["min_clearance"],
           "connected": all(vp.connected(pat, sol, n) for n in nets),
           "junctions": junctions(tracks)}
    (out / "result.json").write_text(json.dumps(res, indent=1) + "\n")
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    W, H = pat.size
    fig, axs = plt.subplots(1, 2, figsize=(16, 16 * (H + 1) / (2 * (W + 1)) + 1.0))
    e = pat.expected["reference"]
    vp.draw_board(axs[0], pat, pat.ref, title=f"EXPECTED: {e['vias']} vias, {e['length_mm']:.2f} mm, price {e['price']:.1f}")
    vp.draw_board(axs[1], pat, sol, title=f"TopoR 5.0: {len(vias)} vias, {res['total_mm']:.2f} mm, price {res['price']['price']:.1f}; "
                  f"KiCad DRC {drc.get('violations')} viol / {drc.get('unconnected')} unconn; {variant}")
    fig.suptitle(f"{name}: expected vs TopoR Lite 5.0 (autoroute defaults)", fontsize=10)
    fig.tight_layout()
    fig.savefig(out / f"{name}_topor.png", dpi=100)
    print(json.dumps({k: res[k] for k in ("vias", "per_net_length_mm", "total_mm", "kicad_drc", "connected", "junctions")}))


def junctions(tracks):
    """Points where 3+ track ends of one net meet away from pads (free T junctions)."""
    from collections import Counter

    c = Counter()
    for t in tracks:
        for p in (t.pts[0], t.pts[-1]):
            c[(t.net, round(p[0], 2), round(p[1], 2))] += 1
    return [{"net": k[0], "pos": [k[1], k[2]], "ends": n} for k, n in c.items() if n >= 3]


if __name__ == "__main__":
    main(sys.argv[1:])
