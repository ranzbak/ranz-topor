# tw4-eval: the loop's evaluation sees the pad exit with its neck and the via room

Branch `tw4-eval` (off main 551add2e = tw2-metric 7bdff784; same tree). Stage TW4 of
~/.claude/plans/topor-way-plan.md §4 (decisions D2, D3; DANGLE-TURN D2 note). Phase 1 = this note and the
baseline; no engine code before the lead's reply. Data: job `tw4b-d06210a` (score_runs, cpu 2400 s, cut check on,
seed 0, `rounds=1 runs=1`, realise erbs) and the throw-away dump `tw4d-fb73277` (branch `tw4-dbg`, never merge:
every drawn run's ERBS source). Copper of the rounds=1 runs = the tw2 tip's (copper_hash equal on eprom, esp,
qmtech). Numbers: `tools/tw4_analyse.py` → scratch/agent-wt/wt-tw4-runs/tw4b-analysis.json.

## Why
Polubasov 2010 «Пространство решений»: the evaluation of a topology must correlate with the result ("an exact
match is not required"). Today it does not: the unseen part of the drawn violations stays flat while the loop
drives the predicted ones down (esp s0: rounds 1 / 16 / 64 / 200 → predicted 141 / 78 / 56 / 19, drawn 112 / 70 /
63 / 54, unseen 45 / 35 / 44 / 46). The descent optimises the wrong number the longer it runs.

## Baseline (main 551add2e, seed 0, rounds=1 unless named)
| board | drawn | unseen (all) | unseen share | plan def.¹ | long run | forced | of them pad exit |
|---|---|---|---|---|---|---|---|
| eprom | 0 | 0 | – | – | – | 0 | 0 |
| wiccon | 5 | 5 | 100 % | 100 % | – | 2 | 1 (RXD; CM1's class rule makes it static) |
| esp | 112 | 45 | 40 % | 39 % | r64 70 %, r200 85 % | 10 | 0 |
| qmtech | 160 | 109 | 68 % | 66 % | r16 86 % | 19 | 4 (ADV_D6, MAX_LRCLK, USB_C_D-, VIN5V) |

¹ pad_exit + drawing_loss + model_gap (the loop-diversity definition). Unseen (all) = drawn minus the drawn in the
classes the measure predicts (planner_overfull, via_site, unlayerable, foreign_copper, forced_path). Pad-exit
forced 12 → 5 already (TW2). Gain ratio Δdrawn/Δpredicted on esp: r1→r16 pooled over seeds 0–4 **0.64** (per seed
0.67, 0.55, −0.08, 1.00, 0.95), s0 r1→r64 **0.58**, r1→r200 **0.48**. A share can rise while the count falls
(5.10 stubs on esp: unseen 45 → 21, share 40 → 84 %): counts are reported beside every share.

## Classification of what is left (unseen violations, rounds=1)
| sub-class (first match) | esp | qmtech | wiccon | owner |
|---|---|---|---|---|
| **pad exit with its neck**: compass pad_exit, or on a connection the drawing necked to the neck floor | **33** | **90** | 5 | **TW4 T1** |
| – of it: pad_exit class (neighbour pin; not predicted) | 13 | 32 | 5 | |
| – of it: drawing loss on a floor-necked connection | 20 | 58 | 0 | |
| ERBS kept gap / band conflict, not floor-necked (USB_D- pair, JS_SCK, JS1_9) | 9 | 12 | 0 | TW2b |
| model gap (cut test over-full, per-edge fits) | 0 | 2 | 0 | measure: cut check |
| other (static pad, via of another net, wrong side of a via, no band "ends moved") | 3 | 5 | 0 | CM1 / TW6 |

The floor neck is the sharpest signal of the plan: **every** connection drawn necked to the floor has violations
(esp 6/6, qmtech 10/10, wiccon 1/1), necked above the floor 1/8, planned-necked but drawn full width 6/92. The plan
already decides these necks (compass `necks`, `neck_from_pad`: from the pad to the bottleneck) and prices them as a
shrinkage only, the same as a 0.01 mm neck. The forced ones lost their band as Static (pad exits: ADV_AVDD,
MAX_LRCLK, VIN5V, JS1_6), Predicted/Gap (FPGA_SD_CLK 23, JS2_3 20: a BGA/SSOP exit necked 0.25→0.09 mm) or No band
"ends moved" (esp IO2/IO5/IO10/IO20/IO21 at U4). If T1 makes this class predicted, unseen falls to esp 12/112 =
11 %, qmtech 19/160 = 12 % (targets 20 % / 30 %).

## Proposed terms (all graded, all parts of `MeasureParts`, priced locally like TW1's parts)
**T1 Pad exit as a plan object with its neck** (the largest class). Per connection end at a tight pad (another net's
copper beside it closer than w + 2s: the 5.10 `exit_stub` test), the plan holds an exit: side, neck width w_e and
stub to where the wire may turn.
- The side is read from the plan (the triangle the planned path leaves the pad through; candidates: both axis tips,
  a free long side), not from the straight line before the topology. 5.10's straight-line tip is the documented flaw
  and the measured cost (qmtech with `pad_exit_stubs`: forced 19 → 32, via site 5 → 11; esp improves: forced
  10 → 4, drawn 112 → 25). A re-lay that changes the side moves its exit (one wire, local).
- w_e = the widest width in [w_floor, min(w, pad width)] that fits (manual p. 56: the minimum width is used "near pads
  that are narrower than the wire"; Maley 1987 p. 29: no trace wider than its terminals).
- Room: the stub is a feature (Maley p. 29: features are segments); its cuts to the neighbour pins and to other
  exits are feature cuts with CM1's machinery (items = crossers + the stub width, price 1 + over/pitch, counted in
  `violations()`, compass predicted `pad_exit`).
- Neck depth: a planned neck costs its shrinkage plus viol_mm × d², d = (w − w_e)/(w − w_floor) (one violation at
  the floor, where 17 of 17 fail; graded, no cutoff). Manual p. 56: the minimum width is used "unless a different
  path is available", so the loop must see a necked fit as dearer than a clean alternative. Per stretch, not per mm
  (user rule 2026-11-05). `[factors] neck_depth` weight (default 1, 0 = today).
**T2 Via room (CM1) on the incremental measure.** cm1-room bbde76b3's via cuts (Maley pp. 32–33, 40) in the local
CapModel over W' (TW1): W' gets the vias of F and the via slots whose cuts an F pass crosses (reverse index). The
geometry of a slot depends only on (via triangle, net, layer), so it is cached per run, shared read-only, not per
model (CM1's +57–60 % came from per-build work under the global judge). Brings CM1's `violation_class` rule.
**T3 Dangling lead as an exit (DANGLE-TURN D2).** An exist-term lead is an exit with a fixed direction; its length
in [0, lead_len] is a plan choice priced in S by the turn after it (graded past 90°, like junction_angle), so a
lead that points away from the target is shortened in the plan. The lead's via end stays TW6.

Order: T2 (feature-cut machinery, already written) → T1 (reuses it) → T3. Compass fixes first (measurement only):
located shrinkage items (today `neck_loss` can never match: no item has a place), the `pad_exit_items` hook
(compass.rs:219), the per-run ERBS source in result.json (the tw4-dbg dump as `route.erbs.runs_list`).

## Lead decisions (2026-11-18; the user confirmed phase 2 the same day)
- D1 approved (T1, exit side from the plan, neck depth price): the price is a smooth graded cost, no cutoff at the
  floor; the neck-from-pad rule stays (a neck may reach further back from the footprint to free room, and the planner
  knows the neck floor); the exit tip comes from the plan, never from the drawing (the 5.10 straight-line A/B).
- D2 approved (order T2 → T1 → T3): CM1's cycle cost ≤ 10 %, measured on the T2 commit before T1 builds on it; the two
  dead meter hooks first (located necks for `neck_loss`; `pad_exit_items`), so the meter sees T1.
- D3 approved (acceptance form as below), plus: every changed connection judged per connection (`ladder_diff --json`),
  and the D5 overall best reported next to the predicted gain.
- Synthetic pattern first for T1 (PNG + text for approval). Budget for phase 2 ≤ €1.5.

## Meter, invariants, tests
- Meter (the stage's own): unseen count and share per board at rounds=1 and esp r64/r200, qmtech r16; gain ratio
  pooled r1→r16 seeds 0–4 and s0 r1→r64 / r1→r200 (ratio ≥ 0.5 on all three); CM1 cycle cost on the local judge.
- Each term an option; off = today's copper bit-identical (hash on 4 boards). Copper independent of thread count.
- Local Δ = whole-board measure for every new part: TOPO_LOCALCHECK extended, 0 mismatches on fixtures + boards;
  dense fuzz of the feature cuts against an exact slow reference (degenerate: stub touching a pin, zero-length stub,
  via on a stub end).
- Synthetic fixtures (PNG + text for approval first): fine-pitch row whose planned path leaves through the far tip;
  a bottleneck that fits only at the floor beside a clean path a little longer (expected: the clean path); CM1's
  via-in-pad-gap fixture on the local judge. Mutants per term. Ladder eprom → wiccon → esp → qmtech per connection;
  bornhack once at the end.

Risk: T1 changes plans on qmtech, as the straight-line 5.10 did; the plan-chosen side addresses its documented cause,
the A/B per connection decides the default. T1's neck price can add detours or vias: watch detour > 1.5 and vias per
connection (a direct path that needs a floor neck is not a legal direct path today). Out of scope: TW5 search, via-
site moves (TW6), custom rules.
