# tw5-escape: TW5 ESCAPE (P4-ESCAPE) design note

Branch `tw5-escape` off main dcf4c0ef (TW3 table and TW4 eval merged in when they land). Stage TW5 of
~/.claude/plans/topor-way-plan.md (row TW5, steps T2, T4, T5, Appendix F points 2, 3, 4, 7, decision D8, D9).
Evidence: reports/loop-diversity-20261118 (causes 3, 4), route-improve-plan-20261118 (capacity class 14).

## What and why
Today the loop is a pure descent with three weak escapes: the oscillation narrows x0.7 per cycle and
never touches the violation term that decides 63-76 % of the tries; the layer re-optimisation changes
1.6-5.2 % of the positions (random ties only), is gated by the evaluation (0 of 300 better, 178
refused) and runs once per cycle (~0.1 s of a cycle); a cut that stays over-full gets no dearer inside
the loop (history only in the 6 pre-loop P4 rounds, stop after 2 stalls). Each part below is an option
in topor.toml with the new behaviour as default and the old one kept.

**(a) Bounded oscillation, no cooling** (`[optimize] oscillation = "bounded"` | `"cooling"` = today).
Cycle width fixed at `fluctuation` (no narrowing, no reheat). Per wire amplitude, graded, bounded:
`amp_w = lo + (hi - lo) * b / (1 + b)` with the badness `b = detour + crowd + stuck/(1+stuck)`:
detour = max(0, len/straight - 1) of its connection, crowd = mean loop history (c) on its passes,
stuck = its fruitless tries in a row (today's widening). `lo`, `hi` = `[optimize] osc_amp_min` 0.1,
`osc_amp_max` 0.4 (fluctuation 0.2 in between). The violation price of a re-lay oscillates with the
rest: `viol_mm * k_osc(violation)` per wire and try; the archive/table keeps constant coefficients.
Source: Polubasov 2010 «Перекладка проводников» (coefficients "chosen for every wire by the quality of
its path and its surroundings, and they oscillate within limits", "раскачивают функцию стоимости").

**(b) Layering as a radical escape** (`[optimize] escape = "layering"` | `"ties"` = today).
- 2 layers: the exact via optimum (unchanged solver), then a seeded plateau walk on the unreduced
  domain graph: `escape_walk` x domains tries (default 4), each picks a random free domain, grows a
  random cluster across edges that are satisfied (no via) with probability 1/2 per edge and across via
  edges with 1/4, and flips it when the via count does not rise (Δ <= 0: the cluster's satisfied and
  via boundary edges balance). Before the clusters, every connected part without node 0 (lone domains
  too) flips as a whole with probability 1/2. Any other optimum differs from the present one by
  connected parts of change 0 each, so every optimum stays reachable (the via edges must be crossable:
  with satisfied edges only, 147 of 226 small cases with several optima reached another one). The
  result is a random via-optimal layering far from the old one; then the capacity refinement as today.
  Source: Polubasov 2010 «Оптимизация расслоения» figs. 8-10 ("12 optimal layerings, only two get out",
  "choose at random", "the gain came from a radical change of the topological situation", also with 0
  vias, fig. 10); domain graph and cut = vias: Polubasov 2001 pp. 3-9.
- N layers: each escape re-solves one random layer pair (α, β), any two layers, with the same plateau
  walk; capacity enters the pair's price: an event's unary term on α / β gets the graded over-full
  price (`overflow` weight x over-fullness / (1 + over-fullness)) of its edge-layer under the present
  layering. `assign_n`'s pair swaps take an equal-cost swap at random (Appendix F 7). Target:
  inner_layer_room (S1/S5 F.Cu, S2-S4 In1) DRC 18 -> 0.
- Always taken (`escape_take = "always"` | `"not_worse"` = today's gate). Vias cannot rise; predicted
  violations may, the table keeps the bests (user rule, Appendix F 2). Then the wires whose via count
  rose are re-laid first (moved to the front of the sweep), with the re-lay counter; the counter's price
  fades per cycle (`[optimize] relay_count_fade` 0.8) so it damps an A-B-A oscillation (fig. 11)
  without freezing a wire for the rest of a long run (no hidden cooling).
- Throttle, deterministic (results never depend on time): work units = traversals a step layers
  (global layering: all alive traversals x κ; a re-lay: its local free + model traversals). The next
  escape comes when the re-lay work since the last one >= (1/s - 1) x the last escape's work,
  `s = [optimize] escape_share` 0.35 (<= 0.5: "not sooner than its own last duration"). κ is set once
  from a TW0 profile (time per traversal of a global layering / of a local judge), a documented
  constant; the measured time share is reported (target 20-50 %). Escapes run inside the sweep.
  κ = 0.07 (`[optimize] escape_kappa`), measured in tw5r1 (16 cycles, 1 thread, ccx43) as the time per
  alive wire of an escape against the time per unit of a re-lay: eprom 0.062, wiccon 0.069, qmtech
  0.069, esp 0.123. With κ = 1 the work share was 32-35 % but the time share only 3-6 %: a re-lay's
  path search, not its layering, dominates its time. The copper never depends on the clock.
- After each escape and its via-rose re-lays the state is offered to the table (TopoR: every step is
  tested against the kept variants; manual pp. 72-73) once TW3's table is in; before that it is counted.

**(c) History on cuts that stay over-full, inside the loop** (`[optimize] loop_history = true`).
After every whole-board measure (cycle end, escape): `h_e <- f * h_e + (1+d)/(2+d)` on each edge of an
over-full edge-layer (cut model: the edges where the over-full cut's wires cross it; d = its graded
depth, so an edge just over-full adds 1/2 and a deep one up to 1), `f = [optimize] loop_history_fade`
0.8 on all edges; bounded by 1/(1-f) = 5, no cutoff. Priced as
today by the `congestion` factor per pass (search and keep price). Loop-wide memory, not part of a
variant (restores keep it). Source: McMurchie & Ebeling 1995 p. 113 eq. 1 (c_n = (b_n + h_n) p_n), p. 112
("penalty gradually increased"); Dayan & Dai 1993 p. 5 (iterative reassignment by best gain).

**(d) Floating up per wire** (`[optimize] float_wire = true`). In the sweep, after a wire's re-lay was
not kept: one local layering of the wire and the wires it crosses (`assign_local`, random among the
equal ones), priced by the local judge (violations + vias at the wire's oscillating coefficients), kept
when that price drops. The board-wide label descent stays, its board gate removed (Appendix F 3), and the
crossing passes are taken as they come (Appendix F 4, `cross_take = "always"`). Source: Polubasov 2010
«Процедура всплытия» fig. 12 ("used during routing to change the situation").

**(e) Pre-loop P4 rounds fold into the loop** (D9). `[optimize] preloop_p4 = false` (default with
`loop = "topor"`) skips the negotiated rip-up rounds before the loop; (c) does their job inside it.
`[search] reroute_rounds` stays an option for one release. Decided by an A/B per connection.

## Invariants
Escape layering: via count equal to the exact optimum (2 layers) / not above the pair's before (N);
deterministic per seed and thread count. Amplitude in [lo, hi] for every cycle (no decay). History
bounded, fades, grows only on over-full edges. Throttle share <= s on any work stream. Copper equal to
main with every option at its old value (`escape = "ties"`, `oscillation = "cooling"`, ...).

## Tests (rule 14)
Unit: plateau walk vs brute force on small random domain graphs (cost = optimum, ≥ 2 distinct optima
reached when they exist, deterministic); amplitude bounds and no decay; history bound/fade; throttle
share; N-layer random pair keeps the cost <= before and moves inner_layer_room's S2-S4 to In1 (fit
test). Board, VM, rounds=1 off, same seeds: ladder eprom -> wiccon -> esp (5 seeds) -> qmtech, old vs
new options in the same binary plus main for copper = base; bornhack once at the end (4 threads).
Measured per escape (loop json): Hamming of free positions (mask with > 1 layer), vias before/after,
rows archived per 20 escapes, layering time share; per connection: esp IO3/IO5/IO6 and the 14 capacity
connections (layers, vias, compass planner_overfull), esp copper-to-edge, inner_layer_room DRC.
D8 patterns (Polubasov figs. 6, 8-9, 10, 11) proposed as PNG + text to the lead, not in the suite.

## Lead decisions (2026-11-19)
1. Throttle by deterministic work units with the profile-calibrated κ; no `escape_every` (it would
   cost an extra code path). The plan's bound (not sooner than its last duration, share <= 50 %) in
   work units.
2. The table offer hook in `optimise()` after TW3 merges; counted until then.

## As built (2026-11-19, measured on the VM: tw5r1-tw5r5)
- **Old options = main copper**: every TW5 option at its old value (`oscillation = "cooling"`, `escape = "ties"`,
  `escape_take = "not_worse"`, `relay_count_fade = 1`, `loop_history = false`, `float_wire = false`,
  `preloop_p4 = true`) gives main's copper on eprom, wiccon, esp, qmtech (copper hash, 1 cycle, tw5r1).
- **Throttle**: κ = 0.07 (above). An escape comes after nearly every re-lay on small boards; the measured work
  share is 0.17-0.21 (bound 0.35), the time share 16-26 % (esp 24-26 %).
- **Escape front**: the via-rose wires are laid again first, once per cycle per connection. Without that bound the
  front refilled faster than it drained at κ 0.07 and a 16-cycle esp run did not end (tw5r2, 1180 s against 165 s).
- **Plateau walk**: clusters grow to a log-uniform random size (every scale equally likely). A fixed growth
  probability per edge is supercritical on a board's graph (degree about 4): every cluster swells to a whole part
  and its flip adds vias. Small sizes alone (2^-m) missed the large equal-via flips (wiccon 0.0 % changed).
  Lone domains and parts without an SMD pad flip as a whole with probability 1/2.
- **(e) folds P4's priced via moves into the loop**: the escape's capacity refinement may add a via when it removes
  more over-full price than the via costs (`priced && [capacity] cut_vias`, as the P4 label moves). Without that, a
  wire that crosses nothing has no event to relabel and never leaves an over-full cut.
- **inner_layer_room is no valid routing test as generated**: the board's KiCad layer types are all "signal", so
  topor's 4-layer default roles make In1.Cu a gnd plane (not routed). The expected answer needs In1, so the runs use
  `--declared-layers` (tools/tw5_job.sh `q-` boards). With In1 routed, main's options reach DRC 0 in 3 of 10 seeds
  (through the pre-loop P4 rounds) and the TW5 defaults before the priced vias in 0 of 10 (4 vias, DRC 4 each).
- **D8 proposals**: harness/tw5_patterns.py (figs. 10, 11, 8-9, read from the paper's PDF pp. 107-108), not in the
  suite. Fig. 8 at the paper's scale has a shorter 0-via answer than the paper's end state 9c (W2 on F.Cu between U2
  and W4 instead of the left loop): the expected end state needs a decision.

## (f) P4 rip-up inside the loop (round 3, 2026-11-19; lead OK 2026-11-19)
**What.** `[optimize] loop_p4 = false` (opt-in until the A/B). Once per cycle, after the sweep (and its
escapes) and before stage 2 (uncross, layer re-opt, via-rose relay, floating up, crossing rules): when the
present layering has over-full cuts (cut model; per-edge model: over-full edge-layers), vias without a site
or over-full via cuts, one negotiated round exactly as a pre-loop P4 round: the same victim choice (factored
out of the pre-loop loop into one function, no second copy): per over-full cut its wires routed last first
until the rest fits, the no-site wires, a bad via cut's wire + one crosser; at most `(alive/4).max(10)`;
the tree dependents with them. All victims are ripped together and placed again in rank order with the
cycle's fluctuated coefficients (a per-cycle seed), then uncross and one whole-board layering (the loop's
tie seed). Taken as it comes (Appendix F 2, the user's keep-best rule: the archive keeps the bests; the
stages after it clean up); undone only when a victim finds no path (counted, the state before stays).
The pre-loop round's `via_rose` part is not carried over: the loop has its own via-rose relay.
**History.** `loop_p4 = true` uses the (c) loop history (fading, bounded by 1/(1-f), graded by depth), on
when (c) is off too (one `History`, updated by `history_step` before the round's victims are chosen). The
pre-loop rounds' unbounded +1 per round is not repeated per cycle (16+ cycles would grow it without bound).
**Why.** The sweep re-lays one wire at a time and keeps a move only when its price drops: a full cut is a
local minimum for every single wire (the other wires still hold the room; Polubasov 2010 «Оптимизация
расслоения», fig. 7 point 1, «застревание»). The pre-loop P4 rounds rip a cut's wires together (qmtech: over-full
278 -> 99 in 5 rounds, tw5r11 O) and (e) without them loses (tw5r9: DRC 363 -> 610 before via_cuts). The
joint rip lets the first re-placed wire take a detour or two vias for the others (Polubasov 2010 fig. 6a-c:
wire 2 first takes two vias, then wire 1 moves and the vias go) - the inner_layer_room answer (3 wires on
In1, 6 vias) needs exactly that step. Negotiated congestion: McMurchie & Ebeling 1995 (PathFinder, eq. 1
c_n = (b_n + h_n) p_n, history grows on overused resources; NOT in docs/literature, INDEX "still to fetch":
cited from the known paper, page to be checked); Dayan & Dai 1993 p. 5 (iterative reassignment of the
worst-gain items, docs/literature/dayan-dai1993-ucsc-crl-93-04-layer-assignment.pdf).
**Invariants.** Off = tip copper (copper hash, 4 boards, 1 cycle). Deterministic per seed and thread count.
At most one round per cycle; victims <= the cap; no wire lost (a failed placement restores the state).
**Tests.** Unit: the shared victim function on a small fixture (over-full cut -> the routed-last wires until
the rest fits; the same set the pre-loop round chose before the refactor: pre-loop copper = tip). Loop
integration (loop_local, smallest fixture): a cut that stays full under single-wire moves is emptied by one
in-loop round, and a mutant (round skipped / victims empty) fails it. Board A/B on the VM, 16 cycles:
O+f and Ne+f (pre-loop off, loop_p4 on) vs O on qmtech s0-2 and esp s1-3, inner_layer_room 10 seeds (KiCad
types, In1 routed), per connection; a default flips only where it wins at equal time with no new detour.

**Lead conditions (2026-11-19) and as built.** Deterministic (cycle seed, `mix_seed`), thread-independent; prof key
`loop_p4`; throttled by and charged to the escapes' work budget (`Throttle::ran_work`: its counted layering and
measuring plus each placing at the mean re-lay's work), so a stuck board cannot spend the loop on it; the cap is a
safety bound only, the victims ordered by their graded weight (`p4_keep_heaviest`); a round whose placing fails is
restored (test hook `loop_p4_test_fail`; `State::any_path` ignores room and the rips only free room, so a real failure
is not expected). Changed against the note: a round is kept like the loop's other whole-board moves, by
`escape_take` (default "not_worse": undone when the evaluation rises; "always": as it comes). With "always" every
round was taken and inner_layer_room got worse (2 vias every seed, engine route, 16 cycles): the round's rough
placings destroy better states faster than the archive's restarts recover them. Engine route on inner_layer_room
(16 cycles, seeds 0-9, layering with the answer's 6 vias): O 4/10, O + loop_history 8/10, O + loop_p4 8/10 (every
round undone: the gain is its history), pre-loop off 0/10, pre-loop off + loop_history 0/10, pre-loop off +
loop_p4 5/10.

## Escapes offer their state to the variant table (`[optimize] escape_offer`, after the TW3 merge)

Lead 2026-11-19: "escapes offer their result to the table; the table keeps the bests". The state
after a taken layering escape (b) or a taken in-loop P4 round (f) is offered to the run's archive
and to the TW3 shared table before the cycle's own end-of-cycle offer, so a best reached there is
kept even when the rest of the cycle (front re-lays, layer re-optimisation, crossing passes) leaves
it (Polubasov 2010 «Многовариантная оптимизация»: the table keeps the variants optimal for some
criterion).

- `Space::offer_floor` (before each cycle: the archive's criteria) and `Space::take_offers`
  (drained after a finished cycle, before its own offer). A state some held criteria cover is not
  copied (`mid_covered`); a cycle cut short drops its offers.
- `SharedTable::offer_mid`: held under the key (cycle, run, seq), the cycle-end offer is seq
  `usize::MAX`, so the commit order is (cycle, run, order reached) and the table stays a function
  of the seeds (same rule as TW3). A mid offer whose run never offered that cycle's end is dropped
  at the commit, not counted.
- Default true; it acts only with `escape = "layering"` or `loop_p4 = true`, so the defaults keep
  main's copper. Report: the escapes' warning ("states after escapes / in-loop P4 rounds offered to
  the variant table N (A taken into this run's archive, C not offered: covered by its rows)") and
  JSON `escape_offer {on, offered, archived, covered}`.
- Tests: `table::tests::mid_cycle_offers_commit_with_their_cycle_and_drop_when_unfinished`,
  `tw5_escape::escape_offer_puts_the_in_loop_round_states_into_the_table` (two runs: the restarts
  go on from a row that came from a round's state; off: none). Mutants o1-o6 in tools/tw5_mut_f.sh.
