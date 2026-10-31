### Task B7 (S): routing scope from the selection, copper/lock policies, priorities (`@N`, `topor_pN`, power pattern) — `plugin/topor/scope.py`, `priority.py`

**Files:** Create both; tests `tests/plugin/test_scope.py`, `test_priority.py`.

**Interfaces:**
- Produces: `scope_from_selection(sel, model, cfg) -> ScopePlan` (`engine_scope` = `Scope {pads, footprints, nets, netclasses, area, unrouted_only}`; `between` = connections whose pads are all on selected footprints/pads, `touching` = at least one; empty selection → whole board); `rip_plan(model, plan, cfg) -> RipPlan` (`remove_uuids` by `existing_copper`: `fixed` none; `rip` in-scope nets' tracks/vias except locked when `respect_locked` and except `do_not_touch`; `smooth` → engine `existing_copper = "smooth"`).
  `parse_marker(net_name) -> int | None` (46 §2.1); `net_priorities(nets, netclass_of, cfg) -> (dict[str, int], list[Finding])` (marker > `topor_pN` > `[nets] priority` > none; conflicts → `MARKER_CONFLICT` marker wins; merged index → `MARKER_IGNORED`; diff-pair halves → `MARKER_PAIR` higher wins; power nets by netclass name containing `power` or `power_pattern` for nets in `Default` → tier 2 unless set, `POWER_BY_NAME`).

- [ ] **Step 1:** `test_marker_parse_table` (`USB_D@1+`→1, `CLK@1_P`→1, `DQ@1_3`→1, `DQ@13`→None + ignored, `/ddr/DQ@1_0`→1, `~{RESET}@2`→2, `~{RESET@2}`→2, `A@4`→None, `@2`→2), `test_priority_sources_and_conflicts`, `test_power_pattern_only_for_default_netclass`. Run → FAIL. Implement. PASS.
- [ ] **Step 2:** `test_scope_between_vs_touching`, `test_scope_pads_two_pins`, `test_scope_area_polygon`, `test_scope_unrouted_only_keeps_copper`, `test_rip_plan_policies`. Wire into `run.py`. Commit `feat(plugin): scope from selection, copper policies, @N priorities`.

