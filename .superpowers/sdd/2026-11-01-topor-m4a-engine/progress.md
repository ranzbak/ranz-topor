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
- Ruling A-C5: existing_copper = "smooth" stays "treated as fixed" in 0.5.0 (documented, backlog row in 45 §3; no A-task in the plan);
  [nets] pour_served implemented in A9 (per-net override of the auto pour-served decision); [run] detour_reference implemented by A7
  (engine Options.detour_reference + budget share; plugin key maps to it) — cost if wrong: smoothing of existing copper missing in 0.5.0
  (documented, user-visible doc string says so).

## Baseline (0.4.0 engine, main 6ea3040)
(filled after the baseline run)

## Log
