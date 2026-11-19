#!/usr/bin/env python3
"""tw7-moves mutants of the lines work round 1 changed (A detectors, B pattern moves, D detour coefficient,
E-detect cut_components), each killed (or not) by the targeted tests (topo_tw7_patterns + lib pourplan).

    tools/tw7_mutate.py list
    tools/tw7_mutate.py run <out_dir>     (VM: each mutant applied, tests run, restored; one line per mutant)
    TW7_MUT_ONLY=a,b ...                  (only these mutants)

A mutant = (file, old text, new text); `old` must occur exactly once (else 'stale')."""
import os, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
C = "engine/src/topo/core.rs"
X = "engine/src/topo/xrules.rs"
P = "engine/src/topo/pourplan.rs"
S = "engine/src/topo/escape.rs"
M = {
    "d_off": (C, '} else if name == "length" && self.tw7.detour_k > 0.0 {', '} else if false && name == "length" && self.tw7.detour_k > 0.0 {'),
    "d_amt_ignored": (C, "1.0 + self.tw7.detour_k * self.tw7.detour_amt.get(ci).copied().unwrap_or(0.0)", "1.0 + self.tw7.detour_k"),
    "d_no_refresh": (C, "self.tw7.detour_amt = self.detour_amounts();", "self.tw7.detour_amt = Vec::new();"),
    "d_linear": (C, "if d > 0.0 && l > 0.0 { (l / d - 1.0).max(0.0).powi(2) } else { 0.0 }", "if d > 0.0 && l > 0.0 { (l / d - 1.0).max(0.0) } else { 0.0 }"),
    "b_stage_off": (C, "                if !self.tw7.kinds.is_empty() {", "                if false {"),
    "b_judge_always": (C, "                    if e1 < e0 - 1e-6 {\n                        for &ci in &cis {\n                            self.reroutes[ci] += 1;\n                        }\n                        kept = true;\n                    }\n                }\n                if !kept {\n                    self.restore(&before);\n                }\n                let mut own_u",
                       "                    if true {\n                        for &ci in &cis {\n                            self.reroutes[ci] += 1;\n                        }\n                        kept = true;\n                    }\n                }\n                if !kept {\n                    self.restore(&before);\n                }\n                let mut own_u"),
    "b_reverse_order": (C, "                let mut cis = own.clone();", "                let mut cis: Vec<usize> = own.iter().rev().copied().collect();"),
    "b_evasion_once": (C, "if !kept && p.kind == super::xrules::Kind::MutualEvasion", "if false && !kept && p.kind == super::xrules::Kind::MutualEvasion"),
    "b_no_plain_filter": (C, "if p.first == p.second || !plain(p.first) || !plain(p.second) || !seen.insert", "if p.first == p.second || !seen.insert"),
    "a_parity": (X, "if n % 2 == 1 && ", "if n >= 1 && "),
    "a_cross_check_off": (X, "if n % 2 == 1 && !crossed.get_or_insert_with(|| st.crossings(w).into_iter().map(|x| x.other).collect()).contains(&o) {", "if n % 2 == 1 {"),
    "a_halfopen_closed": (X, "if (sp > 0.0) == (sq > 0.0) {", "if (sp >= 0.0) == (sq > 0.0) {"),
    "a_t_any": (X, "(0.0..=1.0).contains(&t)", "t.is_finite()"),
    "a_evasion_one_way": (X, ".filter(|&&(a, b)| a < b && set.contains(&(b, a)))", ".filter(|&&(a, b)| a < b || set.contains(&(b, a)))"),
    "a_loop_any_net": (X, "if st.wires[*a].net != st.wires[*b].net || !boxes_meet(*ba, *bb) {", "if !boxes_meet(*ba, *bb) {"),
    "a_amount_linear": (X, "    (r - 1.0).max(0.0).powi(2)\n}", "    (r - 1.0).max(0.0)\n}"),
    "e_one_side": (P, "ras.island_at(p + nrm * off) == l || ras.island_at(p - nrm * off) == l", "ras.island_at(p + nrm * off) == l"),
    "e_base_ignored": (P, "c.pads.iter().copied().filter(|p| !base.cut_pads.contains(p)).collect()", "c.pads.to_vec()"),
    "e_probe_on_track": (P, "let off = t.width as f64 / 2.0 + clr + 1.5 * res;", "let off = 0.0;"),
    # round 3: [optimize] pattern_share (escape::Share) and the per-kind counters
    "s_budget_off": (C, "                        if !self.tw7.share.allows() {", "                        if false {"),
    "s_no_spend": (C, "                        self.tw7.share.spend(w);", "                        let _ = w;"),
    "s_no_earn": (C, "                    self.tw7.share.earn(other);", "                    let _ = other;"),
    "s_no_mark": (C, "                    self.tw7.mark = Some(self.work_mark());", "                    self.tw7.mark = None;"),
    "s_no_floor": (C, "let w = self.work_since(k0).max(tries as f64);", "let w = self.work_since(k0);"),
    "s_ratio_plain": (S, "self.credit += u * self.share / (1.0 - self.share);", "self.credit += u * self.share;"),
    "s_debt_forgiven": (S, "            self.credit -= w;", "            self.credit = (self.credit - w).max(0.0);"),
    "s_allows_ge": (S, "self.unbounded() || self.credit > 0.0", "self.unbounded() || self.credit >= 0.0"),
    "k_kind_zero": (C, "                        cs.pat_kinds[p.kind.idx()][0] += 1;", "                        cs.pat_kinds[0][0] += 1;"),
    "k_line_share": (C, "    if share < 1.0 {\n        s.push_str(&format!(\n            \"; work budget", "    if share <= 1.0 {\n        s.push_str(&format!(\n            \"; work budget"),
}
TESTS = ["--lib", "--test", "topo_tw7_patterns", "--", "pourplan", "u_clinch", "evasion", "unknown_pattern", "detour_coefficient", "pattern_share", "the_pattern_moves_line"]


def run(out: Path) -> int:
    out.mkdir(parents=True, exist_ok=True)
    rows = []
    only = [x for x in os.environ.get("TW7_MUT_ONLY", "").split(",") if x]
    env = {**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target-mut")}
    r = subprocess.run(["cargo", "test", "--release", *TESTS], cwd=ROOT / "engine", capture_output=True, text=True, timeout=2400, env=env)
    (out / "baseline.log").write_text(r.stdout[-20000:] + r.stderr[-20000:])
    if r.returncode:
        (out / "summary.txt").write_text(f"BASELINE-FAIL rc {r.returncode}: no mutant run (see baseline.log)\n")
        print("BASELINE-FAIL", flush=True)
        return 1
    rows.append("baseline ok")
    for name, (f, old, new) in M.items():
        if only and name not in only:
            continue
        p = ROOT / f
        src = p.read_text()
        if src.count(old) != 1:
            rows.append(f"{name} stale ({src.count(old)} hits)")
            print(rows[-1], flush=True)
            continue
        p.write_text(src.replace(old, new))
        try:
            r = subprocess.run(["cargo", "test", "--release", *TESTS], cwd=ROOT / "engine", capture_output=True, text=True, timeout=2400, env=env)
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
        print("\n".join(M))
    else:
        sys.exit(run(Path(sys.argv[2])))
