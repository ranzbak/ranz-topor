# Task DX report: dcsgonebadge completion diagnosis

Status: DONE (diagnosis note written; engine behaviour unchanged; profile-only instrumentation added)

## Files
- `docs/progress/notes/m4-dcsgone-diagnosis.md` (new, ~4 pages): accounting 231 vs 255, the 67 failed connections with
  mechanism, per-iteration profile incl. metrized-OK counts, detours, experiments, ranked fix list.
- `engine/src/route.rs`: `IterProfile` gains `ok`, `blocked`, `metrize_failed`, `vias`, `cost_nm`, `best` (filled after
  `tally`, only when `opts.profile`; `..Default::default()` at the push). No result field changes; `profile` stays `None`
  by default, so the result JSON of a default run is byte-identical (verified: `routed/vias/length` of the default
  route_probe run before and after the edit are identical, 188/255, 199 vias, 1926.938 mm).
- `engine/examples/route_probe.rs`: prints the new columns.
- No Options / schema change (nothing user-facing; the knobs tried in the experiments already exist).

## Method / commands (worktree at cfff05d, own engine build via PYTHONPATH=target/pyeng, scratch snapshot
`scratch/boards/dcsgonebadge/3e7e66f862a7`)
- Harness gate run: `run_bench("dcsgonebadge", rip="all", threads=4, options={"wall_clock_s": 600, "partial_routes": False})`
  (script in the session scratchpad) → 188/255, failed 67, vias 199, iterations 42, stop `no gain`, routing_new 0,
  preexisting 4, unconnected_items 62 (via_dangling 4), 22 locked GND items kept, drc_loop rounds 1, router_s 153.5,
  runtime_s 197.0. Model JSON exported by the harness to `target/dx/gate4/dcsgonebadge--model.json` (not committed).
- `route_probe --profile` with the gate options (twice: 155.7 s and 92.6 s wall under load, identical counts);
  `gate_diag --iters 30` (empty graph: 221 found / 13 blocked; replay: 209 routed, 22 blocked, 0 rips after iteration 2);
  `gate_diag --skip-a --detour 34,204,207,98,45`; hand model exported unripped (`harness.model.export_model` on a
  snapshot copy) and summarised per net (pads, hand track widths per layer, vias).
- Experiments (route_probe, threads 2): strict_unfit_edges 64/232; patience 15 + min_gain 0: 188/255 after 120
  iterations; via_cost 0.5 mm: 174/245; max_vias 6 + rip cap 3: 189/261. No existing knob moves completion by more
  than ±1 in the right direction.

## Key numbers
- 231 decomposition connections; 255 after three pour-completion passes (231 − 15 unfrozen + 26 − 12 retried + 22 + 3).
  The gate's `routed == connections_total` is well defined; `== 231` only when no completion pass runs. Recommend A12
  asserts `routed == connections_total`, `failed == partial == 0`, `unconnected_items == 0` and records the first-pass
  count (`connections_initial`).
- 67 failures: own-pad / fine-pitch capacity 23 (12 of them need a neck: 0.5 mm nets into 0.25–0.6 mm pads),
  metrization rejects of found paths 37 (taut pinch 11 + wall grazed 7 + NPTH hole clearance 6 + via-site pair 4 +
  sketch 4 + self-check item 3 + inside obstacle 1 + pass cap 1), pour completion 5, dynamic 2. Zero "no room": the hand
  routes all nets with 0.2–0.4 mm necks at U4/U5/U7/J1/J3.
- Profile (pass 1): negotiator holds 205–215 paths per iteration, metrized OK 170→175 (best, iteration 4) → 156
  (iteration 9); metrize-failed 34–53 per iteration; blocked 15–25; commit (sequential re-search of 132–150 invalidated
  paths, F4 revalidates 0–5) = 44 % of the run; search 33 %; metrize 5 %. Stop rule `no gain` is correct (E2: 120
  iterations change nothing). Wall clock and iteration cap never bind.
- Detours (ghost ref.): median 1.003, p90 1.52, max 11.6 (VCC U3.1→U3.3, 4 vias), 20 of 188 > 1.5; 7 of the top 10 are
  own-pad capacity consequences (escape vias), 3 are completion-pass artefacts.

## Ranked fixes (recovered connections, estimate)
1. A4 own-net / same-net capacity, wall-aware: ≈ 25–35 (11 direct (b) at 0.2 mm + the 15 taut/sketch pinches on own
   edges + fewer escape vias / metrize rejects). First.
2. NX "neck to the pad" (new, fits A6's stub machinery; options `neck_to_pad`, `neck_max_mm`): ≈ 12 (all 0.5 mm nets at
   U4/U5/J1/U3 are unroutable without it: 0.5 + 2 × 0.2 > 0.5 pitch).
3. HC NPTH hole clearance in relax/taut (NPTH pad anchored as net-0 pad at 0.2 mm, Checker wants 0.35): 6 (+1 zero-slack).
4. A2 fine-pitch entry + los convergence: 6 direct (U7 nets USB-/CC2, #197) + the 5.2× / 3.6× detours.
5. A5 same-net Steiner / T-junction + net ordering: ≈ 8–10 (siblings holding the one exit). Then A11 pour-cut cost
   (removes the 26/22/3 completion connections and 15 re-routes; ≈ 5 + 2), taut slack band for sub-µm grazes (2–3).
   Not levers: A16 rip cost (rips 1–9/iteration, no ping-pong), A13/A10 vias (0 "no via site" failures, 679 sites),
   stop-rule params (E2), via_cost (E3 worse).

## Tests
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo test --release`: 36 test binaries, 381 passed, 0 failed (loaded machine; the load-sensitive
  `anytime_timeout_emits_committed_subset` passed).
- `.venv/bin/pytest -q` (PYTHONPATH=target/pyeng): 469 passed, 7 skipped, 26 deselected, **2 failed, both pre-existing
  on main and unrelated to this task**: (1) `tests/test_docs_links.py::test_operate_md_limitations_section` — A9 removed
  the `net_priority` "no effect in 0.4.0" wording from `docs/user/operate.md` Limitations, the test still expects the word
  (docs/test to reconcile in the A9 follow-up); (2) `tests/test_scaffold.py::test_cargo_workspace_builds` runs
  `cargo test -p topor-engine` in the **debug** profile, where the geometry proptest
  `dist_props::prop_arc_arc_matches_sampling` found a new counter-example (`d=1524.80` vs sampled `1531.46` nm, two
  arcs; same oracle class as 2894bed "arc-arc sampling oracle basin trap"); the release run of the same test binary
  passed (random cases differ). proptest appended the case to `engine/tests/dist_props.proptest-regressions`; I reverted
  that file so the failing case is not pinned by this commit (the case is quoted below for the geometry owner).

## Concerns
- The machine was shared (load 7–13): wall times in the note are inflated 1.5–2×; all counts are exact.
- Mechanism letters for the 37 metrization rejects are inferred from the reason strings and the escape dumps, not from
  a per-connection replay of taut; the A4 worker should re-run the classification after the capacity change.
- The detour reference is still the ghost (A7 not on main); the "true" detours quoted use the hand route as reference.
- Pre-existing geometry proptest counter-example (debug profile, `dist_props::prop_arc_arc_matches_sampling`, not
  pinned): `a = Arc { c: (-800549.6885420157, -807637.6065073739), r: 1966315.95029234, Cw, a_start: 2.5195661285126523,
  sweep: 3.2089898132921144 }`, `b = Arc { c: (-800000.0, -800000.0), r: 1972284.7145767987, Ccw, a_start:
  3.383681098996437, sweep: 5.311841872798016 }` → `d = 1524.798` vs sampled `1531.462` nm (proptest seed line
  `cc 8a48703a…`). Two nearly concentric arcs 0.55/7.6 µm apart in centre, radii differing by 6 µm: the sampled
  oracle sits 6.7 nm above the analytic value — either an oracle basin trap again or a near-concentric branch in
  `arc_arc`; for the geometry owner, outside DX scope.
