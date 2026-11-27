# HANDOFF bundle-turn r3 → next round

Read scratch/agent-wt/reports/bundle-turn-r3-20261127-2310.md first (r2: bundle-turn-r2-20261127-1705.md). The branch has
not been pushed. Do not push this file.

## Done in r3

- **Classification of the user's 10 qm detours.**
  - None of them is a crossing-pass miss.
  - HDMI5V and DDCSDA are via-vs-length trades the objective makes → 5d.
  - PLVDD, ESD_BYP and JS_CS_5V are forced, and the ghost metric does not see the blocking pads. CEC is mostly forced.
  - TX2- and TXC- are diff pairs.
  - JS_INTB_5V and JS1_3 are local optima of the user run. Det s0 does not reproduce them.
- **The crossing pass on qmtech-bh26.** XRDBG shows that every refused group or split part adds predicted
  violations. `crossing_split` stays opt-in.
- **Attribution on qmtech-bh26 (the user's rules).**
  - All of the branch's new detours come from item 4. With oblique 0 they match main.
  - The JS2 bundle under J4 is forced. JS2_6 has a U-turn spike, and it is not item 1.
  - JS_INTA_5V has via sites placed far off its path.
  - Compass: drawing_loss 21 → 31 and planner_overfull 20 → 12. So item 4 under-predicts the room.
- **cpu, bit-identical.**
  - octi_straighten skips runs that `refit_run` refuses whatever the form (`Drawer::refit_counts`).
  - Test: `straightening_skips_a_run_the_refit_refuses`. The mutant is killed.
  - The copper hash is unchanged on qm-bh26 and on esp.

## Open, in priority order

1. **Item 4 under-predicts at J4 (qmtech-bh26).** Write a design note before any code.
   - Measure `reach_factor` against the drawn lane pitch on the JS2 bundle edges (TOPO_LANEDBG, wires 844–846).
   - Suspect: the sleeve cone gives turning room along a pin row that the drawing cannot use.
   - Or the lead decides to make item 4 opt-in.
2. **cpu.** Re-time on the old qm board in a ccx33 slot (the +42 % was measured there; on bh26 the tip is faster than
   main). Split draw_s per stage.
3. **From r2.**
   - tw5 seed 2.
   - F1 open_strip wall.
   - Cleanup re-run.
   - esp real r13.
   - The lead's OK on the loop_local / site_social pins.

## Tools

- tools/bundle_turn_r3_rca.sh. Modes:
  - clean / dbg / split / fork / obl0 / prov / board / tests
  - top / top2 / top3 / vnew / vbase
- Scratch analysis scripts (not in git): `/tmp/claude-1000/.../scratchpad/bt3/{crop.py,ov2.py,conns.py,excess.py,pl.py}`.
  - `excess.py` lists drawn − plan taut per connection.
  - `conns.py` gives the per-connection table across runs.
- Main baseline for qmtech-bh26: `ladder_diff.py cache:316e86ef <dir> --board qmtech-bh26`.
