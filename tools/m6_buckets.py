#!/usr/bin/env python3
"""m6_buckets.py BASE TIP [BOARD...]: changed connections between two runs bucketed per board: lost / gained, KiCad DRC per
connection (compass) better / worse, length shorter / longer (> 0.05 mm), and whether the base drew it with engine
violations (forced, never drop). Prints one line per board and, with --list, every changed connection."""
import glob, json, os, sys
from collections import Counter

def load(root, board):
    f = [x for x in sorted(glob.glob(os.path.join(root, '**', 'result.json'), recursive=True)) if '/reports/' in x and f'/{board}' in x]
    if not f:
        return None
    r = json.load(open(f[-1]))
    route = r['route']
    eng = Counter(v['conn'] for v in route.get('violations') or [])
    kd = {c.get('conn'): c.get('violations') or 0 for c in (r.get('compass') or {}).get('connections') or []}
    out = {}
    for c in route['connections']:
        k = (c['name'], *sorted(map(str, (c['from']['label'], c['to']['label']))))
        out[k] = dict(ok=c['status'] == 'routed', eng=eng.get(c['id'], 0), drc=kd.get(c['id'], 0), len=(c.get('length_nm') or 0) / 1e6, vias=c.get('vias') or 0)
    return out

args = [a for a in sys.argv[1:] if not a.startswith('--')]
base, tip, boards = args[0], args[1], args[2:]
for bd in boards:
    b, t = load(base, bd), load(tip, bd)
    if b is None or t is None:
        continue
    cnt = Counter(); rows = []
    for k in b:
        if k not in t:
            continue
        x, y = b[k], t[k]
        if x == y or (x['ok'] == y['ok'] and x['drc'] == y['drc'] and x['vias'] == y['vias'] and abs(x['len'] - y['len']) <= 0.05):
            continue
        tag = 'forced' if x['eng'] else 'legal'
        if x['ok'] and not y['ok']:
            cnt['lost/' + tag] += 1; kind = 'lost'
        elif y['ok'] and not x['ok']:
            cnt['gained'] += 1; kind = 'gained'
        else:
            kind = ('drc-' if y['drc'] < x['drc'] else 'drc+' if y['drc'] > x['drc'] else 'drc=') + \
                   ('/shorter' if y['len'] < x['len'] - 0.05 else '/longer' if y['len'] > x['len'] + 0.05 else '/len=') + \
                   ('/vias-' if y['vias'] < x['vias'] else '/vias+' if y['vias'] > x['vias'] else '')
            cnt[kind + '/' + tag] += 1
        rows.append((k, kind, tag, x, y))
    ddrc = sum(t[k]['drc'] - b[k]['drc'] for k in b if k in t)
    print(f"{bd}: changed {len(rows)}; per-connection KiCad DRC sum {ddrc:+d}; " + ", ".join(f"{k} {v}" for k, v in sorted(cnt.items())))
    if '--list' in sys.argv:
        for k, kind, tag, x, y in rows:
            print(f"   {kind:22s} {tag:6s} {k[0]} {k[1]}->{k[2]}: len {x['len']:.2f}->{y['len']:.2f} vias {x['vias']}->{y['vias']} drc {x['drc']}->{y['drc']}")
