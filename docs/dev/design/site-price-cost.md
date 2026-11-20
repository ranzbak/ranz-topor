# site-price-cost: the graded site price's per-call cost, and an escape budget in measured cost

Branch site-price-cost (off main 77754d9f). Source of the problem: reports/recount-tw-20261119-2310.md §3, §7.1, §7.3.

## (1) `[vias] site_price = "graded"`: a memo, not a new model

Cause (code reading + profile): `Frame::site_over` (refine.rs) called `cutflow::point_free` for every planned via
of every wire `reprice_vias` reprices, on every via position `CapModel::optimise` tries. `point_free` walks every
obstacle of the board (O(obstacles), thousands on qmtech) and was not cached. Fix: `Packer::point_free_cached`, a
memo per (point bits, net, clearance bits) on the packer (a pure function of the static board, shared by every
capacity model and lane; the same bits as the uncached value). Nothing else changes.

Measured (local, 6 threads, det qmtech s0 rounds=1 runs=1, `[vias] site_price = "graded"`): user cpu 805 -> 102 s,
CapModel::optimise 14.97 -> 0.56 ms per call (off: 0.77 ms on the VM), copper hash equal (1e10cc4fcbe69df6).

## (2) The escape budget in measured cost (new model: this note asks for the lead's OK)

**What.** The TW5 throttle ([`escape::Throttle`], tw5-escape.md (b)) counts work in wire units: a re-lay = the wires
its local judge laid + modelled; an escape = `escape_kappa` x alive wires. A per-run deterministic cost counter
(`LoopCtr::cost`, units of about 1 ms at 4 threads; round 2 weights) replaces both for the escapes and the in-loop
P4 rounds:

| step (call site in core.rs) | adds (deterministic counts only) |
|---|---|
| one escape (fixed part) | 1.415 |
| whole-board layering (`assign_tw`) | 0.0954 x alive + 0.0804 x refine evaluations + 1.25e-3 x passes |
| whole-board measure | 1.07e-3 x passes |
| local judge (`local_free`, `local_model` counted) | 0.0765 x free + model wires |
| local layering (`assign_l`) | 0.19 x refine evaluations |
| one sweep re-lay (path search, placing) | 5.26 + 2.5e-4 x passes + 2.25e-5 x judged wires x passes |

An escape's work is the counter's change over the escape itself (as `loop_p4` already does with `ran_work`), no
kappa. The first escape is due after `(1/s - 1)` x the cost of one whole-board layering + measure at the loop's
start (the same formula, evaluations of the last one). Refine evaluations are the par-refine `RefineStats::evals`,
which the lane executor promises equal to the serial loop's for any lane count.

**Carry.** In the measured budget a re-lay dearer than an escape's due does not reset the counter: its excess is
carried over to the next escape (`Throttle::carry`), else about half a re-lay per escape is lost and the share
undershoots (esp X with `loop_p4 = false`: 0.28 against 0.35 before the carry, 0.33 after).

**Why.** κ = 0.07 was measured at TW5 (1 thread, before TW4r2's via discs / pad exits) and does not follow the cost
now. Local det runs (4 threads) of X = A-sp-e: one escape costs 54-231 ms, one re-lay 48-55 ms; κ x alive says
22 units (qmtech) and 6 (esp) against a re-lay's 34-50 units. Real (escape + in-loop P4) share of the sweep: qmtech X
0.57, against `escape_share` 0.35; the engine's own figure says 0.196. The per-escape cost depends on the refine
evaluations (qmtech X 2005 per escape, X+graded 131, esp 20-52), so no single kappa can hold.

Calibration: least squares over 4 det runs (qmtech, esp x X, X+graded; 315 escapes, 884 re-lays, wall ms at 4
threads, after (1)). Per-run real/predicted ratio of escape vs re-lay cost: 0.87, 1.02, 1.01, 1.13 (a ratio f moves a
0.35 share to f s / (f s + 1 - s): 0.32-0.38).

**Literature.** Polubasov 2010 (docs/literature/polubasov2010-topor-inside.html, «Оптимизация расслоения»): the layer
optimisation runs "not sooner than after a time equal to its own last duration" — a budget in time, which a
deterministic counter calibrated to time approximates (TW5 kept the copper clock-free; so does this). Deterministic
work units calibrated to time are the MIP solvers' practice (Gurobi `WorkLimit` "work units", CPLEX deterministic
ticks; manuals, not in docs/literature, no page).

**Invariants.** Copper never depends on the clock, the thread or the lane count (only counts enter). Default copper
unchanged: escapes (`escape = "ties"`) and in-loop P4 (`loop_p4 = false`) are off by default. TW7's
`pattern_share` keeps the old units (out of scope; its real share is reported as a finding).

**Round 2 (lead 2026-11-20): refit on 4 boards, default rule.** The first fit (qmtech + esp only) gave eprom f
0.73 and wiccon 2.77 out of sample. The refit adds a fixed part per escape (the lead's term). It also adds a fixed
part per re-lay and a judged wire x pass term: an eprom re-lay costs 1.15 ms per judged wire, a wiccon one 0.27 ms,
and no per-wire constant fits both. The fit covers 18 local det s0 runs on eprom, wiccon, esp and qmtech (1173
escapes, 3058 re-lays). In-sample f is 0.77-1.25 on every run.

Leave one board out: eprom 0.86, esp 0.84, qmtech 0.45, wiccon 1.99. With the lead's model (fixed escape term only)
the values are 0.33, 0.76, 0.52 and 2.84. qmtech fails in every variant tried (15): only qmtech's escapes carry heavy
refine evaluations (about 1400 per escape, esp 12), so without it the evaluation weight is not identified. Rule (lead):
not all 4 within 0.8-1.25, so the default is "units".

**Option.** `[optimize] escape_budget = "units"` (default: kappa x alive per escape; `escape_kappa` applies) |
`"cost"` (opt-in, this note). Documented in schema.py, configuration.md.

**Memo cap.** `pack::POINT_FREE_MEMO_CAP` = 2^20 entries: at the cap the memo is cleared and fills again. The values
are pure, so a clear changes only the time; a test with cap 3 gives the same qmtech crop copper. Measured with the
graded price, det s0: qmtech 14.5-14.7k entries, esp 5.0k.

**Tests.** Throttle unit test: a stream of escapes and re-lays of measured costs keeps the share; loop_local test of
the escape share on the fan board (both budgets); determinism (same copper and counts for 1 and 4 threads); mutants on
the changed fns. Acceptance on the boards: (escape + in-loop P4) time share of the sweep within 0.35 +- 0.05 on esp
and qmtech s0 det (X and X+graded), copper/metric deltas vs main per board.
