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
| dcsgone PNGs | harness.progress uses wall_clock 60 for dcsgone (M3 info) | gate options are 600 s → A-C3 |
| .venv | maturin develop by one owner; parallel Rust workers | PYTHONPATH engine override per worktree → A-C2 |

## Rulings
- Ruling A-C1: docs/benchmarks/ untracked (main ab42758 + user): progress PNGs / benchmark dirs written on disk only, never committed;
  workers produce them in their worktree, the coordinator copies them to /home/paul/work/kicad/plugins/topor/docs/benchmarks/progress/;
  tracked docs quote numbers inline — cost if wrong: none (user ruling).
- Ruling A-C2: the shared .venv belongs to the coordinator (maturin develop from main after each engine merge); workers use their own
  engine build via `cargo build --release --features python` + copy of libtopor_engine.so to <wt>/target/pyeng/topor_engine/ +
  PYTHONPATH (verified: import resolves to the override) — lets several Rust workers run Python/harness in parallel — cost if wrong:
  a worker forgets the override and measures the 0.4.0 engine (reports must print topor_engine.__file__).
- Ruling A-C3: M4a progress PNGs / measurements for dcsgonebadge use the GATE options (wall_clock_s 600, partial_routes False,
  threads 4); the plan's "dcsgone 60 s ≥ 148" check (A1) is reported in addition — cost if wrong: ~10 min per dcsgone PNG.
- Ruling A-C4: carry-over mapping as global-constraints.md "M4 carry-over" 1–10 (A9 includes plugin wiring of priorities /
  exclude_nets / ignore_pads + [nets] pour_served; A16 includes the relax_iter_cap dedupe; A10 the via [vias] keys + per-netclass
  overrides + preflight sync; AP = plugin minors 10/11) — cost if wrong: re-split of tasks.
- Ruling A-C6 (A0 concern 1): D-6 listed same-net pour vias as goals stays behind `Options.pour_listed_via_goals` / `[search]
  pour_listed_via_goals` default false (on: wiccon vias 37→36 but outside 1381.4 mm > 1380 bound, GND J2.2 13.9 vs 6.0 mm) — cost if
  wrong: listed pour vias unused as goals (M3 behaviour), revisit in A5 (same-net attachment).
- Ruling A-C7: group 2 = DX (F dcsgone diagnosis now, plan A12 Step 2 moved forward because A1 made dcsgone stop-rule-limited, not
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
- Ruling A-C11 (DX §1): dcsgone gate predicate = routed == connections_total (incl. pour-completion connections; 255 today),
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
  65.874 > 64.7 mm gate) and wiccon (58/61, 83 vias, 35.6 s); on dcsgone 1.0 keeps GND items in the pour (lost 17 → 2) but routes less
  (164 vs 171/231 under load). Revisit only with a value sweep — cost if wrong: extra pour-completion passes on dcsgone.
- Ruling A-C16 (A2): los convergence rule applied only to lines leaving a terminal (everywhere breaks the M2 front-only guard,
  92.35 > 67.8 mm); LOS_STALL_CAP 2 internal; roundrect corner-tangent discretisation not done (not needed after state_point shrink) —
  cost if wrong: residual los runaway away from terminals (detour stats watch it).
- Ruling A-C17 (A10): via sizes = netclass via, then the union of KiCad pre-defined vias and the fab-profile vias (eprom/wiccon/dcsgone
  unchanged); fallback 0.8/0.4 warning only when it changes a size; engine/plugin rule agreement test on the six snapshots — cost if
  wrong: an extra smaller via size offered where the user listed none.
- Ruling A-C18 (main, user): topor is GPL-2.0-or-later (93f2126); porting from the gEDA PCB toporouter (GPL-2.0-or-later) is allowed
  with the original copyright + attribution at the port site, origin in the commit message, one A-C ruling per port; no new
  Apache-2.0-only runtime crates — cost: none.
- Ruling A-C19 (main): detour count > 1.5 and max (reference + ghost) are non-regression acceptance items in every task, next to
  completion — cost: none.
- Ruling A-C5: existing_copper = "smooth" stays "treated as fixed" in 0.5.0 (documented, backlog row in 45 §3; no A-task in the plan);
  [nets] pour_served implemented in A9 (per-net override of the auto pour-served decision); [run] detour_reference implemented by A7
  (engine Options.detour_reference + budget share; plugin key maps to it) — cost if wrong: smoothing of existing copper missing in 0.5.0
  (documented, user-visible doc string says so).

## Baseline (0.4.0 engine, main 6ea3040)
dcsgone gate options (600 s, threads 4) on 6ea3040, coordinator run: 172/231, failed 59, vias 178, 3 iterations, stop timeout,
router 600.3 s, runtime 659.8 s, routing_new 0, preexisting 4 (clearance, hand copper), unconnected 69 (new), via reduction skipped
(timeout), detour median 1.018 p90 1.484 max 4.229 >1.5: 16. Main's run (scratch/v040): 171/231, 60 failed, 152 vias, 2 it, max detour
16.175 (GND SW1.0→pour 14.07 vs 0.87), USB- J1.A7→J1.B7 4.4×, VCC U3.1→U3.3 3.0×.

## Log
- Group 1 (A0, A1, A9, AP; sonnet worktree workers) all DONE(_WITH_CONCERNS); merged afcd794 (A1 1bcbe3b), a51815d (A0 0a6fa5f),
  48e954f (A9 20be763+db325b4), 7d87e7e (AP ae72705+d673dd3); no conflicts. clippy clean, docs_gen --check clean, maturin develop from main.
- A1: bend_ok (not clearance_bend) was ~86 % of planar search; memo + cached candidates + face clear radius (rarely fires), exact:
  candidates/query 109→11, search 775→28 ms/run, dcsgone 60 s 86→175/231 (6 it); eprom/wiccon/nametag byte-identical; wiccon 9.7→2.9 s @4.
  Gate options: 188/255 (bench count incl. pour completion), failed 67, vias 199, routing_new 0, pre 4, unconnected 62, router 144 s
  (stop rule, not timeout), runtime 190 s, detour med 1.003 p90 1.52 max 11.6 (VCC U3.1→U3.3), 20 > 1.5. PNG
  docs/benchmarks/progress/2026-10-31-m4-A1-dcsgonebadge.png. Concern: anytime_timeout_emits_committed_subset (0.5 s) load-flaky.
- A0: D-1..D-10 done, relax_iter_cap single source (legacy key folded in from_json), strict_unfit_edges live key (eprom 2-layer no-op;
  front-only 26/27 strict → default off), harness --options + m4 dcsgone 600 s defaults + bench columns; eprom 2dcde8ce40237b0c / wiccon
  d24bb79b58c7cbeb byte-identical. D-6 → A-C6.
- A9: tiers/exclude/ignore_pads/pour_served in engine + plugin wiring, no-effect tails removed; eprom/wiccon hashes unchanged.
  Concerns: tests after code; ignored pads not in connections_total; scope.py redundant net list left.
- AP: run_cli Popen+poll+timeout+cancel ([run] kicad_cli_timeout_s 120, KICAD_CLI_TIMEOUT), early setup progress, fingerprint before
  commit (BOARD_CHANGED_DURING_RUN); cancel during DRC rounds/preview export not killing kicad-cli (bounded by timeout); live UNVERIFIED rows.
- A3 DONE 99c7aab (merge f15895b): note docs/progress/notes/m4-capacity-steiner-design.md; concerns → A-C12, wiccon outside headroom
  6.4 mm (stubs must report delta), Steiner changes eprom (not-worse, not identity).
- DX DONE fcc66c4 + 5a3399c (profile-only IterProfile) (merge 50103c8): note docs/progress/notes/m4-dcsgone-diagnosis.md. 231 = first
  pass, 255 = after 3 pour-completion passes. 67 failed: own-pad/fine-pitch capacity 23 (12 need a neck), metrization rejects 37
  (taut pinch 11, wall grazed 7, NPTH hole clearance 6, via-site pair 4, sketch 4, self-check 3, other 2), pour completion 5, dynamic
  2; 0 "no room". Negotiator holds 205–215 paths/it, 156–175 survive metrization. Commit phase 44 % runtime. No option is a lever.
  Ranked: A4 ≈ 25–35, NX neck ≈ 12, HC NPTH hole clearance 6, A2 6 + detours, A5 8–10, A11 pour-cut ≈ 5+2, taut slack band 2–3;
  not levers: A16, A13, A10, stop rule, via cost.
- Verify @7d87e7e (merged group 1): cargo 36 suites ok; pytest default 1 failed (test_operate_md_limitations_section expects
  'net_priority' — A9 removed that limitation; test fixed c502dda), rest passed; dcsgone gate options 188/255, failed 67, vias 199,
  42 it, stop "no gain", routing_new 0, unconnected 62, router 111 s, runtime 162 s.
- Debug proptest counter-example dist_props::prop_arc_arc_matches_sampling (DX) → task HC part 2.
- Group 2 dispatched: A4 (opus), A2 (opus), HC (sonnet: NPTH clearance + proptest oracle + TopoR backlog rows), A7 (sonnet),
  A10+A11 (sonnet).
- Group 2 results (merged 4f1d15d A2 search.rs memo-field conflict, 606baa4 HC, A4 merge, 4a339e6 A7 (operate/model/bench/detour/
  test_config_docs conflicts; field ref_len_nm; bench detour + detour_ghost + columns), A10/A11 merge (route_with: ignore pads →
  via option resolution → pool; search fields)); clippy clean, docs check clean, maturin develop from main.
  - A4 72b9d90/e847c82: dcsgone (loaded) 190/242, failed 52, vias 190, unconnected 51, 28 it, detour max 3.5; metrize_failed/it 34–53
    → 20–26, overused edges 315–475 → 29–51; eprom guards identical; wiccon 62/62 vias 36, 1365.5 mm. Concern: anytime_timeout test
    flaky (5/6 in binary on A4). → A-C14.
  - A2 33d170b: #208 los runaway fixed; U7 middle pads need an off-centre in-pad stub (A6); dcsgone 180/239 failed 59, vias 146, detour
    max 2.71, 11 > 1.5; VCC U3.1→U3.3 31 mm/4 vias → 3.29 mm/0; wiccon 62/62 vias 35, 1379.36 mm (0.64 mm headroom!). → A-C16.
  - HC 2b359ba/fca57b2/86e839b: NPTH clearance via BoardModel::pad_own_clearance (CDT kept first anchor per position = the pad's 0.2
    mm), slot holes as capsules in the Checker; arc_arc correct, oracle fixed (1-D profile scan); 44/45 TopoR rows, placement non-goal.
    dcsgone (600 s, loaded, timeout) 170 → 178/231, hole failures 8 → 0. eprom/wiccon identical.
  - A7 f7983f0: reference search per connection (fixed copper, full clearance) under budget pct; eprom ref max 1.147, wiccon 1.434, 0
    fallbacks; dcsgone 60 s: 120/231 ghost fallbacks at 10 %; at 100 % it ate the budget. Reference = search est_len (not metrized).
  - A10/A11 4162039/3e7a9ae: predefined vias, allowed drills, via-in-pad, per-net via cost (plugin by netclass > by priority >
    via_cost_mm), engine/preflight agreement test; layer costs default off (A-C15). eprom/wiccon identical.
- Main messages: detours got worse after A1 (20 > 1.5) — hairpin in A1 PNG is VCC U3.1→U3.3 (11.6×, 31 mm/4 vias), fixed by A2
  (3.29 mm, 0 vias); Blake toporouter: detour-optimisation-by-rip-up phase + variants (→ successor tasks DO/VR); GPL (A-C18).
