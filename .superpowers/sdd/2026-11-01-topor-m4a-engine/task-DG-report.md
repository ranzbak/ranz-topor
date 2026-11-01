# Task DG report — dcsgone completion, diagnosis round 2 (F)

Status: **DONE_WITH_CONCERNS** (diagnosis complete; the gate is not reachable without new capabilities, see below).
Branch `worktree-agent-ac0668f5fc5d35104`, fast-forwarded to main bde9602 (engine = 789e050 / ea1f064).
The data is in `docs/progress/notes/m4-dcsgone-diagnosis.md`, section "Round 2 (rotation 2, base 789e050)".

## What changed

| file | change |
|---|---|
| `docs/progress/notes/m4-dcsgone-diagnosis.md` | new section Round 2: R2.0 numbers, R2.1 accounting (229 → 241, per pass, the 36 failures with mechanism / alone / hand route / routed-in-N-runs, unconnected per net, per-iteration failure classes), R2.2 A5+A6 vs A6, R2.3 levers (variants, Blake estimate, parameters, findings, repair prototypes), R2.4 ranked fix list + gate statement |
| `docs/progress/notes/m4-dcsgone-dg-prototypes.patch` | the measured prototypes (NOT for merge): net-order variants `decompose::order_variant` (env `TOPOR_DG_VARIANT`), generalised / strict / depth-2 repair + null re-metrization test in `junction_repair_pass` (env `TOPOR_DG_REPAIR`); applies to bde9602 with `cd engine && patch -p2` |
| `engine/src/route.rs` | profile only: `IterProfile.fail_classes` / `blocked_classes`, `pub fn reason_class` (O(len)); filled only with `Options::profile` |
| `engine/examples/route_probe.rs` | prints the two histograms under each profile row |
| `engine/tests/route_engine.rs` | `reason_class_keeps_the_mechanism`; the profile test asserts the histograms sum to `metrize_failed` / `blocked` |

No Options / schema change (nothing user-facing). Behaviour unchanged: the dcsgone `route_probe` dump of the head
build equals the base build's byte for byte without the `profile` field.

## Method
Load 18–39 on 8 cores throughout (4–5 other workers): every dcsgone number is `wall_clock_s 0` (deterministic), times
inflated 2–3×. 1 harness `run_bench` (base), 25 engine-only `route_probe` runs on the harness's exported model:
base, head (×2), A6-only 3cddcc9 and A6+A5 a233d62 (built from `git archive` under `target/`), `steiner false`,
9 net-order variants, 4 option experiments, 7 repair-prototype runs. Hand routes from an unripped model export of
the snapshot. Scripts in `target/dg/s/` (worktree, untracked).

## Key numbers (dcsgone, wall_clock 0)
- Base = head: 205/241, failed 36 (29 / 2 / 5 / 0 per pass), vias 196, routing_new 1 (harness, DRC loop not run —
  see concern 1), unconnected 38, router 257.6 s / runtime 407 s loaded, detour ref med 1.000 p90 1.294 max 4.62
  > 1.5: 9, ghost max 4.71 > 1.5: 15, stop no gain 24 it. eprom 27/27 0 vias 64.628 mm ref max 1.15 0.33 s; wiccon
  62/62 34 vias 1363.375 mm ref max 1.28 13.4 s (loaded) — head = base.
- First pass is 229 (A-C20 merges), total 229 − 22 + 7 + 26 + 1 = 241.
- 34/36 failures route alone on the fixed copper; 33/36 route in ≥ 1 of 13 other runs; never: +5V U4.2→U4.4,
  U4.2→U4.19, GND C11.2→pour.
- A6 3cddcc9 210/241 (31) vs A6+A5 205/241 (36) = base; A16/A13/A14 neutral; `steiner false` 211/242 (31). Pass 1
  equal (30 vs 29 failed); the loss is in the pour completion passes (USB corner cut, pass-2 GND vias in front of
  U7.3/U7.8, pass-3 re-routes) + the post-pass veto (below).
- **Post-pass veto:** the best snapshot does not re-metrize (null test: 1 of 194 fails with steiner, 1–4 of 197 in
  the v5/v7 orders without it) → via reduction 0 of 194 (A6: −20), via relaxation 0, junction repair 0, zero-slack.
- Generalised repair prototype with no veto (steiner off): 9 of 30 pass-1 failures repaired → **216/241, failed 25**.
- Variants: best of 8 orders v5 219/248, failed 29 (spread 29–46 failed).
- Blake detour rip-up: ≤ 3 of 36. `exit_stubs all` 196/239, `pour_listed_via_goals` 206/242, no small vias 210/247.

## Ranked fixes (note R2.4)
1. FR reproducible re-metrization + generalised repair (+9 measured, revives via reduction −19 vias).
2. FF fine-pitch fan-out: pour pads bridged inside the footprint, no via in front of a neighbour's tip (≈ 5–9).
3. FG geometry trio: via drill hole clearance in graph/sites, taut deep-wall wrap, sub-margin grazes (≈ 8).
4. Variants (A-C10) after 1–3; 5. small: copper polygons as terminals, harness drcloop budget 0.
Gate needs FR + FF + FG, then variants; residual joint congestion → multi-connection repair.

## Tests
- `cargo test --release --test route_engine -- reason_class profile_absent`: 2 passed.
- `cargo test --release --no-fail-fast` (loaded): reds = the known base reds (graph_ownnet front_only,
  route_control async cancel (load), route_detour_ref, route_engine anytime_timeout (load), route_steiner 4) **plus
  `route_stubs::exit_stubs_off_matches_previous`, also red on the pristine base** (built from `git archive HEAD`).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `.venv/bin/pytest -q` (PYTHONPATH = own build, `test_scaffold::test_cargo_workspace_builds` deselected = the cargo
  reds above in debug): 502 passed, 7 skipped, 4 failed — the 3 known base reds (`test_config_docs::
  test_keys_without_effect_say_so`, `test_run_flow::test_run_wiccon_refill_gives_harness_connection_count`,
  `test_router::test_progress_m4_name_and_caption_lines`) + `test_engine_surface::
  test_route_async_cancel_returns_anytime_result` (Python twin of the known load-flaky route_control test; load 16–25).

## Concerns
1. `wall_clock_s 0` disables the harness DRC loop (`drcloop.out_of_budget` treats budget 0 as exhausted), so the
   rotation-2 rule "wall_clock_s 0 = the quiet gate run" holds for router numbers, not for `routing_new` /
   `unconnected` (base: routing_new 1 = the NT JP1 net-tie short the 600 s run's 2nd DRC round removes).
2. All timings loaded (2–3×); counts exact. No 600 s gate run was valid (load > 6 all session).
3. `exit_stubs_off_matches_previous` red on base, not in the coordinator's list (TE's fixture).
4. The non-idempotent metrization (FR) also invalidates every A/B that relies on via reduction / zero-slack /
   junction repair since A5 — numbers of rotation 1 tasks after A5 carry it.
5. No progress PNG (no outcome change).
