#!/usr/bin/env python3
"""m6_tables.py JSONDIR: markdown tables of the M6 measurement from the ladder_diff --json --extra files
(<cfg>.json = ladder_diff seqD <cfg>): per public board and the pattern total, routed/total, drc_new, unconnected,
vias, detour > 1.5, per config. Numbers only from those files."""
import json, os, sys
J = sys.argv[1]
cfgs = ['seqD', 'seqOFF', 'seqForce', 'seqLadder', 'seqRoom', 'seqPull', 'seqFollow', 'erbsD', 'erbsOFF']
d = {c: json.load(open(os.path.join(J, c + '.json')))['metrics'] for c in cfgs}
keys = sorted(d['seqD'])
boards = [k for k in keys if any(b in k for b in ('eprom', 'wiccon', 'esp', 'qmtech'))]
pats = [k for k in keys if k not in boards]
F = ['routed', 'drc_new', 'unconnected', 'vias', 'detour_gt_1_5']
def cell(m):
    return f"{m.get('routed')}/{m.get('total')} · {m.get('drc_new')} · {m.get('unconnected')} · {m.get('vias')} · {m.get('detour_gt_1_5')}"
print('| run | ' + ' | '.join(cfgs) + ' |')
print('|---' * (len(cfgs) + 1) + '|')
for k in boards:
    print(f'| {k} | ' + ' | '.join(cell(d[c].get(k, {})) for c in cfgs) + ' |')
def tot(c, ks):
    s = {f: 0 for f in F + ['total']}
    for k in ks:
        for f in s:
            s[f] += d[c].get(k, {}).get(f) or 0
    return s
print(f'| 4 boards | ' + ' | '.join(cell(tot(c, boards)) for c in cfgs) + ' |')
print(f'| {len(pats)} patterns | ' + ' | '.join(cell(tot(c, pats)) for c in cfgs) + ' |')
print()
print('Patterns that differ between configs (routed/total · drc_new · unconnected · vias · detour > 1.5):')
print()
print('| pattern | ' + ' | '.join(cfgs) + ' |')
print('|---' * (len(cfgs) + 1) + '|')
for k in pats:
    cs = [cell(d[c].get(k, {})) for c in cfgs]
    if len(set(cs)) > 1:
        print(f'| {k} | ' + ' | '.join(cs) + ' |')
same = [k for k in pats if len({cell(d[c].get(k, {})) for c in cfgs}) == 1]
print()
print('Same numbers in every config: ' + ', '.join(same))
