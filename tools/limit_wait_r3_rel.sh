#!/usr/bin/env bash
# release suite (all binaries, no fail fast), docs_gen, full pytest
R=$1
export CARGO_TARGET_DIR=/root/lw3-rel RUST_TEST_THREADS=4
cargo test --release --no-fail-fast --manifest-path engine/Cargo.toml > "$R/cargo-rel.txt" 2>&1; echo "cargo rel rc $?"
grep -cE "^test result" "$R/cargo-rel.txt"; grep -E "^test .*FAILED|^test result: FAILED" "$R/cargo-rel.txt"
PYTHONPATH=plugin python -m topor.docs_gen --check; echo "docs_gen rc $?"
python -m pytest tests -q -p no:cacheprovider --deselect tests/test_scaffold.py::test_cargo_workspace_builds -rfEs > "$R/pytest.txt" 2>&1; echo "pytest rc $?"
tail -8 "$R/pytest.txt"
# debug build of the branch's own tests (the full debug suite ran in r2's verification)
CARGO_TARGET_DIR=/root/lw3-dbg cargo test --no-fail-fast --manifest-path engine/Cargo.toml --lib --test topo_limit_wait --test topo_drawing_grace > "$R/cargo-dbg.txt" 2>&1; echo "cargo dbg rc $?"
grep -E "^test result|FAILED" "$R/cargo-dbg.txt"
