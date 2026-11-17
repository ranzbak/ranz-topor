"""fx_hash.py PCB DUMPDIR -> one line: sha1 of the routed copper (segments, arcs, vias at nm) and of the ERBS dumps (realise-L*.json)."""
import hashlib
import json
import sys
from pathlib import Path

import pcbnew

b = pcbnew.LoadBoard(sys.argv[1])
rows = []
for t in b.GetTracks():
    c = t.GetClass()
    if c == "PCB_VIA":
        p = t.GetPosition()
        rows.append(("v", p.x, p.y, t.GetWidth(), t.GetNetname()))
    elif c == "PCB_ARC":
        rows.append(("a", t.GetStart().x, t.GetStart().y, t.GetMid().x, t.GetMid().y, t.GetEnd().x, t.GetEnd().y,
                     t.GetLayerName(), t.GetNetname()))
    else:
        rows.append(("s", t.GetStart().x, t.GetStart().y, t.GetEnd().x, t.GetEnd().y, t.GetWidth(), t.GetLayerName(),
                     t.GetNetname()))
cu = hashlib.sha1(repr(sorted(rows)).encode()).hexdigest()[:12]
dumps = []
for f in sorted(Path(sys.argv[2]).glob("realise-L*.json")):
    dumps.append(json.dumps(json.load(open(f)), sort_keys=True))
du = hashlib.sha1("\n".join(dumps).encode()).hexdigest()[:12]
print(f"copper {cu} dump {du} nseg {len(rows)}")
