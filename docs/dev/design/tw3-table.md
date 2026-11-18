# tw3-table: one variant table for all runs, long descents, the four bests drawn

Branch `tw3-table` (off main 34877dc0 = TW1 merged; merged with main 01e85fce, which carries F6/F7). Stage TW3 of the topor-way plan (section 4,
Appendix E, Appendix F points 8-12, D5, D6, D7). Sources: TopoR 6.3 manual
(docs/literature/eremex2017-topor63-user-manual.pdf) pp. 72-73 (variant table, criteria, sorting,
25 mm/via, the user picks); Polubasov 2010 «Многовариантная оптимизация» (fig. 13; "periodically stops
working on the current variant and picks one from the table at random"; descents independent, so
parallel from one table).

## What changes
1. **One table (Appendix F 8, 9).** Today every run (worker) has its own `Archive<Gen<LoopVar>>`
   (core.rs `route_round`). New: one `SharedTable` in `Coord`. Rows keep the existing Pareto rule
   (`archive.rs` covers/dominates, unchanged). **Never pruned:** the four bests, by the predicted
   criteria: *overall* = fewest ranked errors (shorts first among equals), then least
   `length + mm_per_via x vias` + penalty (reads `[optimize] mm_per_via`, D5); *fewest errors* =
   ranked errors, shorts, penalty, then the overall value; *shortest* and *least vias* = among the
   rows with the fewest errors. The cap `variants` applies to the other rows (worst by the overall
   order goes). A row carries its origin run and a state fingerprint.
   **Determinism:** offers are buffered and committed in (cycle, run) order once every live run has
   finished that cycle (an ended run counts as past every cycle). The table a run reads at cycle c
   is T(c-1), the commit of all offers of cycles <= c-1: the same on any thread count and load.
   A run waits for T(c-1) only at its sync points (restart checks), giving its compute slot back
   while it waits (like the DrawGate); the wait is bounded by stop/limit checks.
2. **Long descents (F 10, 11; D6, D7).** `[optimize] restart = "periodic"` (new default) |
   `"stall"` (today: `loop_restart` flat cycles) | `"off"`. Periodic: run k checks at every period
   boundary; the period is a **work clock** (re-lays tried in the descent, deterministic; a re-lay
   is the unit of time after TW1) `restart_relays` (default 2400: about 8 qmtech, 25 esp cycles),
   never shorter than `restart_min_cycles` (default 8); run k's first boundary is staggered by
   k/runs of a period. At a boundary the run restarts **only when it is free**: its descent made no
   new table row during the period, or its state equals the state of a lower run (same fingerprint
   in T). A descent that still gains is never cut ("a restart adds a descent to a free worker
   instead of replacing one"); a free worker gets the new descent. Start row: seeded uniform over
   all rows of T(c-1) (worse ones too) except a row equal to the run's own state; none left = no
   restart (counted). The run's own state is never reset to a best otherwise (F 8).
   Workers = `[optimize] runs` (thread-independent, as today); threads beyond the runs stay refine
   lanes: using them as extra descents would make copper depend on the thread count (decision Q1).
   **Stop:** headless "stable" = the table's four bests gained nothing over `loop_stable_window`
   committed cycles **and**, since their last gain, >= 2 period checks found a descent free
   (restarted, or no other row to go to) (D6), decided at sync points. Counted since the last gain,
   not inside the window: a run's checks are >= `restart_min_cycles` apart, so two never fall in a
   window of 8 (found on the VM: eprom with runs = 1 never got stable). The
   dialog never stops by itself and has **no cycle cap** (`rounds` is ignored for the loop when
   `stop_when_stable` is false; the 600 s limit, OK and Cancel end it). Headless `rounds` stays the
   work cap for comparisons.
3. **Four bests drawn (F 12; D5).** The DrawGate no longer chooses runs by their own predicted
   picks: when every loop has ended it takes the distinct rows among the four bests of the final
   table (priority overall, fewest errors, shortest, least vias) and gives each to one run
   (origin run first when it has no row yet, else the least loaded compatible run, so the
   drawings run in parallel; compatible = same pre-loop state:
   identical set of connections failed before the loop). That run restores the row and draws it,
   so up to 4 drawings run in parallel on the existing slots. `[drawing] candidates` = how many of
   the four are drawn (new default 4; 1 = the overall best only). After Cancel / OK only the
   overall best is drawn (as today). `route_topo` compares the drawn rows at 100 % drawn
   (`obj.better` on the drawn criteria). `[optimize] pick = "best"` (default, the D5 order;
   `errors_weighted` an alias) | `"errors"` | `"shortest"` | `"vias"` says which drawn row goes to
   KiCad; `errors_vias_length` stays as the old lexicographic order. Report line + `result.json`
   `bests: [{role, row, predicted, drawn, applied}]`; the dialog's summary lists the four.
   With runs < distinct bests only the first `runs` of them are drawn (said in the report).
4. **F6 / F7 (exact; done on branch tw3-f67, merged to main 01e85fce).** F6: a spatial index for the pairwise segment tests of the drawing and the
   via reduction (seg_clear_w, seg_seg, Grid::query_counted) and FxHash for their maps; F7: bbox
   prefilter + index for planes.rs in_piece / in_fill_on / far_candidates. Copper hash = main.

## Lead conditions (2026-11-18, approved Q1-Q4)
- **Penalty (condition 2).** Criteria.penalty = the `violation` factor's graded price beyond 1 per
  violation (predicted: `viol_mm x (graded - count)`; drawn: the depth x length price of the
  written violations and the open connections), in mm at the `violation` weight (default 1000 mm
  per unit). D5 is strict: overall = ranked errors, shorts, then `length + mm_per_via x vias`, then
  shrinkages. The penalty is NOT in it by default; `[optimize] best_penalty = true` adds it (then
  "best" orders exactly as the old `errors_weighted`). Reason for the option: within one error
  count a shallower violation is closer to legal (graded factors); the user decides. The *fewest
  errors* best keeps the penalty as its second key (that is what makes it differ from overall).
- **Each distinct row drawn once (condition 1).** Two bests on one row = one drawing. A run draws
  its first assigned row in its own round; further rows (runs < distinct rows, e.g. runs = 1) are
  drawn by the same run one after another in a draw-only round (no initial routing, no loop: the
  row's state restored into a fresh round context). The report gives the drawing cost per board
  at runs = 1 / 1 thread (four drawings serial); if it is over the reserve the fix proposal goes
  in the report (F6/F7 first; then draw the overall best plus only the bests that differ in
  predicted errors, `[drawing] candidates`).
- **Drawn variants are row sources (TW2).** A drawn row gives the variants the drawing makes (as
  drawn, after reduction + fuse; TW2 may add "as drawn + pure fuse"). Each is pour-stitched and
  judged by D5; the row's drawn value is the best of them. The pour check is folded into the
  table this way (no snapshot of "as drawn" after the polish); a new drawn variant from TW2 is one
  more source of the same row.

## Invariants and tests
- Table: the four bests are never pruned (property test over random offer streams, any cap >= 1);
  no row dominated by another except a protected best; commit result independent of the arrival
  order of offers (shuffled arrival = same table; mutant: commit on arrival).
- Restart: never to the run's own state; a gaining descent is never restarted; seeded (same
  seed = same restarts); `restart = "stall"` + per-run table reproduces today's loop on the toy.
- Transfer: a row drawn by a non-origin run gives copper identical to the origin run drawing it
  (fixture, 2 runs); thread count 1 vs 4 vs 8: identical copper and table (fixture + esp).
- Stop: headless stable needs >= 2 restarts in the window; the dialog run reaches no cycle cap.
- F6/F7: copper hash = main on 5 boards (rounds=1, seed 0), timing on an exclusive ccx43.
- Boards (rounds fixed, runs 5): the TW3 row of the plan; every changed connection judged.

## Decisions for the lead
- Q1: free threads beyond `runs` are NOT used as extra descents (keeps "same copper on any
  thread count"). Alternative: `[optimize] runs = "auto"` = threads (then thread-dependent).
- Q2: "time based" = the deterministic re-lay clock (`restart_relays`); a wall-clock period would
  break the fixed-cycle comparisons. Default 2400 re-lays, floor 8 cycles: OK?
- Q3: default `pick` order changes to D5 (errors, then length + mm_per_via x vias): copper changes
  on every board where today's errors-vias-length order picked differently (expected, D5).
- Q4: `loop_time_share` 0.5 kept in this stage (four drawings run in parallel, so the reserve is
  about one drawing); the TW0-measured reserve comes later.

## Lead decisions 2026-11-18 (after r1 / r2)
- **Hybrid restart (default `restart = "hybrid"`).** A stall (main's trigger: `loop_restart` = 2 cycles without
  a gain of the run's own pick, no cycle floor) restarts the run through the shared table: a row drawn at
  random (seeded) among the compatible rows, never the run's own state. The periodic re-lay check stays as
  the backstop with its 8-cycle floor. Every restart or free check starts a new descent (stall count and
  period reset). Without a table (`runs` with no shared table) the hybrid is main's stall restart.
- **`restart_relays` 2400 -> 800** (measured: ~90 re-lays a cycle on esp, ~320 qmtech, ~57 wiccon, ~27 eprom).
- **Comparisons at an equal re-lay budget:** `[optimize] loop_relays` (new, 0 = off) ends each run's loop
  after that many re-lays; the budget per board and seed = main's re-lays in a 600 s wall run on the ccx43.
- **`stable_gain_pct = 0` means never stable** for the shared table too (its window is unbounded then),
  as for the run's own meter.

## Result at an equal re-lay budget (r4, 2026-11-18)
Main dcf4c0ef vs this branch, esp s0-4 / qmtech / wiccon / eprom, wall 600 s, runs 5, budget = main's
re-lays (report scratch/agent-wt/reports/tw3-table-20261118-1915.md). The tip's plans are better by their own
measure (esp predicted errors 24 -> 8, vias 194 -> 147) and draw 53 fewer vias (284 -> 231) and 111 mm less copper on esp;
KiCad DRC rises 692 -> 936 (distinct net pairs 180 -> 223). Per connection the rise sits in nets that fail
on both sides on different seeds (drawing next to different neighbours) and in two connections: esp s3
IO21_TX (the plan drops a via for +6.4 mm through the crowded U3/U4 area) and qmtech FPGA_SD_DATA1 (same
plan, drawing only). Every restart was a stall restart; the 800 re-lay period never fired.
