# HANDOFF bundle-turn fix round r2 → next agent

This handoff picks up from report scratch/agent-wt/reports/bundle-turn-r2-20261127-1705.md. Read that report first.
The tip is the last commit on this branch. The branch has not been pushed.

## Done

- **Item 1, fork zone.** It is now a tent 2·gap deep, and a zone over the run's own end is dropped. Both live in
  octi.rs at the fork block.
  - The dvn avdd, m3, grace and first_draw_45 tests all pass.
  - turn_fork_corner asserts that N2 has 3 bends.
- **Killing tests.**
  - exact.rs: the item-2 test.
  - pack.rs: the `reach_factor` test.
  - All 7 mutants are killed. The verdicts are in hz-runs/bt2g/rc.txt.
- **Stale tests re-expressed.** dvn pad_off_centre, thick_layer ×6, loop_local depth_two and site_social now pin
  `oblique_price` 0. Each one has a reason in its doc comment.
- **Docs.** The sleeve rule is written into the design note and the code docs, and the schema has been regenerated.
- **Option.** `[optimize] crossing_split` was added in core.rs crossing_pass. It is opt-in (default 0).
  `TOPO_XRDBG` prints the group judgements.

## Open, in priority order

1. **Item-4 detours (verifier F2).**
   - Mechanism found: the crossing-rules pass is judged as one batch. Under the oblique price, stage 1 is refused
     as a whole and the clinch resolutions are lost. XRDBG on qm shows this.
   - The split recovers the paths, but they draw dirty (JS_CS drc 11; wiccon 0 → 11).
   - Next steps:
     - Compare the predicted CapModel over-fill on JS_CS's direct path (tip vs obl0) with what is drawn there. Is
       the item-4 sleeve factor under-predicting the U5 approach?
     - Check whether the pre-loop priced judge's trade (~65 mm per predicted violation) is the real culprit.
   - esp IO4/IO8/IO10 have no RCA yet. Crop: hz-runs/bt2g-b/crops/esp/IO4.png.
2. **tw5_escape loop_p4_rounds_fold seed 2.** It fails at the defaults, caused by item 4 alone. Possibly real; not
   changed.
3. **cpu +42 %.** Item 4 is the main cost: obl0 ≈ main, and the fork cost shows only with item 4. Steps:
   - Split draw_s per stage (straightening 22.7 → 43.8 s).
   - Re-time on a ccx33 slot.
   - Profile with frame pointers (prof_run.sh --flame needs inferno on the VM; perf without frame pointers gave
     self time only).
4. **Not re-run.** esp real r13, Cleanup per net qm/esp, wiccon vias / esp GND island (esp unconnected 8 → 12 comes
   from item 4).
5. **Lead's OK needed.** The pins in loop_local and site_social (weaker evidence than the others).

## Notes

- **Do not edit a job script while its job runs.** bash reads the script lazily; editing it killed the bt2f top
  with exit 2.
- **hz.sh sync replaces the tree.** It "adopts" an existing tree in place, so a main tree synced for a qsub is lost
  at the next job. Keep main copper from vbt1 (uploaded by ssh).
- **qsub quoting.** hz.sh flattens it, so `bash -c '...'` arguments break. Put the commands in a script file.
