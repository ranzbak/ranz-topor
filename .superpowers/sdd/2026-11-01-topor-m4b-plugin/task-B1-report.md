# Task B1 report: settings schema, topor.toml, generated config reference

Status: DONE. Branch `worktree-agent-ab81dfe69b7e821bd`, commit `6b5906f`.
(Report written inside the worktree, uncommitted: the shared-checkout path was refused by the sandbox.)

## Files
- `plugin/topor/__init__.py` (`__version__ = "0.4.0"`)
- `plugin/topor/findings.py`: `Finding(code, severity, message, fix="", items=())` frozen dataclass (severity info|warning|error). B2/B6 reuse it (re-export from ports/messages is fine).
- `plugin/topor/schema.py`: `Setting`, `SETTINGS`, `SETTINGS_BY_KEY`, `SECTIONS`, `KICAD_ONLY`, `ENGINE_INTERNAL` (name -> reason), `BACKLOG_VALUES`, `available(setting)`, `CONFIG_VERSION=1`, `CURRENT_VERSION="0.4.0"`.
- `plugin/topor/config.py`: `Config`, `load_config`, `update_config`, `render_toml`, `write_default`, `coerce`, `toml_value`.
- `plugin/topor/docs_gen.py`: `render_reference_md(settings)`, CLI.
- `docs/user/configuration.md` (generated), `tests/plugin/conftest.py`, `tests/plugin/test_config.py`, `tests/plugin/test_config_docs.py`.

## Public API
- `Setting(key, section, type, default, unit="", range=None, doc="", kicad_place=None, engine_field=None, harness_field=None, plugin_field=None, choices=None, elem=None, since=None)`. type: int|float|bool|str|list|table; `range` (lo, hi); `choices` for enums; `elem` list element type; `since="0.5.0"` = M4a feature (commented out in the file until CURRENT_VERSION >= since; omitted from engine_options()). Settings applied by the plugin carry `plugin_field="section.key"`.
- `Config.values: dict[(section,key)] -> value` (always complete); `Config.defaults()`, `.get(section, key)`, `.with_values({(sec,key): v})`, `.non_defaults() -> {"sec.key": v}`, `.engine_options() -> dict`, `.report_dir_for(project_dir: Path, project_name: str, timestamp: str) -> Path` (ruling C1).
- `load_config(project_dir: Path) -> tuple[Config, list[Finding]]`. Findings: CFG_CREATED (info), CFG_WRITE, CFG_PARSE, CFG_VERSION, CFG_UNKNOWN_KEY, CFG_VALUE, CFG_VALUE_BACKLOG (`output.geometry="45deg"`); invalid value -> default used; every finding has a fix. B6 must add these codes to messages.py / troubleshoot.md.
- `update_config(path: Path, changes: dict["section.key", value]) -> None`: validates first (ValueError), line-edits (keeps comments and user edits, uncomments `# key = ...`, appends missing keys with doc comment), atomic write.
- `render_toml(cfg) -> str`, `write_default(path)`, `render_reference_md(SETTINGS) -> str`. `timeout_s` also accepts `"none"` (= 0).
- `engine_options()` keys (defaults): wall_clock_s 60.0, threads 0, partial_routes False, existing_copper "fixed", via_cost_nm 2000000, max_vias_per_connection 4, via_reduction True, detour_factor 2.0, margin_nm 5000, zero_slack_retry True, ripup_growth 2.0, ripup_max_per_pair 8, pathfinder_iter_cap 30, patience 5, min_gain_pct 1.0, drc_loop_cap 5, expert {hist_fail, hist_block, hist_overuse, present_overuse, zero_slack_candidates, relax_iter_cap}.

## Decisions
- `relax_iter_cap` mapped as nested `expert.relax_iter_cap` (coordinator text); `drc_loop_cap` top-level. If P1 keeps `relax_iter_cap` top-level, change its `engine=` in schema.py; the engine-field test reports mismatches in both directions.
- `report_dir` default "" per ruling C1 (semantics in the doc and `report_dir_for`, tested with TOPOR_REPORT_ROOT / XDG_CACHE_HOME / HOME in tmp).
- since 0.5.0: exit_stubs, steiner, dangling_ends, via_relaxation, preferred_direction, direction_cost, pour_cut_cost_mm, cost_by_priority, cost_by_netclass, allowed_drills_mm, via_in_pad, strict_unfit_edges.
- UNVERIFIED: choices `dangling_ends continue|drop`, `exit_stubs auto|on|off` are my picks (brief gave only defaults).

## Tests
- `.venv/bin/pytest -q tests/plugin` -> 19 passed, 1 xfailed (engine-option test: `topor_engine.default_options` missing -> pytest.xfail; skipped when wheel absent).
- Docs check: `PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen --check` (cwd-independent; `--write` regenerates); covered by `test_cli_check_passes`.
- Full default suite started but slow (>120 s, benchmark-heavy); only new files were added. UNVERIFIED: full-suite result.

## Concerns
- `vias.reduction_len_slack_pct` has no engine field in the Options list I had; documented "not yet applied by the engine". Needs a P1/M4a decision.
- Commit attribution uses the session reminder line (Sonnet 5.5) rather than the contract's Opus line.

## Follow-up (M4-R7, C13, C14)
- ENGINE_INTERNAL += relax_iter_cap (legacy alias); `vias.reduction_len_slack_pct` now since 0.5.0; exit_stubs choices auto|all|off; new `nets.ground_pattern`; `layers.route` removed.
- `[layers]` roles: loader flattens bare `F.Cu`/`In1.Cu` ({F:{Cu:v}}) and quoted keys into `Config.layer_roles`; non-.Cu unknown keys -> CFG_UNKNOWN_KEY; bad role -> CFG_VALUE. render_toml writes roles (or a commented example block).
- New `plugin/topor/layers.py`: `resolve_layer_roles(copper, cfg_roles) -> (roles, findings)`, `resolve_layer_roles_ex(...) -> (roles, sources, findings)` (sources: config|kicad|default per layer), `is_routed`, `ROLES`. KiCad "power" type stays role "power". Finding code LAYER_ROLES_NEEDED (error) listed in the module header. O(L).
- Tests: tests/plugin/test_layers.py; `pytest -q tests/plugin` 36 passed, no xfail; docs_gen --check clean.
