#!/usr/bin/env python3
"""first-draw-45 r9 (TEMP, read-only forensics): classify every free-angle track segment of a routed board by the
step that wrote it and why.

  fd45r9_free.py --log S0LOG[.gz] --pcb ROUTED.kicad_pcb --input INPUT.kicad_pcb --json OUT.json [--md OUT.md]
                 [--png CLUSTER.png] [--board NAME]

S0LOG: the engine stderr of a run with TOPO_FREEDBG=1 TOPO_RUNPROV=1 (copper-neutral debug lines):
  FREESTAGE <stage> / FREESNAP <stage> w.. net .. L.. x1 y1 x2 y2   free-angle segments of the drawn wires after each
    drawing step (draw, straighten, reduce, fuse, polish; core.rs free_snap)
  RUNPROV ladder|forced w.. ... src S octi TAG ... pts [...]            every committed run of the drawing (draw.rs run_prov)
  FREEDP w.. L.. forced B (x1, y1) (x2, y2) len .. legal_octi_forms k/n  free-angle legs the octilinear DP chose (octi.rs)
A final free-angle segment (not at 0/45/90 degrees, not in the input board) is traced back:
  writer = the earliest snapshot step from which a covering segment exists in every later snapshot (else "after polish":
  the fuse pour check's undo, tidy, route.rs finishing).
  class (writer draw): forced run | ERBS kept | no legal octilinear form | DP chose it while a legal octilinear form
  existed | other (necked run, ...); writer straighten/reduce/fuse/polish/after: pull/tidy/fuse after drawing.
Output: count + mm per class, per writer, every segment, and a PNG of the largest cluster (3 mm linkage).
"""
import argparse, gzip, json, math, re, sys
from collections import defaultdict

STAGES = ["draw", "straighten", "reduce", "fuse", "polish"]
TOL = 0.002  # mm: distance of a segment's end points to a covering segment's line


def opener(p):
    return gzip.open(p, "rt", errors="replace") if p.endswith(".gz") else open(p, errors="replace")


def octi(ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    L = math.hypot(dx, dy)
    if L < 1e-6:
        return True
    for k in range(8):
        a = k * math.pi / 4
        if abs(dx * math.sin(a) - dy * math.cos(a)) <= 2e-6 + 1e-6 * L and dx * math.cos(a) + dy * math.sin(a) > 0:
            return True
    return False


def covers(s, f):
    """Segment s (x1,y1,x2,y2) covers f: f's end points within TOL of s's line, the overlap along s >= half of f."""
    ax, ay, bx, by = s
    dx, dy = bx - ax, by - ay
    L = math.hypot(dx, dy)
    if L < 1e-9:
        return False
    ux, uy = dx / L, dy / L
    ts = []
    for (px, py) in ((f[0], f[1]), (f[2], f[3])):
        if abs((px - ax) * uy - (py - ay) * ux) > TOL:
            return False
        ts.append((px - ax) * ux + (py - ay) * uy)
    lo, hi = max(0.0, min(ts)), min(L, max(ts))
    lf = math.hypot(f[2] - f[0], f[3] - f[1])
    return hi - lo >= 0.5 * lf - TOL


def tokens(txt):
    return re.findall(r'\(|\)|"(?:[^"\\]|\\.)*"|[^\s()]+', txt)


def parse(tok, i=0):
    out = []
    while i < len(tok):
        t = tok[i]
        if t == "(":
            sub, i = parse(tok, i + 1)
            out.append(sub)
        elif t == ")":
            return out, i + 1
        else:
            out.append(t.strip('"'))
            i += 1
    return out, i


def board(pcb):
    root = parse(tokens(open(pcb, errors="replace").read()))[0][0]
    segs, vias, pads = [], [], []
    for n in root:
        if not isinstance(n, list) or not n:
            continue
        d = {c[0]: c[1:] for c in n if isinstance(c, list) and c}
        if n[0] == "segment":
            segs.append(dict(a=(float(d["start"][0]), float(d["start"][1])), b=(float(d["end"][0]), float(d["end"][1])),
                             w=float(d["width"][0]), layer=d["layer"][0], net=d.get("net", [""])[0]))
        elif n[0] == "via":
            vias.append(dict(at=(float(d["at"][0]), float(d["at"][1])), size=float(d["size"][0])))
        elif n[0] == "footprint":
            fx, fy = float(d["at"][0]), float(d["at"][1])
            fr = float(d["at"][2]) if len(d["at"]) > 2 else 0.0
            for c in n:
                if isinstance(c, list) and c and c[0] == "pad":
                    pd = {x[0]: x[1:] for x in c if isinstance(x, list) and x}
                    if "at" not in pd or "size" not in pd:
                        continue
                    px, py = float(pd["at"][0]), float(pd["at"][1])
                    pr = float(pd["at"][2]) if len(pd["at"]) > 2 else fr
                    a = math.radians(-fr)
                    cx, cy = fx + px * math.cos(a) - py * math.sin(a), fy + px * math.sin(a) + py * math.cos(a)
                    sx, sy = float(pd["size"][0]), float(pd["size"][1] if len(pd["size"]) > 1 else pd["size"][0])
                    lay = pd.get("layers", [])
                    pads.append(dict(c=(cx, cy), s=(sx, sy), rot=pr, layers=lay))
    return segs, vias, pads


def read_log(path):
    snaps = defaultdict(list)  # stage -> [(seg, w)] (last occurrence block of each stage)
    cur_stage, block = None, defaultdict(list)
    runprov_blocks, cur_runs = [], []
    freedp_blocks, cur_dp = [], []
    num = r"(-?[\d.]+)"
    rx_snap = re.compile(rf"FREESNAP (\w+) w(\d+) net (-?\d+) L(\d+) {num} {num} {num} {num}")
    rx_rp = re.compile(r"RUNPROV (\w+) w(\d+) net (-?\d+) L(\d+) src (\S+) octi (\S+) any \d+ [\d.]+ pts \[(.*)\]")
    rx_dp = re.compile(rf"FREEDP w(\d+) L(\d+) forced (\w+) \({num}, {num}\) \({num}, {num}\) len [\d.]+ legal_octi_forms (\d+)/(\d+)")
    for line in opener(path):
        if line.startswith("FREESTAGE "):
            st = line.split()[1]
            if st == "draw":
                # a new drawing's first snapshot: the runs and DP lines since the last one are this drawing's
                runprov_blocks.append(cur_runs)
                freedp_blocks.append(cur_dp)
                cur_runs, cur_dp = [], []
            block[st] = []
            cur_stage = st
            continue
        m = rx_snap.match(line)
        if m:
            st = m.group(1)
            block[st].append(((float(m.group(5)), float(m.group(6)), float(m.group(7)), float(m.group(8))), int(m.group(2))))
            continue
        m = rx_rp.match(line)
        if m:
            pts = [(float(a), float(b)) for a, b in re.findall(r"\((-?[\d.]+), (-?[\d.]+)\)", m.group(7))]
            # engine frame printed as (x, -y): the KiCad frame
            cur_runs.append(dict(kind=m.group(1), w=int(m.group(2)), net=int(m.group(3)), layer=int(m.group(4)), src=m.group(5),
                                 tag=m.group(6), pts=pts))
            continue
        m = rx_dp.match(line)
        if m:
            cur_dp.append(dict(w=int(m.group(1)), forced=m.group(3) == "true",
                               seg=(float(m.group(4)), float(m.group(5)), float(m.group(6)), float(m.group(7))),
                               legal=int(m.group(8)), forms=int(m.group(9))))
    # the last drawing: the runs/DP lines before its "draw" snapshot
    runs = runprov_blocks[-1] if runprov_blocks else []
    dps = freedp_blocks[-1] if freedp_blocks else []
    return dict(block), runs, dps


def classify(args):
    snaps, runs, dps = read_log(args.log)
    segs, vias, pads = board(args.pcb)
    inp = board(args.input)[0] if args.input else []
    in_free = [(s["a"][0], s["a"][1], s["b"][0], s["b"][1]) for s in inp if not octi(*s["a"], *s["b"])]
    out = []
    for s in segs:
        f = (s["a"][0], s["a"][1], s["b"][0], s["b"][1])
        if octi(*f) or math.hypot(f[2] - f[0], f[3] - f[1]) < 1e-3:
            continue
        L = math.hypot(f[2] - f[0], f[3] - f[1])
        if any(covers(x, f) or covers(f, x) for x in in_free):
            out.append(dict(seg=f, mm=L, net=s["net"], layer=s["layer"], writer="input", cls="other", why="kept input copper"))
            continue
        present = [any(covers(x[0], f) or covers(f, x[0]) for x in snaps.get(st, [])) for st in STAGES]
        writer = "after polish"
        for k in range(len(STAGES)):
            if all(present[k:]):
                writer = STAGES[k]
                break
        wire = None
        if writer in STAGES:
            for x in snaps.get(writer, []):
                if covers(x[0], f) or covers(f, x[0]):
                    wire = x[1]
                    break
        rec = dict(seg=f, mm=L, net=s["net"], layer=s["layer"], writer=writer, wire=wire)
        if writer != "draw":
            rec["cls"], rec["why"] = "pull/tidy/fuse after drawing", f"written by the {writer} step"
            out.append(rec)
            continue
        run = None
        for r in reversed(runs):
            if wire is not None and r["w"] != wire:
                continue
            if any(covers((p[0][0], p[0][1], p[1][0], p[1][1]), f) or covers(f, (p[0][0], p[0][1], p[1][0], p[1][1])) for p in zip(r["pts"], r["pts"][1:])):
                run = r
                break
        dp = [d for d in dps if (wire is None or d["w"] == wire) and (covers(d["seg"], f) or covers(f, d["seg"]))]
        rec["run"] = None if run is None else dict(kind=run["kind"], src=run["src"], tag=run["tag"])
        rec["dp"] = [dict(legal=d["legal"], forms=d["forms"], forced=d["forced"]) for d in dp]
        if run is None:
            rec["cls"], rec["why"] = "other", "no committed run of the last drawing covers it"
        elif run["kind"] == "forced":
            rec["cls"], rec["why"] = "forced run", f"never-drop run, octilinear outcome {run['tag']}"
        elif run["tag"] in ("erbs-kept", "erbs-long-kept"):
            rec["cls"], rec["why"] = "ERBS kept", run["tag"]
        elif run["tag"] in ("rejected",):
            rec["cls"], rec["why"] = "no legal octilinear form", "the DP's form failed the checker, the taut run kept"
        elif dp:
            nonforced = [d for d in dp if not d["forced"]] or dp
            if any(d["legal"] > 0 for d in nonforced):
                rec["cls"], rec["why"] = "DP chose it while a legal octilinear form existed", f"FREEDP legal forms {[(d['legal'], d['forms']) for d in nonforced]}"
            else:
                rec["cls"], rec["why"] = "no legal octilinear form", "the DP's connectors between its ends all fail the leg test"
        elif run["tag"] == "none":
            rec["cls"], rec["why"] = "no legal octilinear form", "the DP returned no form (no chain, or the taut run itself)"
        elif run["tag"] == "ok":
            rec["cls"], rec["why"] = "pull/tidy/fuse after drawing", "not in the DP's chain: the taut pull (P6 c) after the DP"
        else:
            rec["cls"], rec["why"] = "other", f"run tag {run['tag']} (src {run['src']})"
        out.append(rec)
    return out, segs, vias, pads


def clusters(recs, link=3.0):
    n = len(recs)
    par = list(range(n))

    def fnd(i):
        while par[i] != i:
            par[i] = par[par[i]]
            i = par[i]
        return i

    def d(a, b):
        pa = [(a[0], a[1]), (a[2], a[3]), ((a[0] + a[2]) / 2, (a[1] + a[3]) / 2)]
        pb = [(b[0], b[1]), (b[2], b[3]), ((b[0] + b[2]) / 2, (b[1] + b[3]) / 2)]
        return min(math.hypot(p[0] - q[0], p[1] - q[1]) for p in pa for q in pb)

    for i in range(n):
        for j in range(i + 1, n):
            if d(recs[i]["seg"], recs[j]["seg"]) <= link:
                par[fnd(i)] = fnd(j)
    g = defaultdict(list)
    for i in range(n):
        g[fnd(i)].append(i)
    return sorted(g.values(), key=lambda v: -sum(recs[i]["mm"] for i in v))


COLORS = {"forced run": "#d62728", "ERBS kept": "#9467bd", "no legal octilinear form": "#ff7f0e",
          "pull/tidy/fuse after drawing": "#17becf", "DP chose it while a legal octilinear form existed": "#e377c2",
          "other": "#7f7f7f"}


def png(path, recs, idx, segs, vias, pads, title):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.patches import Rectangle, Circle
    from matplotlib.transforms import Affine2D
    xs = [c for i in idx for c in (recs[i]["seg"][0], recs[i]["seg"][2])]
    ys = [c for i in idx for c in (recs[i]["seg"][1], recs[i]["seg"][3])]
    x0, x1, y0, y1 = min(xs) - 2, max(xs) + 2, min(ys) - 2, max(ys) + 2
    fig, ax = plt.subplots(figsize=(11, 11 * (y1 - y0) / max(x1 - x0, 1e-3) + 1.2))
    inb = lambda x, y: x0 - 1 <= x <= x1 + 1 and y0 - 1 <= y <= y1 + 1
    for p in pads:
        if not inb(*p["c"]):
            continue
        cu = [l for l in p["layers"] if l.endswith(".Cu") or l == "*.Cu"]
        col = "#c8a000" if any(l.startswith("F") or l == "*.Cu" for l in cu) else "#3a7bd5"
        r = Rectangle((p["c"][0] - p["s"][0] / 2, p["c"][1] - p["s"][1] / 2), p["s"][0], p["s"][1], fc=col, ec="none", alpha=0.35)
        r.set_transform(Affine2D().rotate_deg_around(p["c"][0], p["c"][1], -p["rot"]) + ax.transData)
        ax.add_patch(r)
    for s in segs:
        if inb(*s["a"]) or inb(*s["b"]):
            ax.plot([s["a"][0], s["b"][0]], [s["a"][1], s["b"][1]], color="#b03030" if s["layer"] == "F.Cu" else "#3060b0",
                    lw=max(0.5, s["w"] * 72 * 11 / (x1 - x0) / 25.4), alpha=0.45, solid_capstyle="round")
    for v in vias:
        if inb(*v["at"]):
            ax.add_patch(Circle(v["at"], v["size"] / 2, fc="#909090", ec="k", lw=0.3))
    seen = set()
    for i in idx:
        r = recs[i]
        f = r["seg"]
        lab = r["cls"] if r["cls"] not in seen else None
        seen.add(r["cls"])
        ax.plot([f[0], f[2]], [f[1], f[3]], color=COLORS.get(r["cls"], "k"), lw=3.0, label=lab, solid_capstyle="round")
    ax.set_xlim(x0, x1)
    ax.set_ylim(y1, y0)
    ax.set_aspect("equal")
    ax.legend(loc="upper left", fontsize=8, framealpha=0.85)
    ax.set_title(title, fontsize=9)
    ax.set_xlabel("mm (KiCad frame); tracks F.Cu red / B.Cu blue, pads amber (F) / blue (B), free-angle segments thick by class")
    fig.tight_layout()
    fig.savefig(path, dpi=110)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", required=True)
    ap.add_argument("--pcb", required=True)
    ap.add_argument("--input")
    ap.add_argument("--json", required=True)
    ap.add_argument("--md")
    ap.add_argument("--png")
    ap.add_argument("--board", default="")
    a = ap.parse_args()
    recs, segs, vias, pads = classify(a)
    by_cls, by_writer = defaultdict(lambda: [0, 0.0]), defaultdict(lambda: [0, 0.0])
    for r in recs:
        by_cls[r["cls"]][0] += 1
        by_cls[r["cls"]][1] += r["mm"]
        by_writer[r["writer"]][0] += 1
        by_writer[r["writer"]][1] += r["mm"]
    cl = clusters([r for r in recs if r["writer"] != "input"])
    live = [r for r in recs if r["writer"] != "input"]
    res = dict(board=a.board, segments=len(recs), mm=round(sum(r["mm"] for r in recs), 3),
               by_class={k: dict(count=v[0], mm=round(v[1], 3)) for k, v in sorted(by_cls.items())},
               by_writer={k: dict(count=v[0], mm=round(v[1], 3)) for k, v in sorted(by_writer.items())},
               clusters=[dict(count=len(c), mm=round(sum(live[i]["mm"] for i in c), 3),
                              bbox=[round(min(min(live[i]["seg"][0], live[i]["seg"][2]) for i in c), 2), round(min(min(live[i]["seg"][1], live[i]["seg"][3]) for i in c), 2),
                                    round(max(max(live[i]["seg"][0], live[i]["seg"][2]) for i in c), 2), round(max(max(live[i]["seg"][1], live[i]["seg"][3]) for i in c), 2)],
                              nets=sorted({live[i]["net"] for i in c})) for c in cl[:5]],
               segs=recs)
    json.dump(res, open(a.json, "w"), indent=1, default=str)
    if a.md:
        with open(a.md, "w") as f:
            f.write(f"# free-angle forensics {a.board}: {res['segments']} segments, {res['mm']} mm\n\n| class | count | mm |\n|---|---|---|\n")
            for k, v in res["by_class"].items():
                f.write(f"| {k} | {v['count']} | {v['mm']} |\n")
            f.write("\n| writer | count | mm |\n|---|---|---|\n")
            for k, v in res["by_writer"].items():
                f.write(f"| {k} | {v['count']} | {v['mm']} |\n")
            f.write("\n| net | layer | mm | writer | class | why | seg |\n|---|---|---|---|---|---|---|\n")
            for r in sorted(recs, key=lambda r: (r["cls"], -r["mm"])):
                f.write(f"| {r['net']} | {r['layer']} | {r['mm']:.3f} | {r['writer']} | {r['cls']} | {r['why']} | {tuple(round(x, 3) for x in r['seg'])} |\n")
    if a.png and cl:
        c0 = cl[0]
        png(a.png, live, c0, segs, vias, pads, f"{a.board}: largest free-angle cluster, {len(c0)} segments, {sum(live[i]['mm'] for i in c0):.2f} mm")
    print(json.dumps({k: res[k] for k in ("board", "segments", "mm", "by_class", "by_writer")}, indent=1))


if __name__ == "__main__":
    sys.exit(main())
