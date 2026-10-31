# Task P1 report: engine prerequisites (stop rule, cancel + progress, route_async, default_options, option fields)

Status: DONE_WITH_CONCERNS (minor). Branch worktree-agent-af7071c05b4adf702, commit 9415324.

## Changes
- engine/src/control.rs (new): RunControl{cancel,progress}, Progress, expired()/cancelled() (O(1)). Low-level searches/relax keep their `Instant` deadline signatures; the run's cancel flag is installed in a thread-local on every thread of the run's private rayon pool (start_handler), so concurrent runs never cancel each other. Ctx::stop() = deadline or cancel at all route.rs sites; negotiate/relax/search call control::expired.
- route.rs: STALL_ITERS removed; stop rule `no_gain` (pub): over last `patience` iterations best routed did not grow and best cost (sum len + via_cost*vias) gained < min_gain_pct. RouteResult.stop_reason in converged/cap/no gain/timeout/cancel/stuck. route_with(model, opts, &RunControl); route() delegates. Progress per iteration (iteration cumulative across completion passes; routed/total/vias/elapsed high-water marks). A cancel seen at assembly (late stages) gives timed_out=true / "cancel". wall_clock_s 0.0 = disabled, also in pour-completion budget.
- model.rs: Options pathfinder_iter_cap 30, patience 5, min_gain_pct 1.0, detour_factor 2.0, ripup_growth 2.0 (stored; A16), ripup_max_per_pair 8, expert{hist_fail 1.0, hist_block 0.5, hist_overuse 0.5, present_overuse 4.0, zero_slack_candidates 4, relax_iter_cap 200}; all serde default. Options::relax_cap(): top-level relax_iter_cap if != 200 else expert's.
- negotiate.rs: constants -> Negotiator cfg via configure(opts); detour factor, present_overuse from options. smooth.rs: deadline_after(0.0) = far future.
- py.rs: route_async -> RouteJob (progress/cancel/done/wait; wait releases GIL, idempotent), default_options().
- harness/progress.py: --milestone {m3,m4} (name only), optional last caption line stop_reason; tests/test_router.py tolerates it.
- Tests: engine/tests/route_control.rs (5; cancel test release-only, ignored in debug; heavy ones serialised), tests/test_engine_surface.py (+3).
- Existing tests adapted (assertions kept): wall_clock_s 0.0 "immediate timeout" -> 1e-6 (route_engine, smooth_engine x2); model_json default cap 50 -> 30; rip_up_ping_pong_is_damped pins patience 8 and ripup_max_per_pair = RIP_PAIR_CAP (3), warning prefix "no gain:"; eprom front-only guard accepts "no gain:|stuck after".

## Numbers (run_bench rip=all threads=4; before = unchanged 0.3.0 engine)
| board | metric | before | after |
|---|---|---|---|
| eprom-to-kernal | routed/total | 27/27 | 27/27 |
| | vias / routing_new / unconnected | 0/0/0 | 0/0/0 |
| | router_s / iterations | 0.182 / 1 | 0.080 / 1 |
| | stop_reason | n/a | converged |
| | result JSON minus runtime_s, threads_used, stop_reason | | byte-identical |
| wiccon-2023 | routed/total | 62/62 | 62/62 |
| | vias / routing_new / unconnected | 37/0/0 | 37/0/0 |
| | router_s / iterations | 6.87 / 26 | 4.61 / 20 |
| | stop_reason | n/a | no gain |
| | outside-pad length nm | 1373553063 | 1373553063 (items identical; iterations/warnings differ) |

PNG: docs/benchmarks/progress/2026-10-31-m4-P1-wiccon-2023.png.

## Tests
cargo test --release all pass (route_control 5/5 x3); debug cargo (tests/test_scaffold) pass; clippy -D warnings clean (also --features python). Scaling: 3 passed. Gate: 17 passed. Default suite green after fixing a caption test and debug-mode test sizes; test_router, test_engine_surface, test_scaffold re-run green.
TDD note: implementation preceded tests; cancel test hardened after a timing flake.

## Concerns
- ripup_max_per_pair default 8 (brief) vs M3 constant 3; eprom/wiccon unchanged; damping test pins 3.
- expert.relax_iter_cap duplicates top-level relax_iter_cap (relax_cap()).
- Late cancel reports timed_out=true even if result complete; cancel timing is CPU/board dependent so tests assert cancel + >=1 routed + clean check, not partial count.
- scratch/boards in the worktree is a symlink to the main checkout (ignored).
