#!/usr/bin/env python3
"""Tiled KiCad DRC: violation counts and distinct net pairs past KiCad's report caps.

KiCad's DRC stops reporting a violation type at a cap (pcbnew/drc/drc_engine.cpp: ERROR_LIMIT 199,
EXTENDED_ERROR_LIMIT 499 for clearance and unconnected items; kicad-cli has no option to raise them). On a
dense board (bornhack) the capped counts say nothing about better or worse. This tool runs the DRC on copies
of the routed board that keep only the tracks, vias and arcs whose bounding box meets one tile + margin; a
tile at a cap splits in 4, down to --depth. Violations are deduped by (type, sorted item uuids). Unconnected
items are left out (cutting tracks away makes new ones): take them from the full-board report.

Judge a board by the distinct net pairs per type (`net_pairs`), not by the capped full report. Tiles still
at a cap at the deepest level are listed (`still_capped`): their counts are lower bounds, and KiCad reports a
different subset of a capped tile's violations run to run, so the COUNTS are noisy (bornhack, the same file
twice, depth 4: clearance 1251 vs 1228, shorting 1453 vs 1431) while the net pairs are stable (1 clearance pair
of 1079 pairs swapped; tw2r2tl-6062058). Tiling removes tracks only (pads and zones stay in every tile), so
some tiles stay capped however deep (bornhack depth 4: 21-23 tiles).

Method from the TW2 verification (reports/tw2-metric-verify-20261118-1804.md, section 5; validated on qmtech,
which has no cap: tiles = full report, 0 missing, 0 extra). Needs KiCad's pcbnew Python module and
kicad-cli (the VM image); run it on the VM, never in a desktop session.

  drc_tiles.py run <pcb or dir> <out> [--nx 3 --ny 4 --margin 1 --depth 4 --jobs 4 --area REF]
      --area REF: split the counts into "area" (an item inside REF's courtyard) and "rest" (bornhack: U6)
  drc_tiles.py diff <a/tiles.json> <b/tiles.json>   counts and net pairs a -> b, new and gone pairs
  drc_tiles.py tile ...                              one tile (internal)
"""
import argparse
import collections
import glob
import json
import os
import re
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

CAP = {"clearance": 499, "unconnected_items": 499}
DEFAULT_CAP = 199
SKIP = {"unconnected_items"}
ERR = {"clearance", "shorting_items", "hole_clearance", "tracks_crossing", "solder_mask_bridge",
       "copper_edge_clearance", "hole_to_hole", "annular_width"}


def tile(pcb, x0, y0, x1, y1, margin, outdir):
    import pcbnew
    os.makedirs(outdir, exist_ok=True)
    b = pcbnew.LoadBoard(pcb)

    def nm(v):
        return int(round(float(v) * 1e6))
    m = nm(margin)
    r = pcbnew.BOX2I(pcbnew.VECTOR2I(nm(x0) - m, nm(y0) - m),
                     pcbnew.VECTOR2I(nm(x1) - nm(x0) + 2 * m, nm(y1) - nm(y0) + 2 * m))
    gone = [t for t in b.GetTracks() if not t.GetBoundingBox().Intersects(r)]
    for t in gone:
        b.Remove(t)
    base = os.path.splitext(os.path.basename(pcb))[0]
    dst = os.path.join(outdir, base + ".kicad_pcb")
    for ext in (".kicad_pro", ".kicad_dru"):
        s = os.path.splitext(pcb)[0] + ext
        if os.path.isfile(s):
            shutil.copy(s, os.path.join(outdir, base + ext))
    pcbnew.SaveBoard(dst, b)
    j = os.path.join(outdir, "drc.json")
    # The harness's own DRC flags (toporlib/drc.py).
    subprocess.run(["kicad-cli", "pcb", "drc", "--format", "json", "--severity-all", "--units", "mm",
                    "--refill-zones", "--all-track-errors", "-o", j, dst],
                   check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for f in os.listdir(outdir):
        if f != "drc.json":
            os.remove(os.path.join(outdir, f))  # the tile's board copy (12 MB on bornhack): only its DRC is kept
    print(json.dumps({"kept": len(b.GetTracks()), "removed": len(gone)}))


def _nets(v):
    return tuple(sorted({m for i in v.get("items", []) for m in re.findall(r"\[([^\]]*)\]", i.get("description", ""))}))


def run(pcb, out, nx=3, ny=4, margin=1.0, depth=4, jobs=4, area=None):
    import pcbnew
    if os.path.isdir(pcb):
        pcb = sorted(glob.glob(os.path.join(pcb, "*.kicad_pcb")))[0]
    b = pcbnew.LoadBoard(pcb)
    bb = b.GetBoardEdgesBoundingBox()
    X0, Y0 = bb.GetX() / 1e6 - 1, bb.GetY() / 1e6 - 1
    W, H = bb.GetWidth() / 1e6 + 2, bb.GetHeight() / 1e6 + 2
    crt = None
    if area:
        fp = b.FindFootprintByReference(area)
        if fp is None:
            sys.exit(f"drc_tiles: no footprint {area}")
        fp.BuildCourtyardCaches()
        crt = fp.GetCourtyard(pcbnew.B_CrtYd if fp.GetLayer() == pcbnew.B_Cu else pcbnew.F_CrtYd)
    todo = [(X0 + i * W / nx, Y0 + k * H / ny, X0 + (i + 1) * W / nx, Y0 + (k + 1) * H / ny, 0)
            for i in range(nx) for k in range(ny)]
    seen, tiles = {}, []
    me = os.path.abspath(__file__)

    def one(rect):
        d = os.path.join(out, "t_%.3f_%.3f_%.3f_%.3f" % rect[:4])
        subprocess.run([sys.executable, me, "tile", pcb, *map(str, rect[:4]), str(margin), d],
                       check=False, stdout=subprocess.DEVNULL)
        return rect, d
    while todo:
        batch, todo = todo, []
        with ThreadPoolExecutor(jobs) as ex:
            res = list(ex.map(one, batch))
        for rect, d in res:
            try:
                with open(os.path.join(d, "drc.json")) as f:
                    rep = json.load(f)
            except (OSError, ValueError) as e:
                print("tile failed", rect, e, file=sys.stderr)
                continue
            c = collections.Counter(v["type"] for v in rep["violations"])
            capped = sorted(t for t, k in c.items() if t in ERR and k >= CAP.get(t, DEFAULT_CAP))
            tiles.append({"rect": list(rect), "counts": dict(c), "capped": capped})
            if capped and rect[4] < depth:
                x0, y0, x1, y1, dp = rect
                xm, ym = (x0 + x1) / 2, (y0 + y1) / 2
                todo += [(x0, y0, xm, ym, dp + 1), (xm, y0, x1, ym, dp + 1),
                         (x0, ym, xm, y1, dp + 1), (xm, ym, x1, y1, dp + 1)]
                shutil.rmtree(d, ignore_errors=True)
                continue
            if capped:
                print("still capped at the deepest level:", rect, capped, file=sys.stderr)
            shutil.rmtree(d, ignore_errors=True)
            for v in rep["violations"]:
                if v["type"] in SKIP:
                    continue
                seen.setdefault((v["type"], tuple(sorted(i.get("uuid", "") for i in v.get("items", [])))), v)

    def cls(v):
        if crt is None:
            return "all"
        for i in v.get("items", []):
            p = i.get("pos")
            if p and crt.Contains(pcbnew.VECTOR2I(int(round(p["x"] * 1e6)), int(round(p["y"] * 1e6)))):
                return "area"
        return "rest"
    counts, pairs = {}, {}
    for v in seen.values():
        k = cls(v)
        counts.setdefault(k, collections.Counter())[v["type"] + "|" + v["severity"]] += 1
        if v["type"] in ERR and v["severity"] == "error":
            pairs.setdefault(k, {}).setdefault(v["type"], set()).add(_nets(v))
    still = [t["rect"] for t in tiles if t["capped"] and t["rect"][4] >= depth]
    res = {"pcb": pcb, "area": area, "counts": {k: dict(sorted(c.items())) for k, c in counts.items()},
           "net_pairs": {k: {t: sorted(map(list, x)) for t, x in sorted(d.items())} for k, d in pairs.items()},
           "still_capped": still, "tiles": tiles, "violations": list(seen.values())}
    with open(os.path.join(out, "tiles.json"), "w") as f:
        json.dump(res, f, indent=1)
    for k in sorted(counts):
        print(k, "counts", json.dumps(res["counts"][k]))
        print(k, "net pairs", json.dumps({t: len(x) for t, x in res["net_pairs"].get(k, {}).items()}))
    print("still capped tiles (lower bounds):", len(still))


def _load(p):
    """A tiles.json; the TW2 verifier's first format (by_class u6/rest, clearance split by rule) is read as
    counts area/rest per type|severity."""
    with open(p) as f:
        d = json.load(f)
    if "counts" not in d:
        ren = {"u6": "area"}
        counts = {}
        for k, c in d.get("by_class", {}).items():
            cc = counts.setdefault(ren.get(k, k), collections.Counter())
            for t, n in c.items():
                ty, sev = t.split("|")
                cc[ty.split("/")[0] + "|" + sev] += n
        d["counts"] = {k: dict(c) for k, c in counts.items()}
        d["net_pairs"] = {ren.get(k, k): v for k, v in d.get("net_pairs", {}).items()}
    return d


def diff(a, b):
    A, B = _load(a), _load(b)
    for k in sorted(set(A["counts"]) | set(B["counts"])):
        ca, cb = A["counts"].get(k, {}), B["counts"].get(k, {})
        print(f"[{k}] counts (a -> b):")
        for t in sorted(set(ca) | set(cb)):
            print(f"  {t}: {ca.get(t, 0)} -> {cb.get(t, 0)}")
        pa, pb = A["net_pairs"].get(k, {}), B["net_pairs"].get(k, {})
        print(f"[{k}] distinct net pairs (a -> b; new / gone):")
        for t in sorted(set(pa) | set(pb)):
            sa, sb = {tuple(x) for x in pa.get(t, [])}, {tuple(x) for x in pb.get(t, [])}
            new, gone = sorted(sb - sa), sorted(sa - sb)
            print(f"  {t}: {len(sa)} -> {len(sb)} (new {len(new)}, gone {len(gone)})")
            for x in new:
                print("    + " + " / ".join(x))
            for x in gone:
                print("    - " + " / ".join(x))
    print(f"still capped tiles: {len(A['still_capped'])} -> {len(B['still_capped'])}")


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "tile":
        tile(sys.argv[2], *sys.argv[3:7], sys.argv[7], sys.argv[8])
        return
    a = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    s = a.add_subparsers(dest="cmd", required=True)
    r = s.add_parser("run")
    r.add_argument("pcb")
    r.add_argument("out")
    r.add_argument("--nx", type=int, default=3)
    r.add_argument("--ny", type=int, default=4)
    r.add_argument("--margin", type=float, default=1.0, help="mm around a tile whose copper is kept")
    r.add_argument("--depth", type=int, default=4, help="how often a capped tile splits in 4")
    r.add_argument("--jobs", type=int, default=4)
    r.add_argument("--area", help="footprint reference whose courtyard splits the counts (area / rest)")
    d = s.add_parser("diff")
    d.add_argument("a")
    d.add_argument("b")
    o = a.parse_args()
    if o.cmd == "run":
        os.makedirs(o.out, exist_ok=True)
        run(o.pcb, o.out, o.nx, o.ny, o.margin, o.depth, o.jobs, o.area)
    else:
        diff(o.a, o.b)


if __name__ == "__main__":
    main()
