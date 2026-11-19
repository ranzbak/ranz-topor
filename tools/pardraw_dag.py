import json, sys, heapq
# Trace format: TOPO_PDTRACE jsonl of the throw-away branch par-draw-exp (5dcdf3c5). usage: pardraw_dag.py <tag wire|force|via|stub> <board>...
def load(p, tag):
    us=[json.loads(l) for l in open(p)]
    us=[u for u in us if u["tag"]==tag]
    for u in us:
        u["r"]=set(map(tuple,u["r"])); u["wr"]=set(map(tuple,u["wr"])); u["ww"]=set(u["ww"])
        u["fail"]= (not u["ok"])
    return us
def conf(a,p, fail_global=True):
    if p["fail"] and fail_global: return True
    return (not a["r"].isdisjoint(p["wr"])) or p["w"] in a["ww"]
def dag(us, fail_global=True):
    n=len(us); dep=[[] for _ in range(n)]
    for i in range(n):
        for j in range(i):
            if conf(us[i],us[j],fail_global): dep[i].append(j)
    return dep
def crit(us, dep):
    d=[0.0]*len(us)
    for i,u in enumerate(us):
        d[i]=u["t"]+max((d[j] for j in dep[i]), default=0.0)
    return max(d)
def listsched(us, dep, L, fixed_window=None):
    # dataflow list schedule with perfect dependency knowledge, L workers, in order priority
    n=len(us); done=[None]*n; free=[0.0]*L
    t=0.0
    # simple: process in index order; start = max(earliest free worker, max finish of deps)
    for i in range(n):
        ready=max((done[j] for j in dep[i]), default=0.0)
        k=min(range(L), key=lambda x: free[x])
        s=max(free[k], ready); done[i]=s+us[i]["t"]; free[k]=done[i]
    return max(done)
def spec(us, dep, L):
    # speculative, in order: lane takes next unit, snapshot = commits so far; commit in order;
    # invalid if a dep committed after its snapshot -> redo (re-speculate at once with fresh snapshot)
    n=len(us); import heapq
    depset=[set(x) for x in dep]
    commit_time=[None]*n
    now=0.0
    nxt=0; lanes=[]  # (finish, unit, snapshot_committed_count)
    committed=0
    events=[]
    free=L
    pending={}  # unit -> (finish, snap)
    t=0.0
    while committed<n:
        while free>0 and nxt<n:
            pending[nxt]=(t+us[nxt]["t"], committed); nxt+=1; free-=1
        # advance to next commit possibility: unit 'committed' finishes
        u=committed
        fin,snap=pending[u]
        t=max(t,fin)
        # validate: deps in [snap, u)
        if any(snap<=j<u for j in depset[u]):
            # redo now on master (snapshot = all before u)
            pending[u]=(t+us[u]["t"], u)
            t=t+us[u]["t"]
        del pending[u]; committed+=1; free+=1
    return t
TAG=sys.argv[1]
for b in sys.argv[2:]:
    us=load(b+".jsonl",TAG); tot=sum(u["t"] for u in us)
    for fg in (True, False):
        dep=dag(us,fg)
        cp=crit(us,dep)
        print(f"{b} fail_global={fg}: serial {tot:.2f}s, critical path {cp:.2f}s (max speedup {tot/cp:.2f}x), mean deps {sum(map(len,dep))/len(us):.1f}")
        for L in (2,4,8,16):
            print(f"   L={L}: dataflow {tot/listsched(us,dep,L):.2f}x, in-order speculation {tot/spec(us,dep,L):.2f}x")
