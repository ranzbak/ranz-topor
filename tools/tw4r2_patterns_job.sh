#!/usr/bin/env bash
# tw4r2_patterns_job.sh (branch tw4-r2): render the two proposed TW4 r2 patterns (harness/tw4r2_patterns.py) on the VM
# and check them with KiCad: kicad-cli pcb drc --refill-zones on reference and careless (expected: reference clean,
# careless shows the problem). Output in $HZ_RUN_DIR/pat.
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
python -m harness.tw4r2_patterns $R/pat > $R/gen.log 2>&1; echo "gen rc $?" >> $R/rc.txt
K=$R/kicad-home; mkdir -p $K/.config/kicad/10.0  # isolated KiCad home (kicad-cli rewrites its config)
for f in $R/pat/*/{reference,careless,problem}/*.kicad_pcb; do
  HOME=$K XDG_CONFIG_HOME=$K/.config XDG_DATA_HOME=$K/.local/share XDG_CACHE_HOME=$K/.cache XDG_STATE_HOME=$K/.local/state \
  KICAD_CONFIG_HOME=$K/.config/kicad kicad-cli pcb drc --refill-zones --format json --severity-all --units mm -o ${f%.kicad_pcb}.drc.json $f >> $R/drc.log 2>&1
  echo "drc $f rc $?" >> $R/rc.txt
done
echo "all done" >> $R/rc.txt
