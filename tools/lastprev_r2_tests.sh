#!/usr/bin/env bash
# last-preview fix round 2: targeted tests + mutants + full pytest (bulk). usage: lastprev_r2_tests.sh
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
rustup component add clippy >/dev/null 2>&1
( hzq run --threads 4 --name lpr2-rs -- bash -c "cd $ROOT && cargo clippy --release --all-targets > $R/clippy.log 2>&1; echo clippy rc \$? >> $R/rc.txt; cd engine && cargo test --release --test last_preview --lib sketch > $R/rs.log 2>&1" 9>&-; note "unit rs rc $?" ) &
( hzq run --threads 4 --name lpr2-py -- bash -c "cd $ROOT && CI=1 python -m pytest -q -p no:cacheprovider tests/plugin/test_last_preview.py tests/plugin/test_ui_wx.py > $R/py-t.log 2>&1; python -m pytest -q -p no:cacheprovider tests/plugin/test_run_flow.py -k sketches > $R/py-rf.log 2>&1; CI=1 python -m pytest -q -p no:cacheprovider > $R/pytest.log 2>&1" 9>&-; note "unit py rc $?" ) &
# mutants, serial (wx): own tree copy
( hzq run --threads 2 --name lpr2-mut -- bash -c "
M=$R/mut; rm -rf \$M; mkdir -p \$M; rsync -a --exclude engine/target --exclude .git $ROOT/ \$M/; cd \$M
mut() { n=\$1; f=\$2; a=\$3; b=\$4; shift 4; cp \$f \$f.orig; python3 - \"\$f\" \"\$a\" \"\$b\" <<'P'
import sys
f,a,b=sys.argv[1:4]; s=open(f).read(); assert a in s, 'mutation target missing'; open(f,'w').write(s.replace(a,b,1))
P
  python -m pytest -q -p no:cacheprovider -x \"\$@\" > \$M/mut-\$n.log 2>&1; echo \"mutant \$n rc \$? (0 = SURVIVED)\" >> $R/rc.txt; mv \$f.orig \$f; }
mut ui_gate_always plugin/topor/ui_wx.py 'if available(pv) or unreleased_allowed():' 'if True:' tests/plugin/test_last_preview.py
mut sketch_when_off plugin/topor/run.py 'callable(wanted) and wanted() is not None and' 'callable(wanted) and' tests/plugin/test_run_flow.py -k sketches
mut no_ctx_refetch plugin/topor/run.py ' and ctx == sk.get(\"ctx\"):' ':' tests/plugin/test_last_preview.py -k refetches
" > $R/mut.log 2>&1 9>&-; note "unit mut rc $?" ) &
wait; note "all done"
