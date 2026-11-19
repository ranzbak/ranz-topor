"""parocti_mut.py M (run in engine/): apply one par-octi mutant to the sources.
M1 the lanes' results in completion order, in par::map and in the crew (must fail); M2 the memo overlays merged in reverse order (equivalent by
design: the memo values are pure functions of the leg); M3 the prune against the successor's row as the node found it
(must fail the reference test); M4 a unit's results applied to its successors in reverse order (must fail)."""
import sys

m = sys.argv[1]


def sub(p, a, b):
    s = open(p).read()
    assert s.count(a) == 1, (m, a)
    open(p, "w").write(s.replace(a, b))


ZIP = "                            for (&(a, b), (ups, own)) in jb.runs.iter().zip(res) {"
APPLY = "                                for (&j, up) in jb.js[a..b].iter().zip(ups) {"
if m == "M1":
    sub("src/topo/par.rs", "    let next = AtomicUsize::new(0);\n", "    let next = AtomicUsize::new(0);\n    let done = AtomicUsize::new(0);\n")
    sub("src/topo/par.rs", "        let r = unit(i);\n", "        let r = unit(i);\n        let i = done.fetch_add(1, Ordering::Relaxed);\n")
    # the crew (octilinear): a unit's result goes to the slot of its completion rank
    sub("src/topo/par.rs", "        let slot = i;\n", "        let slot = FILLED.fetch_add(1, Ordering::SeqCst);\n")
    sub("src/topo/par.rs", "        sh.next.store(0, Ordering::SeqCst);\n", "        sh.next.store(0, Ordering::SeqCst);\n        FILLED.store(0, Ordering::SeqCst);\n")
    sub("src/topo/par.rs", "/// Spins of a waiting lane", "static FILLED: AtomicUsize = AtomicUsize::new(0);\n\n/// Spins of a waiting lane")
elif m == "M2":
    sub("src/topo/octi.rs", ZIP,
        "                            let mut res = res;\n"
        "                            let mut owns: Vec<Memo> = res.iter_mut().map(|x| std::mem::take(&mut x.1)).collect();\n"
        "                            owns.reverse();\n"
        "                            for o in owns.drain(..) {\n"
        "                                memo.merge(o);\n"
        "                            }\n" + ZIP)
elif m == "M3":
    sub("src/topo/octi.rs", "                let mut dj = dj;\n", "                let dj0 = dj;\n                let mut dj = dj;\n")
    sub("src/topo/octi.rs", "                        if d0 + c >= dj[dout] {", "                        if d0 + c >= dj0[dout] {")
elif m == "M4":
    sub("src/topo/octi.rs", APPLY, APPLY.replace("jb.js[a..b].iter().zip", "jb.js[a..b].iter().rev().zip"))
else:
    raise SystemExit(f"unknown mutant {m}")
