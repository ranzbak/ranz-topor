#!/usr/bin/env bash
# Build the engine wheel and install the KiCad plugin files (31 §3.3).
#   tools/build.sh          wheel -> plugin dir + plugin/wheels
#   tools/build.sh --dev    additionally pip-reinstall the wheel into the dev venv and the KiCad venv
# Environment overrides:
#   TOPOR_WHEEL          an already built engine wheel to install (skips the maturin build)
#   TOPOR_VENV           venv holding maturin/pip (default: <repo>/.venv)
#   TOPOR_KICAD_HOME     the versioned KiCad 10.0 user dir to install into (e.g. ~/.local/share/kicad/10.0);
#                        wins over KICAD_DOCUMENTS_HOME
#   KICAD_DOCUMENTS_HOME as KiCad reads it: the UNversioned documents root (e.g. ~/.local/share/kicad);
#                        10.0 is appended
#   TOPOR_KICAD_CACHE    the versioned KiCad 10.0 cache dir whose plugin environment --dev updates
#                        (default ${XDG_CACHE_HOME:-~/.cache}/kicad/10.0)
# Default: ${XDG_DATA_HOME:-~/.local/share}/kicad/10.0. The plugin goes to <that>/plugins/org.topor.router.
# A separate KiCad home for tests (tools/gui_e2e.sh): set XDG_DATA_HOME and XDG_CACHE_HOME (or the two
# TOPOR_KICAD_* variables) to folders of that home; the user's own KiCad folders are then never written.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
DEV=0
[ "${1:-}" = "--dev" ] && DEV=1
VENV="${TOPOR_VENV:-$ROOT/.venv}"
KICAD_VER="10.0"
if [ -n "${TOPOR_KICAD_HOME:-}" ]; then
    KICAD_HOME="$TOPOR_KICAD_HOME"
elif [ -n "${KICAD_DOCUMENTS_HOME:-}" ]; then
    KICAD_HOME="${KICAD_DOCUMENTS_HOME%/}/$KICAD_VER"
else
    KICAD_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/kicad/$KICAD_VER"
fi
ID="org.topor.router"
PLUGIN_DIR="$KICAD_HOME/plugins/$ID"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' engine/Cargo.toml | head -1)"

if [ -n "${TOPOR_WHEEL:-}" ]; then
    WHEEL="$TOPOR_WHEEL"
    case "$(basename "$WHEEL")" in topor_engine-"$VERSION"-*.whl) ;; *) echo "TOPOR_WHEEL is not a topor_engine $VERSION wheel: $WHEEL" >&2; exit 1 ;; esac
else
    "$VENV/bin/maturin" build --release --manifest-path engine/Cargo.toml --features python --out target/wheels
    WHEEL="$(ls -t target/wheels/topor_engine-"$VERSION"-*.whl | head -1)"
fi

install_into() { # $1 = plugin-layout dir
    local d="$1"
    mkdir -p "$d/wheels"
    rm -f "$d"/wheels/topor_engine-*.whl
    cp plugin/plugin.json "$d/plugin.json"
    rm -rf "$d/icons"
    cp -r plugin/icons "$d/icons"  # toolbar button images named in plugin.json
    # Python packages: plugin/topor, toporlib (pure, no pcbnew), user docs. No harness/ (dev/test only).
    rm -rf "$d/topor" "$d/toporlib" "$d/docs"
    cp -r plugin/topor "$d/topor"
    cp -r toporlib "$d/toporlib"
    mkdir -p "$d/docs/user"
    if [ -d docs/user ]; then cp -r docs/user/. "$d/docs/user/"; fi
    find "$d" -name __pycache__ -type d -prune -exec rm -rf {} +
    sed "s/^SHIPPED_VERSION = .*/SHIPPED_VERSION = \"$VERSION\"/" plugin/topor_plugin.py > "$d/topor_plugin.py.tmp"
    mv "$d/topor_plugin.py.tmp" "$d/topor_plugin.py"
    cp "$WHEEL" "$d/wheels/"
    printf '%s\n' '--find-links wheels' 'kicad-python>=0.8,<0.9' "topor-engine==$VERSION" > "$d/requirements.txt"
}
install_into "$PLUGIN_DIR"
# In-repo copy: wheels/ is git-ignored; plugin/topor_plugin.py is the source, so only
# the generated requirements and the wheel are refreshed here.
mkdir -p "$ROOT/plugin/wheels"
rm -f "$ROOT"/plugin/wheels/topor_engine-*.whl
cp "$WHEEL" "$ROOT/plugin/wheels/"
cp "$PLUGIN_DIR/requirements.txt" "$ROOT/plugin/requirements.txt"

if [ "$DEV" = 1 ]; then
    "$VENV/bin/pip" install --force-reinstall --no-deps "$WHEEL"
    KV="${TOPOR_KICAD_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/kicad/$KICAD_VER}/python-environments/$ID"
    if [ -x "$KV/bin/pip" ]; then
        "$KV/bin/pip" install --force-reinstall --no-deps "$WHEEL"
    fi
    echo "Installed $WHEEL. Restart PCB Editor or use Recreate Plugin Environment."
fi
echo "Plugin dir: $PLUGIN_DIR"
