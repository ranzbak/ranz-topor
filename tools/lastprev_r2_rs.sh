#!/usr/bin/env bash
# F4 test + mutant (loop_restarts = 0 must fail it)
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT/engine
cargo test --release --test last_preview the_progress_restart > $R/f4.log 2>&1 9>&-; echo "f4 rc $?" >> $R/rc.txt
sed -i 's/p.loop_restarts = restarts as u32;/p.loop_restarts = 0;/' src/topo/core.rs
cargo test --release --test last_preview the_progress_restart > $R/f4-mut.log 2>&1 9>&-; echo "f4 mutant rc $? (0 = SURVIVED)" >> $R/rc.txt
git checkout src/topo/core.rs 2>/dev/null || true
echo "all done" >> $R/rc.txt
