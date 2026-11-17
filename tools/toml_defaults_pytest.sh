PYTHONPATH=plugin python -m pytest tests/plugin -q -p no:cacheprovider > $HZ_RUN_DIR/pytest.txt 2>&1
echo rc=$? >> $HZ_RUN_DIR/pytest.txt
