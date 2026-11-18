#!/usr/bin/env bash
# exist-term r3: the schema / config / engine-surface pytest files (new option dangling_lead_via).
set -u
R=${HZ_RUN_DIR:-/tmp/xt3p}; mkdir -p "$R"
hzq run --threads 2 --name xt3p -- env CI=1 python -m pytest -q -p no:cacheprovider tests/test_engine_surface.py tests/test_docs_links.py tests/plugin/test_config.py tests/plugin/test_toml_defaults.py > "$R/pytest.log" 2>&1 9>&-; echo "pytest rc $?" >> "$R/rc.txt"
