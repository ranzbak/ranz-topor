# STATE

**M1 (taut smoother) — DONE 2026-10-31** (gate verified by main session: `pytest tests/gate -m "gate or not gate"` → 7 passed).
Rotation 3 coordinator ended on an auth error right after gate green; this header written by main.
eprom: 25/27 wires taut + 2 zero-slack fallbacks (+5V, user ruling), 8 arcs, DRC routing 0, unconnected 0,
outside-pad length 71.494 vs hand 76.598 mm, router ~4 ms. Benchmark docs/benchmarks/2026-10-30-topor-0.1.0/ (eprom, wiccon,
dcsgone, nametag-sao, wedding-gift). dcsgone routing_new fixed (882f94c). Version 0.1.0.
**Carry into M2 (first tasks):** F-C2 via holes as obstacles in the checker; CDT failure "constraint crosses another
constraint" (all wires fall back on wiccon-2023 and wedding-gift); M1.5 F2 SS mitre, F3–F5 (docs/progress/notes/m1-gate-review.md);
zero-slack check O(emitted×fallback) → spatial-index local query; nametag fallbacks (self-check 13, hand vertex on CDT vertex 8).


**M2 DONE (0.2.0)** — single-layer router, 2026-10-31, rotation 3 coordinator (HEAD after 0d9e614). Ruling R9 (main): B1–B3 fixed, no budget relaxed, scaling not waived.
Gate: `.venv/bin/pytest tests/gate -m "gate or not gate" -q -s` → 10 passed (M0+M1+M2). eprom: 27/27 conns, 25/25 nets, failed 0, partial 0,
routing_new 0, unconnected 0, width 25/25, outside pads 67.095 vs hand 76.598 mm (was 67.034; ≤ 67.1 bound), KiCad 110.564 vs 112.886, arcs 13,
router 0.169 s, 16 it, zero_slack_wires 1 (+5V, DRC-kept), byte-identical runs. Info: wiccon 8/60 0.029 s; dcsgone 24/231 45.8 s (was 22).
**Scaling PASS:** `.venv/bin/pytest tests/scaling -m scaling -q -s` → 2 passed: n=10 0.002 s 10/10 (1 it), n=100 0.0352 s 100/100 (2 it),
n=1000 0.5704 s 1000/1000 (7 it), t100/t10 17.20 (≤ 40), t1000/t100 16.21 (≤ 30), no timeout (was n=100 75/100 25.3 s, n=1000 timeout).
Other: cargo test --release all green (24 suites), clippy -D warnings clean, `.venv/bin/pytest -q` 122 passed. ~/work/kicad/projects sha256 unchanged.
Fixes: B3 5578d9b (synth pairs ≤ 9 mm, leftovers skipped; exact collinear grid kept as gating fixture; `jitter_nm` extra variant);
design note 1ad0464 docs/progress/notes/m2-metrize-design.md; B1 c65a5df (SlotNesting stacks with dynamic membership — static slot
membership rejected, it broke eprom J1 pin gap → 91 mm; penetrations never add own terminals / far adjacent anchors; funnel collinear tie
= bend on portal side, re-added anchors take slot side); B2 b8444a6 (Relaxer kept across PathFinder iterations, work set = searched/ripped
wires + dyn neighbours + slot dependents, first complete pass ignores the deadline → anytime result; n=1000 tautens 52 000 → 1 627).
Profile n=1000 after B2 (0.56 s): search 0.15, metrize 0.22 (relax 0.11, self-check 0.13 over 12 calls), zero-slack pass 0.18.
Benchmark 0.2.0 re-run 0d9e614 docs/benchmarks/2026-10-31-topor-0.2.0/ (eprom as above; wiccon 8/60; dcsgone 24/231 44.4 s; nametag 17/31
0/6 nets 44.2 s 30 it (was 14/31, 10 it); wedding 32/81, 20/43 nets (was 21/43), 0.11 s; routing_new 0 everywhere).
PNGs docs/benchmarks/progress/2026-10-31-m2-gate-B12{,-nametag-sao,-wedding-gift}.png.
**M3 carry-over:** vias/layers (27/27 eprom via B.Cu if a zero-slack casualty appears), rayon/threads (wiccon < 10 s on 4 threads),
dangling G1 → M4; RC3 search clearance_bend O(V) per expansion (dcsgone ghost searches ~0.58 s each, router 45 s); self-check
`check_items` re-inserts the whole base per call (largest metrize cost at n=1000) → persistent item index; zero-slack pass searches;
re-sketch only changed wires; D1 terminal clr = min_clearance; D2 statically unfit edge only 4× penalised; D3 direct path lacks
third-vertex clearance; D4 contended test never exercises history switch; D5 A4 split-vertex no e2e test; D6 stubs from terminal not
last_reached; D7 spot vertex blocks all nets; D8 clinches len(u) cosmetic; D9 block probe no deadline (dcsgone 5.3 s); D10 bend_ok skips
point walls/foreign vertices; Negotiator::record_best unused; wedding-gift 20/43 nets (−1 vs pre-B1) and nametag 30 it at 44 s (info boards).
Gate review: docs/progress/notes/m2-gate-review.md. Ledger .superpowers/sdd/2026-10-31-topor-m2-router/progress.md.

**M3 IN PROGRESS** — two layers + vias. Plan docs/superpowers/plans/2026-10-31-topor-m3-layers-vias.md; ledger
.superpowers/sdd/2026-10-31-topor-m3-layers-vias/progress.md. Rotation 1 coordinator from 8ace64c. INTEGRATION BRANCH = worktree-agent-a141224bcf3a2052b (coordinator pinned to that worktree; main must ff-merge it).
Main rulings: M3-R1 per-connection layer assignment by layered search (SURF step 4), planar max-cut → M5 (>2 layers), 45 §5a wording
updated in Task 12. M3-R2 via-reduction pass replaces double-crossing removal. M3-R3 discrete via sites; Weiszfeld placement → M4.
M3-R4 zero_slack_wires not a hard 0-assertion; a kept zero-slack wire must be DRC-accepted; count reported; Task 10 F diagnosis if > 0 on
eprom. M3-R5 wiccon ≥ 90 % over all router connections incl. pour completion; signal-only ratio reported too. M3-R6 M2 gate threads
assertion → `== r["threads"]`; eprom still meets M2 numbers (27/27, routing_new 0, unconnected 0, outside ≤ hand 76.598 mm).
M3-R7 rayon 1.10 approved; SWIG pour-fill names verified by a real test on a board copy before use.
| # | Task | Status |
|---|------|--------|
| 1 | persistent checker ItemIndex | DONE 15a614c |
| 2 | RC3 sleeve queries + D2/D3/D7/D10 (+ fix: dcsgone gate 45.7→16.9 s) | DONE 4010c90, 85c21fb |
| 3 | D1/D6/D9/D4/D5/D8, drop record_best | DONE 72a1843, 3b8e0bd |
| 4 | harness: pour fills (SWIG names verified), 2-layer fixture, via spots, M3 progress | DONE f239f6b |
| 5 | F design note m3-layers-vias-design.md (amendments P1–P8) | DONE a0db5a4 |
| 6 | decompose layers + pour completion | DONE 887e92d |
| 7 | vias.rs + multigraph.rs | DONE 07c4b56 |
| 8 | layered slot A* | DONE 80e30d8 |
| 9 | two-layer route | DONE 4b928ec — eprom 27/27 vias 0 zs 0 outside 64.6/76.6 0.05 s; wiccon 50/60 (83 %, gate 90 %) 9.1 s; dcsgone M2 info router 120 s > 65 FAIL |
| 10 | via-reduction pass | DONE 6674cab — eprom no-op; wiccon removed 0 of 28 (Ruling P8 overuse <= best) |
| 11 | rayon batch + deterministic commit | DONE 9005d56 — threads 1/2/4 byte-identical; wiccon 1.90 s @4; dcsgone 120 s |
| 12 | Steps 1–4 DONE 6c4cd28 (0.3.0, test_m3_gate.py, 2-layer scaling, M2 threads assertion, 45 §5a wording); Steps 5–7 pending |
| F | gate diagnosis docs/progress/notes/m3-gate-diagnosis.md 48e95bc — fix tasks F1–F7 |

**Rotation 1 handover (2026-10-31, 15 round-trips, integration head on branch worktree-agent-a141224bcf3a2052b;
shared main checkout at e530f37 — main must fast-forward main to worktree-agent-a141224bcf3a2052b).** Gate run @6c4cd28: 11 passed, 2 failed.
M3 eprom PASS: 27/27, 25/25, vias 0, F.Cu 90.323 / B.Cu 17.773 mm, outside 64.628 vs hand 76.598, DRC 0/0, arcs 13, router 0.095 s, 1 it,
zero_slack_wires 0, byte-identical threads 1/2/4. M2 gate eprom PASS. FAIL: wiccon 50/60 (83 %, signal 48/57) 3.5 s; dcsgone 160/231 120.2 s > 65.
Scaling: layers=2 PASS (ratios 20.9/21.6, vias == routed); **layers=1 FAIL t1000/t100 32.4 > 30** (n=1000 17.7 s, 25 it; was 0.60 s at M3
start — regression since T9/T11, undiagnosed). test_scaffold (debug cargo) FAIL: route_engine partial_on/off stub self-check in debug only.
M2 dcsgone info test passes with 0/231 in 60.04 s (check its options). PNGs m3-T9/T10/T11{,-wiccon}.png sent to main.
**Next (rotation 2):** (1) main's ruling on R-i (wall_clock_s = whole bench run incl. DRC rounds vs per route() call; diagnosis
recommends whole-run, bound unchanged); (2) fix tasks F1 (outline sub-arcs; measured wiccon 60/60) -> F2 topological ghost -> F3 entry
pre-check/static dead set -> F4 batch re-validation -> F5 harness remaining budget -> F7 timers (F6 only if eprom byte-identical), each S
with progress PNG; (3) diagnose the layers=1 scaling regression and the debug-only partial-stub test failures; (4) Task 12 Step 5
benchmark 0.3.0 + Step 7 F gate review (incl. deferred minors in the ledger: D5 test now accepts routing, Rule T test hand-inserted strand,
Ruling P3 strict D2/D10 retest now that eprom zs = 0, note §7 amendment for ghost precompute/dead set). Rulings P0–P9 in the ledger.

**Rotation 2 (2026-10-31, coordinator on main in the shared checkout, from 87fbe54 = rotation 1 integration head).**
Main rulings: **M3-R8** (answer to R-i) `wall_clock_s` is the budget of the WHOLE bench run incl. DRC re-route rounds and zero-slack
re-runs; ≤ 65 s dcsgone bound and all other bounds unchanged; implemented as F5 (harness passes the remaining budget).
**M3-R9** F1 (outline sub-arcs ≤ 25 µm sagitta), F2 (topological 0-1 BFS ghost), F3 (terminal enterability pre-check + sound static
dead set), F4 (O(k) batch re-validation, per-via reach), F5, F7 (per-iteration timers) authorised as M3 fixes; F6 only if eprom stays
byte-identical; TDD, regression test per fix; progress PNG + numbers to main after each outcome-affecting fix.
**M3-R10** one diagnosis round for the layers=1 scaling regression (F, measure first) + fix; fix the debug-profile partial-stub
self-check failures at the root cause (not by changing the test profile); explain the M2 dcsgone info test routing 0/231.
Then Task 12 Step 5 (benchmark 0.3.0) and Step 7 (independent F gate review incl. ledger deferred items). Stop rule (user): no
budget/requirement relaxation; if a gate item still fails after these fixes: commit, numbers + profile here, report to main.

**M2 history (rotation 1, 2026-10-31).** Plan docs/superpowers/plans/2026-10-31-topor-m2-router.md;
ledger .superpowers/sdd/2026-10-31-topor-m2-router/progress.md (briefs, reports, rulings R1–R6).
| # | Task | Status |
|---|------|--------|
| 1 | via holes as Checker obstacles (F-C2) | DONE 2552333 |
| 2 | CDT add_constraint_and_split, ConstraintSet (F note m2-cdt-diagnosis.md) | DONE cb9ca73 (wiccon 0 "crosses" fallbacks) |
| 3 | graph.rs routing graph + cut capacity + slots (+A1–A5 from Task 5 note) | DONE c0f3de6 |
| 4 | pours export, decompose.rs | DONE ae7bd7c; Fix-4a 6f04bc6 (teardrops were exported as pours) |
| 5 | F note m2-search-design.md | DONE 5b55c14 |
| 6 | search.rs slot A* + ghost | DONE 1ebb17e |
| 7 | negotiate.rs | DONE a86d660 |
| 8 | route.rs + PyO3 route/wire_stats | DONE 670f162 |
| 9 | harness router.py, bench --router topor, DRC spots, progress | DONE 68efda8 |
| 10 | partial_routes | DONE bba4488 |
| F-diag | m2-route-diagnosis.md (A13 pinch, A14 enclosure, non-convergence) | DONE f8b57c9 |
| Fix-ABC | bend-pinch admissibility, topological rip-up, stuck stop | DONE 5f1a4a8 |
| Fix-R6 | local zero-slack retry (+5V corridor, DRC judge) + stall stop (STALL_ITERS 8) | DONE 663e7d9 |
| 11 | gate test, scaling test, 0.2.0, benchmark 0.2.0, compare PNGs, F gate review | eprom gate PASS ac31995; bench+PNGs f314966; scaling FAIL; review m2-gate-review.md (B1–B3 blocking, escalated) |
| Fix-B3 | synth ≤ 9 mm pairs, collinear grid kept, jitter variant | DONE 5578d9b |
| Fix-B1 | slot-faithful metrization (note m2-metrize-design.md 1ad0464) | DONE c65a5df |
| Fix-B2 | incremental metrization, anytime first pass | DONE b8444a6 (scaling PASS) |
| Bench-R3 | benchmark 0.2.0 re-run, gate PNGs B12 | DONE 0d9e614 |
**M2 rulings:** R1 work on main, never push. R2 verification = acceptance commands + diff stat; F reviews at the gate (Tasks 3, 6, 7 incl.).
R3 parallel worktree workers don't touch the shared .venv. R5 (main): length gate failing ONLY on zero-slack +5V nets → compare remaining
nets, report +5V separately. **R6 (main, 2026-10-31):** keep margin_nm 5000 globally; after negotiation, a connection still failed and
blocked solely by a sub-ε corridor gets ONE local retry with margin 0 on its corridor edges (no new negotiation loop); accept the
zero-slack wire only if kicad-cli DRC gives routing_new 0, else drop it ("zero-slack rejected by DRC") and gate per option (i) on 26/27
with the casualty named; report zero_slack_wires = N + net names in metrics and PNG caption; the casualty (if any) becomes an M3 gate
item (27/27 via B.Cu + via). Nothing else relaxed. **R6b (main):** R6 applies to the +5V corridor whose sub-ε pinch forces a detour; keep only if DRC routing_new 0 AND unconnected 0 AND outside length strictly drops, else revert; zero_slack_wires in metrics/caption; no further analogies — new relaxations need main. R8 (coord.): Fix-R6 calls accepted — 1 nm quantisation tolerance for the zero-slack self-check (DRC judges), corridor as taut-stage obstacles, ≤ 4 candidates one attempt each. (Superseded note:) coordinator applied R6 by analogy to the +5V corridor whose pinch forces A13's
detour (told main; if main narrows R6, the length gate is reported FAIL).
**M2 eprom numbers (rip all, --router topor):** T9 26/27, outside 82.2 vs 76.6, router 0.31 s. Fix-ABC: 27/27 conns, 25/25 nets,
failed 0, arcs 15, routing_new 0, unconnected 0, outside pads 95.32 vs hand 76.60 (FAIL; +5V 26.92 vs 23.90, A13 29.47 vs 9.38 left loop,
others 38.93 vs 43.32), A14 5.47 vs 5.79, KiCad 138.8 vs 112.9, router 0.60 s, iterations 50 (alternates 26/27↔27/27, not converged).
**Fix-R6 (current, 663e7d9): 27/27 conns, 25/25 nets, failed 0, partial 0, arcs 12, routing_new 0, unconnected 0, outside pads 67.03 vs hand 76.60 (no net above hand: +5V 19.00/23.90, A13 9.10/9.38, A14 5.47/5.79), zero_slack_wires 1 (+5V #24 U2.28→J1.2, DRC accepted), KiCad 110.5 vs 112.9, router 0.10–0.24 s, iterations 16, deterministic.**
PNGs: docs/benchmarks/progress/2026-10-31-m2-{T9,T10,FixABC,FixR6}.png (all sent to main). Tests (663e7d9): cargo all green, clippy clean, pytest 99 passed, M1 gate 3 passed.
**Deferred to the F gate review:** terminal clr = min_clearance in graph (provisional); non-fitting edge only 4× penalised; direct
zero-crossing path has no third-vertex clearance check; contended-channel test never exercises the history switch; A4 split-vertex
bend → connection fails (no end-to-end test); partial stubs start at the terminal, not last_reached; spot vertex blocks every net in
search; clinches summary prints len(u); block probe cost on large boards; Fix-ABC bend test skips foreign vertices / point walls.
**M2 next 3 actions (rotation 2):** (1) Task 11 (S): tests/gate/test_m2_gate.py with the plan numbers + zero_slack_wires reported, scaling test, version 0.2.0, build.sh --dev, benchmark 0.2.0 + gate compare PNGs (send to main); the brief is task-11-brief.md (run scripts/task-brief); (2) F gate review (complexity of new loops incl. Fix-ABC rip-up and Fix-R6 retry, Tasks 3/6/7 algorithms, deferred list above); (3) fix findings, re-run gate, report to main (main pushes).

---

# topor — progress state

## Current milestone
M2 — see header. (Below: M1 history.) M1 (taut smoother) was IN PROGRESS at this point. Coordinator rotation 1 handed over 2026-10-30 at main@HEAD (after ~17 worker round-trips).
Plan: docs/superpowers/plans/2026-10-30-topor-m1-smoother.md. SDD ledger (rulings, briefs, reports, eprom dumps):
.superpowers/sdd/2026-10-30-topor-m1-smoother/progress.md. M0 history: .superpowers/sdd/2026-10-30-topor-m0-harness/.
Gate BLOCKED on 2 user rulings (see Open issues 1–2). Tasks 1–8 done; Task 9 (0.1.0, gate test, benchmark) + F gate review remain.

## M1 task status (plan task # / id / tier)
| # | Task | Status |
|---|------|--------|
| 1 | obstacle model (T1.1, S) | DONE 9fc15f5 |
| 2 | wire extraction (T1.2a, S) | DONE 34e615f |
| 3 | CDT + crossings, spade 2.15.1 (T1.2b, S) | DONE 3dce729 |
| 4 | funnel + sketch (T1.2c, F note → S) | DONE 5825b4e; note docs/progress/notes/m1-sketch-design.md |
| 5 | taut chain (T1.3a, F note → S) | DONE (merge 60e33df); note m1-taut-design.md |
| 6 | stacks + relax (T1.3b, S) | DONE 4773a89 |
| 7 | harness smoother/remove_items/bench/progress (T1.4b, S) | DONE 709f874; caption fix feced21 |
| 8 | smooth() + self-check + fallback + PyO3 (T1.4a, S) | DONE 155cd9f |
| R/R2/R3 | eprom re-baseline (KiCad 10 hand re-route), fab floors, teardrops, netclasses, harness.measure, edit-proof tests | DONE da7544d, 0c8b158, b32ba9f |
| Fix-1 | F diagnosis (m1-eprom-diagnosis.md) fixes 1–4 | DONE dedd7b3 |
| 9 | M1 gate, version 0.1.0, benchmark 0.1.0, images (S, F review) | TODO (needs rulings) |

## Decisions / rulings
- M1 (rotation 1): commit attribution Opus 5.5. Fab minimums are FLOORS (user): design values below the JLCPCB profile are raised and reported (`apply_fab_floors` opt-out); JLCPCB copper-edge floor 0.2 mm (user); hole_clearance floor = PTH 0.35 (not binding on eprom).
- Edge clearance read from .kicad_pro via LoadProject (eprom: 0.25 mm). Netclasses via GetEffectiveNetClass (regex patterns ok).
- Engine self-check runs at design clearance (margin 0); geometry is built with ε_build 5 µm (ε is headroom, KiCad DRC is the rule).
- Nets without hand tracks (eprom GND: pour + 17 free stitching vias) → net status "skipped", reason "no tracks (pour-served)", excluded from nets_total.
- Teardrops: when a wire is replaced, same-net teardrop zones touching removed items are deleted and counted (`teardrops_removed`); KiCad regenerates them. Pour/teardrop zones are not engine obstacles (rule areas only). DRC runs with --refill-zones.
- eprom tests are edit-proof: expectations derived from the copy at test time; `python -m harness.measure --board eprom-to-kernal` prints the numbers to pin before the gate.
- Engine is installed into .venv with `VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python` (build.sh --dev touches the KiCad plugin dir).
- F-designed tasks 4–6 are reviewed in the F gate review (not per task).
- Arc-centre agreement with KiCad <= 100 nm (tested: 200 random arcs vs pcbnew, engine + writer). EPS_BUILD_NM = 5000 nm.
- kicad-python pinned `>=0.8,<0.9`. Source boards read-only (sha256 checked in every gate test). Python only via .venv.
- Wheel tag is cp39-abi3-manylinux_2_34_x86_64 (maturin default), not linux_x86_64.
- build.sh was run with KICAD_DOCUMENTS_HOME=<scratch>; the real KiCad plugin dir is untouched (install = `bash tools/build.sh --dev`).
- Grid: i64 cells clamped ±2^31 nm; non-finite bbox → try_insert Err / insert panics; items > 65 536 cells in a side list; remove() uses the stored range.
- Clearance = max(item net, other net/pad net, pad local, min_clearance) (7cbc33e).
- rip uses board.Delete (Remove corrupts SWIG state on real boards).
- Fuzz gate: track_dangling ≥ 1 (not ≥ 1000: same-net items merge); written item count asserted.
- dcsgonebadge orig DRC 192 vs baseline 210: diff only courtyards_overlap/lib_footprint_issues/starved_thermal (environment); other types exact.
- dcsgonebadge rip keeps 2 free vias / removes 201 (baseline 7/196) — pinned; rip policy open (see Open issues).
- Image compare: baseline alpha flattened on white, 60 px caption cropped. Topola-routed images cannot be regenerated (boards gone).
- Rulings T5/T6/T8/T9 from rotation 1 still hold (see ledger).

## Open issues
1. USER RULING PENDING (asked via main): length gate. Emitted chains attach at pad centres; eprom KiCad track+arc 114.894 mm vs hand 112.886 mm (FAIL) while outside-pad length 71.494 vs 76.598 mm (PASS). Options: gate on outside-pad length, or clip at pad edge (connectivity risk). Do not relax without the ruling.
2. USER RULING PENDING: +5V wires 22/23 on eprom have zero slack (hand at exactly 0.25 mm to edge and J1:1/J1:3) → infeasible with ε → hand fallback; gate wants 27/27 taut. Options: accept zero-slack fallback / ε=0 for walls / gate fails.
3. eprom source may still be edited by the user: re-run harness.measure on a fresh copy right before the gate and report the pinned numbers.
4. Known engine limits (for F gate review): sketch stack-order fan weakness (wires entering a fan on different edges); taut drop rule (d) bbox branch removed in Fix-1 (negative arcs); degenerate arc→segment not merged (pattern may contain "SS", violates the plan constraint); T-split kept item replaced only if all its wires are taut; wall parallel runs need foot anchors (M1.5).
5. From M0: rip stitching-via policy question (before M3); F-C2 via holes as obstacles for NEW vias (before M2); polygon holes exported as outer ring only.
6. Old M0 worktrees under .claude/worktrees/ (9) are merged and can be removed; target/release/trace* are stray F scratch build outputs.

## Last gate results (M0, main@7cbc33e; eprom numbers = OLD KiCad 5 board)
- cargo test --release: 9 suites ok; clippy --all-targets -D warnings clean; pytest default 63 passed; gate+scaling 9 passed (30 s).
- No-router bench runtime_s: eprom 5.2, wiccon 6.6, dcsgone 12.5 (< 60). Orig metrics exact (dcsgone track 4167.082 + arc 43.197 mm). Orig DRC 93 / 48 / 192.
- Hand image RMSE vs baseline: 0.0036 / 0.0003 / 0.0025 (< 0.02). Images: docs/benchmarks/2026-10-30-topor-0.0.1/images/{eprom-to-kernal,wiccon-2023,dcsgonebadge}-hand.png.
- Fuzz (fixture, margin 5000): seeds 1/2/3 → 1000 accepted each (2814/2544/3620 attempts), truncated false, KiCad routing_new 0. Eprom ripped copy seed 1: 1000/20252, routing_new 0. Negative-margin control (−20 µm): 59–98 clearance violations.
- Scaling: grid_probe 1k→100k candidates/query 4.87→4.92 (1.01×), seconds 1.19× (bounds 4× / 20×).

## File map
topor/
  .gitignore                     target/, plugin/wheels/, __pycache__/, .venv/, scratch/, *.kicad_pcb-bak, docs/.ruff_cache/
  Cargo.toml                     workspace: members = ["engine"]
  pytest.ini                     testpaths = tests; pythonpath = .; markers gate, scaling
  engine/
    Cargo.toml                   package topor-engine 0.0.1, lib name topor_engine, crate-type ["rlib","cdylib"],
                                 features python = ["dep:pyo3"]; pyo3 0.23 features ["abi3-py39","extension-module"]
    pyproject.toml               [build-system] maturin>=1.7; [tool.maturin] features=["python"], module-name="topor_engine"
    src/lib.rs                   pub mod geom; pub mod model; pub mod check; pub mod fuzz; #[cfg(feature="python")] mod py;
    src/geom/mod.rs              re-exports; pub const EPS_BUILD_NM: f64 = 5000.0; MIN_SAGITTA_NM = 5.0; MIN_CHORD_NM = 10.0
    src/geom/vec2.rs             Vec2, Orient
    src/geom/prim.rs             Circle, Seg, Arc, Aabb, Prim
    src/geom/tangent.rs          point_circle_tangent, bitangent, all_bitangents
    src/geom/dist.rs             point_seg … arc_arc, dist_prim, Skeleton, dist_prim_skeleton
    src/geom/grid.rs             Grid<T>
    src/geom/quant.rs            IVec2, round_nm, calc_arc_center, TrackPrim, quantise, dequantise
    src/model.rs                 serde types: BoardModel, Pad, PadShape, Hole, CopperShape, Outline, KeptItem, Rules, NetRule, Options; Y flip
    src/check.rs                 Checker: build grid from model, check_items() → CheckReport
    src/fuzz.rs                  fuzz_items(model, options, seed, n) → Vec<TrackPrim>
    src/py.rs                    PyO3 module topor_engine
    tests/                       cargo integration tests per module (props with proptest)
  plugin/
    plugin.json                  identifier "org.topor.router", runtime python, entrypoint "topor_plugin.py"
    topor_plugin.py              M0: prints engine version; checks engine version == shipped version
    requirements.txt             generated by tools/build.sh
    wheels/                      ignored
  harness/
    __init__.py
    paths.py                     kicad_cli(), plugin_dir(identifier), venv_dir(identifier), nm/mm helpers
    fixtures.py                  make_fixture_board(), write_project_file()
    copy.py                      copy_project(), sha256_tree()
    rip.py                       record_widths(), rip_tracks()
    model.py                     export_model(board, rules) → dict; load_board(pcb) (loads project first)
    rules.py                     resolve_rules(), parse_dru(), JLCPCB profile
    writer.py                    write_items(), group handling
    drc.py                       run_drc(), classify(), summarise()
    render.py                    strip_zone_fills(), render_png(), compose_compare()
    metrics.py                   copper_metrics(), runtime fields
    bench.py                     run_bench() end-to-end for one board
  tools/
    dev-venv.sh                  creates .venv (--system-site-packages), installs maturin pytest
    build.sh                     wheel → plugin/wheels → plugin dir; --dev reinstalls into KiCad venv
    bench.sh                     wrapper around harness.bench
    render.sh                    wrapper around harness.render
  tests/
    conftest.py                  fixtures: tmp fixture board path, engine import, kicad_cli skip guard
    test_scaffold.py … test_fuzz_gate.py
    gate/test_m0_gate.py         marked `gate`; uses benchmark board copies
    scaling/test_grid_scaling.py marked `scaling`
  docs/progress/STATE.md         living state (45 §4), created in Task 1
  M1 additions: engine/src/{obstacle,wires,cdt,sketch,taut,relax,smooth}.rs (+ tests/*.rs per module),
    engine/examples/extract_wires.rs, py.rs smooth(); harness/{smoother,progress,measure}.py, writer.remove_items,
    bench --router smoother; docs/progress/notes/m1-{sketch,taut}-design.md, m1-eprom-diagnosis.md; docs/benchmarks/progress/

## Last results (M1, eprom-to-kernal KiCad 10 copy, src sha256 bfaf705c…)
- measure: 53 seg, 0 arc, 17 via (all free), 112.886 mm, widths 0.25×43 / 0.4×10, 25/26 nets with copper, orig DRC 0/0, engine wires 27.
- smoother (T1.4a-fix1): 24/25 nets, 25/27 wires taut, 8 arcs, DRC routing 0 / unconnected 0, width_match 25/25, router 0.004 s, KiCad length 114.9 mm, outside pads 71.5 vs 76.6 mm.
- Progress PNGs: docs/benchmarks/progress/2026-10-30-m1-T1.4a.png, 2026-10-30-m1-T1.4a-fix1.png (both sent to main).
- Tests: cargo all suites green, clippy clean; pytest default 76 passed.

## Next 3 actions
1. Get the user's rulings on Open issues 1–2 (main relays).
2. Task 9: version 0.1.0, tests/gate/test_m1_gate.py with the ruled criteria, benchmark 0.1.0 (eprom + informational wiccon/dcsgone runs: must complete, no new routing violations), progress PNG T1.5, re-measure eprom first.
3. F gate review (complexity of new loops, Tasks 4–6 algorithms, Open issue 4), then M1 handover.
