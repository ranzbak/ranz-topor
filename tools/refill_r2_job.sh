#!/usr/bin/env bash
# refill-default fix round 2: targeted tests + python mutants (2 hzq units)
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
hzq run --threads 4 --name rf2-tests -- bash -c 'PYTHONPATH=plugin python -m pytest tests/plugin tests/test_docs_links.py -q -p no:cacheprovider -rfEs > '"$R"'/pytest.txt 2>&1; echo rc=$? >> '"$R"'/pytest.txt; PYTHONPATH=plugin python -m topor.docs_gen --check > '"$R"'/docs_gen.txt 2>&1; echo rc=$? >> '"$R"'/docs_gen.txt' &
hzq run --threads 4 --name rf2-mut -- bash -c 'T=/root/rf2-mut; rm -rf $T; mkdir -p $T; tar --exclude=./target --exclude=./engine/target -cf - . | tar -xf - -C $T; python tools/refill_r2_mut.py $T '"$R"'/mut > '"$R"'/mutants.txt 2>&1' &
wait
echo "all done" > "$R/rc.txt"
