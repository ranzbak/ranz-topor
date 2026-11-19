"""Mutants of fix round 2 (no-finished-notice): the 8 verifier mutants (re-expressed: ui.done is gone, so "the notice
is back" = a spurious ui.error window) + mutants for the NOTHING_ROUTED error and the press-B preview hint.
Run in a throw-away tree copy (cwd): python tools/nofin_r2_mut.py M1 M2 ..."""
import subprocess, sys, pathlib
PY = sys.executable
RUN, REP = pathlib.Path("plugin/topor/run.py"), pathlib.Path("plugin/topor/report.py")
SPUR = 'ui.error(Finding("FINISHED", "info", "finished"))'
LOOP = '    for w in post_failed:\n        try:'
T = "tests/plugin/test_run_flow.py"
R = "tests/plugin/test_refill_matrix.py"
M = [
 ("M1 notice after Cancel", RUN, '    if not out.items:  # nothing to show or apply (cancelled early, or nothing to route): silent, no window\n        status = "cancelled" if out.cancelled else "rejected"\n',
  '    if not out.items:  # nothing to show or apply (cancelled early, or nothing to route): silent, no window\n        status = "cancelled" if out.cancelled else "rejected"\n        if status == "cancelled": ' + SPUR + '\n', [T, R]),
 ("M2 NOTHING_ROUTED error dropped", RUN, '        ui.error(nr)\n', '        pass\n', [T, R]),
 ("M3 notice after Reject", RUN, 'ctx()  # no "finished" window (user 2026-11-19): the preview', 'ctx(); ' + SPUR + '  # the preview', [T, R]),
 ("M4 notice before the post-commit dialogs", RUN, LOOP, '    ' + SPUR + '\n' + LOOP, [T, R]),
 ("M4b notice after a clean Accept", RUN, LOOP, '    if not post_failed: ' + SPUR + '\n' + LOOP, [T, R]),
 ("M5 no post-commit error dialogs", RUN, LOOP, '    for w in post_failed[:0]:\n        try:', [T, R]),
 ("M6 only the last post-commit dialog", RUN, LOOP, '    for w in post_failed[-1:]:\n        try:', [T, R]),
 ("M7 ZONE_FILLS_SKIPPED not shown", RUN, LOOP, '    for w in [x for x in post_failed if x.code != "ZONE_FILLS_SKIPPED"]:\n        try:', [T, R]),
 ("A1 zero connections also errors", RUN, 'out.routed == 0 and out.total > 0 and not out.cancelled', 'out.routed == 0 and out.total >= 0 and not out.cancelled', [T]),
 ("A2 Cancel also errors", RUN, 'out.routed == 0 and out.total > 0 and not out.cancelled', 'out.routed == 0 and out.total > 0', [T]),
 ("A3 no-copper stays a quiet reject", RUN, 'out.routed == 0 and out.total > 0 and not out.cancelled', 'out.routed == 0 and out.total > 0 and not out.cancelled and False', [T]),
 ("B1 hint always", RUN, 'report.facts["zone_hint"] = not cfg.get("run", "commit_fills") and not cfg.get("run", "refill_after_accept")', 'report.facts["zone_hint"] = True', [T]),
 ("B2 hint never", RUN, 'report.facts["zone_hint"] = not cfg.get("run", "commit_fills") and not cfg.get("run", "refill_after_accept")', 'report.facts["zone_hint"] = False', [T]),
 ("B3 hint when either is off", RUN, 'report.facts["zone_hint"] = not cfg.get("run", "commit_fills") and not cfg.get("run", "refill_after_accept")', 'report.facts["zone_hint"] = not cfg.get("run", "commit_fills") or not cfg.get("run", "refill_after_accept")', [T]),
 ("B4 hint line not in the summary", REP, '*([ZONE_HINT_LINE] if f.get("zone_hint") and self.status == "pending" else []),', '*([]),', [T]),
]
sel = sys.argv[1:] or [m[0].split()[0] for m in M]
orig = {p: p.read_text() for p in (RUN, REP)}
try:
    r = subprocess.run([PY, "-m", "pytest", "-q", "-p", "no:cacheprovider", T, R], capture_output=True, text=True)  # unmutated baseline
    print("BASE (unmutated, must pass):", [l for l in r.stdout.splitlines() if l.strip()][-1:], "rc", r.returncode, flush=True)
    if r.returncode not in (0,):
        print(r.stdout[-3000:]); sys.exit(2)
    for name, f, old, new, tests in M:
        if name.split()[0] not in sel:
            continue
        assert orig[f].count(old) == 1, (name, orig[f].count(old))
        f.write_text(orig[f].replace(old, new))
        r = subprocess.run([PY, "-m", "pytest", "-q", "-x", "-p", "no:cacheprovider", *tests], capture_output=True, text=True)
        tail = [l for l in r.stdout.splitlines() if l.strip()][-1:]
        print(f"{name}: {'KILLED' if r.returncode == 1 else 'RC' + str(r.returncode) + ' SURVIVED' if r.returncode == 0 else 'ERROR rc' + str(r.returncode)} ({tail[0] if tail else ''})", flush=True)
        f.write_text(orig[f])
finally:
    for p, t in orig.items():
        p.write_text(t)
