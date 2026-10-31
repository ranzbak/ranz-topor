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
