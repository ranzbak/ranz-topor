### Task GR (sonnet, rotation 2): `[ripup] growth` default by measurement (main Q1 / A-C22) + pour_cut_cost sweep (measurement only)

Read rotation-2-common.md first. Background: task-A16-report.md "Concerns up front" 1 and ledger Ruling A-C22. With the shipped default
`ripup_growth 2.0` (max_per_pair 8) the F.Cu-only M2 guard `engine/tests/route_engine.rs::eprom_ripped_front_only_is_the_m2_result`
gives 27/27 clean but 92.29 mm (> 67.8 bound); growth 1.0/1.5 reproduced M3 (67.03 mm), 1.75/1.9/3.0 lost a connection (26/27). The
guard currently runs with growth 1.0 / max_per_pair 3 — main's ruling: **guard tests must run with the shipped default; do not pin a
guard to non-default options to hide the default's behaviour.** Also red: `engine/tests/graph_ownnet.rs::front_only_plus5v_needs_zero_
slack_for_failed` (runs Options::default(); at growth 2.0 +5V routes without the zero-slack wire, the test expects ["+5V"]).

Part 1 — measure growth ∈ {1.0, 1.25, 1.5, 1.75, 2.0, 3.0} (max_per_pair 8 = default) on: dcsgonebadge, the eprom F.Cu-only guard
fixture (both guard tests' numbers: routed, outside mm, zero_slack wires, rips), eprom-to-kernal, wiccon-2023, nametag-sao,
wedding-gift (bornhack2026 as info if it runs in ≤ 600 s). Harness boards with gate-style options {"wall_clock_s": 600,
"partial_routes": False, "ripup_growth": g}, threads 4 (dcsgone measuring rules in rotation-2-common.md: use wall_clock_s 0 when the
machine is loaded; nametag/wedding/bornhack likewise). Per cell: routed/total, failed, routing_new, unconnected, detours > 1.5 (ref) +
max, outside length, vias, router_s, rips total + pairs at backstop, stop_reason. Also run graph_ownnet front_only per value.
Part 2 — choose and apply the default by main's rule: completion first (sum of failed over the boards; eprom/wiccon not-worse bounds and
BOTH eprom front-only guard tests are hard constraints at the chosen default), then routing_new 0 everywhere, then the sum of detours
> 1.5, then length / vias. If one value wins: set it as the default in engine Options (model.rs), plugin/topor/schema.py, docs
(docs/design/46-configuration.md row 17 text and the generated reference: `.venv/bin/python -m plugin.topor.docs_gen`, --check clean), make
`eprom_ripped_front_only_is_the_m2_result` run with Options::default() rip settings (no growth/max_per_pair pin), keep the default-pin
test, and make graph_ownnet green by root cause at the default (if it still fails at the chosen default, explain why with numbers and
stop — do not edit its assertions). If NO value satisfies the hard constraints, change nothing and report BLOCKED with the table (an
engine fix will be needed). Use your own engine build via PYTHONPATH (worker-contract.md) for every run.
Part 3 — measurement only, no default change: `[layers] pour_cut_cost_mm` ∈ {0.0 (default), 0.25, 0.5, 1.0} on dcsgone, eprom, wiccon at
the chosen growth (A-C15 asks for a value sweep before revisiting). Same columns.
Commit `feat(ripup): growth default <g> by measurement (Q1)` (or none if BLOCKED). Put both tables in task-GR-report.md.
