"""pad-snap (A1b): count the track ends that end inside a pad of their net, at and off the pad centre.

    python tools/padsnap_terminals.py <routed.kicad_pcb> <out.json>

A terminal = a track end no other track end of its net and layer meets (within 2 um) and no via covers,
lying inside a pad of its net on its layer (pcbnew PAD.HitTest). Off centre = more than 1 um from the
pad's anchor (KiCad's push-and-shove router links a pad only there: pns_node.cpp NODE::addSolid).
Writes {"terminals", "off_centre", "by_ref": {ref: [terminals, off]}, "off": [rows]} and prints one line.
Read only (pcbnew in the harness's isolated KiCad home).
"""
from __future__ import annotations

import collections
import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import harness  # noqa: F401,E402  (KiCad isolated before pcbnew)
import pcbnew  # noqa: E402


def main(pcb: str, out: str) -> int:
    b = pcbnew.LoadBoard(pcb)
    segs = [t for t in b.GetTracks() if t.GetClass() in ("PCB_TRACK", "PCB_ARC")]
    vias = [t for t in b.GetTracks() if t.GetClass() == "PCB_VIA"]
    pads_by_net = collections.defaultdict(list)
    for f in b.GetFootprints():
        for p in f.Pads():
            if p.GetNetCode() > 0:
                pads_by_net[p.GetNetCode()].append((f.GetReference(), p))
    ends = collections.defaultdict(list)
    for t in segs:
        for pt in (t.GetStart(), t.GetEnd()):
            ends[(t.GetNetCode(), t.GetLayer(), pt.x // 2000, pt.y // 2000)].append(pt)
    terms, off, by_ref = 0, [], collections.defaultdict(lambda: [0, 0])
    for t in segs:
        for pt in (t.GetStart(), t.GetEnd()):
            net, ly = t.GetNetCode(), t.GetLayer()
            near = sum(1 for dx in (-1, 0, 1) for dy in (-1, 0, 1)
                       for q in ends.get((net, ly, pt.x // 2000 + dx, pt.y // 2000 + dy), ())
                       if math.hypot(q.x - pt.x, q.y - pt.y) <= 2000)
            if near > 1 or any(v.GetNetCode() == net and v.HitTest(pt) for v in vias):
                continue
            for ref, p in pads_by_net.get(net, ()):
                if not (p.IsOnLayer(ly) and p.HitTest(pt)):
                    continue
                terms += 1
                c = p.GetPosition()
                d = math.hypot(pt.x - c.x, pt.y - c.y) / 1e6
                by_ref[ref][0] += 1
                if d > 0.001:
                    by_ref[ref][1] += 1
                    off.append({"ref": f"{ref}.{p.GetNumber()}", "net": t.GetNetname(), "layer": b.GetLayerName(ly),
                                "x": pt.x / 1e6, "y": pt.y / 1e6, "d_mm": round(d, 4), "w": t.GetWidth() / 1e6})
                break
    doc = {"pcb": pcb, "terminals": terms, "off_centre": len(off), "by_ref": dict(sorted(by_ref.items())),
           "off": sorted(off, key=lambda r: (r["ref"], r["x"], r["y"]))}
    Path(out).write_text(json.dumps(doc, indent=1) + "\n")
    print(f"terminals {terms} off_centre {len(off)} U6 {by_ref.get('U6', [0, 0])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(*sys.argv[1:3]))
