### Task B9 (S, from the B4 wireframes): wx UI — `plugin/topor/ui_wx.py`

**Files:** Create `ui_wx.py`; tests `tests/plugin/test_ui_wx.py`. Before implementing, invoke `ecc:make-interfaces-feel-better` and `ecc:accessibility` via the Skill tool and follow the B4 note verbatim (wording table, focus order, keyboard map, sizes via `FromDIP`, system colours for dark theme).

- [ ] **Step 1:** `WxUi(UiPort)` implementing dialogs (1)–(6); `test_wx_ui_dialogs_construct_headless` (`wx.App(False)` with a virtual display if `DISPLAY` is unset is not required; skip without `wx`), `test_wx_ui_wording_matches_wireframes` (labels/buttons equal the B4 wording table parsed from the note). Commit `feat(plugin): wx dialogs from the reviewed wireframes`.

