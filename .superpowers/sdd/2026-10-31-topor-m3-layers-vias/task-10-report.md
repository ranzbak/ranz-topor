# Task 10 report: via-reduction pass

Branch worktree-agent-ae64269b3a3afb8ae (merged worktree-agent-a141224bcf3a2052b, fast-forward).

## Changes
- model.rs: `Options.via_reduction: bool` (default true).
- route.rs: `REDUCTION_VIA_FACTOR = 4.0`, `REDUCTION_LEN_SLACK = 1.10`, `via_reduction_pass` (O(m_v*(V' log V' + metrize)) + one Snap copy per candidate). Runs after the loop (not on timeout), before zero_slack_pass. Candidates: vias>=1, ordered (-vias, id), pour-goal connections excluded (their via is the goal). Each trial on a clone of best.mg + best.metr; reject = drop clone + re-sync anchors from best.mg (P7). Accept iff search at via_cost*via_weight*4 (P8) finds fewer vias, metrize leaves every previously Ok connection Ok, c's outside length <= old*1.10, overuse_edges <= best's. Deadline checked before each candidate. Strings: "removed N of V vias in C candidates", "off", "skipped: timeout" (loop timed out), "skipped: timeout (removed ... )" (mid-pass).
- Deviation: overuse criterion is "not above best's count" rather than "== 0" (wiccon stalls with 14 overused edges, == 0 would reject everything).
- harness/bench.py: surfaces via_reduction in the route summary and prints it.
- tests/route_layers.rs: 3 tests (wall fixtures; cheap-via options: 400k/800k nm since the 4x pair must exceed the detour).

## Verification
Full engine suite green, clippy -D warnings clean, `pytest -q` 130 passed, gate eprom+wiccon: 2 passed.

## Numbers (before = Task 9)
- eprom: vias 0 -> 0, via_reduction "removed 0 of 0 vias in 0 candidates", routed 27/27, nets 25/25, routing_new 0, unconnected 0, outside 64.628 vs hand 76.598 mm, zero_slack_wires 0, router 0.05 s. Unchanged.
- wiccon: vias 28 -> 28, "removed 0 of 28 vias in 16 candidates", routed 50/60, nets 20/27, routing_new 0, unconnected 10, outside 1124.254 vs 1690.138 mm, zero_slack_wires 0, router 9.74 s (Task 9: 9.14).
  Diagnosis (temporary debug, removed): of 16 candidates, 6 found no fewer vias at 4x cost; 3 found fewer vias but 2 failed the 10 % length bound and 1... (c31, c47: self-check/overuse rejection); rest same via count. So no accepted reduction; limits are legitimate (length bound, self-check), not tuned.
PNGs: docs/benchmarks/progress/2026-10-31-m3-T10.png, 2026-10-31-m3-T10-wiccon.png (both opened: real routed boards).
