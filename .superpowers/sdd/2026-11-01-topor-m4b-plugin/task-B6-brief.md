### Task B6 (S): pre-flight checks and messages — `plugin/topor/preflight.py`, `messages.py`

**Files:** Create both; tests `tests/plugin/test_preflight.py`, `test_messages.py`.

**Interfaces:**
- Produces: `preflight(model, rules, cfg, sel: Selection, orig_drc: DrcReport, cfg_findings) -> list[Finding]`, codes (each in `MESSAGES` with cause + fix): `ENGINE_VERSION`, `KICAD_CLI_MISSING`, `CFG_CREATED/CFG_UNKNOWN_KEY/CFG_VALUE/CFG_VERSION/CFG_VALUE_BACKLOG`, `VIA_NOT_SET_UP` (warn; fallback 0.8/0.4; fix: Board Setup → Pre-defined Sizes → Vias or `[vias] fallback_*`), `DRILL_OUTSIDE_FILTER` (accept → `update_config`), `VIA_IN_PAD_UNPROTECTED` (`via_in_pad` while `.kicad_pro` via protection is not filled + capped — JSON key names UNVERIFIED, documented), `POWER_BY_NAME`, `MARKER_IGNORED/MARKER_CONFLICT/MARKER_PAIR` (B7), `PAD_OUTSIDE_BOARD` (pad centre outside the Edge.Cuts polygon; accept → `ignore_pads`), `PREEXISTING_DRC` (info; error when `fail_on_preexisting`; from `kicad-cli` on the untouched copy), `TRACK_ANGLE_RULE` (error), `LOCKED_COPPER`, `DANGLING_ENDS`, `DO_NOT_TOUCH`, `LAYER_TYPES`, `SCOPE_EMPTY`, `NON_DEFAULTS`, `DEFAULTS_USED`, `GROUPS_UNAVAILABLE`. `RunSummary` = connections in scope, nets, layers, threads, timeout.

- [ ] **Step 1:** one test per code on fixture models (pad 1 mm outside the outline; dru with `track_angle`; factory-default vias; `allowed_drills_mm = [0.3]` with a 0.4 drill). Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_every_code_has_cause_and_fix` (fix names a KiCad menu path or a `SETTINGS` key). Wire into `run.py`. Commit `feat(plugin): pre-flight findings and actionable messages`.

