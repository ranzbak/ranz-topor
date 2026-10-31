# M3 scaling diagnosis (F, 2026-10-31) — layers=1 regression, debug-only stub failures, dcsgone 0/231

Scope (ruling M3-R10): (A) the `tests/scaling` layers=1 regression since M3 start, (B) the debug-profile-only
failures of `partial_on_emits_two_checked_stubs` / `partial_off_emits_nothing_for_blocked_connection`,
(C) why the M2 dcsgone informational gate passes with 0/231 routed. Method for A: the synth models of the
test (`synth_model(n, 1, layers=1)`, n = 10/100/1000, byte-identical at every commit — `synth.py` only gained
the `layers` parameter) run through a minimal `route()` runner (`Options {wall_clock_s: 120}`, default
threads, best of 3) built at each commit in a detached worktree; counters from `route_probe` where it
exists (85c21fb on), and a temporary per-iteration trace (negotiate / metrize phases, rip events).
Machine: 8 cores, idle apart from the run.

## A — layers=1 scaling regression

### Measurements (best of 3, seconds; `routed / iterations`)

| commit | what | n=10 | n=100 | n=1000 | n=1000 routed / it | t100/t10 | t1000/t100 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8ace64c | M3 start | 0.0022 | 0.036 | 0.61 | 1000 / 7 conv | 16.5 | 16.7 |
| 85c21fb | T2b (sleeve + memo) | 0.0027 | 0.045 | 0.64 | 1000 / 7 conv | 16.6 | 14.3 |
| 72a1843 | T3 (D1/D6/D9) | 0.0041 | 0.067 | 0.70 | 1000 / 7 conv | 16.3 | 10.4 |
| 26c19c0 | merge T3+T6+T7 | 0.0031 | 0.050 | 0.74 | 1000 / 7 conv | 16.1 | 14.9 |
| 80e30d8 | T8 layered A* (planar entry) | 0.0050 | 0.081 | 1.18 | 1000 / 7 conv | 16.1 | 14.7 |
| **4b928ec** | **T9 two-layer route.rs** | **0.023** | **0.40** | **11.0** | **998 / 19 stalled** | 17.4 | 27.5 |
| 4b928ec + no sites (experiment) | | 0.0053 | 0.100 | 1.70 | 1000 / 17 stalled | 18.9 | 16.9 |
| 9005d56 | T11 rayon batch | 0.016 | 0.54 | 14.3 | 999 / 25 stalled | 33.7 | 26.4 |
| 6c4cd28 | T12 (head of the failing run) | 0.025 | 0.87 | 14.6 | 999 / 25 stalled | 34.7 | 16.7 |
| **fixed** (this branch) | | **0.0031** | **0.052** | **1.25** | **1000 / 17 stalled** | 16.7 | 24.2 |

(The test bounds are t100/t10 ≤ 40 and t1000/t100 ≤ 30; the pytest run that failed measured 0.0146 / 0.546 /
17.69 s, ratio 32.4 — same shape with the pytest overhead on n=10.) T9 is the regression commit; T11 added
iterations at n=100 (3 → 10) but no new cost class.

### Root cause RC-A1 — via sites on a single-layer board (`route.rs`, the `via_sites` call before `MultiGraph::build_on`)

T9 generates via sites for every board (`route.rs` "Via sites (P3)": escape sites at every SMD terminal pad plus
the incentre of every union-triangulation face that fits a via) and `MultiGraph::build_on` (`multigraph.rs:111`,
`obs.add_site_anchors`) makes every site a dormant anchor, i.e. a CDT vertex of **every** layer's graph. On the
synth layers=1 boards: n=10 → 110 sites, n=100 → 1 154, n=1000 → **11 837 sites** for 2 000 pads. With one
routable layer nothing can ever use them (`try_via` needs a second layer) but the single graph now has ~6 ×
the vertices and faces. `route_probe` counters at n=1000, 6c4cd28 vs 80e30d8:

| | runs | expansions/run | clearance candidates/run | ms/run | search total |
| --- | --- | --- | --- | --- | --- |
| 80e30d8 (no sites) | 1 505 | 17 | 77 | 0.24 | 0.36 s |
| 6c4cd28 (11 837 sites) | 3 562 | 210 | 5 546 | 5.31 | 18.9 s CPU |
| fixed | 2 221 | 22 | 98 | 0.37 | 0.83 s |

Three consequences, all gone without the sites: (1) 12 × the expansions and 70 × the clearance candidates per
search (the sleeve queries of T2 walk the extra faces; `ms/run` × 22); (2) **overuse that never clears** —
iteration 1 ends with 24 overused edges and every later iteration keeps 5–24 (edges with a dormant "free" end,
`GEdge::free` / `end_gaps`, are shared by the wires squeezed between site vertices), so `converged` is never
reached and the loop runs to the stall stop (`STALL_ITERS` 8 after the last improvement): 25 iterations;
(3) 85 metrization failures in iteration 1 instead of 15 (the sketch triangulation differs) and a best snapshot
of **999/1000** (iteration 14/17) — the 1000th never lands before the stall stop. The post-loop passes
(zero-slack, via reduction, blocks) also cost 1.8 s instead of 0.15 s on the bigger graph.

**Fix (this branch):** `route.rs` builds via sites only when `routable.len() >= 2` or a live connection targets a
pour (a pour goal is a site; `route_pour_completion_*` tests keep their behaviour). A two-layer board is
untouched (eprom: 31 sites before and after, output byte-identical). Regression tests
`single_layer_board_generates_no_via_sites` (`RouteResult.sites == 0`, 6/6, 1 iteration, converged) and
`single_layer_pour_target_keeps_via_sites` (`sites >= 1`) in `engine/tests/route_engine.rs`; the first fails
before the fix (110 sites).

### Residual RC-A2 — rip-up ping-pong of the absurd-detour rule (not fixed, evidence for F2)

Without sites the run is 1.25 s against 0.61–0.74 s at M3 start, i.e. still ~0.5 s over: n=1000 takes 17
iterations (stall stop) instead of 7 (converged), and 1000/1000 is reached at iteration 8. Per-iteration trace
(fixed engine, n=1000): iteration 1 = 0.37 s (negotiate 0.21 s incl. 600 of 1 000 batch paths invalidated by
P4 and re-searched sequentially — RC2c of the gate diagnosis, F4 — and metrize 0.15 s); iterations 2–17 are
0.01–0.06 s each **except iteration 7, whose metrize takes 0.36 s: one relax run hitting the 200-sweep cap for an
"infeasible: pass cap" wire** (M2 behaviour, 1.8 ms/sweep over 9 legs). The extra iterations are a cycle:
```
it 6  RIP detour by #43:  est 39.7 ghost 9.0 blockers [171]       ripped [43, 171, 263, 594]
it 7  RIP detour by #171: est 20.5 ghost 8.5 blockers [43, 594, 670]   ripped [43, 171, 594, 670]
it 8  RIP detour by #43 [594], #171 [43, 594], #670 [43, 171, 498, 594]  (routed 1000)
it 9  RIP detour by #171: blockers [43, 594, 670]                   (routed 1000, ripped 4 -> no convergence)
it 10..17 same four connections rip each other; stale 8 -> stall stop
```
`negotiate.rs` commit phase: a found path longer than `DETOUR_FACTOR` (2×) the ghost rips ≤ `MAX_RIP` blockers of
the ghost (history `HIST_BLOCK` on their edges, a bump for the detouring connection). The ripped blockers are
active next iteration; whichever of the mutually crossing straight paths is routed second detours and rips the
first back. The rule compares lengths, so the accumulating history never settles it; only an empty or > 3
blocker set ends it. T8 (80e30d8) has the identical rule and the identical iterations 1–5; it escapes at
iteration 7 because #594's ghost had no blockers (`blockers []`) — luck, not design: iteration 1's metrization
differs slightly between T8 and T9 (`relax.rs`/`sketch.rs` leg changes: 7042.3 vs 7028.7 mm, failed set
{395, 712} vs {320, 383}) and the chaos diverges from there. The same mechanism explains T11's n=100 3 → 10
iterations. Not fixed here: it is the detour/ghost rule, which F2 (topological ghost: different `est_len` and
blockers) rewrites anyway — **F2 must add a damping rule** (e.g. a connection does not rip a blocker that ripped
it in the previous iteration, or a per-connection rip budget), with a test that pins `iterations` on a fixture
of two crossing straight pairs. Expected gain at n=1000: 17 → ≤ 9 iterations, ≈ 0.4 s (plus the 0.36 s
pass-cap relax that the cycle provokes), i.e. back to ≈ 0.6–0.7 s and ratio ≈ 15.

### Verification of the fix
- Synth layers=1 (best of 3): n=10 0.0031 s 10/10 1 it; n=100 0.052 s 100/100 4 it; n=1000 1.25 s
  1000/1000 17 it; t100/t10 16.7 (≤ 40), t1000/t100 24.2 (≤ 30); no timeout. Before: 0.025 / 0.87 / 14.6 s.
- Synth layers=2: see the table below (two-layer boards keep their sites: byte-identical to 6c4cd28).
- eprom (`docs/benchmarks/2026-10-31-topor-0.2.0/work/eprom-to-kernal--model.json`, two routable layers):
  `route()` JSON (minus `runtime_s`/`threads_used`) byte-identical to 6c4cd28; 27/27, 1 iteration, vias 0,
  31 sites, outside 64.628 mm, zero_slack_wires [].
- Threads 1 vs 4: byte-identical dumps on synth n=100/1000, layers 1 and 2.

| synth layers=2 (best of 3) | n=10 | n=100 | n=1000 | n=1000 routed / it / vias |
| --- | --- | --- | --- | --- |
| 6c4cd28 | 0.090 | 1.53 | 27.18 | 1000 / 4 conv / 1000 |
| fixed | 0.084 | 1.73 | 27.26 | 1000 / 4 conv / 1000 (dump byte-identical to 6c4cd28 at n=100 and n=1000) |

- `eprom_ripped_front_only_*` (`route_engine.rs`): the F.Cu-only eprom is the M2 board again and gives the M2
  gate result — 27/27, +5V zero-slack corridor kept, outside 67.095 mm, 15 iterations stall stop — instead of
  T9's 26/27 without the corridor (the T9 docstring attributed that to the free via-site vertices). The test
  now pins the M2 result (`eprom_ripped_front_only_is_the_m2_result`: sites 0, 27/27, `zero_slack == "kept"`
  for `+5V`, outside ≤ 67.1 mm, clean at margin 0 except the zero-slack wire within the 1 nm tolerance,
  byte-identical reruns).

## B — debug-profile-only partial-stub failures (`engine/tests/route_engine.rs`)

**Root cause.** `opts()` gave every test a 10 s `wall_clock_s`. On the wall fixture the negotiation finishes in
0.05 s, then `collect_blocks` runs the geometric block probe (`route.rs` `probe_path`: 6 steps, each
`check_items` over the base plus up to 28 × 28 = 784 candidate stubs, D9 cost `O(steps · (|base| + 784) · q)`):
0.35 s in release, **~6 s in the unoptimised build**, and the suite runs its 22 tests concurrently, so the probe
ends after the deadline. `partial_pass` (`route.rs`, after the stub specs) then returns at
`Instant::now() >= cx.deadline` with every stub still `Partial::Dropped` — the connection is reported as
`"partial stub failed self-check"` although no stub was ever checked, and `timed_out` stays `false` (the loop
had finished in time). Traced with a 6 s budget in debug: `before collect_blocks 0.045 s, after 6.09 s,
partial_pass early return: deadline true`. Release passes because the whole run takes 0.4 s. The test runs
alone in debug in 7.1 s — a race with the 10 s budget whenever the machine is loaded (`partial_off` fails the same
way when its sibling's probe pushes it over; its assertion on the blocking point still holds, `reason` differs).

**Fix.** (1) Engine: `Partial::Skipped` — a deadline in the partial pass (before or inside its relax) reports
`"partial stubs skipped: deadline"` instead of a self-check failure; `Dropped` is now only a real self-check
drop. (2) Tests: `opts()` uses a budget that cannot fire (600 s, the precedent of 3b8e0bd for the probe test);
these tests pin geometry and iteration counts, every test of the deadline sets its own (`0.0`, `0.05`). No
assertion was weakened. Both profiles pass (`cargo test --test route_engine` debug and `cargo test --release`).

## C — M2 dcsgone informational gate: 0/231 in 60.04 s (explanation, no fix)

`tests/gate/test_m2_gate.py:74` runs `run_bench(..., options={"wall_clock_s": 60, "partial_routes": False})`,
`threads` 4 (`bench.py:225` default), rip all. In the engine the negotiation loop (`route.rs`, `for _ in
1..=pathfinder_iter_cap`) calls `neg.iteration` and on `out.timed_out` breaks **before** `metrize` and before a
`Snap` is recorded. `Negotiator::iteration` (`negotiate.rs`) times out when any search returns
`SearchOutcome::Timeout` (`search.rs:1198`, checked every 16 expansions) in the parallel phase or in the
sequential re-search of an invalidated path. On dcsgone iteration 1 needs more than 60 s on this machine (gate
diagnosis: 379 searches at 140 ms, 77 ghosts at 607 ms = 47 s CPU, one 24 s exhaustive search for an
unenterable goal, 149 sequential re-searches; T11 on a quiet machine got 164/231 through iteration 1). So
`best` stays `None`, every connection gets `reason = "timeout"` (`no_result`), `routed = 0`, `items = []`,
`timed_out = true`; the harness writes nothing, DRC finds `routing_new = 0`, one round, `router_s = 60.04 ≤ 65`:
the informational test asserts only those two things and passes. Reproduced on the fixed engine with the
0.2.0 model JSON and the same options (default threads): `wall 60.007 s, routed 0 / 231, iterations 0,
timed_out true`, every connection `reason: "timeout"`, `iteration: 0`. It is a defect of the anytime contract (the
paths already committed in iteration 1 are in `mg` but are never metrized / emitted), masked by a test that does
not assert `routed`. F2 (ghost 607 → ~5 ms), F3 (no 24 s exhaustive search) and F4 (≈ 20–40 instead of 149
re-searches) are expected to bring iteration 1 to a few seconds, which removes the symptom on dcsgone; F5 does
not touch it. The structural gap stays: a board whose first iteration exceeds the budget returns nothing. Not
scheduled — recommended as an F4 extension or an F8: on `out.timed_out` in iteration 1, metrize the committed
subset (`out.paths`) and emit it as the anytime result, plus a gate assertion on `routed > 0` for dcsgone.

## Tooling
`engine/examples/route_probe` counters; the bisect used a 60-line `route()` runner (scratch, not committed) that
prints `routed / iterations / converged` and dumps the result JSON without the run-dependent fields, built at each
commit in `git worktree add --detach` checkouts; env-guarded traces (negotiate / metrize phases, rip events,
relax sweeps) applied by scratch patch scripts and removed again. Models: `tests/scaling/synth.py` via the venv
python (pure Python), written to the scratch dir.
