#!/usr/bin/env bash
# fx_full.sh <run dir> <tree>: the whole engine suite once (release) with the new tests in it
set -u
R=$1; T=$2; W=/root/fxfull
mkdir -p $R
rm -rf $W; mkdir -p $W; rsync -a --exclude target --exclude .git --exclude scratch --exclude .venv $T/ $W/
cd $W/engine && CARGO_TARGET_DIR=/root/fxfull-target cargo test --release --no-fail-fast > $R/cargo-release.log 2>&1
echo "cargo release rc $?" >> $R/rc.txt
