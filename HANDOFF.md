# M0 handoff (branch defaults-m0, 2026-11-13)

Commits on top of main 09c18ca:
- bab3ff9 M0 defaults = TopoR method; hard capacity refusals become prices (pre-reboot)
- 174ee4b cancel_latency waits for the phase it means; progress stage "TopoR loop on the topology"
- (next) layers.rs: a wire placed through another net's wall is layered (no panic); test topo_wall_layers

Measurements (score_runs, cpu 2400, RAYON 4):
- eprom/wiccon/dcs base+new: pre-reboot (numbers recovered from the transcript, run dirs lost)
- qmtech base s0-s4: scratch/agent-wt/base-runs/09c18ca-qmtech-default (shared)
- qmtech new s0-s1, new-edges (panic x2), new-edges-refuse s0, new dcs s0 rerun: scratch/agent-wt/wt-m0-runs/
- tools: wt-m0-runs/tools/{score.sh,agg.py,loopinfo.py}

Key findings: see the final report (qmtech new default: the cut model's P4 eats the whole
CPU budget, the TopoR loop never starts and no signal wire is drawn; dcs: loop gets 1 cycle,
pour stitching skipped after the limit -> GND unconnected 15 -> 30).
