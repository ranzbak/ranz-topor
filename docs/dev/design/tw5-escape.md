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
  random cluster across edges that are satisfied (no via) with probability 1/2 per edge, and flips it
  when the via count does not rise (Δ <= 0: the cluster's satisfied and via boundary edges balance). The
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
- After each escape and its via-rose re-lays the state is offered to the table (TopoR: every step is
  tested against the kept variants; manual pp. 72-73) once TW3's table is in; before that it is counted.

**(c) History on cuts that stay over-full, inside the loop** (`[optimize] loop_history = true`).
After every whole-board measure (cycle end, escape): `h_e <- f * h_e + d/(1+d)` on each edge of an
over-full edge-layer (cut model: the edges where the over-full cut's wires cross it; d = its graded
depth), `f = [optimize] loop_history_fade` 0.8 on all edges; bounded by 1/(1-f) = 5, no cutoff. Priced as
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

## Open points for the lead
1. κ from a profile is a constant that ages; alternative: count re-lays only (`escape_every` re-lays,
   auto = alive wires / 4). I take the work units unless you prefer the simpler count.
2. Offering after each escape needs a Space hook inside `optimise` (TW3 territory): I add it after the
   TW3 merge, counted until then.
