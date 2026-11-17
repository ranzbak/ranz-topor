# DESIGN: limit-wait round 3 (2026-11-17) — soft-step wait, `[run] drawing_grace_s`

Status: note before coding (B3); the soft-step wait (lead decision) is described here too.

## 1. The wait counts the whole soft step (lead decision)
A *soft step* = what the soft limit lets finish (operate.md "timeout_mode"; M2.TL):
- `loop = "topor"`: **initial routing** (plane stubs, projection, topological search, layer assignment /
  rip-up), **TopoR loop**, **drawing** (drawing the traces, feedback, never-drop, via reduction, fuse,
  pour stitching of the variants);
- `loop = "cycles"`: the **whole-route cycle** (all of the above for one cycle; the next cycle is the next step).
Each progress stage maps to one soft step (`control::soft_step_of(stage)`, one table; a unit test reads every
`ctl.stage(..)` literal of core.rs via `include_str!` and fails on an unmapped one). Per run the soft step
id = (round, class). The run's wait is noted once, at the first change of that id after the limit (or
`idle()`, or the explicit end of the loop step) — not at every stage change. Runs > 1: the shared plane
stubs / projection is part of every run's initial routing (a run's first stage inherits it).
`note_loop_end()` (finding 2: it fetch_max'ed loop-end - limit for every limit) goes; the loop step ends by
`ctl.end_soft_step()` right after `optimise()` (single run: no stage change until "drawing the traces").
The last step (no change after it) keeps the `runtime - limit` fallback, now tested.

## 2. `[run] drawing_grace_s` (user B3)
**What.** Seconds the drawing may run past the limit before its *optional* passes stop. Clock: from the
later of the limit and the drawing's start (so the loop's own grace is not counted twice), engine wall clock
(timeout_clock = cpu: from the moment the plugin reached the limit, like limit_grace_s). Soft mode only;
hard / past the overrun cap the drawing is cut as today (TIME_DRAWING_CUT).
**Never cut by it:** the drawing of every wire (vias, wires, feedback redraw, never-drop, junctions), fuse,
M-45 elbows, diff pairs (D7), pour stitching (joins pour pieces: cutting it leaves islands = unconnected in
KiCad DRC, i.e. a drop). **Cut by it (optional, anytime):** the via reduction (S4/P6: every accepted relabel
stays; a cut leaves vias it had not removed, no wire lost). Asked for B3: ERBS (`realise = "erbs"`, opt-in)
today uses the limit itself as its deadline in soft mode (stretches past it are drawn sequentially, every
wire still drawn); proposal: its deadline becomes the same bound (limit-or-draw-start + drawing_grace_s).
**Values.** Default **30 s** (same as limit_grace_s; qmtech: wires ~50 s, via reduction ~5 s, so a qmtech
drawing that starts after the limit skips the via reduction; 60 s would keep it — lead/user pick). `0` = the
optional passes stop at their first check once past the limit; any negative = no bound (as limit_grace_s,
tests -1 and -5); schema range `<= 86400`.
**Report.** LimitReport `draw_grace_s` (None = unbounded), `draw_grace_hit`; the via-reduction warning says
"stopped by [run] drawing_grace_s"; the TIME_LIMIT_WAIT line for a drawing step names it; result.json,
report.md, log.txt; schema doc (→ generated topor.toml + configuration.md), operate.md, troubleshoot.md.
**Invariants.** (a) every wire is drawn whatever the value (same connections drawn as with no bound);
(b) a run that does not reach the limit is bit-identical (the bound is read only once the limit passed);
(c) only the via reduction (and ERBS if agreed) ask it.
**Literature.** TopoR draws the chosen variant once after the judging (TopoR 6.3 manual pp. 72-73, p. 237);
the via reduction is a post-pass that keeps any accepted relabel (anytime), so cutting it is a quality, not a
completeness, loss.
**Tests.** Engine (eprom fixture, test hook reaches the limit at "via reduction"): grace 0 → reduction stopped,
`draw_grace_hit`, routed = reference routed, same wires; grace -1 / -5 → reduction runs to its end;
no limit → bit-identical to main; mutant of the check killed. Python: schema accepts -5, report/run log line.
