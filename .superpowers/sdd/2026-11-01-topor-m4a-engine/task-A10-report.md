# Report: A10 (via rules from the board setup) + A11 (layer cost terms)

Worker: A10 then A11, two commits on branch `worktree-agent-a348fe40021811dba` (base 68a77b6). Never pushed.
Status: DONE_WITH_CONCERNS (A11 default is OFF: the plan default 1 mm/mm is worse on eprom and wiccon, see numbers).

## A10 (commit "feat(vias): predefined via list, fallback warning, allowed_drills, via-in-pad, per-net via cost")

Engine
- `model.rs`: `Rules.predefined_vias: Vec<[i64;2]>`, `Rules.via_fallback: [i64;2] = [800000, 400000]`, `Rules.allowed_drills`
  (serde-skip, baked by `route`); `Options.allowed_drills_nm`, `via_in_pad = false`, `via_cost_by_net` (net name -> nm),
  `via_cost_by_code` (run-time form, filled by `route`), `reduction_len_slack_pct = 10.0`; helpers `via_cost_for(code)`,
  `reduction_len_slack()`.
- `vias.rs`: `via_sizes(rules, net, opts)` (+ `via_sizes_baked(rules, net)` for the search, which has rules but not opts; both call one
  core `sizes_for`). Order: netclass via first (or `via_fallback` clamped to board minimums only when `predefined_vias` is empty AND the net's class
  is at the factory 0.8/0.4 AND the fallback differs: `via_fallback_used`), then smaller alternatives = predefined entries UNION the fab-profile
  `routing_vias`, filtered by `drill >= min_via_drill`, `d >= min_via_diameter`, annular ring, `d < first.d`, `drill < first.drill`, allowed drills.
  `via_sites_opts(..., in_pad)`: two extra candidates per own-net SMD pad (on the long axis at max(0.35 * half-length, 0.15 mm) either side of the centre,
  must lie inside the pad, fitted with the pad's own net so its copper is exempt), `SiteOrigin::InPad(pad)`, `ViaSite.net = Some(net)`;
  `MultiGraph::site_free` now honours it (a foreign net cannot use an in-pad site). `via_sites` = `via_sites_opts(.., false)`.
- `route.rs`: `route_with` resolves `via_cost_by_net` -> by code and bakes `allowed_drills` into a model clone (inputs untouched); per-net via cost
  in all four search-parameter sites (`negotiate.rs` one line); reduction slack from the option (const `REDUCTION_LEN_SLACK` removed);
  warning `"via fallback 0.8/0.4 used"` (pass 0, only when the fallback changed a size); `RouteResult.vias_in_pads` (KiCad frame, sorted).
- `py.rs`: new `topor_engine.via_sizes(rules_json, net, options_json)` for the agreement test.
- Existing tests: `ViaSite` literals gained `net: None` (graph_capacity, multigraph, negotiate_loop, search_layers), `via_sizes` calls got the opts argument
  (via_sites.rs, examples/gate_diag.rs). No assertion changed.

Python
- `toporlib/rules.py`: `predefined_vias_from_pro`, `via_setup_missing_from(predefined, classes)` / `via_setup_missing(pro)` (one implementation, used by the
  harness SWIG path, the plugin and the pre-flight), rules export `predefined_vias`, warning "via sizes not set up: ... (the engine's via fallback applies)".
  `harness/rules.py` passes the SWIG `m_ViasDimensionsList` as `via_dimensions` so pure == SWIG (tests/test_rules.py).
- `plugin/topor/preflight.py`: `via_setup_missing` delegates; `smaller_profile_vias` mirrors `sizes_for` incl. predefined + allowed drills.
  Agreement test: `tests/plugin/test_via_rule_agreement.py` feeds the six snapshot projects (with / without plugin fallback, with / without drill filter)
  to both and compares the engine call with the Python function (24 + 1 cases).
- `run.py`: `via_costs_by_net` (netclass > priority > via_cost_mm; composite class "Power,Default" = first listed class with an entry),
  `build_options` passes it; `apply_via_fallback` also writes `rules["via_fallback"]` (the engine's own field; plugin pre-patch and engine fallback agree;
  the engine is the single place of truth, documented in the schema doc strings).
- `schema.py`/`config.py`: live keys `via_in_pad`, `allowed_drills_mm` (list mm -> nm), `reduction_len_slack_pct`, `cost_by_netclass`, `cost_by_priority`
  (`since` removed, doc strings state precedence); `messages.py` VIA_NOT_SET_UP fix text no longer says pre-defined vias are not routed with;
  configuration.md + troubleshoot.md regenerated; `docs_gen --check` clean.
- Test adjustments forced by the behaviour change (not weakened): `test_config_docs.py` floor `n >= 10` of not-yet-live keys -> `n >= 1`
  (the count shrinks as keys go live; 5 now), `test_preflight.py` / `test_messages.py` assertion that pre-defined vias are "not read ... 0.5.0" now asserts
  they are routed with.

Judgement calls made configurable: `via_in_pad`, `allowed_drills_mm`, `reduction_len_slack_pct`, `cost_by_netclass`, `cost_by_priority`, `via_fallback`.
Decisions: alternatives = predefined UNION fab profile (keeps eprom/wiccon/dcsgone behaviour identical; `allowed_drills_mm` is the user's way to prune);
fallback warning only when the fallback differs from the factory size (keeps results byte-identical for default boards).

Tests (A10): `engine/tests/via_rules.rs` (6: the brief's names), `tests/test_rules_vias.py` (the toporlib test), `tests/plugin/test_via_rule_agreement.py`.
Numbers (harness `run_bench`, rip all, threads 4, option `wall_clock_s 300`; base = the shared .venv engine, new = this worktree):
- eprom: 27/27, vias 0, routing_new 0, unconnected 0, outside pads 64.628 mm, router_s 0.19, result sha 523e638dedbc == base (byte-identical).
- wiccon: 62/62, vias 37, routing_new 0, unconnected 0, outside pads 1373.553 mm, router_s 4.6, result sha ad7b87a642bc == base (byte-identical).

## A11 (commit "feat(search): pour-cut and preferred-direction layer costs")

- `model.rs`: `Options.pour_cut_cost_nm` (cost nm per mm; **default 0**, see below), `layer_direction: BTreeMap<layer name, degrees>`, `direction_cost = 0.0`.
- `multigraph.rs`: `LayerCtx.face_pour` (per face: net of the pour fill covering the face centroid on that layer; O(F * R) once per build, bbox culled),
  `MultiGraph.layer_cost`, `set_layer_costs(model, opts)` (called in `route_pass` right after `build_on`).
- `search.rs` (additive only): `Search.lc` + `layer_extra(fin, delta, apex, p)` added to the `push_state` step cost: `delta * pour_per_nm` when the face the
  step was made in is covered by a foreign (net != connection net, != 0) pour, plus `|p - apex| * direction_cost * |sin a|` against the layer direction.
  O(1) per step; both terms >= 0 so the ghost heuristic stays admissible. Via/goal pushes are not charged (documented gap).
- Plugin: `[layers] pour_cut_cost_mm` (default 0.0), `direction_cost`, `preferred_direction` ("h" = 0, "v" = 90, "none"/unknown skipped) are live
  (`run.build_options`); `layer_direction` listed in `ENGINE_INTERNAL`; docs regenerated.
- Tests: `engine/tests/route_layer_cost.rs` (3: the brief's names), `tests/plugin/test_a10_a11_options.py` (precedence of via costs, unit mapping, tables).

### Default decision (coordinator rule: keep plan default 1 mm/mm only if not worse)
| board | pour_cut 0 (= M3) | pour_cut 1.0 mm/mm |
|---|---|---|
| eprom | 27/27, vias 0, outside 64.628 mm, 1 iteration, router_s 0.19 | 27/27, vias 0, **outside 65.874 mm (> 64.7 gate)**, 7 iterations, router_s 1.24 |
| wiccon | 62/62, vias 37, outside 1373.6 mm, router_s 4.6 | **58/61, 3 unconnected, vias 83**, outside 1326 mm, detour max 1.76, router_s 35.6 |

=> worse on both: **default 0.0** (engine `pour_cut_cost_nm: 0`, schema `pour_cut_cost_mm = 0.0`); with 0 the result is byte-identical to M3
(`costs_zero_match_m3`, eprom sha 523e638dedbc, wiccon sha ad7b87a642bc). A value between (0.1..0.5) was not measured.
eprom GND pour islands (raster of the B.Cu / F.Cu fills, via a throwaway diagnostic, not committed): before routing 4, after routing 12 with cost 0 and
12 with cost 1.0 (lost items 0 in both): no effect on eprom.

### dcsgonebadge (gate options: wall 600, partial false, threads 4; machine shared, counts only)
| | pour_cut 0 | pour_cut 1.0 |
|---|---|---|
| routed / total | 171 / 231 (failed 60) | 164 / 231 (failed 67) |
| vias | 152 | 192 |
| stop / iterations | timeout / 2 | timeout / 2 |
| routing_new / preexisting | 0 / 4 | 1 / 4 |
| unconnected items (new) | 70 | 70 |
| GND islands after routing / lost items | 103 / 17 (SW4, SW5, D11, C11, D10, C27, C26, BT1, J3, U3, C23, C15, Q3, C10) | **61 / 2** (SW4.0, SW4.1) |
| pour-completion passes | 0 (no budget left: the first pass uses the whole 600 s) | 0 (same) |
| outside pads | 1398.0 mm | 1262.9 mm |
| detour max / count > 1.5 | 16.2 / 10 | 3.5 / 11 |
So on dcsgone the pour cost does what the diagnosis said (15 fewer GND items cut off the fill, 42 fewer islands), but both runs end in the first pass at the
wall clock, so completion connections / re-routes cannot be counted; vias +40 and routed -7 in the same time. The M4a gate (231/231) is not reached by A11;
re-measure when A1/A2/A4 (search speed, capacity) have landed, ideally with a value between 0 and 1.

PNG (harness.progress with the A-C3 600 s option patched in at run time): `/home/paul/work/kicad/plugins/topor/.claude/worktrees/agent-a348fe40021811dba/docs/benchmarks/progress/2026-11-01-m4-A11-dcsgonebadge.png`
(untracked, not committed). Caption: 175/231 connections, 41/66 nets, partial 0, failed 56, vias 183, arcs 225, zero slack 0, iters 5, pcb 3e7e66f862a7;
DRC routing 0, preexisting 4, unconnected 71, outside pads 1639.5 vs hand 3574.7 mm, F.Cu 972.7 / B.Cu 864.5 mm, router 600.07 s on 4 threads (loaded machine);
detours > 1.5: 18 (max 11.6) — VCC 11.6, USB- 5.2, USB+ 3.6; stop_reason timeout. (pour_cut 0, the default; not a gate result.)
wiccon numbers did not move with the default (identical sha), so no wiccon PNG.

## Verification
- `cargo test --release --manifest-path engine/Cargo.toml --no-fail-fast`: all green (route_control `route_async_cancel_returns_anytime_result` is a 2 s timing
  test: it failed once under load while pytest ran in parallel and passed alone and in the final full run).
- `cargo clippy --all-targets --manifest-path engine/Cargo.toml -- -D warnings`: clean.
- pytest (PYTHONPATH engine override), full default suite at the A10 state: 464 passed, 7 skipped; 3 failures were (a) the Options field test (source had A11
  fields, engine build was A10) and (b)(c) pure == SWIG rules (fixed in the A10 commit: SWIG path now passes `via_dimensions`); after the fixes the touched
  suites (test_rules, test_rules_vias, test_engine_surface, tests/plugin/test_config*, test_messages, test_preflight, test_via_rule_agreement, test_scope,
  test_a10_a11_options): 151 + 3 passed. I did not re-run the whole 53 min suite after the last fixes.
- Determinism: option JSON with the new fields only changes the result when used; default result sha identical to base on eprom and wiccon. threads 1/2/4
  byte-identity of the new terms was not run separately (the terms are pure functions of the graph; no map iteration order reaches the output).

## Concerns
1. A11 default is 0 (opt-in) because 1 mm/mm is worse on eprom (length gate) and wiccon (58/61); dcsgone improves pour connectivity but is time-bound.
2. Conflict surface for the merge: `ViaSite.net` field (4 test literals), `search.rs` (Search struct, push_state cost, Layered::new), `route.rs`
   (route_with, params closure now takes the net), `multigraph.rs` (site_free, LayerCtx literal), `model.rs` Options fields.
3. A foreign pour face on the whole layer (a full GND plane) charges every signal step on it; with a plane on only one layer this is what steers the layer choice,
   with planes on both layers (eprom) it only distorts (hence no gain).
4. `test_config_docs` floor lowered from 10 to 1 (not-yet-live keys shrink by design).
5. Attribution line uses the system-reminder's "Claude Sonnet 5.5" instead of the contract's "Claude Opus 5.5".
