### Task B8 (S): run report — `plugin/topor/report.py`

**Files:** Create `report.py`; tests `tests/plugin/test_report.py`. Layout per the B4 wireframe (5).

- [ ] **Step 1:** `build_report(result, drc, cfg, findings, scope, before_stats, timing) -> Report` (summary: routed/total, nets, partial/failed with reasons and blocking points, vias per net vs current board, length vs current, stop reason, elapsed, threads; DRC `routing_new` by type / `preexisting` / unconnected; detours; zero-slack; vias in pads; partial routes; ignored pads; accepted fallbacks; non-defaults and defaults used; findings), `Report.summary_lines() ≤ 20`, `to_markdown()`, `write_report(dir, …)`. Tests `test_report_sections_present_and_values`, `test_report_summary_lines_le_20`. Commit `feat(plugin): run report`.

