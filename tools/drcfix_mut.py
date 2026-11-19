"""Apply one mutant (by id) to the mutant tree; run the tests; restore. usage: mut.py <tree> <id>"""
import subprocess, sys, pathlib
tree = pathlib.Path(sys.argv[1]); mid = sys.argv[2]
R, P = "plugin/topor/run.py", "plugin/topor/postrun.py"
M = {
 "M1": (R, """    for ext, data in project.items():
        (out_dir / src_pcb.with_suffix(ext).name).write_bytes(data)""",
          """    for ext in (".kicad_pro", ".kicad_dru"):
        s = src_pcb.with_suffix(ext)
        if s.is_file():
            (out_dir / s.name).write_bytes(s.read_bytes())"""),
 "M2": (R, "        lost = project_missing(pcb, proj_snap)\n", "        lost = []\n"),
 "M3": (P, "        no_net = [default] if any(not item_net(i) for i in items) else []\n", "        no_net = []\n"),
 "M4": (R, "            if not state.get(\"no_project\"):\n", "            if proj_snap.get(\".kicad_pro\") is not None and not state.get(\"no_project\"):\n"),
 "M5": (R, "        mismatch = state[\"no_project\"]  # the DRC", "        pass  # the DRC"),
 "M6": (R, "    return bad\n", "    return []\n"),
 "M7": (P, "if any(not item_net(i) for i in items)", "if all(not item_net(i) for i in items)"),
 "M8": (P, "if req is None or not items or", "if req is None or not nets or"),
 "M9": (R, "    return {e: pcb_copy.with_suffix(e).read_bytes() for e in PROJECT_EXTS if pcb_copy.with_suffix(e).is_file()}",
          "    return {}"),
 "M10": (R, "        lost = project_missing(pcb, proj_snap)\n", "        lost = project_missing(pcb_copy, proj_snap)\n"),
 "M11": (R, "            if not state.get(\"no_project\"):\n", "            if proj_snap.get(\".kicad_pro\") is None and not state.get(\"no_project\"):\n"),
 "M13": (R, "        lost = project_missing(pcb, proj_snap)  # again AFTER kicad-cli", "        lost = []  # again AFTER kicad-cli"),
 "M14": (P, "key = nets + ((\"<no net>\",) if any(not item_net(i) for i in items) else ())", "key = nets"),
 "M12": (R, "        if lost and state.get(\"no_project\") is None:\n", "        if lost and state.get(\"no_project\") is not None:\n"),
}
f, old, new = M[mid]
p = tree / f; src = p.read_text()
assert src.count(old) >= 1, (mid, src.count(old))
p.write_text(src.replace(old, new, 1))
try:
    tests = ["tests/plugin/test_fail_fast.py", "tests/plugin/test_postrun.py::test_rules_mismatch_item_without_net_uses_the_default_netclass_clearance",
             "tests/plugin/test_postrun.py"]
    sel = ["tests/plugin/test_fail_fast.py::test_project_file_deleted_mid_run_the_drc_copies_keep_the_original_rules",
           "tests/plugin/test_fail_fast.py::test_drc_copy_without_project_is_its_own_error_never_rules_mismatch",
           "tests/plugin/test_fail_fast.py::test_rules_read_wrong_is_a_loud_error_after_the_drc",
           "tests/plugin/test_postrun.py",
           "tests/plugin/test_fail_fast.py::test_project_deleted_between_the_copy_and_kicad_cli_is_drc_without_project",
           "tests/plugin/test_fail_fast.py::test_board_without_kicad_pro_keeps_the_rules_mismatch_check"]
    r = subprocess.run(["python", "-m", "pytest", "-q", "-p", "no:cacheprovider", *sel],
                       cwd=tree, capture_output=True, text=True)
    fails = [l for l in r.stdout.splitlines() if l.startswith("FAILED")]
    last = r.stdout.strip().splitlines()[-1] if r.stdout.strip() else r.stderr[-300:]
    print(f"{mid}: {'KILLED' if r.returncode else 'SURVIVED'} | {last}")
    for l in fails: print("   ", l[:200])
finally:
    p.write_text(src)
