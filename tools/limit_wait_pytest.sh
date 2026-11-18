#!/usr/bin/env bash
R=$1
PYTHONPATH=plugin python -m topor.docs_gen --check; echo "docs_gen rc $?"
python -m pytest tests -q -p no:cacheprovider --deselect tests/test_scaffold.py::test_cargo_workspace_builds -rfEs 2>&1 | tail -80
echo "pytest rc ${PIPESTATUS[0]}"
