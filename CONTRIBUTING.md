# Contributing to topor

topor is a topological curved-trace router for KiCad 10: a Rust engine packaged as a Python wheel, a KiCad
IPC plugin, and a test / benchmark harness. This page is for people changing the code. Users start at
[docs/user/install.md](docs/user/install.md); debugging tools are in [docs/dev/debug.md](docs/dev/debug.md).

## Layout

| Path | What | Rules |
|---|---|---|
| `engine/` | Rust engine: geometry kernel, triangulation, topological search, negotiation, smoothing, checker; `src/py.rs` = the Python module `topor_engine`; `examples/` = dev tools (`route_probe`, `gate_diag`, …) | no KiCad types; KiCad frame, integer nm, one Y flip each way; deterministic output |
| `plugin/` | the KiCad plugin: `plugin.json`, `topor_plugin.py` (entry point), `topor/` (the package), `requirements.txt`, `wheels/` (built) | **no pcbnew** (IPC via kipy 0.8 + file formats + `kicad-cli`) |
| `plugin/topor/` | `app.py` (entry, handshake, `--headless`), `run.py` (the flow), `ports.py` (BoardPort / UiPort), `ipc.py` + `model_ipc.py` (KiCad over IPC), `fake.py` (FakeBoard / ScriptedUi for tests), `preflight.py` + `messages.py` (findings), `scope.py`, `priority.py`, `preview.py`, `report.py`, `ui_wx.py` (dialogs), `schema.py` + `config.py` + `docs_gen.py` (`topor.toml`) | |
| `toporlib/` | pure-Python code shared by plugin and harness: DRC runner/classifier, `.kicad_dru` parser, rule resolution, DRC loop, detour stats, s-expression board-file edits, geometry helpers | **no pcbnew**, no plugin imports |
| `harness/` | dev/test only: snapshots, copies, rip, model export, rules, writer, render, bench, progress PNGs | may use pcbnew (the oracle); never shipped |
| `tests/` | pytest: unit + integration (default), `tests/plugin/` (plugin), `tests/gate/` (milestone gates), `tests/scaling/` | |
| `tools/` | `build.sh` (wheel + plugin install), `dev-venv.sh`, `bench.sh`, `render.sh` | |
| `docs/` | user guide (`docs/user/`), developer guide (`docs/dev/`), design and research library, plans, progress, benchmarks | |
| `scratch/` | git-ignored: board snapshots, GUI copies, benchmark outputs | |

## Build

```bash
tools/dev-venv.sh                       # .venv: maturin, pytest, hypothesis; sees the system pcbnew
VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python
                                        # engine into .venv (rebuild before every engine-using pytest run)
tools/build.sh                          # wheel + plugin files into KiCad's plugin folder
tools/build.sh --dev                    # ... and reinstall the wheel into .venv and KiCad's plugin environment
```

`tools/build.sh --dev` writes into KiCad's plugin environment
(`~/.cache/kicad/10.0/python-environments/org.topor.router`): run it on your own machine, not from
automated jobs. `TOPOR_KICAD_HOME=<dir>/kicad/10.0 tools/build.sh` (versioned folder) or
`KICAD_DOCUMENTS_HOME=<dir> tools/build.sh` (KiCad's unversioned root, `10.0` is appended) installs into a scratch
KiCad folder instead (that is what `tests/plugin/test_app.py` does). Version bumps change `engine/Cargo.toml`, `Cargo.lock`,
`plugin/plugin.json`, `plugin/requirements.txt`, `SHIPPED_VERSION` in `plugin/topor_plugin.py` and
`CURRENT_VERSION` in `plugin/topor/schema.py` together (`tests/test_scaffold.py` compares the first four;
`CURRENT_VERSION` decides which *since* keys are active).

## Tests

```bash
.venv/bin/pytest -q                                   # default suite (unit + integration, no gates)
.venv/bin/pytest -q tests/plugin tests/test_docs_links.py   # plugin + documentation
.venv/bin/pytest tests/gate -m "gate or not gate"     # milestone gates on benchmark copies (minutes)
.venv/bin/pytest tests/scaling -m scaling             # runtime-growth budgets
cargo test --release --manifest-path engine/Cargo.toml
cargo clippy --all-targets --manifest-path engine/Cargo.toml -- -D warnings   # must be clean
PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen --check                   # configuration.md current
tools/gui_e2e.sh wiccon-2023 scratch/k1/gui/wiccon --from-hand                  # live KiCad on Xephyr (minutes)
```

`tools/gui_e2e.sh` is the live end-to-end test of the plugin in a real KiCad 10 on a nested X display with a separate
KiCad home (never your own); it is not part of pytest ([gui-test-checklist.md](docs/user/gui-test-checklist.md#automated-run)).

- **TDD**: write the failing test first, see it fail, implement, see it pass. Never weaken, skip or delete a
  test or a bound to make it pass; never relax a budget.
- Tests that need a running KiCad (IPC) skip without `KICAD_API_SOCKET`; what they and the dialogs cover is
  in [docs/user/gui-test-checklist.md](docs/user/gui-test-checklist.md). wx tests construct dialogs but never
  show them.
- Tests write reports under a temporary `TOPOR_REPORT_ROOT`, never under `~/.cache/kicad`.
- `tests/plugin/test_no_pcbnew.py` fails if anything under `plugin/` or `toporlib/` imports pcbnew;
  `tests/plugin/test_kipy_surface.py` pins every kipy call the plugin uses against the installed kipy.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to a branch other than `main`, on pull requests and on demand:

- **rust**: `cargo test --release` and `cargo clippy --release --all-targets` in `engine/` (not yet `-D warnings`: 5 pre-existing warnings on stable 1.96; make it strict once fixed).
- **python**: builds the `topor-engine` wheel with maturin (feature `python`), installs it with the Python test
  dependencies into a plain venv, and runs `pytest` (junit xml uploaded as the `pytest-junit` artifact).

The runner has neither KiCad nor the private benchmark boards, so `tests/conftest.py` turns "resource absent" into a
**skip with the reason** (never a failure, and never when the resource is present):

- `pcbnew` / `wx` not importable: the module (or test) is skipped. Locally use `tools/dev-venv.sh` (system KiCad Python).
- `snapshot of board ... (snapshot directory missing)`: tests on the pinned boards under `scratch/boards` are skipped.
  Create them with `.venv/bin/python -m harness.snapshot` from `~/work/kicad/projects` (private, never committed).
- tests needing a running KiCad (`KICAD_API_SOCKET`) and the live-KiCad GUI tests skip as before.
- `tests/test_scaffold.py::test_cargo_workspace_builds` skips when `CI` is set (the rust job covers it).

Known failures parked as PT1 are `xfail(strict=False)`, so they are visible but do not turn CI red.
Run the full local suite (nothing skipped) with `.venv/bin/pytest -q`.

## Boards are read-only

- The source projects under `~/work/kicad/projects` are **never modified** — not by code, not by hand.
  Only `harness/snapshot.py` reads them, to make immutable snapshots:
  `.venv/bin/python -m harness.snapshot` → `scratch/boards/<board>/<sha12>/`.
- The harness and the tests read boards only from those snapshots (`harness.paths.BENCH_BOARDS`) and work on
  copies (`harness.copy.copy_project`, which also verifies the source tree is unchanged). Gate tests hash the
  snapshot directory.
- Copies for trying the plugin in KiCad go to `scratch/gui/<board>/`; never open a snapshot in KiCad for
  editing.
- Benchmark and progress images go to `docs/benchmarks/` on disk only — that directory is git-ignored and never
  committed; quote the key numbers in `docs/progress/STATE.md` or a progress note instead.

## Configuration: schema → topor.toml → docs

Every judgement call is a setting with a safe default ("if in doubt, make it configurable"):

1. Add a `Setting` to `plugin/topor/schema.py` (`SETTINGS`): section, key, type, default, unit, range, a doc
   sentence for a user without AI help, the KiCad place that influences it (`kicad="KiCad → …"`), the engine
   field it maps to, and `since=` when the engine feature lands later (the key is then written
   commented-out).
2. `topor.toml` is generated from the schema on the first run (`config.render_toml`, with comments) — never
   hand-maintain a template.
3. Regenerate the reference and check it:
   `PYTHONPATH=plugin .venv/bin/python -m topor.docs_gen --write` → `docs/user/configuration.md` (never edit
   it by hand), then `--check`. `tests/plugin/test_config_docs.py` fails on undocumented keys.
4. If the user can reach the key from a dialog, update [docs/user/operate.md](docs/user/operate.md#options);
   a new KiCad place must also appear in operate.md's KiCad table (`tests/test_docs_links.py` checks it).

## Messages: catalogue → troubleshoot.md

Every finding has a code in `plugin/topor/messages.py` (`MESSAGES`: severity, cause, fix). The fix names a
KiCad menu path (`KiCad → …`) or a `topor.toml` key (`section.key`) that exists. After editing the catalogue,
regenerate the per-code part of [docs/user/troubleshoot.md](docs/user/troubleshoot.md) (everything after
`## Findings`; the hand-written sections above it stay):

```bash
.venv/bin/python - <<'EOF'
import sys; sys.path.insert(0, "plugin")
from pathlib import Path
from topor.messages import render_troubleshoot
p = Path("docs/user/troubleshoot.md")
head = p.read_text().split("\n## Findings\n\n", 1)[0]
p.write_text(head + "\n## Findings\n\n" + render_troubleshoot())
EOF
```

`tests/plugin/test_messages.py` fails when a code is emitted but not catalogued, has no section, or the
section text differs.

## Code rules

- **Complexity**: every new component states its Big-O in its doc comment; no exponential stage; every loop
  capped; every long call checks the deadline and the cancel flag.
- **Determinism**: byte-identical result JSON (minus `runtime_s`) for 1, 2 and 4 threads and across runs; no
  hash-map iteration order reaches the output.
- **UNVERIFIED**: code that relies on behaviour you could not check (live KiCad, a file token you have not
  seen) is marked `UNVERIFIED` in its docstring and gets a row in the GUI checklist.
- **No detours**: a connection that goes around where a direct path exists is a bug; track the detour ratio
  (`harness.bench` prints it, progress PNGs show it).
- Progress PNG after every task that changes routing results (`python -m harness.progress`, see
  [docs/dev/debug.md](docs/dev/debug.md#progress-pngs)).

## Commits

Conventional commits, imperative, scoped: `feat(plugin): …`, `fix(engine): …`, `docs(user): …`,
`test(gate): …`, `chore: …`. One logical change per commit; the full default test suite green before
committing (cargo test + clippy too when Rust changed). Do not push work-in-progress branches to `main`.
