### Task FX2 (S): M4b gate F-review fixes — pre-flight, schema docs, guard, docs — `plugin/topor/{preflight,messages,schema}.py`, `toporlib/pcbfile.py`, `tools/build.sh`, `tests/`, `docs/user/`

Review: /home/paul/work/kicad/plugins/topor/docs/progress/notes/m4b-gate-review.md (read the findings named here in full).
Coordinator rulings C33, C34 in global-constraints.md are binding. Fix, each with a test where code changes:
1. **I-3 (C33)** `preflight` via-setup detection reads `board.design_settings.via_dimensions` (entries with diameter > 0; the `{0,0}` first entry is a
   placeholder) instead of the non-existent `via_sizes_list`, plus the existing net-class check. Tests on the six snapshot .kicad_pro files under
   scratch/boards/<board>/<sha12>/ (bornhack2026, private-badge-b, nametag-sao have pre-defined vias → no finding; wiccon-2023 placeholder only, eprom and
   wedding-gift empty lists → finding iff no net class has a non-factory via size — report which fire) and fix the synthetic tests that used the made-up key.
2. **I-4** docs: operate.md gets a section on fab minimums: the rule resolver applies the JLCPCB 1 oz profile floors (toporlib/rules.py JLCPCB_1OZ,
   `apply_fab_floors`) — list the values, which design values are raised, where the run report shows `floors_applied`, and that the engine's
   routing via sizes come from (check engine/src/vias.rs `routing_vias` + rules) — describe exactly what the code does; whether KiCad pre-defined
   vias are read (they are not in 0.4.0 → say so; A10 in 0.5.0). Reword the VIA_NOT_SET_UP message (messages.py + troubleshoot.md) so it states
   what really happens. Is there a topor.toml key to turn the floors off? If yes document it; if not, say so (do not add one).
3. **I-7 (C34)** schema.py doc strings of keys without effect in 0.4.0 end with "(No effect in 0.4.0; applied from 0.5.0.)" or the precise partial
   statement: `[nets] pour_served`, `[run] detour_reference`, `[nets] priority` (computed and reported, routing order not yet), `[nets] ignore_pads`
   (selection scope only), `existing_copper` choice `smooth` ("treated as fixed in 0.4.0"). Verify each claim against the code first (grep the
   consumers); regenerate docs/user/configuration.md (`PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen` then `--check`). Keep test_priority's
   wording test green (adjust only if the new suffix changes the pinned text, never the order statement).
4. MINOR 1: `toporlib/pcbfile.py:107` via layer default → top-down first/last copper (F.Cu, B.Cu) as `to_kipy_item` does; test on a 4-layer layer list.
5. MINOR 2: tests/plugin/test_no_pcbnew.py imports every `topor` submodule via `pkgutil.walk_packages` (as for toporlib) and the AST scan also
   forbids `harness` imports under plugin/ and toporlib/.
6. MINOR 4: `[run] timeout_s` doc string → what is really charged ("router time; the pre-flight DRC, DRC rounds and kicad-cli exports are not
   counted") — verify in toporlib/drcloop.py; operate.md wording likewise.
7. MINOR 5: Help opens operate.md as a file: drop the claim that it jumps to `#options` from docs/checklist (keep the code as is) and say the guide
   opens in the default viewer for .md files.
8. MINOR 7: operate.md detour wording → routed length / topological ghost length (toporlib/detour.py).
9. MINOR 8: DO_NOT_TOUCH fix text names the Net Inspector (KiCad 10: View → Panels → Net Inspector — mark the exact path UNVERIFIED in the
   checklist's menu-path row) instead of Board Statistics; troubleshoot.md in sync (test_messages keeps them equal).
10. MINOR 9: tools/build.sh: honour `TOPOR_KICAD_HOME` (versioned dir, `…/kicad/10.0`) as the override; if `KICAD_DOCUMENTS_HOME` is set treat it
    as KiCad does (unversioned root) and append `10.0`; update tests/plugin/test_app.py::test_build_layout_in_scratch_kicad_home and install.md.
11. MINOR 10 + 11 docs only: operate.md Limitations — "Cancel reacts within about a second while routing; during KiCad DRC / refill / preview
    export (5–20 s each) the dialog may not respond"; "Do not edit the board while topor runs; Accept commits the result computed from the board
    as it was when you pressed Route." Checklist row R3 cancel latency wording likewise.
12. MINOR 14: tests/plugin/test_ipc_parity.py (live): keepouts compared by containment (IPC ⊇ harness within 1 µm) like copper shapes; the harness
    model is built from a refilled copy (pcbnew ZONE_FILLER on a tmp copy) so fills compare like with like. Offline self-tests stay green.
Parallel worker FX1 edits run.py, app.py, ipc.py, model_ipc.py, engine/, messages.py (new codes), schema.py (new key refill_after_accept),
operate.md ("Accept, Reject and undo"). Keep your edits in those files to the items above; the coordinator merges and regenerates.
Acceptance: `.venv/bin/pytest -q tests/plugin tests/test_docs_links.py` green; default `.venv/bin/pytest -q` green; docs_gen --check clean.
Commits conventional, e.g. `fix(plugin): VIA_NOT_SET_UP reads via_dimensions` / `docs(user): …`.
