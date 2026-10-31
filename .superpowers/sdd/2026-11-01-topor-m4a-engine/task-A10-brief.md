### Task A10 (S): via rules from the board setup — `vias.rs`, `model.rs`, `route.rs`, `toporlib/rules.py`

**Interfaces:** `Rules.predefined_vias: Vec<[i64; 2]>`, `Rules.via_fallback: [i64; 2] = [800_000, 400_000]`; `Options.allowed_drills_nm: Vec<i64>`, `via_in_pad: bool = false`, `via_cost_by_net: BTreeMap<String, i64>`, `reduction_len_slack_pct: f64 = 10.0`; `via_sizes(rules, net, opts)`: netclass via first, then smaller predefined vias (filtered, `≥ min_via_drill`), fallback only when the list is empty **and** the netclass is at KiCad factory values (warning `"via fallback 0.8/0.4 used"`); `RouteResult.vias_in_pads: Vec<[i64;2]>`; `toporlib.rules` exports `predefined_vias` from `.kicad_pro` `m_ViasDimensionsList` and the factory-default warning.

- [ ] **Step 1:** `via_sizes_prefer_netclass_then_predefined_smaller`, `allowed_drills_filters_predefined`, `fallback_only_when_nothing_set_up`, `via_in_pad_site_inside_own_smd_pad_only_when_enabled`, `reduction_slack_from_options`, `via_cost_by_net_overrides_default` (`engine/tests/via_rules.rs`); `test_rules_exports_predefined_vias_and_factory_default_warning`. FAIL → implement → PASS; eprom/wiccon byte-identical. Commit `feat(vias): predefined via list, fallback warning, allowed_drills, via-in-pad, per-net via cost`.


### Task A11 (S): layer cost terms — pour cut and preferred direction — `search.rs`, `route.rs`, `model.rs`

**Interfaces:** `Options.pour_cut_cost_nm: i64 = 1_000_000` per mm inside a foreign pour, `Options.layer_direction: BTreeMap<String, f64>`, `Options.direction_cost: f64 = 0.0`; additive step costs (admissible), `O(1)` per step via `in_pour` of the face.

- [ ] **Step 1:** `leg_through_foreign_pour_prefers_other_layer_when_cost_set`, `preferred_direction_cost_tilts_choice`, `costs_zero_match_m3` (`engine/tests/route_layer_cost.rs`). FAIL → implement → PASS. eprom B.Cu GND pour islands before/after reported. PNG `--task A11`. Commit `feat(search): pour-cut and preferred-direction layer costs`.


## Coordinator additions (binding; carry-over item 8, Ruling A-C4) — one worker does A10 then A11 (two commits)
- 46 row 2 (via sizes) and row 6 (via minimisation): `[vias] via_cost_mm = 2.0`, `max_per_connection = 4`, `reduction_len_slack_pct = 10`
  (since 0.5.0: make it a live key mapped to `Options.reduction_len_slack_pct`), per-netclass / per-priority via cost overrides
  (`[vias] cost_by_netclass = {}`, `cost_by_priority = {}` in the schema → plugin resolves them per net into
  `Options.via_cost_by_net` (ENGINE_INTERNAL); precedence: by netclass > by priority > via_cost_mm — state it in the doc strings),
  `allowed_drills_mm`, `via_in_pad` (both since 0.5.0 → live), fallback 0.8/0.4 only when nothing is set up (C27/C33 in the M4b rulings:
  the plugin already writes the fallback into the rules on VIA_NOT_SET_UP — make engine + plugin agree, single place of truth documented).
- `toporlib.rules` exports `predefined_vias` from `.kicad_pro` `board.design_settings.via_dimensions` (this is where KiCad 10 stores
  the pre-defined via list; C33 used it — check the snapshot .kicad_pro files) for BOTH the harness (harness/rules.py) and the plugin.
- FX2 concern: plugin/topor/preflight.py `smaller_profile_vias` duplicates the engine via rule (engine/src/vias.rs). Make them agree by
  construction or by a shared test that feeds the same inputs to both (engine via `topor_engine` call, plugin via the Python function)
  on the six snapshot projects.
- A11: `[layers] pour_cut_cost_mm = 1.0`, `preferred_direction = {}`, `direction_cost = 0.0` become live keys (since 0.5.0 → live) mapped
  to the engine fields; NOTE default pour_cut_cost 1 mm/mm changes routing vs 0.4.0 — if eprom/wiccon/dcsgone get WORSE with the
  plan default, report numbers for 0 and 1.0 and keep the plan default only if not worse; otherwise default 0.0 and report
  (coordinator rules). Report eprom B.Cu GND pour islands before/after.
- Not-worse numbers (global-constraints.md) on eprom + wiccon after each commit; dcsgone at the gate options (A-C3) once at the end
  (routed/total, failed, vias, routing_new, unconnected, router_s, detour max/count>1.5). PNG `--milestone m4 --task A11 --board dcsgonebadge`
  (and wiccon if its numbers moved).
- Regenerate configuration.md; docs_gen --check clean; remove "(available from 0.5.0)"/"No effect in 0.4.0" tails of keys you make live.
