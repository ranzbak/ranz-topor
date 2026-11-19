#!/usr/bin/env bash
# one unit of nofin_r2_job.sh: "pytest" or "mut <ids...>" (mutants run in a private tree copy)
R=$HZ_RUN_DIR; K=$1; shift
if [ "$K" = pytest ]; then
  CI=1 timeout 3000 python -m pytest -q tests > $R/pytest.log 2>&1 9>&-; echo "pytest rc $?" >> $R/pytest.log; exit 0
fi
D=$R/tree-$1; mkdir -p $D; rsync -a --exclude target --exclude .git ./ $D/ 9>&-; find $D/plugin $D/tests -name '*.py' -exec touch {} +
cd $D && CI=1 python tools/nofin_r2_mut.py "$@" > $R/mut-$1.txt 2>&1 9>&-
