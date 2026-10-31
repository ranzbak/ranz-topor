### Task R8 (S): M4-R8 conservative-direction parity + exact via free flag — `plugin/topor/model_ipc.py`, `toporlib/geom.py`, `toporlib/pcbfile.py`, `plugin/topor/scope.py`, `plugin/topor/schema.py`, tests

Main ruling M4-R8 (verbatim): IPC exporter approximations allowed for the M4b gate ONLY if conservative, and tests assert the direction (not bbox
equality): copper graphics/text → IPC obstacle CONTAINS the pcbnew shape (superset, 1 µm; text bbox ok, never smaller); fills
(Zone.filled_polygons exact) → polygon-equivalent within 1 µm (symmetric-difference area ≈ 0), report why if it fails; Via.free guess must never
rip a stitching/free via — unsure → fixed obstacle (tested); pour reach by probe points errs toward "not reached" (kicad-cli DRC stays the
oracle); pads exact (M4-R5); every remaining approximation listed in the user guide's limitations.

Coordinator rulings C23–C26 (global-constraints.md) are binding for this task. Concretely:
1. **Copper graphics (C25).** `toporlib.geom.capsule_ring` / `circle_ring` / `flatten_arc` are INSCRIBED today (chord error inward) → the IPC
   obstacle is smaller than KiCad's shape by up to max_error. Add circumscribed variants (keyword `outward=True` or new functions; do not change
   harness behaviour unless you show it is unaffected) and use them in `model_ipc.shape_to_polys` (stroked segments/arcs/beziers/circles/rects/
   polygons, filled circle/rect/polygon incl. half stroke). Offline test `tests/plugin/test_ipc_conservative.py` (tests MAY import pcbnew; plugin/
   toporlib may not — test_no_pcbnew must stay green): build kipy shape objects (protos, like tests/plugin/test_model_ipc.py does) from pcbnew shapes —
   every copper graphic on every snapshot board under scratch/boards/<board>/<sha12>/ (harness.paths) plus synthetic pcbnew shapes on an in-memory
   board (segment, arc, bezier, circle filled/unfilled, rectangle filled/unfilled, polygon filled/unfilled, several widths, rotated) — and assert
   pcbnew `TransformShapeToPolygon` (use SHAPE_POLY_SET boolean ops in the test for area math) minus (IPC union inflated by 1 µm) is empty
   (area ≤ ε), and IPC union minus (pcbnew inflated by max_error + 1 µm) is empty (bounded excess).
2. **Copper text (C25).** IPC uses `board.get_item_bounding_box(text)`. Offline: assert pcbnew's text `GetBoundingBox()` (what the server returns —
   mark UNVERIFIED) contains the text's polygon (TransformShapeToPolygon / text-to-polyset) within 1 µm on snapshot copper texts + synthetic texts
   (rotated, mirrored on B.Cu, bold/thick, KiCad font and if available an outline font). If not contained, grow the bbox in model_ipc by the minimal
   margin that makes the test pass (document why).
3. **Fills.** Offline: build `PolygonWithHoles` protos from pcbnew zone filled polygons on every snapshot board (refill a COPY via pcbnew ZONE_FILLER
   if needed) → `zone_to_pour` fills → assert polygon-equivalence with pcbnew within 1 µm (symmetric difference area ≈ 0). Report why if not.
4. **Pour reach (C26).** Offline: for every snapshot board, take the harness model (`harness.model.export_model`, pcbnew connectivity = pours[*].pads/vias)
   and feed its pads/vias/fills to `model_ipc.pour_reach`; assert IPC reached ⊆ pcbnew reached (per pour, pads and vias). Report the counts of
   "pcbnew reached, IPC not" per board (allowed, conservative). Any IPC-only reach = bug → make the rule stricter, never looser.
5. **Via free (C23).** IPC/kipy 0.8 has no free flag. Add `toporlib.pcbfile.via_free_flags(text) -> dict[uuid, bool]` (parse `(via … (free yes) …
   (uuid "…"))`; check the exact KiCad 10 token form on a pcbnew-re-saved snapshot copy; legacy `tstamp` too if cheap). `export_model_ipc` sets
   `free` from it (pcb_copy_text is already passed); uuid missing → free=True (unsure → fixed obstacle). Drop the "no track end touches" guess from
   the exporter. Test: on every snapshot board (re-saved through pcbnew into tmp so the text is KiCad 10), parsed flags == pcbnew `GetIsFree()` per
   via uuid; uuid-missing case → free True.
6. **rip_plan (C24).** New schema key `[run] keep_unconnected_vias` (bool, default true, doc: "Never rip a via that no track of its own net touches
   (stitching vias without KiCad's free flag); false rips them like routing vias."). `scope.rip_plan` keeps a via if `free` or (key true and no
   same-net kept track end lies within the via radius on a layer of its span). Pure, O(V + K) with a per-net index. Tests: stitching via without flag
   kept; flagged free via kept; routing via (track end on it) ripped; key false → untouched-unflagged via ripped. Regenerate
   docs/user/configuration.md (`PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen` then `--check` clean) and make test_config_docs green.
   Measure + report on eprom and wiccon (FakeBoard + harness model, existing_copper=rip, the run-flow fixtures in tests/plugin/test_run_flow.py):
   how many vias are now kept that were ripped before, and routed/total, vias, routing_new, unconnected for both boards (must stay eprom 27/27,
   wiccon 62/62, routing_new 0, unconnected 0 — if not, STOP and report numbers; do not tune).
7. **Live test.** Update `tests/plugin/test_ipc_parity.py` (live, skipped without KICAD_API_SOCKET) so its comparisons assert the same directions
   (copper shapes ⊇, text bbox ⊇, fills sym-diff ≈ 0, pour reach ⊆, free exact, pads M4-R5) instead of bbox/area equality; keep its offline self-tests green.
8. Update the module docstring of model_ipc.py: the list of remaining approximations (each with its direction). Put the same list, final, in your
   report under a heading "LIMITATIONS" (the docs worker's operate.md limitations section is reconciled with it by the coordinator).

Acceptance: `.venv/bin/pytest -q tests/plugin` green; `.venv/bin/pytest -q` default suite green (test_scaffold may flake only under load — rerun it
alone and report); `PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen --check` clean. Commit
`feat(plugin): M4-R8 conservative IPC approximations, exact via free flag, keep_unconnected_vias`.
