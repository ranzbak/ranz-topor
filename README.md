# topor — topological curved-trace router for KiCad

topor routes PCB connections as smooth, taut traces (straight segments joined by tangent arcs),
using topological routing: first decide *which side of every obstacle* a wire passes, then pull
each wire tight. It respects KiCad's design rules and checks its output with KiCad's own DRC.

Engine in Rust (`engine/`), packaged as a Python wheel; KiCad plugin in `plugin/`; test and
benchmark harness in `harness/`. Linux only for now.

## Status (version 0.3.0, milestone M3)

| Works now | Not yet (milestone M4) |
|---|---|
| Two-layer routing with vias, arcs, pours, keepouts, locked copper kept | Routing from inside KiCad (the plugin button only checks the install) |
| eprom-to-kernal 27/27 and wiccon-2023 62/62 routed, 0 DRC errors | `topor.toml` project config, pre-flight checks, preview + Accept/Reject |
| Command-line routing of a **copy** of a board, with DRC and PNG report | Routing scope (selected parts/nets), priority markers `@1..@3` |
| Deterministic output, configurable threads | dcsgonebadge 100 % (now 148/231 in 60 s) |

Benchmarks and progress images: [docs/benchmarks/](docs/benchmarks/README.md).
Design and research library: [docs/](docs/README.md).

## Requirements

- Linux, KiCad **10.0** (with `kicad-cli` and the `pcbnew` Python module, normally `/usr/bin/python3`)
- Rust toolchain (stable, tested with 1.96) — <https://rustup.rs>
- Python 3.9+ with `venv`

## Build

```bash
git clone git@github.com:ranzbak/ranz-topor.git topor && cd topor
tools/dev-venv.sh          # creates .venv (maturin, pytest; sees the system pcbnew)
tools/build.sh             # builds the engine wheel and installs the plugin into KiCad
```

`tools/build.sh` builds `target/wheels/topor_engine-<version>-*.whl`, copies it to `plugin/wheels/`
and installs the plugin files to `~/.local/share/kicad/10.0/plugins/org.topor.router/`
(override with `KICAD_DOCUMENTS_HOME`). `tools/build.sh --dev` also installs the wheel into the dev
venv and into KiCad's plugin environment, so a rebuilt engine is picked up without reinstalling.

## Install check in KiCad

1. KiCad → **Preferences → Plugins**: enable the **API server** (KiCad's IPC API).
2. Open any board in the **PCB Editor**. KiCad creates the plugin's Python environment and installs
   the wheel from `plugin/wheels` (first time: may take a minute).
3. The toolbar shows a **Topor** button ("Topor: route"). In 0.3.0 it only checks that the engine
   is installed and prints `topor engine 0.3.0 ok`.
4. If it reports a version mismatch: rebuild with `tools/build.sh`, then in KiCad use
   **Preferences → Plugins → Recreate Plugin Environment** (or restart the PCB Editor).

## Route a board from the command line (0.3.0)

The harness never touches your project: it routes a **snapshot copy**.

```bash
# 1. snapshot the source boards listed in harness/paths.py into scratch/boards/<board>/<sha12>/
.venv/bin/python -m harness.snapshot
# 2. rip the copy's tracks, route it with topor, run KiCad DRC, write metrics + images
.venv/bin/python -m harness.bench eprom-to-kernal /tmp/topor-run --router topor --threads 4
```

The output directory holds the routed board in `<board>-work/<board>.kicad_pcb` (open it in KiCad to
inspect), DRC json files (original, ripped, routed),
`<board>--metrics.json` (routed, vias, length, detours, DRC) and comparison PNGs (hand vs topor).
To add your own board, add it to `BENCH_BOARDS` in `harness/paths.py` and snapshot it.

## Tests

```bash
.venv/bin/pytest                                   # unit + integration (default)
.venv/bin/pytest tests/gate -m "gate or not gate"  # milestone gates M0–M3 (~10 min)
.venv/bin/pytest tests/scaling -m scaling          # complexity/scaling budget
(cd engine && cargo test --release && cargo clippy --all-targets -- -D warnings)
```

## Repository layout

| Path | What |
|---|---|
| `engine/` | Rust engine: geometry kernel, triangulation, topological search, negotiation, smoothing, checker |
| `harness/` | Python: board snapshot/copy, rip, model export, rules, writer, DRC, render, bench |
| `plugin/` | KiCad IPC plugin (`plugin.json`, entry point, `wheels/`, `requirements.txt`) |
| `tools/` | `build.sh`, `dev-venv.sh`, `bench.sh`, `render.sh` |
| `tests/` | pytest suites (unit, `gate/`, `scaling/`) |
| `docs/` | research library, design, plans, progress notes, benchmarks |

## Documentation still to come (M4)

A user guide (install, `topor.toml` reference, operation, troubleshooting), a GUI test checklist,
and a contributor/debug guide — see [docs/design/46-configuration.md](docs/design/46-configuration.md) §1.1.
