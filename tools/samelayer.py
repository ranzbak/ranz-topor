"""Independent check of a topo_route --out JSON: different-net track segments that properly cross on one layer."""
import json, sys, collections
d = json.load(open(sys.argv[1]))
segs = collections.defaultdict(list)
for it in d['items']:
    p = it.get('prim')
    if p and p.get('kind') == 'seg':
        segs[it['layer']].append((it['net'], p['a'], p['b']))
def orient(a, b, c): return (b[0]-a[0])*(c[1]-a[1]) - (b[1]-a[1])*(c[0]-a[0])
def cross(s, t):
    a, b, c, e = s[1], s[2], t[1], t[2]
    o1, o2, o3, o4 = orient(a, b, c), orient(a, b, e), orient(c, e, a), orient(c, e, b)
    return o1*o2 < 0 and o3*o4 < 0
n = 0
G = 2_000_000
for l, ss in segs.items():
    grid = collections.defaultdict(list)
    for i, s in enumerate(ss):
        xs = [s[1][0], s[2][0]]; ys = [s[1][1], s[2][1]]
        for gx in range(min(xs)//G, max(xs)//G+1):
            for gy in range(min(ys)//G, max(ys)//G+1):
                grid[(gx, gy)].append(i)
    seen = set()
    for cell in grid.values():
        for i in range(len(cell)):
            for j in range(i+1, len(cell)):
                a, b = cell[i], cell[j]
                if (a, b) in seen: continue
                seen.add((a, b))
                if ss[a][0] != ss[b][0] and cross(ss[a], ss[b]): n += 1
print("tracks per layer", {l: len(s) for l, s in segs.items()}, "same-layer different-net proper crossings:", n)
print("length_by_layer_nm", d.get('length_by_layer_nm'))
