#!/usr/bin/env bash
# exist-term r2 targeted checks (Hetzner, one hzq unit): build --all-targets; the release tests of the files the round
# changed (topo_exist_term incl. the known-gap test run with --ignored, the lib tests, the route binaries with kept
# copper: topo_g0 topo_r0 topo_d0 topo_s3b); pytest of the changed Python (drcloop, run flow, config docs).
set -u
R=${HZ_RUN_DIR:-/tmp/xt2}; ROOT=$(pwd); mkdir -p "$R"; L=$R/tests.log; : > "$L"
cd "$ROOT/engine"
cargo build --release --all-targets >> "$L" 2>&1; echo "build-all rc $?" >> "$L"
cargo test --release --no-fail-fast --test topo_exist_term --test topo_g0 --test topo_r0 --test topo_d0 --test topo_s3b -- --nocapture >> "$L" 2>&1; echo "rel rc $?" >> "$L"
cargo test --release --lib >> "$L" 2>&1; echo "lib rc $?" >> "$L"
cargo test --release --test topo_exist_term -- --ignored known_gap >> "$L" 2>&1; echo "known-gap (expected to fail) rc $?" >> "$L"
cd "$ROOT"
CI=1 python -m pytest -q tests/test_drcloop.py tests/plugin/test_run_flow.py tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/test_docs_links.py >> "$L" 2>&1; echo "pytest rc $?" >> "$L"
PYTHONPATH=plugin python -m topor.docs_gen --check >> "$L" 2>&1; echo "docs_gen rc $?" >> "$L"
