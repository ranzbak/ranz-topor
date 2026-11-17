#!/usr/bin/env bash
# refill-default r2: the fallback-notice test green, and N6 killed (one unit)
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
hzq run --threads 4 --name rf2-n6 -- bash -c "PYTHONPATH=plugin python -m pytest tests/plugin/test_refill_matrix.py -k without_counts -q -p no:cacheprovider > $R/green.txt 2>&1; echo rc=\$? >> $R/green.txt; T=/root/rf2-n6; rm -rf \$T; mkdir -p \$T; tar --exclude=./target --exclude=./engine/target -cf - . | tar -xf - -C \$T; RF2_TESTS='tests/plugin/test_refill_matrix.py -k without_counts' python \$T/tools/refill_r2_mut.py \$T $R/mut N6_dialog_fallback_one_undo > $R/mutants.txt 2>&1" &
wait
echo "all done" > "$R/rc.txt"
