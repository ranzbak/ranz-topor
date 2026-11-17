#!/usr/bin/env python3
"""limit-wait r3: judge the phase sweep (tools/limit_wait_r3_sweep.rs JSON lines; from the r2 verifier's checker).

The real wait = from the limit to the end of the SOFT STEP it fell in (lead 2026-11-17): the first poller-seen change
of that run's soft step (initial routing / TopoR loop / drawing; loop = "cycles": the whole-route cycle, i.e. the next
cycle's projection) or the run falling idle, else the end of the call. Runs of a best-of-N call: the longest run counts;
a run's first stages continue the call's shared plane stubs / projection.
usage: limit_wait_r3_check.py sweep-*.jsonl [--show N] [--json out.json]"""
import json
import re
import sys
from collections import Counter, defaultdict

TOL = 0.004  # s: poller period + scheduling
GROUP = [("plane stubs", "initial"), ("projection", "initial"), ("topological search", "initial"), ("layer assignment", "initial"),
         ("TopoR loop", "loop"), ("picking the variant", "drawing"), ("drawing the traces", "drawing"), ("via reduction", "drawing"),
         ("pour stitching", "drawing")]


def group(stage):
    for k, v in GROUP:
        if stage.startswith(k):
            return v
    return stage or "-"


def per_run(s):
    """open_steps text -> {run: stage} (0 = shared / the only run)."""
    if not s.startswith("run ") or ": " not in s:
        return {0: s}
    out = {}
    for part in s.split("; "):
        m = re.match(r"run (\d+): (.*)$", part)
        if m:
            out[int(m.group(1))] = m.group(2)
    return out


def run_stage(rec, k):
    d = per_run(rec)
    if 0 in d:
        return d[0]
    return d.get(k, "")


def main(paths, show, jout):
    viol, info = defaultdict(list), Counter()
    phases = Counter()
    for p in paths:
        for ln in open(p):
            d = json.loads(ln)
            if d["delay"] is None or not d["reached"]:
                info["no limit / not reached"] += 1
                continue
            t, st, wl = d["t_lim"], d["stages"], d["wait_lines"]
            key = f'{d["fix"]}:{d["name"]}'
            cyc = d["variant"] == "cyc"
            near = any(abs(s[0] - t) < TOL for s in st)
            before = [s for s in st if s[0] <= t]
            after = [s for s in st if s[0] > t]
            at = before[-1][1] if before else ""
            runs = sorted({k for s in st for k in per_run(s[1]) if k > 0}) or [0]
            waits = []
            for k in runs:
                sk = run_stage(at, k)
                if not sk:
                    continue
                g = group(sk)
                end = d["t_end"]
                prev = g
                for s in after:
                    sk2 = run_stage(s[1], k)
                    g2 = group(sk2)
                    if not sk2:
                        end = s[0]
                        break
                    if cyc:
                        # the whole-route cycle: it ends where the next cycle's initial routing begins
                        if g2 == "initial" and prev != "initial":
                            end = s[0]
                            break
                    elif g2 != g:
                        end = s[0]
                        break
                    prev = g2
                waits.append(end - t)
            exp = max(waits) if waits else d["t_end"] - t
            step, wait, gh, dropped, mode = d["step"], d["wait_s"], d["grace_hit"], d["cycle_dropped"], d["mode"]
            ph = f"{d['variant']} at {group(per_run(at).get(0) or next(iter(per_run(at).values()), ''))}"
            info[ph] += 1
            if near:
                info["near a stage change (not judged)"] += 1
                continue
            phases[group(per_run(at).get(0) or next(iter(per_run(at).values()), ''))] += 1
            if step != at:
                viol["step != polled stage at the limit"].append((key, step, at))
            if abs(wait - exp) > max(TOL, 0.02 * exp):
                viol["reported wait != real soft-step wait"].append((key, at, round(wait, 4), round(exp, 4)))
            due = wait > 0 or dropped
            if due != bool(wl):
                viol["line presence"].append((key, wait, dropped, wl))
            if len(wl) > 1:
                viol["more than one line"].append((key, wl))
            if not wl:
                continue
            w = wl[0]
            in_loop = "TopoR loop" in step
            if step and step not in w:
                viol["line does not name the step"].append((key, step, w))
            if "finished within" in w and (not in_loop or wait > (d["grace_s"] or 1e9) + 0.0005 or dropped):
                viol["'finished within' but not true"].append((key, wait, d["grace_s"], w))
            if "limit_grace_s" in w and "ran out" in w and "drawing_grace_s" not in w.split("ran out")[0][-40:] and not (in_loop and gh):
                viol["'limit_grace_s ran out' without a grace hit in the loop"].append((key, w))
            if dropped and "dropped" not in w:
                viol["dropped cycle not said"].append((key, w))
            if "was dropped" in w and not dropped:
                viol["'dropped' said but none dropped"].append((key, w))
            if mode == "hard" and ("soft limit" in w or "finished within" in w):
                viol["hard mode line speaks of the soft limit"].append((key, w))
            if d.get("draw_grace_hit") and "drawing_grace_s" not in w:
                viol["drawing bound hit not said"].append((key, w))
            if mode == "hard" or d.get("cap_hit"):
                info[f"line in {mode} mode / cap_hit={d.get('cap_hit')}"] += 1
    print("cases:", dict(sorted(info.items())))
    print("judged per soft step:", dict(phases))
    for k, v in viol.items():
        print(f"VIOLATION {k}: {len(v)}")
        for x in v[:show]:
            print("   ", x)
    if not viol:
        print("no violations")
    if jout:
        json.dump({"cases": dict(info), "judged": dict(phases), "violations": {k: len(v) for k, v in viol.items()}}, open(jout, "w"), indent=1)


if __name__ == "__main__":
    a = sys.argv[1:]
    show, jout = 3, None
    if "--show" in a:
        i = a.index("--show"); show = int(a[i + 1]); del a[i:i + 2]
    if "--json" in a:
        i = a.index("--json"); jout = a[i + 1]; del a[i:i + 2]
    main(a, show, jout)
