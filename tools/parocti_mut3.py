"""parocti_mut3.py M (run in engine/): one fix-round-3 mutant (verifier F1-F3), must fail its test.
M5 no guard at map_leased (the crew invariant test: no panic / a hang); M6 the crew's body not marked (body case);
M7 the reference test's lane switch not set on the pool worker (F2: nodes alone at 2/4/8 lanes); M8 the ERBS fold
keeps a stretch after the first cut one (F3)."""
import sys

m = sys.argv[1]


def sub(p, a, b):
    s = open(p).read()
    assert s.count(a) == 1, (m, a)
    open(p, "w").write(s.replace(a, b))


if m == "M5":
    sub("src/topo/par.rs", '    assert_no_crew("par::map");\n', "")
elif m == "M6":
    sub("src/topo/par.rs", "        let _in = CrewMark::new();\n        let mut c = Crew", "        let mut c = Crew")
elif m == "M7":
    sub("src/topo/exact.rs", "                            OCTI_LANE_MIN.with(|f| f.set(1));\n                            OCTI_NODE_SPLIT", "                            OCTI_NODE_SPLIT")
elif m == "M8":
    sub("src/topo/erbs.rs", "            Ok(v) if stopped.is_none() => kept.push((k, v)),", "            Ok(v) => kept.push((k, v)),")
else:
    raise SystemExit(f"unknown mutant {m}")
