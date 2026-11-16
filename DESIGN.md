# limit-wait design note

Finding: bornhack s0 waited >22 min after the CPU limit. Cause: in soft mode `inner()` (core.rs:3809) = Cancel || limit.cuts() only; the
only other cut is the overrun cap `timeout_overrun_s` (auto = timeout_s headless, none in the dialog), so the loop step ran to its end.
Existing checks: sweep wires, via-rose relay, pair relay/move (`stop()`), refine/moves via `control::stop_requested()` (only true once the limit
acts hard). Unchecked between them: layer re-opt (assign_t), floating up, crossing pass.

## Change
1. control.rs TimeLimit: `grace_ns` (set from `[run] limit_grace_s`, default 30, 0 = stop at the next check), `grace_over()` = passed && elapsed >= limit+grace
   (engine wall clock; with timeout_clock=cpu the limit becomes "time spent" at the CPU limit, so grace counts from there). Records `wait_s` / step name once
   when a stop test first sees it.
2. Scope: grace applies ONLY inside the TopoR loop (initial routing P3/P4 must finish: no plan = NOTHING_TO_DRAW; drawing is never cut by grace).
   `with_control` STOP state gets a loop-scope flag (RAII guard around `optimise`); `stop_requested()` adds `scope && l.grace_over()`, so
   moves.rs:158/587 and refine.rs:887 (candidate/move level) stop. `inner` in core.rs:3809 adds `|| grace_over()`, and cycle() gets extra `stop()` checks before
   layer re-opt, floating up and the crossing pass (cheap, no effect without a limit).
3. Consistency: unchanged contract of M2.TL: a cycle `inner()` cut short is NOT offered; the archive (variants of finished cycles + initial plan) is the
   best plan so far; `arch.pick` + `sp.restore(var)` return it, the loop state is rebuilt from the archived LoopVar, so a half-done move in the cut cycle never
   leaks. Cost: at most the cycle's work since its start is lost (bounded wait, not bounded loss). Rationale not to offer partial cycles: label/refine moves
   stopped mid-way leave priced-but-unevaluated state; same rule as hard mode/cap today.
4. Report: when the loop ends after the limit: `LimitReport.loop_wait_s` (loop end - limit instant), `loop_wait_step`, `grace_hit`. Python: finding
   TIME_LIMIT_WAIT (name + seconds; warning when grace_hit), schema option, messages, report.md, docs.
5. No limit => `grace_over()` false, `deadline()` None: same code path, bit-identical (test compares route output with/without limit far away).
Relation to `timeout_overrun_s` (existing cap, hard-cuts everything incl. drawing): grace is the earlier, loop-only wind-down; cap stays as the outer bound.

## Tests
Unit: optimise() with a toy Space whose cycle is long, injected stop/inner closures with fake clock -> bounded; TimeLimit unit tests for grace_over/wait record;
integration on a tiny fixture with limit_grace_s=0 and a limit reached by a test hook (like the m2 hook); no-limit identical test; mutants (grace ignored,
scope leaks to initial routing, partial cycle offered). All on the Hetzner VM.
