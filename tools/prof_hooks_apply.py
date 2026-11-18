#!/usr/bin/env python3
"""prof_hooks_apply.py <srcdir> [patch]: put the TW0 engine hooks (tools/prof-hooks.patch) on a rev that lacks them.

`git apply --3way` in <srcdir>. One known conflict is resolved here: loop-local (TW1) splits `relabel` / `assign_with`
into thin wrappers plus `relabel_cached` / `assign_cached`; the guards then belong to the cached functions (the wrappers
delegate, so every call is counted once). Any other conflict stops with a message (port the hooks by hand). Exit 0 = applied.
"""
import re
import subprocess
import sys
from pathlib import Path

src = Path(sys.argv[1])
patch = Path(sys.argv[2] if len(sys.argv) > 2 else Path(__file__).with_name("prof-hooks.patch")).resolve()
r = subprocess.run(["git", "apply", "--3way", str(patch)], cwd=src, capture_output=True, text=True)
conf = subprocess.run(["git", "diff", "--name-only", "--diff-filter=U"], cwd=src, capture_output=True, text=True).stdout.split()
if not conf:
    sys.exit(0 if r.returncode == 0 else (print(r.stderr) or 1))
if conf != ["engine/src/topo/layers.rs"]:
    sys.exit(f"hooks conflict in {conf}: port by hand\n{r.stderr}")
p = src / conf[0]
s = p.read_text()
# conflict block: ours (the wrapper + cached fn head) / theirs (the guard lines)
pat = re.compile(r"<<<<<<< ours\n(.*?)=======\n(.*?)>>>>>>> theirs\n", re.S)


def fix(m):
    ours, guards = m.group(1), m.group(2)
    if "_cached(" not in ours:
        raise SystemExit("unexpected conflict shape in layers.rs: port by hand")
    return ours + guards


s2 = pat.sub(fix, s)
if "<<<<<<<" in s2:
    sys.exit("conflict left in layers.rs: port by hand")
p.write_text(s2)
subprocess.run(["git", "add", conf[0]], cwd=src)
print("hooks applied (layers.rs: guards moved into the cached functions)")
