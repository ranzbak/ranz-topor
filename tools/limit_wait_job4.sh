#!/usr/bin/env bash
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
hzq run --threads 4 --name final -- bash -c 'export CARGO_TARGET_DIR=/root/lw-target2; cargo test --release --manifest-path engine/Cargo.toml --lib -- control:: optimise:: model:: ; cargo test --release --manifest-path engine/Cargo.toml --test topo_limit_wait --test time_limit; python -m pytest -q -p no:cacheprovider tests/plugin/test_run_time_limit.py tests/plugin/test_config_docs.py tests/plugin/test_messages.py tests/plugin/test_postrun.py tests/plugin/test_run_cpu_limit.py tests/test_engine_surface.py tests/test_docs_links.py' > "$R/final.log" 2>&1
echo "final rc $?" >> "$R/rc.txt"
echo "all done" >> "$R/rc.txt"
