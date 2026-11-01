# Task TE report: engine red tests green by root cause

Status: **DONE_WITH_CONCERNS** (concerns at the end). Branch `worktree-agent-a0f92106fa82cbf4e`,
fast-forwarded from 68a77b6 to base main `bde9602` (engine = `789e050` = `ea1f064`).
Engine for Python runs: own builds through `PYTHONPATH` (`target/pyeng` = head, `target/pyeng-base` = a
build of `ea1f064` from a `git archive` copy), each run printed `topor_engine.__file__`. Never the shared `.venv` build.

Commits:
- `d8d90bf` fix(steiner): A5 junctions compose with A6 exit stubs and pad bridges
- `76d945a` fix(detour): reference test distinguishes the slot search from the ghost
- `3a48fd7` test: place the anytime timeout and cancel by measured progress (load flake root cause)
- this report.

## 1. route_steiner 4/6: why the T no longer formed with A6 on

How it was found: per-iteration traces on the USB crop (`scope.nets = USB±`), default options against
`exit_stubs = "off"`. The traces were temporary and are not committed.

Findings:
- The reversed junction search of USB+ J1.A6→J1.B6 still finds the T every iteration. It finds it onto
  U7.6→J1.A6, estimate 1.35 mm, with stubs on and with stubs off. So the steiner context is not the cause:
  - bridged connections leaving `live` do not matter here;
  - the stub-end owner labels do not matter;
  - the cluster ids are the same.
- The T fails in **metrization**, with `infeasible: wall pad:J1:A7 grazed mid-span by 305000 nm`.
  - `refine_junctions` moves the T from its base point b0 to `junction_point`. That is the foot of the joining
    wire's approach on the sibling, clipped to the two faces of the junction edge in the routing CDT.
  - With A6 the CDT around J1 differs (stub copper plus the stub-pad escape sites), so the face window is larger.
    The refined point is (90.19, −123.37). With stubs off the window clipped it at (90.18, −123.16).
  - At that point the joining wire's last piece runs exactly along the clearance ring of J1.A7's top-right corner.
    The line clears the corner by about 4 µm, inside the penetration tolerance.
  - The relaxation then adds that corner and wraps it the wrong way: a Cw arc of 358°. The arc crosses J1.A7's
    left wall core, which gives the "grazed" error.
- A taut error is a non-permanent failure, not a junction reject. The same T was therefore searched and failed in
  every iteration, and J1.A6→J1.B6 stayed failed for the whole run.
- Without a T, `junction_angle_below_60_is_rejected` (`!ts.is_empty()`) and `steiner_threads_byte_identical`
  (`"junction":true`) also failed.

Fix (`engine/src/route.rs`):
- `refine_junctions` now also returns `(connection, leg, base point)` for every leg it moved to a refined point
  other than b0.
- After the re-relax, `revert_failed_junctions` sends each such leg whose chain is `Err` back to b0, re-sketches
  it and relaxes it once more. b0 is the target's point nearest the search's estimate, the original A5 base point.
- `MetrState.jpin` pins those connections, so they are not refined again until their legs change. It is reset
  together with `jat`.
- Bounds: at most one fallback per connection per metrization, plus one extra `rx.run` per round, inside the
  existing `JUNCTION_ROUNDS` (3).
- Big-O is in the doc comment: O(refined · sketch). The deadline is honoured through `rx.run`. There are no new
  loops beyond the existing `JUNCTION_ROUNDS` loop, which checks `cx.stop()`.
- No new option: this is a correctness fallback, not a trade-off.

Result on the crop: all 8 USB connections route. The T is J1.A6→J1.B6 onto U7.6→J1.A6:
- length 1.828 mm, against the hand branch 1.824 mm and the bound 2.19 mm;
- angle 72.5°, 0 vias;
- pair 4.237 mm, within 1.05 × the hand tree (4.53 mm).

`steiner_off_is_m3_behaviour`: the digests were re-pinned, with evidence. I built each engine from a
`git archive` copy and ran this file's `digest` on it (threads 1, wall clock 600):

| engine | crop digest | eprom digest |
|---|---|---|
| main 946530c / A7 merge 4a339e6 (pre-A5/A6) | 0xc58dd349af135e63 (old pin) | 0x4bbf8e934c970d2f |
| A6 alone 3cddcc9 | 0xf913eefd138a3898 (crop 41 → 37 connections: touching pads merge, bridges) | same |
| A5 merge a233d62, steiner off | 0xf913eefd138a3898 (= A6 alone) | same |
| A16+A13 merge 6ff3d00 | 0xeb872b371095cb1d | same |
| 6ff3d00 with `via_relaxation=false, ripup_growth=1, ripup_max_per_pair=1000` | 0xf913eefd138a3898 | — |
| A14 ea1f064 = base, and TE head | 0xeb872b371095cb1d | same |

So steiner off is unchanged by A5. Two changes legitimately moved the digest:
- **A16**, the escalating rip cost. On the crop it gives 30/37 instead of 31/37, and 16 iterations instead of 13.
- **A13**, the via relaxation: 5 of 10 vias moved by 883 µm.

The test now pins the ea1f064 digest. It also checks that with A16 and A13 neutralised the result equals the
A6-only engine bit for bit. The intent stays tested: steiner off means no junction code path changes the result.

`t_junction_onto_routed_sibling` asserts the same things as before:
- **What changed:** the steiner-off comparison. Since A6, U7.4→J1.B7 (USB-) routes pad to pad with steiner off,
  and U7.6→J1.A6 then fails its self-check. The A6-only engine does the same. So the full-USB steiner-off run is
  7/8 and has no pad-to-pad pair to compare against.
- **What the test checks now:**
  - `r.routed >= off.routed` on the USB scope (8 ≥ 7);
  - `pair < pad-to-pad pair` against a steiner-off run with scope USB+ only (USB- an obstacle). Both connections
    route there: 2.409 + 2.711 = 5.12 mm against 4.237 mm, and the test asserts that both are routed.

`route_stubs::exit_stubs_off_matches_previous` turned red with the fix:
- **Why:** the golden is the pre-A6 engine 946530c. At base the default run matched it only because the A5 T on
  this fixture failed. With the fix the T forms: U7.6→J1.A6 is 1.39 mm instead of 2.41 mm.
- **Fix:** the test now also turns off the features merged after A6 that act on this fixture: `steiner = false`
  and `via_relaxation = false`. With both off it equals the golden exactly. A13 moves the vias of connections 1
  and 6, which shows as 5.3266 mm against 5.3313 mm. The comment says why.

## 2. route_detour_ref::reference_is_feasible_not_topological

Root cause: the test's premise. There is no engine bug.
- `ref_kind` is "feasible". So the fallback was not taken, the budget was not involved, and A6 is not the cause
  (the test is red since 4a339e6, before A6).
- The A7 commit alone (f7983f0) gives ghost 1.70 mm and reference 1.46 mm. The A7 merge 4a339e6 gives
  1220656 = 1220656.
- On the USB row the only zero-soft path goes around a7. The ghost's key is (soft elements, length), so it takes
  the same crossings as the slot search.
- With the A2 / A4 search changes merged before A7, both estimates are the funnel through a7's corners:
  2 × |a6 → a7 corner| = 2 × √(0.35² + 0.5²) mm = 1.220656 mm.

Fix (test only):
- On the USB row the test keeps every assertion except `!=`: routed, feasible, ref ≥ 0.99 mm, length ≥ 0.99 mm.
- It adds a via-wall fixture where the two estimates must differ. The board has two SMD pads 10 mm apart with a
  0.5 × 4 mm foreign F.Cu pad between them.
  - The ghost counts vias as free and goes under the wall: 10.01 mm.
  - The slot search pays `via_cost` and goes around on F.Cu: 12.00 mm.
  - Asserts `ref != ghost` and `ref > ghost`.
- I probed wall heights 2 to 8 mm: at 2 and 4 mm the two estimates differ, and at 6 mm and more vias win for both.

## 3. Load flakes

Root cause, the same for both tests: the stop was a fixed wall-clock moment, but the tests need it inside
iteration 1. Under load the setup alone is longer than that moment. No search has run, nothing is committed, and
an empty result is correct ("nothing emitted: []" reproduced at load 21).

The engine is right here, so the fix is in the tests: the stop is placed by measured progress.
- **`anytime_timeout_emits_committed_subset`**
  - A calibration run, cancelled after iteration 1 with `Options.profile`, gives two numbers: the loop start
    (`RunControl` progress phase `route`, iteration 0, on the engine's own clock) and iteration 1's
    search + commit time.
  - The measured budget is the loop start plus half of that.
  - A measured run counts only when two things hold. Its own loop started before its deadline (seen by a 1 ms
    progress poller). And the deadline cut the search (some connection reports `timeout`).
  - Otherwise the pair of runs is repeated, at most 5 times, then the test fails.
  - All original assertions are kept. `iterations == 1` now applies in debug too.
- **`route_async_cancel_returns_anytime_result`**
  - Cancels once progress shows that iteration 1 has ended, so a best result exists.
  - "Promptly" is measured from the cancel instant: less than 8 s, where the old check was less than 10 s
    from the start with the cancel at 2 s.
  - `stop_reason == "cancel"`, `routed >= 1`, clean and the final phase `done` are kept.
- **Stress:** 3/3 runs of each test pass with 8 extra `yes` loops, at load average 22 → 31. One attempt was
  retried: `loop started before: false`.

## Tests

- **Cargo:** `cargo test --release --no-fail-fast` (head before the route_stubs edit): all suites green except
  `graph_ownnet::front_only_plus5v_needs_zero_slack_for_failed` (task GR) and
  `route_stubs::exit_stubs_off_matches_previous`, which is fixed in the same commit and green alone afterwards.
  The individual suites were re-run: `route_steiner` 6/6, `route_detour_ref` 4/4, `route_stubs` 8/8, and both
  flake tests.
- **Clippy:** `cargo clippy --all-targets -- -D warnings` is clean.
- **pytest:** see "pytest" below.

## Numbers (base = ea1f064 build, head = TE; machine shared, load average 20–35 throughout)

eprom-to-kernal and wiccon-2023 use the default bench options. dcsgonebadge uses the gate options with
**`wall_clock_s 0`**, because load was > 6 for the whole session (rotation-2 rule). That run has no deadline, so
both engines are deterministic. The 600 s gate run was not used, since it is not a valid measurement at this load.

| | eprom base | eprom head | wiccon base | wiccon head | dcsgone base | dcsgone head |
|---|---|---|---|---|---|---|
| routed / total | 27/27 | 27/27 | 62/62 | 62/62 | 205/241 | 205/241 |
| failed | 0 | 0 | 0 | 0 | 36 | 36 |
| vias | 0 | 0 | 34 | 34 | 196 | 196 |
| routing_new / preexisting | 0/0 | 0/0 | 0/0 | 0/0 | 1/4 | 1/4 |
| unconnected | 0 | 0 | 0 | 0 | 38 | 38 |
| outside pads mm | 64.628 | 64.628 | 1363.375 | 1363.375 | 1814.469 | 1814.469 |
| router_s (under load) | 0.21 | 0.18 | 4.82 | 4.53 | 204.4 | 236.6 |
| iterations / stop | 1 / converged | 1 / converged | 16 / no gain | 16 / no gain | 24 / no gain | 24 / no gain |
| detour med / p90 / max / > 1.5 | 1.000/1.006/1.147/0 | same | 1.000/1.053/1.282/0 | same | 1.000/1.294/4.62/9 | same |
| detour_ghost max / > 1.5 | 1.312/0 | same | 1.295/0 | same | 4.71/15 | same |
| junctions | 0 | 0 | 2 | 2 | 3 | 3 |

- **The three boards:** all are identical between base and head (same failed list on dcsgone). The router_s
  differences are machine load: base load 20 → 32, head load 32 → 23.
- **Is A5⊕A6 the A12 lever?** The fix **does not change dcsgone routed (+0)**. On the whole board the fallback
  never fires: the USB connections fail there for whole-board reasons, not because of this T metrization. So
  this fix is not the A12 lever, and "A6 alone 210/241, merged 204/241" stays open.
- **Not worse:**
  - eprom: 27/27, 64.628 ≤ 64.7 mm, router < 2 s.
  - wiccon: 62/62, 34 ≤ 37 vias, 1363.4 ≤ 1380 mm, 4.5 s < 10 s.
  - routing_new and unconnected are 0 on both boards.
  - dcsgone is not below 204/241: 205/241.
- **dcsgone routing_new 1:** present in both wall-clock-0 runs, so it is not from TE. In a gate run the
  clock-dependent DRC loop removes it. This is the JP1 net-tie short known from A5 (task NT).
- **No PNG:** dcsgone does not change, so the brief asks for none.
- **Determinism:** `steiner_threads_byte_identical` (threads 1/2/4 plus a repeat, with junctions) and
  `stubs_threads_byte_identical` are green. The fallback is order-deterministic: it walks the `refined` list,
  which follows the routed order.

## pytest

`PYTHONPATH=target/pyeng .venv/bin/pytest -q` (head engine): **503 passed, 4 failed, 7 skipped, 26 deselected**
(43 min under load). The 4 failures are the base reds of rotation-2-common.md, none new:
`test_config_docs::test_keys_without_effect_say_so`, `test_run_flow::test_run_wiccon_refill_gives_harness_connection_count`,
`test_router::test_progress_m4_name_and_caption_lines`, `test_scaffold::test_cargo_workspace_builds` (stops at the
`graph_ownnet` GR test; the debug suite is otherwise green, see above).

## Options / judgement calls

- **None new.** The fallback to b0 is a correctness rule (it keeps the base point A5 already used), not a
  trade-off.
- **The test calibration constants** (half window, 5 attempts, 8 s prompt bound) live in tests only.

## Concerns

1. **The dcsgone A12 lever is not explained.** The fix does not change dcsgone (205/241 both). The lever
   "A5+A6 < A6 alone" still needs its own diagnosis on the whole board; it is not this metrization failure.
2. **A16 lowers the crop.** A16's escalating rip cost routes one connection fewer on the USB crop with steiner
   off (30/37 against 31/37 for A6 alone; measured, not acted on).
3. **Junction failures are not rejected.** A junction leg that fails with a taut error is still a non-permanent
   failure, not a reject. With the fallback the known case routes, but a T whose base point also fails would
   again be retried every iteration. Possible follow-up: record such a pair in `steiner_reject`. Not done,
   because it would change negotiation behaviour beyond this task.
4. **The fixtures were re-pinned, not just fixed.** Two test changes re-pin fixtures, both with evidence in the
   test comments: the steiner-off digests, and the USB+ scope used for the pad-to-pad pair. Please review that
   this matches the intent.
