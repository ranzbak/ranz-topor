#!/usr/bin/env python3
"""tw5_abl.py (branch tw5-escape): per-connection A/B summary of TW5 ablation runs, from ladder_diff.

  tw5_abl.py BASE_ROOT:CFG TIP_ROOT:CFG BOARD SEEDS [--list N]

BASE_ROOT/TIP_ROOT = run roots (hz-runs/<tag>), CFG = the tw5_job config prefix without the seed
(e.g. Or16, Nar16), BOARD = qmtech|esp|p-inner_layer_room, SEEDS = comma list (seed 0 = no "s" suffix
when the base has no s0 dir). Per seed: ladder_diff totals (its --json metrics) and per connection (its --all
rows): changed, drc up / down, vias up / down, new detours > 1.5 (no-detour rule: base <= 1.5, tip > 1.5),
detours > 1.5 gone, longer / shorter by > 10 %. --list N prints the N connections with the largest drc
rise and every new detour. Numbers come from ladder_diff only (AGENT_RULES rule 4).
"""
import json, os, re, subprocess, sys

LD = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "ladder_diff.py")
LD = os.environ.get("LADDER_DIFF", "/home/paul/work/kicad/plugins/topor/scratch/agent-wt/tools/ladder_diff.py")
ROW = re.compile(r"^  (.+?)\s+len ([\d.]+)->([\d.]+)\s+vias (\d+)->(\d+)\s+detour ([\d.]+)->([\d.]+)\s+drc (\d+)->(\d+)")


def rundir(root, cfg, seed, board):
    for name in (f"{cfg}s{seed}", cfg if seed == 0 else None):
        if name and os.path.isdir(os.path.join(root, name, board)):
            return os.path.join(root, name, board)
    return None


def compare(a, b):
    js = json.loads(subprocess.run([sys.executable, LD, a, b, "--json"], capture_output=True, text=True).stdout or "{}")
    txt = subprocess.run([sys.executable, LD, a, b, "--all"], capture_output=True, text=True).stdout
    rows = []
    for line in txt.splitlines():
        m = ROW.match(line)
        if m:
            n, l0, l1, v0, v1, d0, d1, r0, r1 = m.groups()
            rows.append((n.strip(), float(l0), float(l1), int(v0), int(v1), float(d0), float(d1), int(r0), int(r1)))
    met = list(js.get("metrics", {}).values())
    base = list(js.get("baseline", {}).values())
    return (base[0] if base else {}), (met[0] if met else {}), rows


def main():
    a = sys.argv[1:]
    lst = 0
    if "--list" in a:
        i = a.index("--list"); lst = int(a[i + 1]); del a[i:i + 2]
    (br, bc), (tr, tc), board, seeds = a[0].split(":"), a[1].split(":"), a[2], [int(x) for x in a[3].split(",")]
    tot = {}
    for s in seeds:
        A, B = rundir(br, bc, s, board), rundir(tr, tc, s, board)
        if not A or not B:
            print(f"s{s}: missing {A} {B}")
            continue
        m0, m1, rows = compare(A, B)
        k = {"changed": len(rows), "drc_up": sum(r[8] > r[7] for r in rows), "drc_dn": sum(r[8] < r[7] for r in rows),
             "vias_up": sum(r[4] > r[3] for r in rows), "vias_dn": sum(r[4] < r[3] for r in rows),
             "new_det": sum(r[5] <= 1.5 < r[6] for r in rows), "gone_det": sum(r[6] <= 1.5 < r[5] for r in rows),
             "longer10": sum(r[2] > 1.1 * r[1] for r in rows), "shorter10": sum(r[2] < r[1] / 1.1 for r in rows)}
        for x, v in k.items():
            tot[x] = tot.get(x, 0) + v
        tm = " ".join(f"{x} {m0.get(x)}->{m1.get(x)}" for x in ("drc_new", "unconnected", "vias", "detour_gt_1_5"))
        print(f"{board} s{s} {bc}->{tc}: {tm} | " + " ".join(f"{x} {v}" for x, v in k.items()))
        if lst:
            for r in sorted(rows, key=lambda r: r[7] - r[8])[:lst]:
                if r[8] > r[7]:
                    print(f"    drc+ {r[0]}: drc {r[7]}->{r[8]} vias {r[3]}->{r[4]} detour {r[5]:.2f}->{r[6]:.2f} len {r[1]:.1f}->{r[2]:.1f}")
            for r in rows:
                if r[5] <= 1.5 < r[6]:
                    print(f"    new detour {r[0]}: {r[5]:.2f}->{r[6]:.2f} len {r[1]:.1f}->{r[2]:.1f} vias {r[3]}->{r[4]} drc {r[7]}->{r[8]}")
    print("SUM " + " ".join(f"{x} {v}" for x, v in tot.items()))


if __name__ == "__main__":
    main()
