#!/usr/bin/env python3
"""prof_fold.py (TW0): fold `perf script -F comm,tid,time,period,ip,sym` (demangled; recorded with `perf record -k mono -g`)
into collapsed stacks whose ROOT frame is the run stage the sample fell in, for inferno-flamegraph.

  perf script ... | rustfilt | python3 tools/prof_fold.py <X--metrics.json|result.json> > stacks-timed.txt

Stage windows come from the run's `profile.stages` (CLOCK_MONOTONIC mono0/mono1 per engine stage); before the first one
"python before the engine", after the last one "python after the engine". Weight = 1 per sample (cpu-clock at the record
frequency F: CPU s = samples / F). Child processes (kicad-cli) keep their comm as the second frame.
Output lines: `stage;comm;root..leaf samples`.
"""
import bisect
import collections
import json
import re
import sys

d = json.load(open(sys.argv[1]))
st = (d.get("profile") or {}).get("stages") or []
if not st:
    sys.exit("no profile.stages in " + sys.argv[1])
if (d.get("profile") or {}).get("mono_ok") is False:
    sys.exit("profile.mono_ok is false: the engine could not read CLOCK_MONOTONIC, the stage windows cannot be folded")
BEFORE, AFTER = "python before the engine", "python after the engine"
ends = [s["mono1"] for s in st]
names = [s["stage"] for s in st]
t_first = st[0]["mono0"]


def stage(t):
    if t < t_first:
        return BEFORE
    i = bisect.bisect_left(ends, t)
    return names[i] if i < len(names) else AFTER


hdr = re.compile(r"^\s*(\S.*?)\s+(\d+)\s+(\d+\.\d+):\s+(\d+)\s*")
fold = collections.Counter()
cur, frames = None, []


def flush():
    global cur, frames
    if cur is not None:
        comm, t = cur
        fold[";".join([stage(t), comm] + [f.replace(";", ",") for f in reversed(frames)])] += 1
    cur, frames = None, []


for line in sys.stdin:
    if not line.strip():
        flush()
        continue
    if line[0] not in " \t":
        flush()
        m = hdr.match(line)
        if m:
            cur = (m.group(1).replace(" ", "_"), float(m.group(3)))
        continue
    p = line.strip().split(" ", 1)
    frames.append(p[1].strip() if len(p) > 1 else p[0])
flush()
for k, v in fold.items():
    print(f"{k} {v}")
