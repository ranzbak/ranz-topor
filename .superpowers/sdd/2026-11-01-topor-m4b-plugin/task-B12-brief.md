### Task B12 (S, gate review F): M4b gate — headless e2e, GUI checklist, parity, version 0.4.0, review

**Files:** `tests/gate/test_m4b_gate.py`, `engine/Cargo.toml` + `plugin.json` + `SHIPPED_VERSION` (0.4.0), `docs/progress/notes/m4b-gate-review.md`, `docs/design/45-mvp-plan.md` (M4 row: split + versions recorded), `README.md` status, STATE.

- [ ] **Step 1:** `test_m4b_gate_headless_eprom_and_wiccon` (marked `gate`): `FakeBoard` on snapshot copies with harness-built models, `ScriptedUi(accept=True)`, real engine: `status == "accepted"`, one commit, eprom 27/27 / wiccon 62/62 routed, `routing_new == 0` and `unconnected == 0` by `kicad-cli` on the committed copy, report dir written, `topor.toml` created, `docs_gen --check` clean; `test_m4b_gate_scope_selection`. PASS. `pytest tests/gate -m "gate or not gate"` green (M0–M3 + M4b).
- [ ] **Step 2 (user):** GUI checklist on eprom and wiccon copies incl. UI acceptance and `test_ipc_parity` on every snapshot board; results in STATE. Failures → fix task, re-run.
- [ ] **Step 3 (F):** review: port boundaries (kipy only in `ipc.py`/`model_ipc.py`; no pcbnew), every 46 setting present with its source order, message coverage, exception paths, undo atomicity, UI vs wireframes, docs vs behaviour; BLOCKING fixed → STATE "M4b DONE (0.4.0)". Commit `docs(state): M4b DONE (0.4.0)`.

