#!/usr/bin/env bash
# drc-project-fix r2: full pytest (Python touched) + 14 mutants (M1-M12 of the verifier, M13 post-DRC recheck, M14 no-net key)
R=${HZ_RUN_DIR:?}; ROOT=$PWD
if [ "${1:-}" = mut ]; then shift; MODE=mut; fi
note() { echo "$*" >> $R/rc.txt; }
mutants() {  # $1 = tag, rest = ids; own tree copy
  local t=$1; shift; local d=$R/mt-$t
  mkdir -p $d; rsync -a --exclude .git --exclude target --exclude .venv $ROOT/ $d/ 9>&-
  [ -e $d/scratch/boards ] || ln -s $ROOT/scratch/boards $d/scratch/boards 2>/dev/null
  for id in "$@"; do (cd $d && CI=1 python tools/drcfix_mut.py $d $id 2>&1 9>&-) >> $R/mut-$t.log; done
  note "mutants $t done"
}
[ "${MODE:-}" = mut ] && { mutants "$@"; exit 0; }
( hzq run --threads 1 --name dfx-pytest -- bash -c "cd $ROOT; CI=1 timeout 3000 python -m pytest -q tests > $R/pytest.log 2>&1 9>&-; echo pytest rc \$? >> $R/rc.txt; tail -4 $R/pytest.log >> $R/rc.txt" ) &
sleep 1
( hzq run --threads 1 --name dfx-mA -- bash $ROOT/tools/drcfix_job.sh mut A M1 M2 M3 M4 M5 M6 M7 ) &
( hzq run --threads 1 --name dfx-mB -- bash $ROOT/tools/drcfix_job.sh mut B M8 M9 M10 M11 M12 M13 M14 ) &
wait; note "all done"
