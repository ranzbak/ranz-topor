# Task PY report

Note: the worktree started on an old base (68a77b6); fast-forwarded to main bde9602 (= 789e050 + briefs) first.

## 1. test_keys_without_effect_say_so (commit test(config))
Premise obsolete: A9/A7 made every no-effect key live, so `md.count(NO_EFFECT) >= 1` can never hold. Rewritten to keep
the rule: the set of docs carrying the marker == the set of live keys with no consumer (no engine_field, never read in
plugin/ or toporlib/); marked keys end with the marker and stay live; PARTIAL statements unchanged; marker count in
configuration.md and rendered topor.toml == number of marked keys (0 now); detour_reference assertion kept.
Doc strings still saying "0.4.0" (version bump = A15, not done): plugin/topor/schema.py:13 CURRENT_VERSION, :71
existing_copper ("smooth is treated as fixed in 0.4.0", still true, PARTIAL-tested), :171 own_net_capacity ("net-blind
0.4.0 rule", historical, fine); plugin/requirements.txt, plugin/topor_plugin.py SHIPPED_VERSION, plugin/topor/__init__.py,
plugin/plugin.json; docs/user: install.md (title, wheel names, requirements), troubleshoot.md:51,54,67, operate.md (title,
lines 30,40,57,84,159,279,289: pre-defined vias "not routed with in 0.4.0", floors, smooth, etc.; several statements
(pre-defined vias, existing_copper smooth) may be wrong at 0.5.0 after A10), configuration.md:18,135, gui-test-checklist.md.
test_config_docs.py: 11 passed. docs_gen --check clean.

## 2. test_progress_m4_name_and_caption_lines (commit fix(progress))
Root cause: test premise `columns["detour"] is None until A7` and `detour_ghost == result["detour"]`. Since A7
`result["detour"]` is the DRC-feasible reference stats (field ref_len_nm) and `detour_ghost` the ghost stats. Code is
right; test updated: columns mirror the result (detour == res[detour], detour_ghost == res[detour_ghost]), detour
has median/p90/max/count_gt_1_5, fields ref_len_nm / ghost_len_nm. Passes (91 s under load 30+).

## 3. wiccon 63 vs 62: ENGINE root cause, no Python change (stopped per brief)
Evidence (script in scratchpad: runs the test flow with a capturing engine wrapper, then routes both models):
- Model sent by the plugin flow vs harness `export_model` after rip: identical JSON except the plugin-only
  `via_fallback` key. So the plugin model/pad merge/pour/ignore_pads/refill match the harness. No plugin bug.
- Options differ (default_options vs flow): `detour_reference: true` and `net_priority {+4V:2, 3.3v:2, 4v:2, GND:2}`.
- Bisect on the harness model with 60 s threads 4: default opts 62/62 (no gain); + detour_reference 62/62; + net_priority
  63 total, routed 62, failed 1, stop "converged". The extra connection id 62 is net GND via->pour
  (via 0b5b6ef8..., at 127.5,119.18 mm), status failed, reason
  `self-check: viahole:topor:249 (gap 304999 < 350000 nm)`: the engine self-check rejects a via-to-hole gap 5001 nm
  short of the 0.35 mm hole clearance after the priority reorders routing, then adds a failed via->pour connection.
So with plugin priorities wiccon is 62 routed of 63, failed 1 (not 62/62): the same defect fails the M4b gate headless
wiccon. Fix belongs in the engine (self-check / via-hole clearance vs net_priority ordering); handed back.
Still red: tests/plugin/test_run_flow.py::test_run_wiccon_refill_gives_harness_connection_count,
tests/gate/test_m4b_gate.py headless wiccon (same cause).
