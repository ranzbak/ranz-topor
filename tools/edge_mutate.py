#!/usr/bin/env python3
"""Mutants of the edge-clear fixes, each killed (or not) by the engine tests topo_pad_end + check_clearance.

    tools/edge_mutate.py list
    tools/edge_mutate.py run <out_dir>      (VM: one tree copy, each mutant applied, tests run, restored; rc per mutant)
    tools/edge_mutate.py run-py <out_dir>   (rule C's Python mutants only; TOPOR_PY = the interpreter with pytest)
    EDGE_MUT_ONLY=a,b ...                    (only these mutants)

A mutant = (file, old text, new text); `old` must occur exactly once (else the mutant is reported as 'stale')."""
import shutil, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
M = {
    "no_anchor": ("engine/src/topo/core.rs", "    dr.set_pad_ends(&pad_ends);\n", "    let _ = &pad_ends;\n"),
    "erbs_no_ends": ("engine/src/topo/core.rs", "&dr.pad_end, &super::board::outline_segments(model)", "&BTreeMap::new(), &super::board::outline_segments(model)"),
    "half_stroke": ("engine/src/model.rs", "    pub fn edge_clear(&self) -> i64 {\n        self.edge_clearance\n", "    pub fn edge_clear(&self) -> i64 {\n        self.edge_clearance + self.outline_half_stroke.max(0)\n"),
    "no_inset": ("engine/src/topo/padend.rs", "point_in_poly(p, poly) && boundary_dist(p, poly) >= margin", "point_in_poly(p, poly) && boundary_dist(p, poly) >= 0.0"),
    "octi_centre": ("engine/src/topo/octi.rs", "        let c = self.run_end(w, end);\n", "        let c = pr.pos[if end == 0 { wi.a } else { wi.b }];\n"),
    "no_push": ("engine/src/topo/padend.rs", "for _ in 0..PUSHES {", "for _ in 0..0 {"),
    "no_rays": ("engine/src/topo/padend.rs", "for k in 0..RAYS {", "for k in 0..0 {"),
    "draw_centre": ("engine/src/topo/draw.rs", "let s = if r == 0 { self.run_end(w, 0) }", "let s = if r == 0 { self.end_at.get(&(w, 0)).copied().unwrap_or(self.b.proj.pos[st.wires[w].a]) }"),
    "run_end_no_pad": ("engine/src/topo/padend.rs", ".or_else(|| self.pad_end.get(&(w, end)))", ""),
    "cutflow_no_ends": ("engine/src/topo/cutflow.rs", "let end_pt = |e: usize| ends[e].unwrap_or(", "let end_pt = |e: usize| ends[e].filter(|_| false).unwrap_or("),
    "anchor_any_legal": ("engine/src/topo/padend.rs", "if best.is_none_or(|b| (p - c).len2() < (b - c).len2()) {", "if best.is_none_or(|b| (p - c).len2() > (b - c).len2()) {"),
}
TESTS = ["--test", "topo_pad_end", "--test", "check_clearance"]
# rule C (toporlib.edge_zero + the run flow), killed by the Python tests below
PY = {
    "c_min_pour": ("toporlib/edge_zero.py", "return max((int(p.get(\"clearance\") or 0)", "return min((int(p.get(\"clearance\") or 0)"),
    "c_zero_not_checked": ("toporlib/edge_zero.py", "if r.get(\"edge_clearance\") is None or int(r[\"edge_clearance\"]) > 0:", "if r.get(\"edge_clearance\") is None:"),
    "c_no_hook": ("plugin/topor/run.py", "model, edge_rec = edge_zero_mod.apply(model, edge_zero_of(cfg))", "edge_rec = edge_zero_mod.resolve(model, edge_zero_of(cfg))"),
    "c_layer_min": ("toporlib/edge_zero.py", "return int(round(max(per_layer.values()))) if per_layer else 0", "return int(round(min(per_layer.values()))) if per_layer else 0"),
    "c_no_window": ("toporlib/edge_zero.py", "if d <= POUR_WINDOW_NM:", "if True:"),
    "c_default_board": ("toporlib/edge_zero.py", "DEFAULT_MODE = \"zone_clearance\"", "DEFAULT_MODE = \"board\""),
}
PY_TESTS = ["tests/plugin/test_edge_zero.py", "tests/plugin/test_run_flow.py", "-k", "edge_zero or edge_non_zero or test_edge_", "-q", "-x", "-p", "no:cacheprovider"]


def run(out: Path, only_py: bool = False) -> int:
    out.mkdir(parents=True, exist_ok=True)
    rows = []
    py = __import__("os").environ.get("TOPOR_PY", sys.executable)
    cases = [(n, m, False) for n, m in M.items() if not only_py] + [(n, m, True) for n, m in PY.items()]
    only = [x for x in __import__("os").environ.get("EDGE_MUT_ONLY", "").split(",") if x]  # a round's own mutants
    cases = [c for c in cases if not only or c[0] in only]
    for name, (f, old, new), is_py in cases:
        p = ROOT / f
        src = p.read_text()
        if src.count(old) != 1:
            rows.append(f"{name} stale ({src.count(old)} hits)")
            continue
        p.write_text(src.replace(old, new))
        try:
            if is_py:
                r = subprocess.run([py, "-m", "pytest", *PY_TESTS], cwd=ROOT, capture_output=True, text=True, timeout=2400)
            else:
                r = subprocess.run(["cargo", "test", "--release", *TESTS], cwd=ROOT / "engine", capture_output=True, text=True, timeout=2400,
                                   env={**__import__("os").environ, "CARGO_TARGET_DIR": str(ROOT / "target-mut")})
            (out / f"{name}.log").write_text(r.stdout[-20000:] + r.stderr[-20000:])
            build_fail = "error[" in r.stderr and "test result" not in r.stdout
            rows.append(f"{name} {'BUILD-FAIL' if build_fail else ('killed' if r.returncode else 'SURVIVED')} rc {r.returncode}")
        finally:
            p.write_text(src)
        print(rows[-1], flush=True)
    (out / "summary.txt").write_text("\n".join(rows) + "\n")
    return 0


if __name__ == "__main__":
    if sys.argv[1:2] == ["list"]:
        print("\n".join([*M, *PY]))
    else:
        sys.exit(run(Path(sys.argv[2]), only_py=sys.argv[1] == "run-py"))
