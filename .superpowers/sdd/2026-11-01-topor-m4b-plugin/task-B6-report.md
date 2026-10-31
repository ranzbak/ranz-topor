# Task B6 report: pre-flight checks and message catalogue

Status: DONE_WITH_CONCERNS. Branch `worktree-agent-a858727044806bf91` (fast-forwarded to main 520ea88 first; the worktree base was stale and lacked run.py).

## Changed
- `plugin/topor/messages.py` (new): `Msg(severity, cause, fix)`, `MESSAGES` (36 codes), `fix_for`, `render_troubleshoot`.
- `plugin/topor/preflight.py` (new): `preflight(model, rules, cfg, sel, orig_drc, cfg_findings, *, pcb_text, pro)`, `preexisting_drc`, `connection_count`, `outline_rings`. Pure, no mutation.
- `plugin/topor/run.py`: only the preflight seam. `preflight_findings` delegates to preflight; `connection_estimate` delegates; new `connections_by_scope` (between/touching/whole all carry the whole-board count until B7, documented); `_preexisting_finding` moved to `preflight.preexisting_drc` (error with `run.fail_on_preexisting`); unused imports removed; `sel` now read before preflight.
- `docs/user/troubleshoot.md` (new, generated from the catalogue; one `### CODE` per code; B11 extends).
- Tests: `tests/plugin/test_preflight.py` (31), `tests/plugin/test_messages.py` (5).

## Codes (default severity)
error: ENGINE_VERSION, KICAD_CLI_MISSING, PCB_FORMAT_OLD, INTERNAL, LAYER_ROLES_NEEDED, TRACK_ANGLE_RULE (PREEXISTING_DRC becomes error with run.fail_on_preexisting; DO_NOT_TOUCH / LOCKED_COPPER vary by case).
warning: CFG_WRITE, CFG_PARSE, CFG_VERSION, CFG_UNKNOWN_KEY, CFG_VALUE, CFG_VALUE_BACKLOG, PLANE_NO_ZONE, LAYER_TYPES, VIA_NOT_SET_UP, DRILL_OUTSIDE_FILTER, VIA_IN_PAD_UNPROTECTED, MARKER_IGNORED, MARKER_CONFLICT, MARKER_PAIR, PAD_OUTSIDE_BOARD, DO_NOT_TOUCH (unmatched pattern), SCOPE_EMPTY, NOTHING_ROUTED, PREVIEW_FAILED.
info: CFG_CREATED, POWER_BY_NAME, LOCKED_COPPER (respect_locked on), DANGLING_ENDS, PREEXISTING_DRC, NON_DEFAULTS, DEFAULTS_USED, GROUPS_UNAVAILABLE, CLEANUP_SKIPPED_PARTIAL, DO_NOT_TOUCH (matched).
Only in MESSAGES (emitted by B7): MARKER_IGNORED, MARKER_CONFLICT, MARKER_PAIR, POWER_BY_NAME.

## M4-R7 PLANE_NO_ZONE
Per plane layer (role gnd / pwr / power): needs a filled pour (no model `warning`, fill on that layer) whose net matches `nets.ground_pattern` (gnd), `nets.power_pattern` and not ground (pwr), either (power). Fix text exactly: "add a zone on In1.Cu with net GND covering the board outline (Place → Add Filled Zone), then refill zones (B), or change the role in topor.toml [layers]"; example net = first ground / power net, else "a ground net" / "a power net". Tests: no zone, GND zone ok, In2 pwr with GND zone (wrong kind), KiCad power either, unfilled zone, cfg ground_pattern, LAYER_ROLES_NEEDED through preflight for 3 layers.

## Tests
`.venv/bin/pytest -q tests/plugin` (shared venv): 148 passed, 7 skipped (10m39s, dominated by existing run-flow tests). `test_every_code_has_cause_and_fix` scans plugin/ and toporlib/ for `Finding("X"` / `_mk("X"` and requires each in MESSAGES; each fix must contain a KiCad `→` path or a `section.key`. `test_troubleshoot_has_section_per_code` + `test_troubleshoot_matches_catalogue`.

## UNVERIFIED (no live KiCad 10 file checked; synthetic tests only)
- Via protection read from the board file `(setup ...)` as `(capping yes)` and `(filling yes)`; the 46 doc names `m_FillVias/m_CapVias`. Not from `.kicad_pro`.
- VIA_NOT_SET_UP: no `board.design_settings.via_sizes_list` entry and no net class with `via_diameter`/`via_drill` other than factory 0.8/0.4 in `.kicad_pro`.
- GROUPS_UNAVAILABLE: top-level `(group` forms at line start in the board file.
- TRACK_ANGLE_RULE parsed from `rules["warnings"]` text `REFUSE: rule X restricts track angle|track segment length`.

## Concerns
- VIA_NOT_SET_UP says topor falls back to `vias.fallback_*`, but nothing in the plugin applies those values to the rules yet (the keys are unused anywhere in plugin/ before B6; rules still carry the JLCPCB-profile via sizes). The wiring is outside the seam; needs a follow-up (rules post-processing in resolve_rules or the engine option map).
- `connection_estimate` stays a spanning-tree estimate (pads - 1 per net), not exact connectivity; scope counts equal until B7.
- Rules `REFUSE ... disallows track/via without supported area` warnings are not surfaced as a finding (not in the brief).
- DO_NOT_TOUCH, LAYER_TYPES, GROUPS_UNAVAILABLE, SCOPE_EMPTY semantics were my interpretation of the brief's one-word codes.
