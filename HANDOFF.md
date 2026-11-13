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

## Verifier follow-up (2026-11-13)
- eb1df2b stop latency: control::with_control / stop_requested; Topo::budget, Labels::apply, P3 refine
  stop per move. Test cancel_in_p3_p4_with_the_cut_model_ends_quickly: 16.6 s -> 0.36 / 0.30 s.
- 20ad7a8 edges+price label moves may add vias (add_vias = priced && cut_vias); P3 per-edge unchanged.
- dc01e31 TOPO_PLANDUMP / TOPO_DUMP per seed.
- Evidence: old options vs base 09c18ca s0 IDENTICAL (eprom 37, wiccon 245 items); base runs in
  base-runs/09c18ca-{eprom,wiccon}-default. Kept new-default wiccon s2: wt-m0-runs/new-keep-wiccon,
  dumps in wt-m0-runs/dumps (picked seed 3985886997381813726). RXD/GPIO0 shorts and CS0 clearance =
  drawing losses (plan: no crossing, foreign 0, room >= need on B.Cu); RXD 2x detour = plan (99.2 mm
  planned vs 50 mm straight).
- Open (M2 decision): Cancel / limit before the TopoR loop still leaves no copper.
