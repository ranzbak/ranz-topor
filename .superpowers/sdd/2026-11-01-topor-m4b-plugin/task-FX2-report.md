# Task FX2 report: M4b gate F-review fixes (pre-flight, schema docs, guard, docs)

Branch `worktree-agent-ac41ce8ebfa08e312`, based on main 369e029 (fast-forwarded). No Rust touched, .venv not touched,
`tools/build.sh --dev` not run (build test runs build.sh into a pytest tmp KiCad home only).

## Commits

| Commit | Item(s) |
|---|---|
| bac2e02 `fix(plugin): VIA_NOT_SET_UP reads via_dimensions and states the real via sizes` | I-3 (C33), I-4 message, MINOR 8 |
| a4bb579 `docs(config): keys without effect in 0.4.0 say so; timeout_s is router time` | I-7 (C34), MINOR 4, I-4 (toml texts) |
| 5b2b8d1 `fix(toporlib): through vias default to F.Cu / B.Cu on 4+ layer boards` | MINOR 1 |
| bff17a0 `test(plugin): no-pcbnew guard walks every topor module and forbids harness` | MINOR 2 |
| 220ec06 `fix(build): TOPOR_KICAD_HOME override; KICAD_DOCUMENTS_HOME as KiCad reads it` | MINOR 9 |
| e63879b `feat(plugin): run report lists the fab floors that raised design rules` | I-4 (report) |
| 7a43ebe `docs(user): fab floors and via sizes, timeout, Help, detours, run limits` | I-4 docs, MINOR 4, 5, 7, 8, 10, 11 |
| 38900f7 `test(plugin): live IPC parity compares keepouts by containment, refilled copy` | MINOR 14 |
| (this report) | |

## Per file

- `plugin/topor/preflight.py`: `via_setup_missing` reads `board.design_settings.via_dimensions`; an entry with
  `diameter > 0` = pre-defined via (the `{0, 0}` placeholder is not); the net-class check is unchanged. The
  invented `via_sizes_list` is no longer read (test pins that). Module docstring corrected (C33 moved out of the
  UNVERIFIED list: it is tested on the six real projects). New pure `smaller_profile_vias(rules, d, drill)` mirrors
  `engine/src/vias.rs via_sizes` (routing_vias drills in `[min_via_drill, drill)`, `d = max(drill + 2·min_annular,
  min_via_diameter)`, kept if `d < net d`; O(R log R)); the VIA_NOT_SET_UP finding message now says
  "the nets without a via size of their own get 0.8 / 0.4 mm vias (diameter / drill, vias.fallback_*); where space
  requires, also the smaller fab-profile via(s) 0.5 / 0.3 mm" (sizes computed from the run's rules, fallback clamped
  to the board minimums exactly like `run.apply_via_fallback`).
- `plugin/topor/messages.py`: VIA_NOT_SET_UP cause/fix rewritten to what happens (fallback via per net with the
  factory/unset via, smaller fab-profile vias, pre-defined vias clear the finding but are not routed with in 0.4.0
  — from 0.5.0; fix points at Net Classes first). DO_NOT_TOUCH fix: "KiCad → View → Panels → Net Inspector lists
  them" (path UNVERIFIED, checklist row added). `docs/user/troubleshoot.md` Findings part regenerated
  (`render_troubleshoot`).
- `plugin/topor/schema.py` (+ regenerated `docs/user/configuration.md`, `docs_gen --check` clean):
  - `[run] detour_reference`, `[nets] pour_served`: doc ends "(No effect in 0.4.0; applied from 0.5.0.)" — verified:
    no reader in plugin/, toporlib/, engine/src (grep).
  - `[nets] priority`: ends "(In 0.4.0 priorities are computed and reported, but the routing order does not follow
    them yet; from 0.5.0.)" — verified: `priority.net_priorities` computes them, MARKER_* findings report conflicts,
    `build_options` passes `net_priority` only if the engine knows the key (it does not). test_priority order test green
    (order statement untouched).
  - `[nets] ignore_pads`: ends "(In 0.4.0 applied to a footprint / pad selection scope only: a whole-board run still
    routes to these pads; whole board from 0.5.0.)" — verified: consumers are `scope._compute` (selection scope pads),
    preflight PAD_OUTSIDE_BOARD suppression, report; `scope_from_selection` whole-board scope has no pad list and the
    engine has no `ignore_pads`.
  - `[run] existing_copper`: ends "(smooth is treated as fixed in 0.4.0.)" — verified `engine/src/route.rs` route_pass
    warning "existing_copper=smooth ignored by route(); treated as fixed".
  - `[run] timeout_s`: "Limit on router time: all routing passes of the run together stop after this long; the
    pre-flight DRC, the DRC rounds and the kicad-cli refill / preview exports are not counted. …" — verified
    `toporlib/drcloop.py` charges only `route_fn` time (`router_s`), rounds > 1 get `budget_s - router_s`.
  - `[vias]` section text and the KiCad-only rows "Via sizes" (Net Classes; pre-defined vias not routed with before
    0.5.0) / "Minimum via, hole and annular ring" (raised to topor's fab floors) — so topor.toml itself states I-4.
  - ENGINE_INTERNAL reason of `detour_reference_budget_pct`: "will switch it (0.5.0)" (dev-facing, was a false claim).
- `toporlib/pcbfile.py`: an engine via without `layers` gets the top-down first/last copper (`_top_down`: 0, 4, 6, …,
  2) = F.Cu / B.Cu, as `ipc.to_kipy_item` (through via); explicit layers unchanged. Complexity note added.
- `plugin/topor/report.py`: report.md Settings line "Fab floors raised (JLCPCB 1 oz profile): min_track_width
  0.15 → 0.2 mm, …" (or "none") when the `floors_applied` fact is present.
- `plugin/topor/run.py` (FX1's file — ONE line, right after `rules = resolve_rules(...)`):
  `report.facts["floors_applied"] = list(rules.get("floors_applied") or [])`. Needed so the report shows the floors
  (they were nowhere visible before; the brief asks "where the run report shows floors_applied"). Merge note.
- `tools/build.sh`: `TOPOR_KICAD_HOME` (versioned dir, `…/kicad/10.0`) wins; else `KICAD_DOCUMENTS_HOME` is KiCad's
  unversioned root and `10.0` is appended; else `${XDG_DATA_HOME:-~/.local/share}/kicad/10.0`. Header documents it.
- `docs/user/install.md`, `CONTRIBUTING.md`: the two variables described as above.
- `docs/user/operate.md` (not the "Accept, Reject and undo" section):
  - new "### Fab minimums and via sizes" (under "Where the settings come from"): JLCPCB 1 oz floors table with the
    `.kicad_pro` rule keys (clearance 0.15, track 0.2, via d 0.45, via drill 0.25, annular 0.1 / 0.075 (4+ layers),
    copper-to-hole 0.35, hole-to-hole 0.25, edge 0.2 mm); unset (0) silently takes the floor; raised values listed in
    report.md → Settings → Fab floors raised; per-net clearance/width/via raised too; the engine's via list
    (`vias.rs via_sizes`, drills 0.25, 0.3, 0.4, 0.5 mm; wiccon example 0.8/0.4 + 0.5/0.3); pre-defined vias
    (`via_dimensions`) NOT read in 0.4.0 (0.5.0, A10), only decide VIA_NOT_SET_UP; **no topor.toml key turns the floors
    off** (`apply_fab_floors` is a harness-only switch; not added). KiCad-settings table rows updated accordingly.
  - Time limit row + Progress section: router time only (MINOR 4).
  - Help: opens operate.md in the desktop's default program for `.md` files, at the top of the file (MINOR 5; code
    unchanged, `#options` anchor claim dropped).
  - Limitations: detour ratio = routed length / topological ghost length (MINOR 7); new "### While topor runs" with
    the two brief sentences (MINOR 10, 11).
- `docs/user/gui-test-checklist.md`: R3 cancel-latency wording + the latency check line; R11 Help (default .md
  viewer, no section jump, record the program); menu-path row: Board Statistics example removed, new UNVERIFIED item
  "View → Panels → Net Inspector"; VIA_NOT_SET_UP row rewritten per C33 (fires on wiccon, not on eprom; adding a
  pre-defined via on the wiccon copy clears it).
- Tests:
  - `tests/plugin/test_preflight.py`: real-structure `PRO_VIAS` (placeholder + 0.6/0.3, factory class);
    `test_via_predefined_in_via_dimensions_no_finding` (the review's probe), `test_via_dimensions_placeholder_only_fires`,
    `test_via_sizes_list_is_not_a_kicad_key`, `test_via_not_set_up_says_what_happens` (0.8/0.4 + 0.5/0.3; fallback
    1.0/0.5 → + 0.6/0.4, 0.5/0.3; no "smaller" without routing_vias), `test_via_setup_on_snapshot_projects[6 boards]`.
  - `tests/plugin/test_run_flow.py`: factory_eprom fixture sets `via_dimensions` to KiCad's placeholder instead of
    popping the made-up key; `test_via_sizes_set_up_leaves_rules_unchanged` also asserts eprom's report lists
    `min_track_width 0.15 → 0.2 mm` and `hole_clearance 0.25 → 0.35 mm`.
  - `tests/plugin/test_messages.py`: DO_NOT_TOUCH names the Net Inspector; VIA_NOT_SET_UP cause/fix content.
  - `tests/plugin/test_config_docs.py`: `test_keys_without_effect_say_so`, `test_live_plugin_keys_have_a_consumer_or_the_marker`
    (review's suggested test: every live key with `engine_field=None` is quoted in a plugin/toporlib consumer module or
    carries the marker — and a marked key must have no consumer), `test_timeout_doc_says_what_is_charged`.
    `test_every_setting_documented` now accepts a doc ending ".)" (the C34 marker ends with a parenthesis) — format
    adaptation, the "ends with a sentence" check stays.
  - `tests/plugin/test_pcbfile.py`: `test_append_via_without_layers_is_through_on_4_layers`.
  - `tests/plugin/test_no_pcbnew.py`: `FORBIDDEN = ("pcbnew", "harness")` in the AST scan (+ scanner self-tests); the
    probe blocks both and walks `topor` with `pkgutil.walk_packages` (ui_wx only if wx is importable); asserts
    model_ipc, docs_gen, run, preflight, report are imported.
  - `tests/plugin/test_app.py::test_build_layout_in_scratch_kicad_home`: build with `KICAD_DOCUMENTS_HOME=<tmp>/kdocs`
    → `<tmp>/kdocs/10.0/plugins/org.topor.router` (full layout checks), then `TOPOR_KICAD_HOME=<tmp>/kicad10` +
    `KICAD_DOCUMENTS_HOME=<tmp>/other` → installs into kicad10, `other` not created; real plugin dir untouched.
    The env drops any inherited TOPOR_KICAD_HOME / KICAD_DOCUMENTS_HOME.
  - `tests/plugin/test_report.py::test_report_settings_list_fab_floors`.
  - `tests/test_docs_links.py::test_operate_md_fab_floors_match_the_profile`: the floor values in operate.md are the
    `JLCPCB_1OZ` values, routing_vias listed, report line / "no topor.toml key" / 0.5.0 / VIA_NOT_SET_UP named,
    Limitations has the two run-time sentences.
  - `tests/plugin/test_ipc_parity.py`: keepouts by containment per (name, copper layers, no_tracks, no_vias) via
    `_keepout_differences` (harness ∖ inflated(IPC, 1 µm) ≤ 1 µm²); `refilled_copy()` = copy_project + pcbnew
    ZONE_FILLER + SaveBoard + reload, used by the live test for the harness model and as the text given to
    `export_model_ipc`; offline self-tests `test_compare_models_keepouts_by_containment`,
    `test_refilled_copy_has_fills_and_leaves_the_snapshot` (wiccon; snapshot bytes unchanged). Live test itself not
    run (needs KiCad; user-run).

## VIA_NOT_SET_UP on the six snapshot projects (C33)

| Board | via_dimensions | net classes | fires |
|---|---|---|---|
| bornhack2026 | placeholder + 0.35/0.25, 0.45/0.3, 0.5/0.3, 0.8/0.4 | Default 0.6/0.3, … | no |
| private-badge-b | placeholder + 0.25/0.15, 0.4/0.2, 0.8/0.4 | Default 0.6/0.3 | no |
| nametag-sao | placeholder + 0.35/0.2, 0.45/0.3, 0.5/0.3, 0.8/0.4 | Default 0.8/0.4, MIPI 0.35/0.2, … | no |
| eprom-to-kernal | `[]` | Default 0.8/0.4, Signal 0.6/0.3, power 0.8/0.4 | no (Signal is non-factory) |
| wiccon-2023 | placeholder only | Default 0.8/0.4 | **yes** |
| wedding-gift | `[]` | Default 0.8/0.4 | **yes** |

Same outcome as before the fix on these six (as the review said); the fix matters for boards with pre-defined vias but
factory net classes.

## Tests

- Focused while iterating (each failing first where code changed): test_preflight 46 passed, test_messages,
  test_config_docs, test_pcbfile 9, test_no_pcbnew 4, test_app build test 1 (24 s), test_report 3,
  test_run_flow `-k via` 4 + the eprom one, test_ipc_parity 6 passed / 6 skipped (live), test_docs_links 15.
- `PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen --check` → "configuration reference is up to date".
- Default suite `.venv/bin/pytest -q` (includes tests/plugin and tests/test_docs_links.py): **426 passed, 7 skipped, 26 deselected** in 917 s.

## Decisions

- Floors visible in the report: they were not shown anywhere (no reader of `floors_applied` in plugin/), so the
  documented "where the report shows it" needed the report line + one run.py line (FX1's file; minimal, distinct spot).
- The finding computes the actual extra via sizes instead of a generic "smaller vias" so the user sees exact sizes;
  the helper mirrors the engine rule and documents that (A10 in 0.5.0 must update both).
- MINOR 11 sentence kept verbatim ("…when you pressed Route") plus "(precisely: when you clicked **Topor: route**,
  when topor saved its copy of the board)", because run.py saves the copy before the pre-flight dialog.
- `[vias] fallback_*` keep their kicad_place (Pre-defined Sizes → Vias): that place does influence whether the
  fallback applies (C33).

## UNVERIFIED

- KiCad 10 menu path View → Panels → Net Inspector (checklist item).
- What `wx.LaunchDefaultBrowser(file://…operate.md#options)` opens on the user's desktop (checklist R11 records it).
- Live `test_ipc_parity` with the refilled copy and keepout containment (needs a running KiCad; user-run).
- KiCad 10 Constraints field labels used in the floors table (the `.kicad_pro` keys are exact).

## Concerns / merge notes

1. run.py: one added line after `resolve_rules` (FX1 owns run.py). messages.py / schema.py: FX1 adds codes / a key →
   after merging, regenerate troubleshoot.md (render_troubleshoot) and configuration.md (docs_gen --write).
2. `test_live_plugin_keys_have_a_consumer_or_the_marker` requires FX1's new `[run] refill_after_accept` to be read
   somewhere in plugin/topor (as `"refill_after_accept"`), which C28 implies.
3. `smaller_profile_vias` duplicates the engine's via-size rule; it must follow A10 (0.5.0) changes.
