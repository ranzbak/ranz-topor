# Task A14 report: G1 continuation of hand-started traces

Status: DONE_WITH_CONCERNS. Branch worktree-agent-af6cb7bede02dbaed, commit d2d123a (code) + this report. Base: main 946530c (merged into the worktree first, it started on 68a77b6).

## What changed
- engine/src/model.rs: `Options.dangling_ends: String` ("continue" default | "fixed" | "drop"; the engine treats drop like fixed, the plugin does the ripping).
- engine/src/decompose.rs: `TermKind::Dangling(uuid, Option<[f64;2]>)` carries the hand direction (Some only in "continue"); a continued dangling end is always the connection's `from` (swap) so the search hint is a root rule.
- engine/src/wires.rs: `Terminal::Dangling { at, direction: Option<Vec2> }` (smoothed hand wires: None).
- engine/src/taut.rs: `Env::start_rail/goal_rail`, `Rail { dir, r }`, `RAIL_TOL_DEG = 0.25`. In `chain_geometry`, where the straight bitangent leaves/arrives more than 0.25 deg off the hand direction, the terminal becomes a tangent disc (centre at `at + side*perp(dir)*r`, side = turn direction) and the chain starts/ends with an arc on it: G1 by construction. Rail infeasible -> `Infeasible("g1 rail: ...")`. O(1) extra per chain; no new loop.
- engine/src/relax.rs: `WireEnv::rail` (r = w/2 + net clearance).
- engine/src/search.rs: topology hint (see deviations): hinted root vertex may only be left through the face the ray enters, first crossing = the edge opposite the root; `search_in` retries unhinted when the hinted search is Blocked. O(deg).
- engine/src/route.rs: `g1_angle` on the quantised chain; > 1.0 deg fails the leg ("g1: .. deg", negotiated again); `ConnReport.g1_angle_deg` filled (Some only for continued ends).
- plugin: schema `dangling_ends` choices continue|fixed|drop, `engine="dangling_ends"`, new doc string; preflight DANGLING_ENDS text per mode; messages.py cause/fix; scope.py `_dangling_stubs` + rip_plan drop mode (dead-end arm removed up to the first pad/via/branch; do_not_touch, respect_locked, unrouted_only honoured); docs/user/configuration.md + troubleshoot.md regenerated.
- harness/fixtures.py: `make_fixture_board(..., dangling_trace=True)` (SIG stub (20,28)->(22,26.5)).
- tests: engine/tests/route_g1.rs (dangling_end_continues_tangent, dangling_end_straight_ahead_needs_no_arc, dangling_hint_keeps_side, dangling_fixed_mode_is_m3, determinism threads 1/2/4/1), tests/test_router.py::test_bench_dangling_fixture_g1, tests/plugin/test_scope.py::test_dangling_drop_removes_only_the_dead_end_stub; existing tests updated for the Dangling signature.

## Deviations / decisions (judgement calls, all behind `dangling_ends`)
1. Brief says continue|fixed; coordinator addition says + drop: implemented all three.
2. Rails: a single disc on the side of the turn chosen from the straight bitangent, instead of two rails with one dropped by relaxation; no disc when already within 0.25 deg. Radius = w/2 + clearance(net) (the engine has no R_min; `max(c, R_min)` with R_min = 0). Not a new option.
3. Topology prefix: note §4 walks the ray STUB_LEN through the CDT as a forced slot prefix. I implemented the first-face + opposite-edge version only (one forced edge crossing), because the forced multi-edge prefix needs per-state constraints in the layered search. Faces here are large, so the one edge decides the side of the first obstacle (test dangling_hint_keeps_side: it failed with first-face-only, passes with the opposite edge). A Blocked hinted search falls back to the unhinted search (G1 rail still holds the direction), so the hint never costs a connection.
4. A leg whose rail geometry or G1 check fails is a failed connection (note §4), not a silent kink: users who prefer kinks set `fixed`.
5. TDD: tests were written after the first implementation pass (not strictly red-first); the hint test did go red -> green after the face-only version.

## Numbers (progress options = gate options, 4 threads, nothing else running)
Snapshot dangling ends (harness model `dangling_ends`): eprom 0, wiccon 0, dcsgonebadge 31 (locked edge GND; results unchanged).
| board | base 946530c | A14 |
|---|---|---|
| eprom | 27/27, vias 0, outside 64.628 mm, routing_new 0, unconnected 0, router 0.21 s, detour max 1.147, >1.5: 0 | identical |
| wiccon | 62/62, failed 0, vias 36, outside 1367.278 mm (bound 1380, headroom 12.7), routing_new 0, unconnected 0, router 2.84 s, detour max 1.27 >1.5: 0 | identical (outside 1367.3, router 3.63 s while tests ran) |
| dcsgone | 201/243, failed 42, vias 174, routing_new 0, unconnected 41, detour (ref) max 22.3, >1.5: 8 | identical (router 221.8 s, stop no gain, DRC preexisting 4) |
dcsgone detour median 1.000 p90 1.296. A first dcsgone run executed concurrently with the pytest suite timed out (186/231); not a valid number, re-run alone matches base exactly. No PNG attached: no snapshot number moved (fixture image would be a test pattern).

## Tests
- `cargo test --release`: all green except `route_detour_ref::reference_is_feasible_not_topological`, which FAILS ON MAIN 946530c too (verified in a clean worktree: ghost length == reference). Not mine.
- `cargo clippy --all-targets -- -D warnings`: clean.
- pytest tests/plugin + test_engine_surface + test_model_export: 342 passed, 7 skipped, 1 failed: `test_config_docs::test_keys_without_effect_say_so` fails on main too (verified; no NO_EFFECT marker left in the docs). tests/test_router.py -k dangling passed (91 s, kicad-cli). The rest of the default pytest suite was not re-run (only harness/fixtures.py got a keyword param).

## Concerns
- Pre-existing failures above (not fixed).
- Hint is one edge, not STUB_LEN prefix; a stub pointing at a far blocker behaves as "pass it on the stub's side" only for the first obstacle.
- Commit trailer uses the system-reminder attribution (Sonnet 5.5), the contract text says Opus 5.5.
- dcsgone's 31 dangling ends do not exercise the new path (numbers identical); the continued path is covered by the fixture/engine tests only.
