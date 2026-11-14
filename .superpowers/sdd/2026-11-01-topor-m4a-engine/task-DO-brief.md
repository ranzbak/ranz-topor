### Task DO (opus, rotation 2): hidden-detour check (user observation) + detour-optimisation-by-rip-up phase (Blake toporouter)

Read rotation-2-common.md (incl. "Round 2 update"), the private-badge-b completion diagnosis (note removed: private board) R2.3 (b), ledger rulings A-C18/A-C19,
M3-R15 in global-constraints.md. User rule: no per-connection detours where a direct path exists. M4a gate item: "detours: no connection
above 1.5 where a direct feasible path exists (count + max per board)".
Part 1 — hidden detours. The user saw in the private-badge-b image (docs/benchmarks/progress/2026-11-01-m4-G3-merged-private-badge-b.png and later
ones; render a fresh one from your base) (i) tracks zigzagging between the pads of the TOP connector row, and (ii) a long B.Cu T-junction
across the open display area. Identify both (net, connection ids, geometry) and check whether either is a detour the metric misses:
e.g. a Steiner junction connection's detour measured against a reference to the junction point instead of per branch / per tree
(a T far from both pads can make each leg look direct), zigzags whose length ratio is < 1.5 but whose path wiggles between pads
(compare with the reference path and the hand route), via-attached legs. Fix the metric if it misses them (per-branch / per-tree
detour for junction connections: tree length vs the reference Steiner/MST of the same terminals; report a zigzag/excess-bend count if
useful) in the engine detour report and the harness/bench/progress columns, keeping the existing columns comparable (new columns rather
than redefining old ones, unless the old one is wrong — then say so). If they are real detours, fix the routing (or show why there is no
direct feasible path), else explain in your report (the coordinator copies it into the ledger).
Part 2 — Blake's toporouter detour-optimisation phase (gEDA PCB src/toporouter.c, GPL-2.0-or-later, Anthony Blake — porting allowed with
copyright + attribution at the port site and origin in the commit message): after negotiation (and repairs), for every connection whose
detour (DRC-feasible reference, A7; per-branch for junction ones) exceeds `[search] detour_limit` (default 1.5), rip its blockers (the
routed connections occupying its direct corridor — e.g. those crossed by the reference path), re-route it first, then the blockers;
accept the change only if routed count is not lower (completion never lost), no new failure, and the total detour excess falls; capped
rounds (`[search] detour_rounds`, default by measurement), deadline + cancel checked; Big-O stated. Options + schema + docs.
Gate classification: per board, list every connection still > 1.5 after the phase with the evidence that no direct feasible path exists
without losing completion (the phase tried and why it failed) — this is the gate's "where a direct feasible path exists" count; report
count + max per board for eprom, wiccon, private-badge-b, nametag, wedding.
Tests: detour phase on a fixture with a blocker that can move (detour removed, completion kept), with an immovable blocker (kept, reason),
threads byte-identical, phase off = today. PNG `--task DO --board private-badge-b`. Commits `fix(detour): ...` (metric) and
`feat(route): detour optimisation by rip-up (after Blake's toporouter)`. Report task-DO-report.md.
