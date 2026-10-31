# Task B10 report: entry point, app wiring, headless, build/install (0.4.0)

## Changed
- `plugin/topor/app.py` (new): `main(argv, *, shipped_version=None, engine=None)`. Handshake first (`ENGINE_VERSION` Finding, fix "Restart the PCB Editor or use Recreate Plugin Environment.", exit 1; GUI mode also shows it via `WxUi.error` and writes the crash log). GUI: `wx.App(False)`, `WxUi`, `KiCad(socket_path=env KICAD_API_SOCKET, kicad_token=env KICAD_API_TOKEN, client_name="topor")` (kipy 0.8 signature checked: `socket_path, client_name, kicad_token, timeout_ms`), `get_board()`, `IpcBoard`, `run(...)`. Exit 0 for accepted/rejected/cancelled/aborted_preflight, 1 for error. Every `BaseException` -> stderr + `<report root>/topor-crash.log` + exit 1. `--headless <pcb> --model <json> [--accept] [--selection JSON]` runs FakeBoard + ScriptedUi; uses `topor.toml` next to the board if present, else `Config.defaults()` (so no toml is written beside the board). Module docstring documents `python -m topor.app --headless ...`.
- `plugin/topor_plugin.py`: puts its own dir on `sys.path`, calls `topor.app.main(argv, shipped_version=SHIPPED_VERSION)`; SHIPPED_VERSION 0.4.0.
- `plugin/plugin.json`: 0.4.0, `min_version` "3.11", action description "Route the board or the selection with Topor".
- `tools/build.sh`: ships `topor/` (plugin/topor), `toporlib/`, `docs/user/` (empty dir until B11), strips `__pycache__`; no `harness/`.
- Ruling C9 bump: `engine/Cargo.toml`, `Cargo.lock`, `plugin/requirements.txt` (`topor-engine==0.4.0`), plugin.json, SHIPPED_VERSION. No test pinned 0.3.0 (test_scaffold compares the files to each other). `maturin develop` run: engine 0.4.0 in the shared .venv.
- Tests: `tests/plugin/test_app.py` (10: handshake, mismatch exit 1, BaseException exit 1 + crash log, usage exit 2, selection JSON, `test_headless_run_on_fixture_accepts` eprom 27/27 accepted, reject exit 0, docstring usage, plugin.json fields, `test_build_layout_in_scratch_kicad_home`); `test_no_pcbnew` now also asserts `topor.app` is imported with pcbnew blocked.

## Tests
- `pytest -q tests/plugin/test_app.py tests/plugin/test_no_pcbnew.py tests/test_scaffold.py`: green (after the fixture fixes below).
- Full default suite: 276 passed, 7 skipped, 22 deselected in 702 s.

## Decisions
- Headless fixture needs `existing_copper = "rip"`: the shipped eprom is hand-routed and the default is `fixed`, which gives 0/0 connections. The test writes a `topor.toml` beside its board copy.
- build.sh test passes `TOPOR_VENV=sys.prefix` (a worktree has no own `.venv`) and `KICAD_DOCUMENTS_HOME=<tmp>`, never `--dev`; the real plugin dir (mtime+size of every file) is compared before/after.

## Install steps (user)
1. `bash tools/build.sh --dev` (builds the wheel, installs plugin files into `~/.local/share/kicad/10.0/plugins/org.topor.router`, pip-reinstalls the wheel into the dev venv and the KiCad plugin venv).
2. KiCad 10: Preferences -> Plugins -> enable "Enable KiCad API"; (also first time: let KiCad create the plugin environment, it installs `requirements.txt`: kicad-python, topor-engine from `wheels/`).
3. Restart the PCB Editor (or Preferences -> Plugins -> Recreate Plugin Environment if the engine wheel changed or import errors appear).
4. Open a **copy** of a board (`scratch/gui/<board>/`), the "Topor: route" toolbar button / Tools -> External Plugins entry appears; click it.

## Plugin dir layout produced
```
org.topor.router/
  plugin.json  topor_plugin.py  requirements.txt
  topor/        (plugin/topor package, no __pycache__)
  toporlib/     (pure shared package)
  docs/user/    (B11 fills)
  wheels/topor_engine-0.4.0-*.whl
```
(no `harness/`)

## UNVERIFIED (GUI checklist; needs a running KiCad, Ruling C4)
- `KiCad(...)` connection with the env token/socket as KiCad launches the plugin; `get_board()` returns the board that launched it.
- `IpcBoard` (save_as copy incl. unsaved edits, selection, commit as one undo step, refill) against live KiCad.
- `WxUi` created in a separate plugin process with our own `wx.App`; modal dialogs and the progress-dialog event pump on the real desktop.
- KiCad's plugin venv resolving `import topor` / `toporlib` via the sys.path insert (shown here only for a simulated layout).
- Whether KiCad shows the toolbar button / menu with the new action name and honours `min_version` "3.11".
- Engine handshake dialog (`ENGINE_VERSION`) rendering via `WxUi.error`.
- `topor.toml` first-run notice path in GUI mode (project dir from IPC `get_project().path`).
