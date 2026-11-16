export PYTHONPATH=plugin
python -m pytest tests/plugin/test_messages.py tests/plugin/test_toml_defaults.py tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_help_doc.py -q -p no:cacheprovider > $HZ_RUN_DIR/pytest.txt 2>&1
python -m topor.docs_gen --check >> $HZ_RUN_DIR/pytest.txt 2>&1
