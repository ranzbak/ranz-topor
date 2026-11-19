#!/usr/bin/env python3
"""tw5_plan.py (branch tw5-escape): the plan numbers of TW5 runs from their result.json warnings.

  tw5_plan.py ROOT:CFG[,CFG...] BOARD SEEDS

Per run: the plan handed to the drawing (over-full edge-layers, vias without a site), the pre-loop P4 line's
over-full before -> after, the in-loop P4 text (TW5 f), the loop's cycles. Text taken as the engine wrote it.
"""
import glob, json, re, sys


def main():
    root, cfgs = sys.argv[1].rsplit(":", 1)
    board, seeds = sys.argv[2], [int(x) for x in sys.argv[3].split(",")]
    for cfg in cfgs.split(","):
        for s in seeds:
            fs = glob.glob(f"{root}/{cfg}s{s}/{board}/reports/*/*/result.json") or glob.glob(f"{root}/{cfg}s{s}/{board}/**/result.json", recursive=True)
            if not fs:
                print(f"{cfg} s{s}: no result.json")
                continue
            w = json.load(open(fs[0]))["route"]["warnings"]
            p = next((x for x in w if "plan handed to the drawing:" in x), "")
            m = re.search(r"plan handed to the drawing: (\d+) over-full edge-layers, (\d+) vias without a site", p)
            pv = re.search(r"checked: (\d+) predicted violations", next((x for x in w if x.startswith("ERBS drawing")), ""))
            p4 = next((x for x in w if x.startswith("P4 negotiated")), "")
            m4 = re.search(r"over-full \(edge-layers \+ vias without a site\) (\d+) -> (\d+)", p4)
            esc = next((x for x in w if x.startswith("TopoR loop escapes")), "")
            lp = esc.split("in-loop P4 rounds ")[1] if "in-loop P4 rounds " in esc else "-"
            print(f"{cfg} s{s}: predicted {pv.group(1) if pv else '?'} plan over-full {m.group(1) if m else '?'} no-site {m.group(2) if m else '?'} | pre-loop P4 over-full "
                  f"{(m4.group(1) + '->' + m4.group(2)) if m4 else 'off'} | in-loop P4 {lp}")


if __name__ == "__main__":
    main()
