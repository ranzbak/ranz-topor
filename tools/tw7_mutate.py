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
    # round 4: b_judge_always / b_reverse_order ported to the held_relay fold (their r3 text is gone)
    "b_judge_always": (C, "                    let (keep, refused) = site_keep(e0, e1, det_old, det_new);", "                    let (keep, refused) = (true, site_keep(e0, e1, det_old, det_new).1);"),
    "b_reverse_order": (C, "                let mut cis = own;\n                cis.dedup();", "                let mut cis: Vec<usize> = own.into_iter().rev().collect();\n                cis.dedup();"),
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
    # round 4: [optimize] pattern_stuck, C via trade (+ hold), E pour move, the report line
    "st_rank_plain": (C, "out.push((rank(amt, &key), PatCand { kind: p.kind,", "out.push((amt, PatCand { kind: p.kind,"),
    "st_rank_fn": (C, "    amt / (1.0 + w * f64::from(stuck))", "    amt / (1.0 + w)"),
    "st_no_count": (C, "*self.tw7.stuck.entry(p.key).or_insert(0) += tries as u32;", "let _ = self.tw7.stuck.entry(p.key).or_insert(0);"),
    "st_no_reset": (C, "                            self.tw7.stuck.remove(&p.key);", "                            let _ = &p.key;"),
    "st_retried_off": (C, "cs.pat_retried += usize::from(self.tw7.stuck.get(&p.key).is_some_and(|&n| n > 0));", "cs.pat_retried += 0;"),
    "vt_off": (C, "                if has(Kind::ViaTrade) {", "                if false && has(Kind::ViaTrade) {"),
    "vt_no_hold": (C, "if let (0, Some(l)) = (k, hold) {", "if let (usize::MAX, Some(l)) = (k, hold) {"),
    "vt_crossed_mover": (X, "if o == w || st.wires[o].net == wi.net || crossed.contains(&o) {", "if o == w || st.wires[o].net == wi.net {"),
    "vt_no_subject_filter": (X, "if !subject(w) || detour_amount(st, w) <= 0.0 {", "if detour_amount(st, w) <= 0.0 {"),
    "vt_movers_unplain": (C, "if !movers.iter().all(|&o| plain(o)) ||", "if false ||"),
    "vt_one_mover_dup": (C, "|| (movers.len() == 1 && seen.contains(&(w.min(movers[0]), w.max(movers[0]))))", "|| false"),
    "e_off": (C, "                if has(Kind::PourCut) {", "                if false && has(Kind::PourCut) {"),
    "e_no_bump": (C, "self.st.history[e] = h + p.bump_h;", "self.st.history[e] = h;"),
    "e_warn_off": (C, "if !pour_on && kinds.iter().any(|k| k == Kind::PourCut.name()) {", "if false {"),
    "e_line_pass": (C, "stuck_w, opts.pour_cuts, &rep.stats));", "stuck_w, false, &rep.stats));"),
    "f_relay_detour": (C, "self.held_relay(&[], obs, None, self.move_detour, seed, amp, via_w)", "self.held_relay(&[], obs, None, 0.0, seed, amp, via_w)"),
}
TESTS = ["--lib", "--test", "topo_tw7_patterns", "--test", "topo_site_social", "--", "--include-ignored", "pourplan", "u_clinch", "evasion", "unknown_pattern", "detour_coefficient", "pattern_share", "the_pattern_moves_line", "pattern_stuck", "the_pattern_rank", "via_trade", "pour_move", "terminal_clinch", "site_move", "branch_move", "detour"]


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
