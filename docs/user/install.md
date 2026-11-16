# Installing topor 0.4.0

topor is a KiCad 10 plugin (IPC API) with a routing engine written in Rust. There are no prebuilt
packages yet: you build the engine wheel once on your machine, and a script installs the plugin into
KiCad. After that, KiCad creates the plugin's own Python environment by itself.

Next: [operate.md](operate.md) (how to use it) · [troubleshoot.md](troubleshoot.md) (when it does not
start) · [configuration.md](configuration.md) (every `topor.toml` key).

## Requirements

| What | Version | Why |
|---|---|---|
| Linux | x86_64; tested on Ubuntu 24.04 | the only platform built and tested in 0.4.0 |
| KiCad | **10.0** (tested 10.0.8), including `kicad-cli` | the IPC API the plugin talks to; `kicad-cli` runs DRC and renders the preview |
| Python for the plugin environment | **≥ 3.11** (`plugin.json` `min_version`) | KiCad builds the plugin's environment with its configured interpreter (normally `/usr/bin/python3`); topor reads `topor.toml` with `tomllib` (3.11+) |
| wxPython | 4.2 from the system (Ubuntu: `python3-wxgtk4.0`, pulled in by the KiCad package) | the dialogs; the plugin environment sees the system packages |
| Rust toolchain | stable (tested 1.96), <https://rustup.rs> | builds the engine wheel (build machine only) |
| Network access | first plugin start only | KiCad's `pip` downloads `kicad-python` (0.8.x) and its dependencies from PyPI; the engine wheel comes from the plugin folder |

`kicad-cli` is looked up as `$KICAD_CLI`, else `/usr/bin/kicad-cli`. If yours lives elsewhere, start KiCad
with `KICAD_CLI=/path/to/kicad-cli` in its environment.

## 1. Build and install the plugin files

```bash
git clone git@github.com:ranzbak/ranz-topor.git topor && cd topor
tools/dev-venv.sh      # once: creates .venv with maturin (and pytest for development)
tools/build.sh         # builds the engine wheel and copies the plugin into KiCad's plugin folder
```

`tools/build.sh` builds `target/wheels/topor_engine-0.4.0-*.whl` and installs the plugin to
`~/.local/share/kicad/10.0/plugins/org.topor.router/`. To install into another KiCad user folder set
`TOPOR_KICAD_HOME` to the versioned folder (`TOPOR_KICAD_HOME=/some/dir/kicad/10.0` → `/some/dir/kicad/10.0/plugins/…`);
`KICAD_DOCUMENTS_HOME` is honoured as KiCad reads it, the unversioned root (`/some/dir/kicad` → `10.0` is
appended). `TOPOR_VENV` selects another venv with `maturin`:

```
org.topor.router/
  plugin.json  topor_plugin.py  requirements.txt
  topor/       the plugin (Python)
  toporlib/    shared pure-Python helpers
  docs/user/   this guide (the dialog's Help button opens operate.md from here)
  wheels/      topor_engine-0.4.0-*.whl
```

For a KiCad home that is not yours (tests, a second KiCad profile) set `XDG_DATA_HOME` and `XDG_CACHE_HOME` to
folders inside it: the plugin goes to `$XDG_DATA_HOME/kicad/10.0/plugins/org.topor.router/` and `--dev` updates the
plugin environment under `$XDG_CACHE_HOME/kicad/10.0/` (or `TOPOR_KICAD_CACHE`). `TOPOR_WHEEL=<file>` installs an
already built `topor_engine` wheel instead of building one. `tools/gui_e2e.sh` uses exactly this
([gui-test-checklist.md](gui-test-checklist.md#automated-run)).

`tools/build.sh --dev` additionally re-installs the freshly built wheel into the repo's `.venv` and into
KiCad's existing plugin environment (`~/.cache/kicad/10.0/python-environments/org.topor.router`), so a
rebuilt engine of the **same version** is picked up without recreating the environment. It writes to
KiCad's plugin environment: use it on your own machine only, with KiCad's PCB Editor closed or restarted
afterwards.

## 2. Enable the KiCad API

KiCad → **Preferences → Plugins**: tick **Enable KiCad API** (the IPC API server the plugin connects to).
On this page you also see the **Python interpreter** KiCad uses for plugin environments; it must be
Python 3.11 or newer (normally `/usr/bin/python3`).

## 3. Let KiCad create the plugin environment

Open a board in the **PCB Editor**. KiCad notices the new plugin and, the first time, creates its Python
environment in `~/.cache/kicad/10.0/python-environments/org.topor.router/` (a venv that also sees the
system packages, e.g. wxPython) and installs `requirements.txt` into it:

```
--find-links wheels
kicad-python>=0.8,<0.9
topor-engine==0.4.0
```

This takes up to a minute. When it is done the toolbar shows the **Topor: route** button (the same action
is listed under **Tools → External Plugins**).

## 4. Check it works — on a copy

Never try a new router on your only copy of a board. Copy the project folder, open the copy, click
**Topor: route**. The first run creates `topor.toml` next to the `.kicad_pro` and tells you so; then the
route dialog opens ([operate.md](operate.md#first-run)). Cancel there changes nothing.

## Updating

1. `git pull`, then `tools/build.sh` again.
2. KiCad → **Preferences → Plugins**, right-click the Topor action → **Recreate Plugin Environment**, or
   simply restart the PCB Editor. If the version number did not change (development builds), use
   `tools/build.sh --dev` instead, or delete `~/.cache/kicad/10.0/python-environments/org.topor.router/` and
   restart the PCB Editor — KiCad then rebuilds the environment from scratch.

The start-up splash shows the version with the build time and git revision (for example
`0.4.0 · built 2026-11-14 01:05 UTC · 7e320c1-dirty`, `-dirty` = uncommitted changes), so you can tell which build is
installed. Running from the source tree shows the version only.

The plugin checks at start that the installed engine is the version it was shipped with; if not, it stops
with [`ENGINE_VERSION`](troubleshoot.md#engine_version) before touching anything.

## Uninstalling

```bash
rm -rf ~/.local/share/kicad/10.0/plugins/org.topor.router      # plugin files
rm -rf ~/.cache/kicad/10.0/python-environments/org.topor.router  # plugin environment
rm -rf ~/.cache/kicad/10.0/topor                                 # run reports and the crash log
```

`topor.toml` files in your projects stay; delete them by hand if you like.

## Where things are

| Path | What |
|---|---|
| `~/.local/share/kicad/10.0/plugins/org.topor.router/` | the installed plugin (`TOPOR_KICAD_HOME` = another `…/kicad/10.0` folder, or KiCad's `KICAD_DOCUMENTS_HOME` root + `10.0`) |
| `~/.cache/kicad/10.0/python-environments/org.topor.router/` | the plugin's Python environment, made by KiCad |
| `<project>/topor.toml` | per-project settings, created on the first run ([configuration.md](configuration.md)) |
| `~/.cache/kicad/10.0/topor/<board>/<YYYYmmdd-HHMMSS>/` | one report folder per run ([operate.md](operate.md#report-folder)); `$TOPOR_REPORT_ROOT` or `$XDG_CACHE_HOME` move it, `[output] report_dir` in `topor.toml` redirects it |
| `~/.cache/kicad/10.0/topor/topor-crash.log` | every fatal error of the plugin process, appended with a time stamp |
