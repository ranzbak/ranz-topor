# topor — topological curved-trace router for KiCad

topor routes PCB connections as smooth, taut traces (straight segments joined by tangent arcs),
using topological routing: first decide *which side of every obstacle* a wire passes, then pull
each wire tight. It respects KiCad's design rules and checks its output with KiCad's own DRC.

Engine in Rust (`engine/`), packaged as a Python wheel; KiCad 10 plugin in `plugin/` (IPC API);
test and benchmark harness in `harness/`. Linux only for now.

## Status (version 0.4.0, milestone M4b: the plugin)

| Works now, from inside KiCad 10 | Comes in 0.5.0 (milestone M4a: the engine) |
|---|---|
| **Topor: route** button: pre-flight findings (code, cause, fix), options, progress with Cancel, before/after preview, **Accept = one undo step** / Reject = nothing changed | dcsgonebadge 231/231 routed, 0 router DRC, ≤ 10 min (now 148/231 in 60 s) |
| Whole board or a selection (between / touching selected footprints); existing copper kept, or ripped and re-routed; locked copper respected | priorities used for routing order (`@N` markers, `topor_pN` net classes, `[nets] priority` — computed and reported now) |
| Self-documenting `topor.toml` per project, generated [reference](docs/user/configuration.md); every finding names the KiCad menu or the key to change | pad exit stubs for fine pitch, same-net T-junctions, via rules from the board setup, bounded via relaxation, layer costs |
| Result verified by `kicad-cli` DRC on a copy before you decide; report folder (report, preview, DRC, log) per run | engine-side `do_not_touch` / `ignore_pads` options (applied by the plugin's scope now) |
| eprom-to-kernal 27/27 and wiccon-2023 62/62 routed, `routing_new 0`, `unconnected 0` (headless, judged by `kicad-cli`) | eprom / wiccon not worse than now |

The plugin needs a live GUI test before the 0.4.0 gate closes:
[docs/user/gui-test-checklist.md](docs/user/gui-test-checklist.md). Known limits:
[operate.md → Limitations](docs/user/operate.md#limitations).

## Requirements

- Linux (x86_64), **KiCad 10.0** with `kicad-cli` (tested 10.0.8)
- **Python ≥ 3.11** as KiCad's plugin interpreter (normally `/usr/bin/python3`), with the system wxPython 4.2
- Rust toolchain (stable, tested 1.96) — <https://rustup.rs> — to build the engine wheel
- For development: the system `pcbnew` Python module (harness and tests only, never the plugin)

## User guide

1. [Install](docs/user/install.md) — build, install into KiCad, enable the KiCad API, plugin environment.
2. [Operate](docs/user/operate.md) — first run, findings, options, scope by selection, progress, preview,
   Accept / Reject / undo, report folder, priorities, limitations.
3. [Configuration reference](docs/user/configuration.md) — every `topor.toml` key (generated).
4. [Troubleshooting](docs/user/troubleshoot.md) — one section per message code, exit codes, install problems.
5. [GUI test checklist](docs/user/gui-test-checklist.md) — the manual acceptance test in KiCad.

## Quick start

```bash
git clone git@github.com:ranzbak/ranz-topor.git topor && cd topor
tools/dev-venv.sh          # creates .venv (maturin, pytest; sees the system pcbnew)
tools/build.sh             # builds the engine wheel and installs the plugin into KiCad
```

Then KiCad → **Preferences → Plugins** → tick **Enable KiCad API**, open a **copy** of a board in the PCB
Editor (KiCad creates the plugin environment the first time) and click **Topor: route**. Details:
[install.md](docs/user/install.md).

## Route a board from the command line

The harness never touches your project: it routes a **snapshot copy**.

```bash
# 1. snapshot the source boards listed in harness/paths.py into scratch/boards/<board>/<sha12>/
.venv/bin/python -m harness.snapshot
# 2. rip the copy's tracks, route it with topor, run KiCad DRC, write metrics + images
.venv/bin/python -m harness.bench eprom-to-kernal /tmp/topor-run --router topor --threads 4
```

The output directory holds the routed board in `<board>-work/<board>.kicad_pcb` (open it in KiCad to
inspect), DRC json files (original, ripped, routed), `<board>--metrics.json` (routed, vias, length,
detours, DRC) and comparison PNGs (hand vs topor). More tools (engine probes, headless plugin runs,
reproducing a plugin run): [docs/dev/debug.md](docs/dev/debug.md).

## Development

[CONTRIBUTING.md](CONTRIBUTING.md): layout, build, tests, read-only boards, configuration and message
catalogues, commit style. Benchmarks and progress images are generated locally into `docs/benchmarks/`
(`harness.bench`, `harness.progress`; not in git) — key numbers are quoted in `docs/progress/STATE.md`.
Design and research library: [docs/](docs/README.md).

```bash
.venv/bin/pytest                                   # unit + integration (default)
.venv/bin/pytest tests/gate -m "gate or not gate"  # milestone gates (~10 min)
.venv/bin/pytest tests/scaling -m scaling          # complexity/scaling budget
(cd engine && cargo test --release && cargo clippy --all-targets -- -D warnings)
```

## Repository layout

| Path | What |
|---|---|
| `engine/` | Rust engine: geometry kernel, triangulation, topological search, negotiation, smoothing, checker |
| `plugin/` | KiCad IPC plugin: `plugin.json`, entry point, `topor/` package (flow, dialogs, pre-flight, config), `wheels/`, `requirements.txt` |
| `toporlib/` | pure-Python code shared by plugin and harness (DRC, rules, DRC loop, board-file edits) |
| `harness/` | dev/test only: board snapshot/copy, rip, model export, rules, writer, DRC, render, bench |
| `tools/` | `build.sh`, `dev-venv.sh`, `bench.sh`, `render.sh` |
| `tests/` | pytest suites (unit, `plugin/`, `gate/`, `scaling/`) |
| `docs/` | user guide, developer guide, research library, design, plans, progress notes, benchmarks |

## License

topor is free software, licensed under the **GNU General Public License, version 2 or (at your
option) any later version** (SPDX: `GPL-2.0-or-later`); see [LICENSE](LICENSE).
Copyright (C) 2026 Paul Honig and contributors. This license allows reusing code from other GPL
routers such as the gEDA PCB toporouter (GPL-2.0-or-later); keep their copyright notices when doing so.
