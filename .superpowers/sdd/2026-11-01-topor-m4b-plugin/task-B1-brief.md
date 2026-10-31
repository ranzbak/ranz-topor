### Task B1 (S; doc text H): settings schema, `topor.toml` loader/writer, generated config reference — `plugin/topor/schema.py`, `config.py`, `docs_gen.py`

**Files:** Create `plugin/topor/__init__.py`, `schema.py`, `config.py`, `docs_gen.py`; `docs/user/configuration.md` (generated); tests `tests/plugin/test_config.py`, `tests/plugin/test_config_docs.py`.

**Interfaces:**
- Produces: `Setting(key, section, type, default, unit, range, doc, kicad_place=None, engine_field=None, harness_field=None, plugin_field=None)`; `SETTINGS` sections/defaults:
  `[run]` `timeout_s = 60` (0 = disabled; engine `wall_clock_s`), `threads = 0`, `partial_routes = false`, `existing_copper = "fixed"` (`fixed|smooth|rip`), `respect_locked = true`, `scope_mode = "between"` (`between|touching`), `unrouted_only = false`, `fail_on_preexisting = false`, `kicad_cleanup = false`, `kicad_teardrops = false`, `detour_reference = true`;
  `[nets]` `do_not_touch = []`, `power_pattern = "^(\\+|-)?\\d|^(V|VCC|VDD|VBAT|GND|AGND|PWR)"`, `priority = {}`, `pour_served = {}`, `ignore_pads = []`;
  `[vias]` `via_cost_mm = 2.0`, `max_per_connection = 4`, `reduction = true`, `reduction_len_slack_pct = 10`, `via_in_pad = false`, `allowed_drills_mm = []`, `fallback_drill_mm = 0.4`, `fallback_diameter_mm = 0.8`, `cost_by_priority = {}`, `cost_by_netclass = {}`, `via_relaxation = true`;
  `[layers]` `route = {}`, `preferred_direction = {}`, `direction_cost = 0.0`, `pour_cut_cost_mm = 1.0`;
  `[search]` `detour_factor = 2.0`, `margin_um = 5`, `zero_slack_retry = true`, `exit_stubs = "auto"`, `steiner = true`, `dangling_ends = "continue"`, `strict_unfit_edges = false`;
  `[ripup]` `growth = 2.0`, `max_per_pair = 8`;
  `[iterations]` `max = 30`, `patience = 5`, `min_gain_pct = 1.0`;
  `[output]` `geometry = "arcs"` (`arcs|segments`), `report_dir = "topor-report"`;
  `[expert]` `hist_fail = 1.0`, `hist_block = 0.5`, `hist_overuse = 0.5`, `present_overuse = 4.0`, `zero_slack_candidates = 4`, `relax_iter_cap = 200`, `drc_loop_cap = 5`;
  plus `KICAD_ONLY: list[(comment_key, kicad_place)]` (clearance, widths, via sizes / pre-defined vias, edge clearance, netclasses, custom rules, layer types, locks, via protection). Keys whose engine feature lands in M4a (`exit_stubs`, `steiner`, `dangling_ends`, `via_relaxation`, `preferred_direction`, `direction_cost`, `pour_cut_cost_mm`, `cost_by_*`, `allowed_drills_mm`, `via_in_pad`, `strict_unfit_edges`) carry `since = "0.5.0"` and are written as commented-out lines with "(available from 0.5.0)" until then; the engine-field test skips them while absent.
  `load_config(project_dir: Path) -> tuple[Config, list[Finding]]` (missing → `write_default` + `CFG_CREATED`; unknown key → `CFG_UNKNOWN_KEY`; type/range → `CFG_VALUE`; `version` ≠ 1 → `CFG_VERSION`); `Config.engine_options() -> dict` (mm → nm, s, `engine_field` mapping); `Config.non_defaults()`; `render_toml(cfg)` (every key preceded by `# <doc> [unit] default <d>, range <r>`; KiCad-only lines `# <name>: KiCad → <place> (not set here)`); `update_config(path, changes)` (keeps comments); `render_reference_md(SETTINGS)`; CLI `python -m topor.docs_gen --check`.

- [ ] **Step 1:** `test_default_file_roundtrips_to_defaults`, `test_unknown_key_warns_not_ignored`, `test_value_out_of_range_finding`, `test_engine_options_units` (`timeout_s 60 → wall_clock_s 60.0`, `via_cost_mm 2.0 → via_cost_nm 2_000_000`, `margin_um 5 → margin_nm 5000`, `timeout_s 0 → 0.0`), `test_update_config_keeps_comments`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_every_setting_documented`, `test_reference_md_matches_committed_file`, `test_every_engine_option_is_a_setting_or_internal` (`json.loads(engine.default_options()).keys()` ⊆ `{s.engine_field} ∪ ENGINE_INTERNAL`, `ENGINE_INTERNAL = {"pinned", "drc_spots", "skip_self_check", "profile", "seed", "scope", "net_priority", "via_cost_by_net", "detour_reference_budget_pct"}` with one-line reasons; every other `Options` field is a Setting's `engine_field`), `test_toml_comments_name_kicad_places`. Generate `docs/user/configuration.md`. Commit `feat(plugin): settings schema, topor.toml, generated config reference`.

