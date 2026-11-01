### Task DG (opus, F, rotation 2): dcsgone completion — second diagnosis round (A12 Step 2)

Read rotation-2-common.md first, then docs/progress/notes/m4-dcsgone-diagnosis.md (round 1 = task DX on cfff05d: 67 failed classified
b/i/g/e) and task-A6-report.md / task-A5-report.md (concerns). Base 789e050: 204/241, failed 37, unconnected 39 (gate options, quiet).
Output: a new section "Round 2 (rotation 2, base 789e050)" appended to docs/progress/notes/m4-dcsgone-diagnosis.md — no behaviour
change merged (profile-only / diagnostic code allowed, e.g. extra IterProfile columns or gate_diag flags, behind no-behaviour-change tests;
prototypes to measure a lever stay in your worktree unless they are profile-only).
1. Accounting: 231 first pass → 241 total; per connection of the 37 failed (+ the 39 unconnected items): net, from → to, width, pads,
   engine reason, mechanism class (DX letters), whether it routes on the empty graph, whether the hand board routes it and how (necks,
   vias). Per-iteration profile (routed / ok / blocked / metrize_failed by reason).
2. Lever 1: why A5+A6 together route less than A6 alone (A6-only main 3cddcc9: 210/241, failed 31; merged: 204/241). Build 3cddcc9's
   engine in your worktree (git worktree add or `git -C` checkout of a second dir under your worktree's target/) and diff the outcomes:
   which connections A6-only routes and the merge does not, and the mechanism (net ordering of A5 `net_rank`, junction cost cap A-C21,
   junction consuming a pad exit, bridges vs Ts, A16/A13/A14 trajectory changes). Task TE (parallel) fixes the U7 crop fixture tests
   (route_steiner) — share nothing with it but say if your root cause is the same.
3. Levers by measurement (each: expected/measured gain on the 37, cost in runtime, risk to eprom/wiccon): (a) variants per A-C10 —
   prototype: K deterministic net-order permutations / seeds (e.g. rotate the net order, reverse within tiers, priority-by-difficulty
   from the first pass), how many of the 37 route in at least one variant, best single variant; (b) a Blake-style detour-optimisation
   rip-up phase (task DO will implement; estimate only whether it also frees failed connections); (c) A13 / zero-slack / neck / stub
   parameters that the failure classes point to; (d) anything else the data shows (e.g. pour completion, via sites, metrization rejects).
   Do not run the ripup growth or pour_cut_cost sweeps (task GR does).
4. A ranked fix list: each fix = mechanism, connections it should recover (ids), files, est. effort, risk, how to test — sized so the
   coordinator can dispatch 1–3 fix tasks. State plainly if the data says the gate (241/241 + unconnected 0) is unreachable without a
   specific capability, and which.
Commit `docs(notes): dcsgone diagnosis round 2` (+ any profile-only code commit). Report task-DG-report.md (short; the note holds the data).
