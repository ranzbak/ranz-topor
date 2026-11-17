"""refill-default fix round 2: python mutants of this round's lines (+ the verifier's M2/M4 survivors), each applied in a
copy of the tree, killed = the branch tests fail. usage: python tools/refill_r2_mut.py <tree-copy> <out-dir> [ids...]"""
import subprocess, sys
from pathlib import Path

M = {
 "M2_moot_by_setting_not_result": ("plugin/topor/run.py", 'or not_applied) and not report.facts["zones_refilled"]:', 'or not_applied) and not cfg.get("run", "refill_after_accept"):'),
 "M4_report_committed_wins": ("plugin/topor/report.py", 'if skip not in ("", "off"):', 'if skip not in ("", "off") and not self.facts.get("zone_fills_committed"):'),
 "N1_hint_always": ("plugin/topor/run.py", 'if not cfg.get("run", "refill_after_accept"):\n        fix +=', 'if True:\n        fix +='),
 "N2_hint_never": ("plugin/topor/run.py", 'if not cfg.get("run", "refill_after_accept"):\n        fix +=', 'if False:\n        fix +='),
 "N3_past_default_dropped": ("plugin/topor/schema.py", '    ("run", "refill_after_accept"): ((False, "2026-11-17"),),\n', ''),
 "N4_outcome_one_undo": ("plugin/topor/report.py", '"accepted": "applied to the board"', '"accepted": "applied as one undo step"'),
 "N5_dialog_one_undo": ("plugin/topor/ui_wx.py", '"done_accepted": "Routed {routed}/{total} connections — applied to the board "', '"done_accepted": "Routed {routed}/{total} connections — applied to the board as one undo step "'),
 "N6_dialog_fallback_one_undo": ("plugin/topor/ui_wx.py", 'else "Applied to the board (Edit → Undo).")', 'else "Applied to the board as one undo step (Edit → Undo).")'),
 "N7_fix_text_hint_in_catalogue": ("plugin/topor/messages.py", 'Fill All Zones (B). The report folder', 'Fill All Zones (B); or set run.refill_after_accept = true. The report folder'),
}
TESTS = ["tests/plugin/test_refill_matrix.py", "tests/plugin/test_config.py", "tests/plugin/test_report.py",
         "tests/plugin/test_ui_wx.py", "tests/plugin/test_messages.py", "tests/plugin/test_config_docs.py", "tests/plugin/test_toml_defaults.py"]

def main():
    tree, out = Path(sys.argv[1]), Path(sys.argv[2]); out.mkdir(parents=True, exist_ok=True)
    r0 = subprocess.run([sys.executable, "-m", "pytest", "-q", "-x", "-p", "no:cacheprovider", *TESTS],
                        cwd=tree, capture_output=True, text=True, env={**__import__("os").environ, "PYTHONPATH": "plugin"})
    print(f"baseline (unmutated) rc {r0.returncode}", flush=True)
    (out / "baseline.log").write_text(r0.stdout[-5000:])
    if r0.returncode:
        sys.exit("baseline red: mutant results would be meaningless")
    for mid in sys.argv[3:] or list(M):
        f, old, new = M[mid]
        p = tree / f; src = p.read_text()
        if src.count(old) != 1:
            print(f"{mid}: PATTERN x{src.count(old)}", flush=True); continue
        p.write_text(src.replace(old, new))
        try:
            r = subprocess.run([sys.executable, "-m", "pytest", "-q", "-x", "-p", "no:cacheprovider", *TESTS],
                               cwd=tree, capture_output=True, text=True, env={**__import__("os").environ, "PYTHONPATH": "plugin"})
            (out / f"{mid}.log").write_text(r.stdout[-5000:] + r.stderr[-2000:])
        finally:
            p.write_text(src)
        print(f"{mid}: {'KILLED' if r.returncode else 'SURVIVED'} (rc {r.returncode})", flush=True)

main()
