"""PAD-CENTRE-MAIN RCA (throw-away): split KiCad-Cleanup removals (cleanup_check replay) into forced-side vs victim
copper, and simulate 'the engine does not write a shorting forced connection' (G3): delete only the forced nets'
shorting items, cascade dangling, then replay the full Cleanup on what is left.
  python tools/pad_centre_main_split.py <routed.kicad_pcb> <result.json> <out.json>"""
import json, sys, collections
from pathlib import Path
import harness.kicad_env  # noqa (before pcbnew)
import pcbnew
from harness import cleanup_check as cc

pcb, res, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
r = json.loads(res.read_text())
forced = sorted({v["name"] for v in r["route"]["violations"] if v["kind"] == "short"})
full = cc.cleanup_items(pcb)
by_net = collections.Counter(i["net"] for i in full)
b = pcbnew.LoadBoard(str(pcb))
cl = cc._Cleaner(pcbnew, b)
c = cl.conn()
rm = {}
for t in cl.tracks():
    if t.IsLocked() or t.GetNetname() not in forced:
        continue
    for o in list(c.GetConnectedPads(t)) + list(c.GetConnectedTracks(t)):
        if o.GetNetCode() != t.GetNetCode():
            if cl.key(t) not in rm:
                cl.record("g3_drop_short", t, f"touches {o.GetNetname()}", o)
            rm[cl.key(t)] = t
for t in rm.values():
    cl.remove(t)
cl.dangling(tracks=True, vias=True)
dropped = list(cl.items)
cl.items = []
cl.run(cc.ALL_ON)
after = cl.items
d = {"forced_nets": forced, "full_count": len(full), "full_by_net": dict(by_net),
     "full_victim": {n: k for n, k in by_net.items() if n not in forced},
     "g3_dropped": len(dropped), "g3_dropped_by_net": dict(collections.Counter(i["net"] for i in dropped)),
     "g3_cleanup_after": len(after), "g3_cleanup_after_items": after}
out.write_text(json.dumps(d, indent=1))
print(json.dumps({k: v for k, v in d.items() if k != "g3_cleanup_after_items"}, indent=1))
