#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
/usr/bin/python3 -m venv --system-site-packages .venv
.venv/bin/pip install --upgrade pip 'maturin>=1.7,<2' 'pytest>=8' 'hypothesis>=6'
.venv/bin/python -c "import pcbnew, pytest, maturin; print('ok')"
