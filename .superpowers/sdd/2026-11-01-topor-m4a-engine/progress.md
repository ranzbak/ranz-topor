# SDD ledger — plan docs/superpowers/plans/2026-11-01-topor-m4-in-kicad.md rev 2, Part 2 (M4a engine 0.5.0)
Coordinator rotation 1 (2026-10-31 → ), on main from 6ea3040 (M4b 0.4.0 gate verified by main). Execution style = M4b ledger
(.superpowers/sdd/2026-11-01-topor-m4b-plugin/progress.md, Ruling C0): worktree workers in parallel, acceptance commands +
coordinator verification after merge, F review at the gate; worker-contract.md + global-constraints.md in this directory.
Briefs: task-<ID>-brief.md (plan text extracted + coordinator additions). Reports: task-<ID>-report.md (committed by the worker).

## Preflight scan
| pair / task | produces vs consumes | finding |
|---|---|---|
| A0 ‖ A1 | A0 multigraph/pourcheck/search (D-1 strict_unfit push_state) + harness; A1 search.rs clearance_bend / graph.rs clear_r | both edit search.rs (different fns) → merge-time conflicts only |
| A0 ‖ A9 | both add Options fields (model.rs), A9 decompose/route; A0 route.rs D-6 goals | trivial merge conflicts in model.rs |
| A9 ↔ plugin | plugin computes net_priority / exclude_nets / ignore_pads (B7) and reports "No effect in 0.4.0" (C34) | A9 must wire plugin → engine + remove the no-effect doc text + regenerate configuration.md → A-C4 |
| A10 ↔ plugin | preflight.smaller_profile_vias duplicates vias.rs rule (FX2 concern) | A10 keeps them in sync (shared test) |
| A16 ↔ P1 | P1 made RIP_PAIR_CAP read ripup_max_per_pair (default 8, M3 constant 3; damping test pins 3) | A16 replaces the cap by growth cost; wiccon/eprom not worse |
| A7 ↔ plugin | `[run] detour_reference = true` live key, engine field absent in 0.4.0 | A7 adds the engine field; plugin default stays as schema says, budget share bounded → A-C5 |
| all PNG tasks | plan says PNGs committed | superseded: docs/benchmarks untracked → A-C1 |
| private-badge-b PNGs | harness.progress uses wall_clock 60 for private-badge-b (M3 info) | gate options are 600 s → A-C3 |
| .venv | maturin develop by one owner; parallel Rust workers | PYTHONPATH engine override per worktree → A-C2 |

## Rulings
- Ruling A-C1: docs/benchmarks/ untracked (main ab42758 + user): progress PNGs / benchmark dirs written on disk only, never committed;
  workers produce them in their worktree, the coordinator copies them to /home/paul/work/kicad/plugins/topor/docs/benchmarks/progress/;
  tracked docs quote numbers inline — cost if wrong: none (user ruling).
- Ruling A-C2: the shared .venv belongs to the coordinator (maturin develop from main after each engine merge); workers use their own
  engine build via `cargo build --release --features python` + copy of libtopor_engine.so to <wt>/target/pyeng/topor_engine/ +
  PYTHONPATH (verified: import resolves to the override) — lets several Rust workers run Python/harness in parallel — cost if wrong:
  a worker forgets the override and measures the 0.4.0 engine (reports must print topor_engine.__file__).
- Ruling A-C3: M4a progress PNGs / measurements for private-badge-b use the GATE options (wall_clock_s 600, partial_routes False,
  threads 4); the plan's "private-badge-b 60 s ≥ 148" check (A1) is reported in addition — cost if wrong: ~10 min per private-badge-b PNG.
- Ruling A-C4: carry-over mapping as global-constraints.md "M4 carry-over" 1–10 (A9 includes plugin wiring of priorities /
  exclude_nets / ignore_pads + [nets] pour_served; A16 includes the relax_iter_cap dedupe; A10 the via [vias] keys + per-netclass
  overrides + preflight sync; AP = plugin minors 10/11) — cost if wrong: re-split of tasks.
- Ruling A-C6 (A0 concern 1): D-6 listed same-net pour vias as goals stays behind `Options.pour_listed_via_goals` / `[search]
  pour_listed_via_goals` default false (on: wiccon vias 37→36 but outside 1381.4 mm > 1380 bound, GND J2.2 13.9 vs 6.0 mm) — cost if
  wrong: listed pour vias unused as goals (M3 behaviour), revisit in A5 (same-net attachment).
- Ruling A-C7: group 2 = DX (F private-badge-b diagnosis now, plan A12 Step 2 moved forward because A1 made private-badge-b stop-rule-limited, not
  time-limited) ‖ A3 (F note) ‖ A2 ‖ A7 ‖ A10+A11 (one S worker, two commits) — cost if wrong: DX re-run after A4–A6.
- Ruling A-C8 (main, user: Fable usage 73 %): no fable workers/reviewers from now on; engine-algorithm tasks (A4, A2, A5, A6+NX,
  A12 fixes) and the F gate review → default model (Opus); routine (docs, tests, mechanical, plugin) → sonnet; ask main before any
  fable use — cost if wrong: weaker design depth on hard tasks (mitigated by DX/A3 notes).
- Ruling A-C9 (TopoR "necking", main request; = DX fix NX): trace necking to the pad is implemented together with A6 (exit stubs) as
  one task A6+NX: first/last piece of a connection at min(w, pad_width − 2ε) ≥ KiCad min_track_width (incl. netclass/rules) for
  ≤ neck_max_mm; keys `[search] neck_to_pad` (default true only if DRC-clean on all snapshot boards, else false) and
  `[search] neck_max_mm` (default 2 × pad length per DX, measured) — cost if wrong: necked pieces narrower than the netclass width
  (DRC-checked; KiCad connection_width rules respected).
- Ruling A-C10 (TopoR "multiple variants", main request): K deterministic variants (net-order permutations / seeds, one per thread,
  keep best by (routed, routing_new, vias, length), stable tie-break) as `[search] variants` — decided after A4/A2/HC/A5/A6 numbers;
  default 1 unless measured as a gain inside the 600 s budget — cost if wrong: an unused completion lever.
- Ruling A-C11 (DX §1): private-badge-b gate predicate = routed == connections_total (incl. pour-completion connections; 255 today),
  failed == partial == 0, unconnected_items == 0, routing_new == 0, runtime ≤ 600 s; `RouteResult.connections_initial` (first-pass
  decomposition = 231) printed and all 231 first-pass connections routed — cost if wrong: none (stricter than "231/231").
- Ruling A-C12 (A3 concern 1): `[search] dangling_ends` ∈ {continue (default, G1 continuation), fixed (M3: hand stub kept as fixed
  copper, route from pads), drop (rip the dangling hand stub in scope)}; engine `Options.dangling_ends` takes the same strings (A14) —
  cost if wrong: one choice's semantics.
- Ruling A-C13 (TopoR "route all, repair later", "teardrop smoothing", BGA strategy, global via minimisation, re-optimise after the
  engineer moves parts, lines-only mode): backlog rows only (task HC part 3); auto placement is a NON-GOAL (user: "components are
  placed by the enigneer, this is not a task of the router.") — cost if wrong: none.
- Ruling A-C14 (A4): new options all default true, all false = M3 exactly: `[expert] own_net_capacity`, `[expert]
  wall_aware_capacity`, `[expert] transit_pinch` (push_state refuses a transit through a pinch no graph edge sees — the F6 guard
  case: via 0.25 mm from the board edge), `[search] zero_slack_failed` (zero-slack retry also for failed connections); own-end radius
  credit min(r, w/2) and wall term in copper terms (note §1 deviations, numbers in task-A4-report.md) — cost if wrong: one default.
- Ruling A-C15 (A11): `[layers] pour_cut_cost_mm` default 0.0 (engine + schema), not the plan's 1.0: 1.0 is worse on eprom (outside
  65.874 > 64.7 mm gate) and wiccon (58/61, 83 vias, 35.6 s); on private-badge-b 1.0 keeps GND items in the pour (lost 17 → 2) but routes less
  (164 vs 171/231 under load). Revisit only with a value sweep — cost if wrong: extra pour-completion passes on private-badge-b.
- Ruling A-C16 (A2): los convergence rule applied only to lines leaving a terminal (everywhere breaks the M2 front-only guard,
  92.35 > 67.8 mm); LOS_STALL_CAP 2 internal; roundrect corner-tangent discretisation not done (not needed after state_point shrink) —
  cost if wrong: residual los runaway away from terminals (detour stats watch it).
- Ruling A-C17 (A10): via sizes = netclass via, then the union of KiCad pre-defined vias and the fab-profile vias (eprom/wiccon/private-badge-b
  unchanged); fallback 0.8/0.4 warning only when it changes a size; engine/plugin rule agreement test on the six snapshots — cost if
  wrong: an extra smaller via size offered where the user listed none.
- Ruling A-C18 (main, user): topor is GPL-2.0-or-later (93f2126); porting from the gEDA PCB toporouter (GPL-2.0-or-later) is allowed
  with the original copyright + attribution at the port site, origin in the commit message, one A-C ruling per port; no new
  Apache-2.0-only runtime crates — cost: none.
- Ruling A-C19 (main): detour count > 1.5 and max (reference + ghost) are non-regression acceptance items in every task, next to
  completion — cost: none.
- Ruling A-C20 (A6): `[search] neck_to_pad` default true (routing_new 0 on all six snapshots), `neck_max_mm` 0 = 2 × pad long
  side, NEW `[search] pad_bridge_max_mm` 2.0 (two pads of one footprint ≤ 2 mm apart joined by one straight track, possibly off
  the centre line — U7.4→U7.7, U7.2→U7.9); overlapping same-net pads merged (total 243 → 241) — cost if wrong: one default each.
- Ruling A-C21 (A5): `steiner_ghost_factor` 1.0 (junction cost capped at the connection's ghost cost; uncapped Ts made wiccon 1385.9 >
  1380 mm); junction angle limit 60°; Weiszfeld junction relaxation not done; `pour_listed_via_goals` stays false (A-C6; wiccon with it
  33 vias / 1368.5 mm — candidate to flip after a private-badge-b measurement) — cost if wrong: fewer T-junctions.
- Ruling A-C22 (A16) OPEN → main Q1: with `[ripup] growth 2.0` (user default) the F.Cu-only M2 guard
  `eprom_ripped_front_only_is_the_m2_result` gives 27/27 clean but 92.29 mm (> 67.8 bound, > hand 76.6); growth 1.0/1.5 = M3, 1.75/1.9/3.0
  = 26/27. Worker runs the guard with growth 1.0 / max_per_pair 3 and adds a default-options pin (27/27, clean). Merged as is; needs
  main's ruling (accept, or default growth change) — cost if wrong: a single-layer board class at +25 mm.
- Ruling A-C23 (A14): topology hint = one forced root face (not the note's prefix walk), rails = one disc on the turn side of radius
  w/2 + clearance; `drop` implemented in the plugin rip plan (engine treats drop as fixed) — cost if wrong: weaker hint on long hand stubs.
- Ruling A-C5: existing_copper = "smooth" stays "treated as fixed" in 0.5.0 (documented, backlog row in 45 §3; no A-task in the plan);
  [nets] pour_served implemented in A9 (per-net override of the auto pour-served decision); [run] detour_reference implemented by A7
  (engine Options.detour_reference + budget share; plugin key maps to it) — cost if wrong: smoothing of existing copper missing in 0.5.0
  (documented, user-visible doc string says so).

## Baseline (0.4.0 engine, main 6ea3040)
private-badge-b gate options (600 s, threads 4) on 6ea3040, coordinator run: 172/231, failed 59, vias 178, 3 iterations, stop timeout,
router 600.3 s, runtime 659.8 s, routing_new 0, preexisting 4 (clearance, hand copper), unconnected 69 (new), via reduction skipped
(timeout), detour median 1.018 p90 1.484 max 4.229 >1.5: 16. Main's run (scratch/v040): 171/231, 60 failed, 152 vias, 2 it, max detour
16.175 (GND SW1.0→pour 14.07 vs 0.87), USB- J1.A7→J1.B7 4.4×, VCC U3.1→U3.3 3.0×.

## Log
- Group 1 (A0, A1, A9, AP; sonnet worktree workers) all DONE(_WITH_CONCERNS); merged afcd794 (A1 1bcbe3b), a51815d (A0 0a6fa5f),
  48e954f (A9 20be763+db325b4), 7d87e7e (AP ae72705+d673dd3); no conflicts. clippy clean, docs_gen --check clean, maturin develop from main.
- A1: bend_ok (not clearance_bend) was ~86 % of planar search; memo + cached candidates + face clear radius (rarely fires), exact:
  candidates/query 109→11, search 775→28 ms/run, private-badge-b 60 s 86→175/231 (6 it); eprom/wiccon/nametag byte-identical; wiccon 9.7→2.9 s @4.
  Gate options: 188/255 (bench count incl. pour completion), failed 67, vias 199, routing_new 0, pre 4, unconnected 62, router 144 s
  (stop rule, not timeout), runtime 190 s, detour med 1.003 p90 1.52 max 11.6 (VCC U3.1→U3.3), 20 > 1.5. PNG
  docs/benchmarks/progress/2026-10-31-m4-A1-private-badge-b.png. Concern: anytime_timeout_emits_committed_subset (0.5 s) load-flaky.
- A0: D-1..D-10 done, relax_iter_cap single source (legacy key folded in from_json), strict_unfit_edges live key (eprom 2-layer no-op;
  front-only 26/27 strict → default off), harness --options + m4 private-badge-b 600 s defaults + bench columns; eprom 2dcde8ce40237b0c / wiccon
  d24bb79b58c7cbeb byte-identical. D-6 → A-C6.
- A9: tiers/exclude/ignore_pads/pour_served in engine + plugin wiring, no-effect tails removed; eprom/wiccon hashes unchanged.
  Concerns: tests after code; ignored pads not in connections_total; scope.py redundant net list left.
- AP: run_cli Popen+poll+timeout+cancel ([run] kicad_cli_timeout_s 120, KICAD_CLI_TIMEOUT), early setup progress, fingerprint before
  commit (BOARD_CHANGED_DURING_RUN); cancel during DRC rounds/preview export not killing kicad-cli (bounded by timeout); live UNVERIFIED rows.
- A3 DONE 99c7aab (merge f15895b): note docs/progress/notes/m4-capacity-steiner-design.md; concerns → A-C12, wiccon outside headroom
  6.4 mm (stubs must report delta), Steiner changes eprom (not-worse, not identity).
- DX DONE fcc66c4 + 5a3399c (profile-only IterProfile) (merge 50103c8): note the private-badge-b completion diagnosis (note removed: private board). 231 = first
  pass, 255 = after 3 pour-completion passes. 67 failed: own-pad/fine-pitch capacity 23 (12 need a neck), metrization rejects 37
  (taut pinch 11, wall grazed 7, NPTH hole clearance 6, via-site pair 4, sketch 4, self-check 3, other 2), pour completion 5, dynamic
  2; 0 "no room". Negotiator holds 205–215 paths/it, 156–175 survive metrization. Commit phase 44 % runtime. No option is a lever.
  Ranked: A4 ≈ 25–35, NX neck ≈ 12, HC NPTH hole clearance 6, A2 6 + detours, A5 8–10, A11 pour-cut ≈ 5+2, taut slack band 2–3;
  not levers: A16, A13, A10, stop rule, via cost.
- Verify @7d87e7e (merged group 1): cargo 36 suites ok; pytest default 1 failed (test_operate_md_limitations_section expects
  'net_priority' — A9 removed that limitation; test fixed c502dda), rest passed; private-badge-b gate options 188/255, failed 67, vias 199,
  42 it, stop "no gain", routing_new 0, unconnected 62, router 111 s, runtime 162 s.
- Debug proptest counter-example dist_props::prop_arc_arc_matches_sampling (DX) → task HC part 2.
- Group 2 dispatched: A4 (opus), A2 (opus), HC (sonnet: NPTH clearance + proptest oracle + TopoR backlog rows), A7 (sonnet),
  A10+A11 (sonnet).
- Group 2 results (merged 4f1d15d A2 search.rs memo-field conflict, 606baa4 HC, A4 merge, 4a339e6 A7 (operate/model/bench/detour/
  test_config_docs conflicts; field ref_len_nm; bench detour + detour_ghost + columns), A10/A11 merge (route_with: ignore pads →
  via option resolution → pool; search fields)); clippy clean, docs check clean, maturin develop from main.
  - A4 72b9d90/e847c82: private-badge-b (loaded) 190/242, failed 52, vias 190, unconnected 51, 28 it, detour max 3.5; metrize_failed/it 34–53
    → 20–26, overused edges 315–475 → 29–51; eprom guards identical; wiccon 62/62 vias 36, 1365.5 mm. Concern: anytime_timeout test
    flaky (5/6 in binary on A4). → A-C14.
  - A2 33d170b: #208 los runaway fixed; U7 middle pads need an off-centre in-pad stub (A6); private-badge-b 180/239 failed 59, vias 146, detour
    max 2.71, 11 > 1.5; VCC U3.1→U3.3 31 mm/4 vias → 3.29 mm/0; wiccon 62/62 vias 35, 1379.36 mm (0.64 mm headroom!). → A-C16.
  - HC 2b359ba/fca57b2/86e839b: NPTH clearance via BoardModel::pad_own_clearance (CDT kept first anchor per position = the pad's 0.2
    mm), slot holes as capsules in the Checker; arc_arc correct, oracle fixed (1-D profile scan); 44/45 TopoR rows, placement non-goal.
    private-badge-b (600 s, loaded, timeout) 170 → 178/231, hole failures 8 → 0. eprom/wiccon identical.
  - A7 f7983f0: reference search per connection (fixed copper, full clearance) under budget pct; eprom ref max 1.147, wiccon 1.434, 0
    fallbacks; private-badge-b 60 s: 120/231 ghost fallbacks at 10 %; at 100 % it ate the budget. Reference = search est_len (not metrized).
  - A10/A11 4162039/3e7a9ae: predefined vias, allowed drills, via-in-pad, per-net via cost (plugin by netclass > by priority >
    via_cost_mm), engine/preflight agreement test; layer costs default off (A-C15). eprom/wiccon identical.
- Main messages: detours got worse after A1 (20 > 1.5) — hairpin in A1 PNG is VCC U3.1→U3.3 (11.6×, 31 mm/4 vias), fixed by A2
  (3.29 mm, 0 vias); Blake toporouter: detour-optimisation-by-rip-up phase + variants (→ successor tasks DO/VR); GPL (A-C18).
- Group 3 (A6+NX opus, A5 opus, A16+A13 sonnet, A14 sonnet) merged: A6 f45dc37 (clean), A5 712a36a (model/route/schema
  conflicts: stub/neck + junction fields), A16+A13 62af5ad/8eb47e3 (route.rs 8 hunks: junction_repair_pass + via_relaxation_pass,
  leg_terminals(via_at) with mut goal_t, rip report), A14 d2d123a (Dangling(dir) + Stub terminals, hint arg) → ea1f064; clippy clean,
  docs check clean, maturin develop.
  - A6 alone: private-badge-b 210/241, failed 31, detour max 4.56 (8 > 1.5). A5 alone: 198/243 failed 45 (worker base 196/246), wiccon 34 vias.
    A16: wiccon identical, A13: wiccon outside −10.8 mm, detour max 1.449 → 1.243. A14: no snapshot change (private-badge-b 31 dangling ends
    = locked GND, not exercised).
- Verify @ea1f064 (quiet machine, coordinator): private-badge-b gate options 204/241 (first pass 231), failed 37, vias 196, routing_new 0,
  pre 4, unconnected 39, 24 it, "no gain", router 120 s, runtime 146 s, detour ref med 1.000 p90 1.296 max 4.62 >1.5 9 (U1-EN 4.6,
  VCC 4.2, TACT_B 3.6), ghost max 4.66 >1.5 15. eprom 27/27, 0 vias, 64.6 mm, ref max 1.1, 0.04 s. wiccon 62/62, 34 vias, 1363.4 mm,
  ref max 1.3, 1.56 s, "no gain". PNGs docs/benchmarks/progress/2026-11-01-m4-G3-merged-{private-badge-b,eprom-to-kernal,wiccon-2023}.png.
  Note: group-2 merge (946530c) quiet run 201/243 failed 42, 113 s; under load (10–25) the same run times out after 1–2 iterations.
- Known failing / flaky tests reported by workers (also on 946530c): route_detour_ref::reference_is_feasible_not_topological,
  tests/plugin/test_config_docs.py::test_keys_without_effect_say_so (NO_EFFECT count ≥ 1 — no key left without effect), test_router m3/m4
  progress test, test_scaffold (debug cargo flake), route_engine::anytime_timeout_emits_committed_subset (0.5 s, load-flaky),
  route_control::route_async_cancel_returns_anytime_result (load-flaky). A5: JP1 net-tie short in unlimited-time private-badge-b runs
  (tracks may touch the other net's pad of a net-tie group) → fix task.
- Rotation 1 worker round-trips: 15 (A0, A1, A9, AP, DX, A3, A4, A2, HC, A7, A10+A11, A6+NX, A5, A16+A13, A14) → hand back.
- Verify @ea1f064 (coordinator, after hand-back numbers): clippy -D warnings clean; docs_gen --check clean.
  cargo test --release --no-fail-fast: 42 suites ok, FAILED: graph_ownnet::front_only_plus5v_needs_zero_slack_for_failed (A4 test vs
  A16 default growth 2.0 — Q1/A-C22), route_steiner 4 of 6 (t_junction_onto_routed_sibling ("routed","failed"),
  junction_angle_below_60_is_rejected (no junction), steiner_threads_byte_identical (no "junction":true), steiner_off_is_m3_behaviour
  (digest differs)) — already failing at a233d62 = A5 ⊕ A6 interaction (stubs / pad bridges change the U7 crop fixture), NOT a
  thread nondeterminism; route_detour_ref::reference_is_feasible_not_topological (since A7 merge); anytime_timeout (flaky).
  pytest default: 503 passed, 4 failed (test_config_docs::test_keys_without_effect_say_so, test_run_flow::
  test_run_wiccon_refill_gives_harness_connection_count (plugin path 63 vs harness 62 connections), test_router::
  test_progress_m4_name_and_caption_lines, test_scaffold (debug cargo = the cargo failures above)).
  gate (M0–M4b): 18 passed, 3 failed: M2 + M3 private-badge-b info tests (60 s): 204/240, failed 36, **drc_new 1** (routing_new must be 0,
  M3-R13 — probably A5's JP1 net-tie short; at 600 s the DRC loop clears it), M4b headless wiccon (total 63 ≠ 62, same as the plugin
  run-flow test). eprom/wiccon M0–M3 gate numbers green (eprom 27/27 64.628 mm 0 vias; wiccon 62/62 34 vias 1363.375 mm 1.49 s).

## Coordinator rotation 2 (2026-11-01 →), base main 789e050
Brief additions for every rotation-2 task: rotation-2-common.md (state, private-badge-b measuring rule under load, acceptance).
- Round 1a dispatched (worktree workers): TE (opus: route_steiner A5⊕A6, route_detour_ref, load flakes), NT (opus: JP1 net-tie short /
  connection_width), PY (sonnet: pytest reds incl. wiccon plugin 63 vs 62), GR (sonnet: ripup growth sweep Q1 + pour_cut sweep),
  DG (opus F: private-badge-b diagnosis round 2). Round 1b after TE: VR (variants, A-C10), DO (detour rip-up phase + item 5 hidden detours).
- Round 1a results (all loaded 14–39: private-badge-b measured with wall_clock_s 0; base = head counts deterministic):
  - NT 09229f3: private-badge-b router DRC item = shorting_items JP1.1 (Net-(JP1-A)) ↔ Net-(J3-Pin_22) track; KiCad 10.0.8 rules measured with
    kicad-cli (tools/net_tie_drc_probe.py, 15 cases): a tied net's track may touch the tie's net-less bridge anywhere and the other tie
    pad only where the contact lies inside its own tie pad (pads overlap); a gapped tie pad is an ordinary obstacle. Model: tie pad
    exempt only for its own net + nets of touching/overlapping tie pads of the same footprint, drills never; checker = KiCad's exact
    rule. private-badge-b 205/241 routing_new 1 → 0; eprom/wiccon identical. connection_width never appeared (necks ≥ 0.2 > 0.127 min). → A-C25.
  - TE d8d90bf/76d945a/3a48fd7: steiner reds = a refined junction point whose leg runs exactly along J1.A7's clearance ring (larger CDT
    faces since A6) → relaxer wraps 358°, "grazed mid-span", T retried every iteration; fix: refined point whose leg fails to relax falls
    back to its base point and is pinned. steiner-off digests re-pinned with per-engine evidence (946530c 0xc58d…, A6-only 3cddcc9 =
    A5-merge steiner-off 0xf913…, A16+A13 6ff3d00 0xeb87…; 6ff3d00 with A16/A13 neutralised = A6-only bit for bit, now asserted).
    route_stubs::exit_stubs_off_matches_previous (green at base only because the T failed) now also turns off steiner + via relaxation
    and matches the pre-A6 golden. detour_ref: ghost == slot search on the USB row since A2/A4 (1.220656 mm = 2 × a6→a7 corner), test
    keeps those assertions and adds a via-wall fixture where they must differ (ghost 10.01 < reference 12.0). Flakes: fixed wall-clock
    moments fell before any search under load; budgets now from a calibration run / cancel after iteration 1, assertions kept, 3/3 under
    8 × `yes`. private-badge-b unchanged (205/241). → A-C26.
  - GR 2c8ac24: growth table (ledger copy below: A-C24); pour_cut_cost sweep: no value improves completion without a cost elsewhere
    (wiccon vias 34 → 41/54/74 at 0.25/0.5/1.0 mm, private-badge-b routing_new 3 at 0.5/1.0, detour max 16.8 at 0.25) → A-C15 stands (0.0).
  - PY 01de2c2/870faf8: no-effect test re-specified (marked set == consumer-less keys, marked keys stay live, md/toml marker counts
    equal; PARTIAL + detour_reference checks kept); progress caption test follows A7 columns (detour = reference stats). wiccon 63 vs 62
    = ENGINE: plugin net_priority {+4V,3.3v,4v,GND: 2} reorders → a GND via→pour completion fails `self-check viahole:topor:249 (gap
    304999 < 350000)` = DG's via drill-hole class (R2.3 (d) 2) → task FG.
  - DG 1f067ad/18c5f82: note "Round 2" R2.0–R2.4. Key facts: first pass 229 (A-C20 merges); 34/36 failures route alone, 33/36 in at least
    one of 13 orderings (29–46 failed: single private-badge-b runs are ±5 chaotic); A5-vs-A6 loss is in pour-completion passes (USB corner) and
    the post-pass VETO: the best snapshot does not re-metrize (1 wire fails a null re-metrization) so via reduction/relaxation/zero-slack/
    junction repair keep nothing (vias 175 → 196). Ranked: FR (idempotent re-metrize + generalised repair, +9), FF (fine-pitch pour-pad
    bridges / no via before a neighbour's tip, ≈ 5–9), FG (via hole clearance, taut deep wall, sub-margin grazes, ≈ 8), then VR (best of 8
    orders −7), small (signal-net polygons as terminals; harness DRC budget for wall_clock 0 — fixed by the coordinator, 27a… below).
    Blake detour rip-up: quality lever, ≤ 3 failures freed.
- Coordinator: harness bench._drc_budget_s (wall_clock_s 0 → no DRC-loop budget cut, as plugin NO_BUDGET_S) + test → A-C27.
- Ruling A-C24 (main Q1 / A-C22 closed): `[ripup] growth` default 2.0 → **1.25** (max_per_pair 8). Measured (GR, wall_clock 0, threads 4):
  private-badge-b 205/241 failed 36 at every g ∈ {1.0, 1.25, 1.5, 1.75, 2.0, 3.0} (1.5: 206/242), > 1.5: 8 at 1.0/1.25/3.0, 9 at 1.5/1.75/2.0;
  eprom/wiccon/nametag (34/34, 3 vias)/wedding (77/89) identical for every g; F.Cu-only guard 27/27 67.034 mm + zero-slack +5V for
  1.0–1.75, 67.095 at 3.0, **92.29 mm without zero-slack at 2.0 (fails both guards)**. 1.25 = 1.0 in every column, keeps the escalation
  of row 17, middle of the plateau. Guards now run with Options::default(); new plateau test pins 1.0/1.25/1.5. 46 row 17 + reference
  updated — cost if wrong: one default (chaotic outside the plateau; a board class where 2.0 helps would need the key set by hand).
- Ruling A-C25 (NT): net-tie exemption = KiCad 10 semantics measured with kicad-cli (above); overlapping tie pads: the routing model
  exempts the whole pad, the checker rejects a contact outside the track's own pad (connection fails, never a DRC error; no snapshot
  board has overlapping tie pads) — cost if wrong: a failed connection on boards with overlapping tie pads.
- Ruling A-C26 (TE): re-pinned expectations accepted on the evidence above (steiner-off = A6-only behaviour + A16/A13 deltas, verified by
  neutralising them; exit_stubs_off compares with all post-A6 features off; detour_ref discriminates on a via-wall fixture) — cost if
  wrong: a pinned digest that hides a later regression of the steiner-off path (the neutralised-equality assertion guards it).
- Ruling A-C27 (DG concern 1): harness wall_clock_s 0 = no DRC-loop budget cut (the plugin did this already) — cost: none.
