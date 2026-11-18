#!/usr/bin/env python3
"""m6_forced.py BASE TIP BOARD: the connections routed in BASE and failed in TIP, split by whether BASE wrote them with
engine violations (route.violations, i.e. drawn by force with a rule broken) or legal (no engine violation: legal by the
normal ladder, or a forced candidate that came out legal). Plus the reverse direction. Matching by net + terminals."""
import glob, json, os, sys
from collections import Counter

def load(root, board):
    f = sorted(glob.glob(os.path.join(root, '**', board + '*', '**', 'result.json'), recursive=True))
    f = [x for x in f if '/reports/' in x]
    r = json.load(open(f[-1]))['route']
    viol = Counter(v['conn'] for v in r.get('violations') or [])
    out = {}
    for c in r['connections']:
        k = (c['name'], *sorted(map(str, (c['from']['label'], c['to']['label']))))
        out[k] = (c['status'] == 'routed', viol.get(c['id'], 0), c.get('reason') or '', c.get('length_nm') or 0, c.get('vias') or 0)
    return out

b, t = load(sys.argv[1], sys.argv[3]), load(sys.argv[2], sys.argv[3])
lost = [(k, b[k], t[k]) for k in b if k in t and b[k][0] and not t[k][0]]
gain = [(k, b[k], t[k]) for k in b if k in t and t[k][0] and not b[k][0]]
lv = [x for x in lost if x[1][1] > 0]
ll = [x for x in lost if x[1][1] == 0]
print(f"{sys.argv[3]}: lost {len(lost)}: {len(lv)} with engine violations in base (forced), {len(ll)} legal in base; gained {len(gain)}")
for k, x, y in ll:
    print(f"  legal-in-base lost: {k}  base len {x[3]/1e6:.2f} vias {x[4]}  tip reason: {y[2][:110]}")
