hzq run --threads 2 --name ptests -- bash tools/toml_defaults_pytest.sh &
hzq run --threads 2 --name mutants -- bash -c 'bash tools/toml_defaults_mutants.sh > $HZ_RUN_DIR/mutants.txt 2>&1' &
wait
echo "all done" > $HZ_RUN_DIR/rc.txt
