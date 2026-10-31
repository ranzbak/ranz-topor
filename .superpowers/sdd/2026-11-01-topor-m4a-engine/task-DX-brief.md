### Task DX (F): dcsgonebadge completion diagnosis (plan A12 Step 2, done early) — `docs/progress/notes/m4-dcsgone-diagnosis.md`

Goal (HARD M4a gate, M3-R14): dcsgonebadge snapshot 3e7e66f862a7, `run_bench(router=route_router, rip="all", threads=4,
options={"wall_clock_s": 600, "partial_routes": False})`: routed == connections_total == 231, partial == failed == 0, routing_new 0,
unconnected_items 0, runtime ≤ 600 s whole run, 22 locked GND items kept, threads 1/2/4 + repeat byte-identical. AND no
per-connection detours where a direct path exists (user rule; detour max ≤ 1.5 is the coordinator's target, report per connection).

State after A1 (search speed, merged on main): the 600 s run is NO LONGER time-limited: router stops by the stop rule at ~144 s
(iterations ~42 incl. DRC-loop / pour-completion passes?), 188 routed of 255 reported (231 + pour completion?), failed 67, vias 199,
routing_new 0, preexisting 4, unconnected 62. 0.4.0 baseline (600 s): 172/231, failed 59, 3 iterations, timeout.
Worst detours (0.4.0): GND SW1.0→pour 14.07 mm vs ghost 0.87 (16×); USB- J1.A7→J1.B7 4.40 vs 1.00 (4.4×); VCC U3.1→U3.3 8.07 vs
2.67 (3×; 11.6× after A1); GND U5.6→pour 2.2×; Net-(D14-DOUT) 1.83×.

Produce ONE diagnosis note (≤ 4 pages) on current main (export the model with the harness; run route_probe --profile and the harness
gate run yourself; engine instrumentation in examples/ or behind Options.profile only, nothing that changes results by default):
1. Exact accounting: connections_total 231 vs the 255 the bench prints (what the extra 24 are, which number the gate uses — read
   harness/bench.py, toporlib/drcloop.py; if the gate's "231/231" definition is ambiguous, say so with numbers for both).
2. Per failed connection (all of them, table): net, endpoints, layer options, failure reason from the engine, which mechanism blocks
   it — (a) terminal entry / fine-pitch pad (#201 class, plan A2), (b) unfit edges / own-net capacity (M4-D1, A4), (c) same-net
   sibling should T-junction (M4-D3, A5), (d) pad exit stub needed (A6), (e) rip-up ping-pong / stop rule / iteration cap (A16,
   [iterations]), (f) via limits / via sites / via_cost (A10, A13), (g) pour completion, (h) genuinely no room (prove with the
   hand layout: the hand board routes all 231 — where does the hand route go?). Use gate_diag / route_probe; group by mechanism with counts.
3. Per-iteration profile: routed/failed/overuse/cost per iteration, why the stop rule fires (no gain? stuck? cap?), time per phase.
4. Detours: top 10 by ratio after A1 with the mechanism (ghost underestimate vs real detour), especially VCC U3.1→U3.3 and GND SW1.0→pour.
5. Ranked fix list: which planned tasks (A2 fine-pitch entry, A4 own-net capacity, A5 Steiner, A6 exit stubs, A13 via relaxation,
   A16 escalating rip cost, stop-rule params) or new small fixes recover how many connections each (estimate with evidence; quick
   experiments with existing Options — e.g. pathfinder_iter_cap, patience, ripup_max_per_pair, via_cost_nm, max_vias_per_connection,
   strict_unfit_edges, margin — are encouraged and reported as numbers). Experiments that improve completion via a non-default option
   value: report them, do not change defaults.
Commit the note only: `docs(diagnosis): M4 dcsgonebadge completion diagnosis`. Do not change engine behaviour.
