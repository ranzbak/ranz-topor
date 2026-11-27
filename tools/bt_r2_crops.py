"""bundle-turn r2 RCA: per focus net, a three-panel crop (main | tip with oblique_price 0 | tip) zoomed on the union of
the net's copper in the three boards (+ margin). Needs KiCad's pcbnew (run on the VM).

usage: python tools/bt_r2_crops.py OUT_DIR MAIN.kicad_pcb OBL0.kicad_pcb TIP.kicad_pcb NET [NET ...]"""
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import matplotlib  # noqa: E402

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import pcbnew  # noqa: E402

from harness.dvn_crop_png import draw  # noqa: E402

MM = 1e6


def net_box(pcb, net):
    b = pcbnew.LoadBoard(pcb)
    xs, ys, length, vias = [], [], 0.0, 0
    for t in b.GetTracks():
        if t.GetNetname() != net:
            continue
        if t.GetClass() == "PCB_VIA":
            vias += 1
            p = t.GetPosition()
            xs.append(p.x / MM)
            ys.append(p.y / MM)
            continue
        for p in (t.GetStart(), t.GetEnd()):
            xs.append(p.x / MM)
            ys.append(p.y / MM)
        length += t.GetLength() / MM
    return (min(xs), min(ys), max(xs), max(ys)) if xs else None, length, vias


def main():
    out, pcbs, nets = sys.argv[1], sys.argv[2:5], sys.argv[5:]
    os.makedirs(out, exist_ok=True)
    labels = ["main 316e86ef", "tip, oblique_price 0", "tip (defaults)"]
    summary = {}
    for net in nets:
        boxes = [net_box(p, net) for p in pcbs]
        bb = [b[0] for b in boxes if b[0]]
        if not bb:
            continue
        x0, y0 = min(b[0] for b in bb) - 1.5, min(b[1] for b in bb) - 1.5
        x1, y1 = max(b[2] for b in bb) + 1.5, max(b[3] for b in bb) + 1.5
        spec = {"view": [x0, y0, x1, y1], "focus": [net], "nominal": {}}
        fig, axs = plt.subplots(1, 3, figsize=(21, 7.8))
        for ax, p, lab, (_, ln, nv) in zip(axs, pcbs, labels, boxes):
            draw(ax, p, spec, f"{lab}: {net} {ln:.2f} mm, {nv} vias")
        fig.suptitle(f"bundle-turn r2 RCA: {net} (bold = focus net)", fontsize=10)
        fig.tight_layout(rect=(0, 0.02, 1, 0.94))
        safe = "".join(c if c.isalnum() or c in "-_+" else "_" for c in net)
        fig.savefig(os.path.join(out, f"{safe}.png"), dpi=90)
        plt.close(fig)
        summary[net] = [{"len": round(ln, 3), "vias": nv} for (_, ln, nv) in boxes]
    json.dump(summary, open(os.path.join(out, "crops.json"), "w"), indent=1)


if __name__ == "__main__":
    main()
