# tw7-moves: pattern detectors that trigger one targeted move each; clean-up of what the loop then covers

Branch `tw7-moves` off main 12699012. Plan: ~/.claude/plans/topor-way-plan.md, row TW7 (T7; replaces Q8 part,
6b remainder, M6 phase 3; F8 P2/P4 caches). Inputs: tw4-r2.md "(3) result and TW7 input" (pour repair move);
the repeated detour nets of the TW4 r2 and TW5 A/Bs. **Design note only. No engine code before the lead's reply.**
TW6 (`tw6-via-sites`, docs/dev/design/tw6-via-sites.md) is in flight; its part E adds `site_move` / `branch_move`.
Data and scripts: scratch/agent-wt/reports/tw7-moves-data/ (detlist.py, detagg.py, detclass.py, cls-*.json, agg.json).

## 1. Baseline (existing runs only; numbers from ladder_diff --json)
Deterministic baseline = main copper 12699012 (= b9d88855): verifier run vtw4r2m-ecb3a5c `K`
(rounds=1, runs=1, 1 thread). `ladder_diff.py hz-runs/t4r2m-56808de2/K hz-runs/vtw4r2m-ecb3a5c/K --json --extra`:
detour_gt_1_5 eprom 0, wiccon 1, esp **15**, qmtech **29** (copper = base on all four). The plan's B (19 / 25) is
older; TW7 judges against these. Per connection (detclass.py on the routed copper, all layers projected; L = the
straight pad-to-pad line; "round" = the net's copper crosses the other net's copper 0 times although L does;
"half-loop" = >= 2 times):

| class | esp | qmtech | who owns it |
|---|---|---|---|
| pour / plane end (+3V3 x4, +5V, +BATT, VBUS; qmtech 1V8) | 7 | 1 | part E (pour repair) if the pour piece near the pad is cut off; dump decides |
| foreign pads only on L (USB-C A6/B6, A7/B7 bridges; ADV7511 pins 21-25, 29-34; J11 A6/B6) | 2 | 3 | nobody: a fixed pin between; via or way round. 3 have ghost = 1.00 mm (clamp?): metric finding |
| nothing on L (USB_D- D5.1-J1.B7) | 1 | 0 | pad exit / shape, not TW7 |
| foreign wires on L, net goes **round** them (clinch / fig. 21b form) | 1 | 17 (+1 crosses) | parts B, C |
| foreign wires on L, **half-loop** with one of them | 4 | 8 | part B |
| same-net self-crossings in the projection (fig. 15 loops; ADV_DVDD 7, ADV_CEC, JS1_2, MAX_I2S0) | 0 | 4 nets | part C (after TW6 E) |

14 of the 26 qmtech wire-class detours have 0 vias. So reaching the plan's targets (esp <= 8, qmtech <= 10) needs the
wire classes nearly all, plus 0 (qmtech) to 2 (esp) pour ends: tight on esp, ambitious on qmtech. Board-wide
crossing rules today (same runs, warnings): esp 179 patterns found, 48 pairs re-planned, 9 resolved in 1 kept pass;
qmtech 828 / 271 / **0 kept**; wiccon 115 / 46 / 0. In long runs the crossing pass is 52.0 % of esp's wall, 24.9 %
of qmtech's, 54.7 % of wiccon's (reports/tw0-prof-20261118-long/compare.md): the second-largest block, nearly fruitless.

**The repeated A/B detour nets** (detagg.py over t4r2q, t4r2p, t4r2e, tw5r12, tw5r14, tw5x1, tw5x2; 300 s / 16
cycles; ratio and vias of the detour runs vs the others):

| connection | > 1.5 in | detour runs | other runs | reading |
|---|---|---|---|---|
| qmtech SDRAM_D13 IC1.50-U4.U218 | 33/50 | 0 vias, 37-41 mm | 1 via, 23 mm | 1 via for 15-18 mm (fig. 21b) |
| qmtech SDRAM_D12 IC1.48-U4.U220 | 4/50 | 0 vias, 41 mm | 1 via, 22 mm | same |
| qmtech JS1_1 J3.1-U6.8 | 22/50 | 0 vias, 33-42 mm | 1 via, 21-28 mm | same |
| qmtech JS2_9 J4.9-U6.27 | 4/50 | 0 vias, 34-39 mm | 0-1 via, 24-30 mm | same |
| esp IO7 R10.1-U3.21 | 39/78 | 0 vias (29 of 39), 5.6-6.9 mm | 2 vias, 4.0-5.4 mm | a via pair for 1.5 mm |
| esp IO8_SCL U4.13-U4.18 | 16/78 | 0 vias (12 of 16) | 1-2 vias | same |
| esp ENABLE U4.27-U5.1 | 28/74 | 1 via (21 of 28), 25-34 mm | 2-4 vias, 17-24 mm | 1-3 vias for ~10 mm |
| esp IO5 U3.19-U4.10, IO6 U3.20-U4.11 | 11/78, 31/78 | 1 via | 1 via | same vias, longer: clinch / half-loop |
| qmtech ADV_DVDD 49-C22.2; C23.2-L1.1 | 29/50; 16/16 | 1 via; 0 vias | 1 via | same vias: clinch, and a tree net |
| qmtech Net-(U7-PVSS) U7.20-U7.22 | 41/41 | 0-2 vias, 3.6-5.0 mm | (none) | foreign pin 21 between: fixed class |
| esp IO10_SDA U3.16-U4.17 | 25/65 | 0-2 vias | 0-2 vias | mixed |

Reading: the loop's objective prices a via at 25 mm (`mm_per_via`, D5), so 1 via for 15 mm is a gain to it. Every
option the A/Bs tried changed the trajectory and landed on such a trade, so the no-detour rule kept it opt-in. The
TopoR answer is not a lower via price but the state the trade misses: fig. 21c (no via, +7 %), which "cannot be
obtained by a single reroute" (Luzin 2015 pp. 9-10). Our `pair_move` reaches 21c only from 21a (a via pair still
there); from 21b (the via already traded) nothing offers it.

## 2. Literature (checked in the PDFs)
- Luzin 2015 p. 7: "Some topological conflicts remain, because rerouting wires one by one cannot eliminate them.
  Identifying a specific topological situation ... takes far less time than finding all possible wire routes";
  loops (fig. 15: same-net projections cross), half-loops (fig. 16: different-net projections cross several times),
  clinches (fig. 17) "are easily spotted". p. 8: clinch signs a) an edge between two objects crosses a wire incident
  to one of them (fig. 18), b) an edge between equipotential objects is crossed by other nets' wires while their own
  wire does not run along it (fig. 19, U-clinch), c) mutual evasion (fig. 20). pp. 9-10 fig. 21: re-layering removes
  no via; a single re-lay removes them at +36 %; 21c no vias at +7 %, not reachable by single re-lays.
- Luzin, Popov & Popov 2012 pp. 116-118: signs 1-4 (sign 3: a wire crosses one triangulation edge more than once;
  sign 4 measured by the wrap angle between entry and exit of a vertex neighbourhood); removal p. 118: regular clinch
  = lay the interfering wire on the other side of b1, connect the long wire directly (also for a long path of several
  wires: remove the wire at b1, branch); U-clinch = interfering wire to the other side of b1, long wire along the
  edge; several interfering wires "in turn"; implicit clinch = swap fragments between the two neighbourhoods. p. 119:
  check widths. p. 119 (conclusion): TopoR removed clinches after autorouting and planned to do it "in the process
  of optimisation" — which is TW7.
- Luzin, Popov & Popov 2013 p. 134 step 8.1 (our `pair_move`): lead the crossers round u.
- TopoR 6.3 manual pp. 177-178 (Refine, F6: re-lays shorten wires moved by automatic procedures; submode with layer
  change), p. 179 (F7 submode 2: vias moved with re-routing), p. 73 (criteria: violations, vias, length, shrinkages).
- Polubasov 2010 «Перекладка проводников»: coefficients per wire from the quality of its path (basis of part D).

## 3. Model
**Detector -> candidate -> one move -> judged alone.** A detector reads the topology and returns candidates
`Cand { kind, subject: Option<WireId>, movers: Vec<WireId>, held: Option<HeldSpan>, price_before }`. Each candidate
is one move: take the movers out (with `tree::dependents`), place the subject first if any (others detached), hold
its stretch if any, lay the movers again in route order with their own coefficients at the end-of-cycle via weight
(`Sp::conn_reg`), local layering (`self.local`), keep when `eval + graded price` drops (the `pair_move` rule; TopoR
table protects the bests, no board gate). Kinds and their move:

| kind | detector (sign) | move | exists today |
|---|---|---|---|
| TerminalClinch | edge b1-x crosses a wire incident to b1 (sign 1 / a) | subject = the clinched wire first, movers = interferers | `xrules::terminal_clinches`, batch only |
| UClinch | equipotential edge crossed by other nets, own wire not along it (sign 2 / b) | movers to the far side of b1, subject along the edge | no |
| MutualEvasion | wrap angle at both ends (sign 4 / c) | joint re-lay both, both orders tried | no |
| HalfLoop | different-net pair crossing >= 2 (fig. 16) | shared edges: `uncross_double`; else joint re-lay in rule order | `double_crossings`, batch only |
| Unlayerable | `lr.unlayerable_pairs` | joint re-lay other order | batch only |
| ViaTrade | wire with graded detour amount and foreign wires on its direct homotopy, no via pair (fig. 21b) | subject laid on its direct path, movers = the wires it crosses there, held at its layer; movers go round its nearer terminal | **no** (the gap of §1) |
| ViaPair | `obstructed_pairs` (fig. 21a) | `pair_move` | yes, per move |
| SameNetLoop | same-net projections cross, or a wire crosses one edge twice (sign 3, fig. 15) | re-branch: the later arm joins at the crossing | no; = TW6 E `branch_move` with another trigger |
| PourCut | pour cut-off component (tw4-r2 (3)) | movers = foreign wires bounding it; the component's edges priced for the move | no |

Order per cycle: candidates sorted by the summed graded detour amounts of their two wires, largest excess length
first (ties by wire ids), budget = the cycle's `max_ripped` like pair_move; a wire moved in this cycle's pattern stage is not a mover again in it
(re-lay counter, Polubasov fig. 11). Graded, no cutoff: the detour amount is `ghost * (r - 1)^2` with `r` = topological
length / ghost; the move set size enters the price (work), it is never refused by a count.
**Incremental detection (F8a):** per-wire candidate cache keyed by the wire's path and the paths it crosses;
refreshed for the wires a move or re-lay changed (`took_part`, `set_lr`'s changed set), not the whole board.

## 4. Parts, functions touched, overlap with TW6
| part | what | functions | TW6 overlap | start |
|---|---|---|---|---|
| **A** detect + measure | `Cand`, all detectors read-only, incremental cache, `TOPO_PATDBG` dump: per detour connection which detector fires; sub-counters of today's crossing pass (detect / replan / assign+score / clone) | xrules.rs (`Pair`->`Cand`, `Kind`, `terminal_clinches`, `double_crossings`, new `u_clinches`, `mutual_evasions`, `same_net_loops`, `via_trades`, `Detect::{new, refresh, cands}`); state.rs (new `edge_passes`, `wrap_angle`, `direct_crossers`); prof.rs (keys); optimise.rs `CycleStats` | none (TW6 does not touch xrules/state crossings/prof) | now |
| **B** targeted crossing moves | loop stage 4: per candidate judged alone, local judge, instead of the batch pass + `p4_better` gate (Appendix F 4); kinds TerminalClinch, UClinch, MutualEvasion, HalfLoop, Unlayerable via `Topo::try_joint` (n movers: `try_joint_n`) | core.rs `crossing_pass` closure (~3742-3821, pre-loop stays as is), `Sp::cycle_body` stage 4 (~5652-5668), new `Sp::pattern_move`; moves.rs `try_joint`, `try_joint_n`; warnings/plan metrics (~6072, ~6204) | text only: cycle_body lines next to pair_move; E inserts after pair_move | now |
| **C** fig. 21b + loops | `ViaTrade` move and `ViaPair` on one core; `SameNetLoop` move | core.rs `Sp::pair_move` / `obstructed_pairs` -> **E's held re-lay** (see decision 1); new `Sp::via_trade_move`; tree.rs read (`dependents`, `search_tree`) through E's `branch_move` | **yes**: E's `site_move` (ii) is the same "hold a stretch, re-lay its crossers" core; `branch_move` is the loop move | after TW6 E |
| **D** detour coefficient | `[factors] detour` (default 0): a wire whose last path had amount `(r-1)^2` > 0 gets its length coefficient raised by `1 + k (r-1)^2` at its next re-lay, oscillating like the others (Polubasov: per-wire k from path quality). Additive search stays exact. | optimise.rs `conn_reg` / `wire_amp`, factors/length.rs read, model.rs, schema.py | none (TW6 B adds `via_site_social` in init.rs; D does not touch init.rs) | now |
| **E** pour repair | detection now: `pourplan::cut_components` (component, layer, cut pads, bounding wires) beside `pour_cuts`; move after C: movers = bounding wires, the component's edges priced through the loop-history overlay (TW5 (c), already in the search), no new search term | pourplan.rs, pourcheck.rs (`Cut` read), core.rs move | detection none; move uses C's core; refine.rs `MeasureParts.pour` text-near TW6's `Frame` edits | detect now, move after C |
| **F** clean + F8 | removals (§6); F8b caches from a fresh TW0 profile of P2-P4 (`poly_dist` callers `board::exit_stub`, `lead_len`, `via_end_on`, `pack::margin_raw`); F8c `Packer::via_site` cache | as listed | F8c = TW6 A's `site_room` cache (pack.rs): **after TW6** | after B (+C) |

## 5. Options and defaults (judgement calls; defaults only after the equal-time A/B)
| option | part | default now | meaning |
|---|---|---|---|
| `[optimize] crossing_rules` | B | `true` = today | unchanged bool: the pre-loop passes (stage 0/1) and, with `pattern_moves` empty, the in-loop batch pass |
| `[optimize] pattern_moves` | B (C, E later) | `[]` | kinds that become per-candidate moves in the loop, replacing the in-loop batch pass: `clinch`, `double`, `unlayerable`, `u_clinch`, `evasion` (later `via_trade`, `loop`, `pour`); unknown entries warned |
| (no `pattern_work`) | B | - | dropped in the work round: the moves share the cycle's `max_ripped` budget and `stop()` like pair_move (fewer knobs) |
| `[factors] detour` | D | 0 | per-wire length coefficient from the last path's detour amount |
Flip rule (lead/TW6 style): a default flips only with no new det > 1.5 and KiCad new not up, per connection.

## 6. Clean-up list (M6 phase 3; each removal needs the user's approval)
| # | entry | covered by | when |
|---|---|---|---|
| R6 | the in-loop batch crossing pass and its `p4_better` / eval gate (core.rs cycle_body stage 4; Appendix F 4) | B | after B's A/B |
| R7 | pre-loop crossing passes (stage 0, 1) | B in the loop | after B's A/B; option one release |
| R8 | 5.4 `pair_min` re-lay half (`cs.pair_relaid`, core.rs ~5671-5702; default off): it *is* the fig. 21b single re-lay | C | after C |
| R9 | 5.4 `[search] branch_relocate` + `split_at` retry (M6 W5 N4, default off; a measured detour cause on wiccon) | TW6 E + C loop move | after both |
| R10 | `Topo::try_joint` 2-wire form | `try_joint_n` | with B |
| keep | `Labels::pair_lifts` (label move, TW5 territory), W1 never-drop ladder, W2-W4 sequential fallback stand-ins (not loop work: ERBS fallback 3.8-12 % of runs), keep list F14-F19 | - | - |
| ask | `loop = "cycles"` and its F1-F3 code (hint_map, soft_cuts, own history): the TopoR loop covers it | user decision | - |

## 7. Patterns (PROPOSED, PNG + text, not in the suite)
Rendered on the VM (tag tw7s-53a2bef, tools/tw7_sketch.py; each state's copper gap checked >= 0.20 mm):
`scratch/agent-wt/hz-runs/tw7s-53a2bef/sketch/{detour_trade,terminal_clinch,u_clinch}.png`.
- `detour_trade` (fig. 21b -> c): rails B1, B2 (0.5 mm, laid first) across A's line. (a) via pair: price 144.0;
  (b) careless, A round the rail ends: A x3.32, price 126.52; (c) expected, both rails dip round A2: A straight, rails
  x1.05 / x1.04, price 97.24, 0 vias. A local minimum of single re-lays; `pair_move` cannot fire (no via pair in (b)).
- `terminal_clinch` (fig. 18): M straight past N's terminal b1; careless N x1.69 (40.4); expected M round b1, N straight (30.69).
- `u_clinch` (fig. 19): M between N's equipotential pads; careless N x5.11 (37.45); expected M round b1 (or b2), N along the edge (22.11).
- Not drawn yet: `same_net_loop` (fig. 15) and a half-loop round an obstacle: built in part C / B from the qmtech
  crops that show them (ADV_DVDD; MAX_SCLK/MAX_I2S0/MAX_LRCLK), so they reproduce a real case; `pour_ring` is
  tw4-r2's proposal (reused for E). Existing guards stay green: via 6, pair_move (fig. 21), untangle 5, fit 7.

## 8. Acceptance (plan row TW7) and tests (next rounds)
- detours > 1.5 (rounds=1 K): esp 15 -> <= 8, qmtech 29 -> <= 10, every changed connection judged; the 4+1 fixed-class
  ones are named, not counted as failures of TW7. Equal time (300 s, 4 thr, esp s0-4 + qmtech s0): the §1 nets
  re-measured by name; then the TW4 r2 / TW5 opt-ins (via_disc_height, pad_exits, float_wire, preloop off) re-A/B'd.
- via patterns + pair_move + detour_trade + the two clinch patterns green; removed list above, approved.
- crossing/pattern stage share of a long esp run 52 % -> <= 20 % (TW0 tool), cycles per 600 s not lower.
- Tests: A: detector unit tests against brute force on small fixtures (every sign), incremental cache = full scan
  (fuzz), dump copper-equal (hash) on 4 boards. B-E: off = main copper hash; each move kept/undone exactly (restore
  bit-equal); thread independence; mutants per changed line.

## 9. Decisions for the lead
1. ViaTrade/ViaPair core: (a) TW6 E creates the held re-lay (from `pair_move`) and TW7 C builds on it (the brief's
   rule; C waits), or (b) TW7 extracts `Sp::held_relay` from `pair_move` now as a copper-equal refactor and E calls it.
   Recommend (a) unless E is more than a day away.
2. Part D (detour coefficient) as an option at all, or only the moves (TopoR has no detour criterion, the user rule does)?
3. Metric finding: ghost = 1.00 mm on USB-C A/B bridges (3 of the 44 baseline detours) looks like a clamp; harness owner.
Budget: this round EUR < 0.02 (one 33 s job on the shared ccx43).

## 10. Work round 1 (after the lead's reply 2026-11-19)
Lead: (1) C builds on TW6 E's held re-lay (`relay_kept`, tw6 8ccfb0e0) after E lands; (2) D stays `[factors] detour`,
default 0; (3) terminal_clinch + u_clinch approved, detour_trade re-drawn (45 deg dip at constant rail pitch, tag
tw7s2-17228bd) and sent back; loop = "cycles" (F1-F3) removal approved; R6-R10 approved if equal-time results stay
equal or better per connection. Implemented opt-in: A (detectors `u_clinches`, `mutual_evasions`, `same_net_loops`,
`detour_amount`, `attribute`; `TOPO_PATDBG` dump; prof keys `xr_detect` / `xr_replan` / `xr_judge` / `pattern_move`),
B (`Sp::pattern_cands`, `Sp::pattern_move` = TW6's `relay_kept` with an explicit order; fold the two in part F),
D (`Sp::detour_amounts`, length coefficient in `conn_reg_w`), E-detect (`pourplan::cut_components`). The
incremental detection cache (F8a) waits for the `xr_*` profile: only if `xr_detect` is a real share.

Results (tw7ab-b8c8b4e6, shared ccx43 at about 100 % load, so equal-time numbers are noisy):
- Off = main copper on eprom, wiccon, esp and qmtech (copper_hash, rounds=1).
- Crossing-pass profile (K, pre-loop): detection takes 0.1 % of it (esp 8 ms of 6.5 s), re-plans 89 %, judging 11 %. So
  F8a (incremental detection) is dropped: the cost is in the re-plans and the judge, which part B does per candidate.
- `TOPO_PATDBG`: every plan wire with taut ratio > 1.5 has at least one detected pattern (esp 10/10, qmtech 24/24).
  Mostly u_clinch and clinch, some same-net loops. Attribution is in reports/tw7-moves-data/patdbg-K-*.txt.
- Moves keep 3-5 % of their tries (esp 33/576, qmtech 52/1311 at 300 s). But they cost cycles: esp 17-22 -> 8-9 cycles,
  qmtech 8 -> 5. So the moves need a work share and a cheaper order before any default flip. Next round: per-cycle
  pattern work share and per-kind counters.
- Equal time sums (esp s0-4, qmtech s0), each as drc_new / unconnected / vias / detour>1.5:

  | cfg | esp base | esp | qmtech base | qmtech |
  |---|---|---|---|---|
  | u | 651/51/279/75 | 595/53/270/73 | 253/20/163/23 | 348/20/149/21 |
  | ud (with detour = 1) | 651/51/279/75 | 640/47/281/69 | 253/20/163/23 | 164/20/147/22 |
  | d alone | 651/51/279/75 | worse drc_new (838) | | |
  | ux (pre-loop passes off) | 651/51/279/75 | worse (825) | | |

  ud is the best of the four. Not judged per connection yet. Defaults stay off.

## 11. Work round 3: the pattern moves' work share (lead brief 2026-11-19)
What: `[optimize] pattern_share` (float, (0.01, 1], default 1 = no budget, as round 1) bounds the work of the
loop's pattern moves (part B) like TW5's escape throttle (`escape::Throttle`), as a graded budget, not a cutoff:
`escape::Share`. At each stage 4 the loop's other work since the last stage 4 (the work units of `work_since`:
locally layered and measured wires, `kappa` per wire of a global layering; never the clock) earns the moves a
credit of `O x s / (1 - s)`; each move spends its measured work (at least one unit per try: a try whose placing
failed still searched). The candidates run in the graded order of `pattern_cands` (most excess length first)
while the credit is positive; the rest wait for a later cycle (counted, `waited`). A move that overdraws is paid
back from the next cycles' work, so over a run the moves take about the share `s` (unit test
`the_pattern_share_keeps_its_share_and_is_graded`: within 2 %), a cycle with little other work runs fewer.
Why: round 1's moves kept 3-5 % of their tries but halved the cycles in equal time (esp 17-22 -> 8-9).
Literature: Luzin 2015 p. 7 (patterns are cheap to find, so the moves should go where they gain); the share
mirrors the escape share of TW5 (Polubasov 2010: escapes as a bounded part of the loop).
Counters: per kind found / tried / kept (`CycleStats::pat_kinds`, JSON `loop.pattern_kinds`, the report line),
`loop.pattern_share` = {share, waited, work, other_work, measured}. Invariants: share 1 = round-1 copper (the
budget is never read); off (`pattern_moves = []`) = main copper (stage 4 branch not entered). Tests:
topo_tw7_patterns `pattern_share_bounds_the_moves_graded` (y 12.4, every move undone: low share runs fewer,
waits, graded, overdraw at most one move, copper unchanged), `pattern_share_still_takes_the_best_move`;
lib `the_pattern_moves_line_is_true_for_every_case`. Default: after the equal-time A/B (u / ud / ud + share on
ccx33 timing slots, esp s0-7 + qmtech s0).

Results (equal time 300 s wall, ccx33 timing slots, 4 threads, tags tw7ta/tw7tb-7dc21366; esp s0-7 sums, ladder_diff
--json, as drc_new / unconnected / vias / detour>1.5; cycles per minute of loop time):

| cfg | esp s0-7 | cycles/min esp | qmtech s0 | cycles/min qm | measured share |
|---|---|---|---|---|---|
| base (batch pass) | 1118/69/473/117 | 13.4 | 253/20/163/23 | 4.8 | - |
| u (share 1) | 1034/78/428/115 | 6.7 | 401/19/151/20 | 2.8 | 0.58-0.64 (prof 65-72 % of loop time) |
| ud (share 1, detour 1) | 1289/72/434/109 | 7.1 | 164/20/147/22 | 2.9 | 0.59-0.64 |
| udh (share 0.25) | 1139/72/453/103 | 15.9 | 146/18/157/22 | 5.6 | 0.250 |
| udq (share 0.1) | 1131/74/458/113 | 22.0 | 240/20/159/24 | 7.3 | 0.100 |

Paired over the 8 esp seeds only these clear 2 standard errors: u vias -5.6/seed (t -2.0), udh detour>1.5 -1.8/seed
(t -2.3); drc_new and unconnected are noise everywhere. The work units track the time (measured 0.10 / 0.25 against
12-17 % / 29-36 % of loop time in the profile). With the budget the loop runs more cycles than the base, because the batch pass
it replaces costs time too. Per connection over seeds (reports/tw7-moves-data/conn3.json): all pattern configs fix
IO8_SCL R17.2-U2.13 (base ratio 1.6-2.0 with DRC), +BATT U4.28 (DRC in 6/8 base seeds), +3V3 U4.2 (vias 22 -> 10);
qmtech JS_CS, ADV_CLK and FPGA_UART1_CTS lose their detours. The detour factor (d) adds a via pair on short
connections (Net-(D9-Pad4) 0 -> 2 vias in 5/8 seeds for ratio 1.24 -> 1.07; IO8_SCL U4.13-U4.18 in ud), u does not.
Default pattern_share = 0.25 (pattern_moves stays opt-in, so default copper is unchanged).

## 12. Work round 4 (lead brief 2026-11-19: C, E, F after TW6 E; stuck-graded order; flip evidence; d retry)
**Budget order by fruitless tries** (`[optimize] pattern_stuck` = `w`, float >= 0, default 1; 0 = the round-3 order).
A found pattern runs by `excess x 1 / (1 + w x stuck)`, `stuck` = the fruitless tries of its key (its two
connections; wire ids change at every re-lay) since its last kept move (`Tw7::stuck`, loop-wide, a restore keeps
it). Graded, no cutoff: a pattern that keeps failing moves down, never out (it runs once the fresher ones have
had their turn, and a kept move resets it). Why: at share 0.25 round 3 kept 0-24 of 548-705 tries on esp, the
same top candidates failing every cycle. Literature: Polubasov 2010 fig. 11 (the re-lay counter: a wire's
fruitless re-lays change its coefficients, so the loop does not repeat itself). Counters: JSON
`loop.pattern_share.retried` (moves on a pattern with earlier fruitless tries) and `stuck_patterns` (most
patterns with fruitless tries at a stage-4 end); report line "order: ...". Tests: lib
`the_pattern_rank_is_graded_by_fruitless_tries`; topo_tw7_patterns `pattern_stuck_lets_a_fresh_pattern_past_a_fruitless_one`
(two u_clinch boards side by side, share 0.25: weight 0 retries only the fruitless one, weight 3 lets the other in).

**C, via trade** (`pattern_moves` "via_trade", `Kind::ViaTrade`; fig. 21b -> 21c, Luzin 2015 pp. 9-10; Luzin,
Popov & Popov 2013 p. 134 step 8.1). Detector `xrules::via_trades`: a plain wire on one layer (no via in the
layering) with excess length > 0 and the other-net wires that cross its straight pad-to-pad line and not the wire
(any count). Each such wire with its movers is one candidate (ranked by its own excess; a single mover already
found as that U-clinch pair is skipped). Move `Sp::via_trade_move`: the subject laid first (movers out), its new
wires held to its layer in `State::layers_now` for the movers' search (a crossing prices the via it implies), then
the movers in route order; judged by the board's evaluation like every pattern move. ViaPair (21a, `pair_move`)
keeps its own code (the stretch hold of a via pair is a different hold); both now end in the one held re-lay.
**F, one core**: `Sp::held_relay(first, obs, hold, detour, ...)` replaces the bodies of `pattern_move` and TW6's
`relay_kept` (copper unchanged: same take-out set, same lay order, same keep rule with detour 0 = `e1 < e0`).
**E, pour move** (`pattern_moves` "pour", `Kind::PourCut`, only with `[capacity] pour_cuts`): candidates from
`pourplan::cut_components` on the plan (now with the piece's centroid and area); the bounding wires' passes within
twice the piece's radius + 1 mm carry the cut's violation price for the move's search only, spread over them
(`viol_mm x g / (congestion price x passes)`, through `State::history`, restored after the move: no new search
term), ranked before the excess lengths (`POUR_RANK` 10 x the pads grade). Without pour_cuts the report says the
kind finds nothing. **Finding** on the pour fixture: the move is kept on the plan's pour check, but the drawn copper
is unchanged (the relaid plan changes the triangles, not the way round the pads; the plan check reads the pass
points, not the taut path), so E needs the plan check on the taut path or the price on the piece's homotopy first.
Not done this round: SameNetLoop (needs a re-branch at the crossing that the plain placer has no form for),
`loop = "cycles"` removal (touches ~10 test files and the drawing feedback path: own round), R6-R10 (wait for the
pattern_moves flip / C's A/B).
**detour_trade** approved by the user 2026-11-19 (sketch `hz-runs/tw7s2-17228bd/sketch/detour_trade.png`, panel
(c): A straight, B1/B2 dip 45 deg round A2 at a constant 1.5 mm pitch, B1 at 0.20 mm from A2). Fixed regression
test `detour_trade_pattern_reaches_fig_21c`, **expected to fail** (`should_panic`) on the pitch only: with
`pattern_moves` = ["via_trade"] (alone and with the other kinds), seeds 0 and 1, C reaches the topology of (c)
(A 14.000 mm straight, B1 43.08 / B2 44.27 mm, B1-A2 0.200 mm, no vias, DRC clean), but the drawing packs B2 against
B1 at the V (copper gap 0.200 mm, expected 1.0 mm). That is the drawing's bundle packing (tight-packed bundles,
the pitch of a rail pair round a bend is not kept), not C; the expected answer is not loosened. With the moves off,
seed 1 ends careless (A round the rails, 49.07 mm; seed 0 reaches (c) without moves), the test asserts that as
the reason for C. The shorter-rail variant (x 22-38, A 25.1 mm careless, only the via trade leaves it) stays as
the ignored `proposed_detour_trade_short_rails_needs_the_via_trade`.
