#!/usr/bin/env bash
# tw0-prof2 fix round 2: targeted release tests (prof lib test + topo_prof) and the new mutants m1/m2/s1/m3.
set -u
R=${HZ_RUN_DIR:-/tmp/tw0r2}; mkdir -p $R
SRC=$(pwd)
( cd engine && cargo test --release --lib prof 9>&- > $R/lib.log 2>&1; echo "lib rc $?" >> $R/rc.txt
  cargo test --release --test topo_prof 9>&- > $R/base.log 2>&1; echo "topo_prof(base) rc $?" >> $R/rc.txt )
mut() { # name file sed-expr test-args
  W=/root/tw0r2-mut/$1; rm -rf $W; mkdir -p $W
  rsync -a --exclude target --exclude .git --exclude scratch $SRC/ $W/; find $W -name '*.rs' -exec touch {} +
  sed -i "$3" $W/$2 || { echo "$1 sed failed" >> $R/rc.txt; return; }
  cmp -s $SRC/$2 $W/$2 && { echo "$1 NOT APPLIED" >> $R/rc.txt; return; }
  ( cd $W/engine && CARGO_TARGET_DIR=/root/tw0r2-mut/target-$1 cargo test --release $4 9>&- > $R/mut-$1.log 2>&1; echo "mutant $1 rc $? (0 = SURVIVED, 101 = killed)" >> $R/rc.txt )
}
mut m1_no_pairmove_lap engine/src/topo/core.rs '/prof::lap(&mut plap, super::prof::K::StPairmove);/d' "--test topo_prof" &
mut m2_no_assign_local_guard engine/src/topo/layers.rs '/pub fn assign_local(/,/^}/{/let _pg = super::prof::g(super::prof::K::AssignWith);/d}' "--test topo_prof" &
mut s1_no_finish_mark engine/src/topo/prof.rs 's/^    mark("finish");$//' "--test topo_prof" &
mut m3_mono_key engine/src/topo/prof.rs 's/num("tv_sec")?/num("tv_secs")?/' "--lib prof" &
wait
echo "all done" >> $R/rc.txt
