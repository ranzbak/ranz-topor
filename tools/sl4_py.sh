#!/usr/bin/env bash
R=$1
PYTHONPATH=plugin python -m topor.docs_gen --check; echo "docs_gen rc $?"
CI=1 PYTHONPATH=plugin python -m pytest -q -p no:cacheprovider tests/plugin/test_ui_wx.py tests/plugin/test_run_flow.py \
  tests/plugin/test_run_flow_guards.py tests/plugin/test_run_cpu_limit.py tests/plugin/test_toml_defaults.py \
  tests/plugin/test_config.py tests/plugin/test_messages.py -rfEs > "$R/pytest.txt" 2>&1; echo "pytest rc $?"
tail -8 "$R/pytest.txt"
