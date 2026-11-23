# limit-p3p4: the soft time limit bounds P3/P4, and a stop there keeps the plan so far

## What (finding F10)

On a large board routed from scratch, the soft time limit is not kept and Accept gives nothing:

- With `timeout_mode = soft` (the default), the initial routing (P2 search, P3 layer assignment,
  P4 rip-up and moves, all under the progress stage "layer assignment, rip-up and reroute") is one
  step that the limit never cuts (`expired()` in `route_round` is Cancel / OK or the hard limit only,
  core.rs ~3443; `control::stop_requested()` ditto, control.rs ~1310). In the KiCad dialog the
  overrun cap is off (`timeout_overrun_s = auto` = no cap with `waits_for_ok`, run.py ~1055), so
  nothing ever bounds it.
- The refinement loops do poll `stop_requested()` (refine.rs `moves_around`, refine_par.rs `run`,
  moves.rs), but that test is false for the soft limit outside the TopoR loop's scope.
- A Cancel / OK during P3/P4 makes the run keep no plan (core.rs ~4544, "the plan counts once P4
  ends", M2.TL), so Accept ends with NOTHING_TO_DRAW.
- Measured on private board P (stripped, 794 connections, 3 signal + 3 plane layers, 1 run, 6
  threads): P2 16 s; one global layering = 369 s, almost all in the capacity refinement (200 000
  evals = the `cap_evals` cap, 64 kept moves, over-full 3916 -> 3722; greedy + swaps 0.08 s). With
  5 runs on 6 threads one layering takes > 1000 s. P3/P4 run several such layerings (crossing pass,
  P4 moves). So it is many long calls, each bounded only by its eval cap.

## Change

1. The layer assignment / rip-up step (from P3 until the TopoR loop begins) is bounded like the
   TopoR loop step: past the limit plus `[run] limit_grace_s` (default 30 s, existing key, its text
   extended) its stop tests answer true (a scope guard sets the loop-scope flag that
   `stop_requested()` already reads; `expired()` in core.rs reads `grace_over()` in that span). The
   refinement keeps every move it made (each kept move lowers the price), the crossing pass and
   P4 rounds keep their best so far (existing restore paths), and the run goes on to its TopoR loop,
   which ends at once (its own grace test) with the initial plan as generation 0, then the drawing.
2. Cancel / OK during that span, after P2 gave every connection its place, keeps the plan so far
   (P2's topology + the best layering so far) as the run's variant and draws it. Cancel / OK during
   P2 still keeps no plan (NOTHING_TO_DRAW: the first plan does not exist yet). The plugin abandons a
   Cancel's job anyway, so only Accept's result is read.
3. P2 itself is not cut by the soft limit: it is the first plan (every connection placed). The hard
   limit still cuts P2 as before.
4. Report: the `time limit wait` line names the bound for the initial routing (new branch); the
   round warns once "initial plan: the layer assignment step was stopped by <limit grace | Cancel /
   OK>; the plan so far was kept".

## Why / literature

TopoR 6.3 manual (docs/literature/eremex2017-topor63-user-manual.pdf), p. 73: "The first saved
variant appears only after the first optimization cycle has been completed or 20 seconds after the
start of the optimization." p. 65: "100-percent complete net routing is done almost instantaneously
... You can stop optimization at any time by clicking Stop". So the complete first routing is the
first plan, the improvement after it is time-boxed and stoppable, and a stop keeps a variant. Our P2
= the complete routing; P3/P4 = improvement of it.

## Invariants

- No limit reached / no stop: every check is false, so copper is bit-identical to main
  (eprom/wiccon/esp/qmtech: copper_hash).
- Once P2 ends, every run that began has a drawable plan: a stop or the limit in P3/P4 never gives
  NOTHING_TO_DRAW.
- The soft limit is kept to limit + grace + one check (one refine eval: ~2 ms on P), plus P2 when
  P2 itself ends after the limit (reported as before).

## Tests

- Synthetic fixture (harness/fixtures.py, a generated many-connection board with a small
  `cap_evals`-heavy refinement) where P3/P4 alone exceed a 1-2 s limit: the run ends within limit +
  grace + slack and draws all connections it placed (engine test, release, < 60 s).
- Stop (`request_stop`) during P3 on the same fixture: a drawn result, no NOTHING_TO_DRAW.
- Stop during P2: still NOTHING_TO_DRAW (unchanged).
- wait_line text for the new branch (Rust); Python formatting if a field changes.
- Mutants: scope guard off, grace read off in `expired()`, the keep-plan branch reverted.
