#!/usr/bin/env bash
# Build the engine wheel and install the KiCad plugin files (31 §3.3).
#   tools/build.sh          wheel -> plugin dir + plugin/wheels
#   tools/build.sh --dev    additionally pip-reinstall the wheel into the dev venv and the KiCad venv
# Environment overrides:
#   TOPOR_VENV           venv holding maturin/pip (default: <repo>/.venv)
#   KICAD_DOCUMENTS_HOME KiCad 10.0 user dir (default: ${XDG_DATA_HOME:-~/.local/share}/kicad/10.0)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
DEV=0
[ "${1:-}" = "--dev" ] && DEV=1
VENV="${TOPOR_VENV:-$ROOT/.venv}"
KICAD_HOME="${KICAD_DOCUMENTS_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/kicad/10.0}"
ID="org.topor.router"
PLUGIN_DIR="$KICAD_HOME/plugins/$ID"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' engine/Cargo.toml | head -1)"

"$VENV/bin/maturin" build --release --manifest-path engine/Cargo.toml --features python --out target/wheels
WHEEL="$(ls -t target/wheels/topor_engine-"$VERSION"-*.whl | head -1)"

install_into() { # $1 = plugin-layout dir
    local d="$1"
    mkdir -p "$d/wheels"
    rm -f "$d"/wheels/topor_engine-*.whl
    cp plugin/plugin.json "$d/plugin.json"
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
    KV="$HOME/.cache/kicad/10.0/python-environments/$ID"
    if [ -x "$KV/bin/pip" ]; then
        "$KV/bin/pip" install --force-reinstall --no-deps "$WHEEL"
    fi
    echo "Installed $WHEEL. Restart PCB Editor or use Recreate Plugin Environment."
fi
echo "Plugin dir: $PLUGIN_DIR"
