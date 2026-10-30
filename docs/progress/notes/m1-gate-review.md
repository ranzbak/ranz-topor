# M1 gate review — taut smoother (F, 2026-10-30, main@e4331f9 + T9a in flight)

Read: plan header/constraints/review focus, 45 §5a, notes m1-{sketch,taut}-design, m1-eprom-diagnosis, engine
obstacle/wires/cdt/sketch/taut/relax/smooth (+ tests), harness smoother/bench/measure. Engine untouched; `cargo test
--release` in a scratch target dir (result at the end). Numeric checks (python, scratch) are quoted where used.

**Verdict: PASS with one BLOCKING item (D, the zero-slack classifier must be tightened in T9a before the gate test
pins it). Everything else is DEFERRED to M1.5/M2 with the reasons below.**

## A. Complexity (45 §5a) — PASS, no cap missing, no all-pairs
Caps present: `relax_iter_cap` (relax.rs:293), deadline before every wire (relax.rs:296), `TAUT_PASS_CAP`
(taut.rs:554), `MAX_SLEEVE` (cdt.rs:146, taut.rs:161/555), funnel step guard (sketch.rs:134/150/170), self-check rounds
≤ wires+1 (smooth.rs:365), `propagate` fixpoint ≤ wires (smooth.rs:218), `inside_interval` 48×3 fixed (smooth.rs:111/124),
arc chording ≤ 64 (smooth.rs:167), `tail_in_pad` ≤ verts (wires.rs:263). Every neighbour lookup is a `Grid`/CDT query
(obstacle.rs:99/114, relax.rs:184, wires.rs:258/283, check.rs:441). Nothing is exponential; no board-wide pairs.
Sub-quadratic-in-spirit but not O(k) spots (all DEFERRED, cosmetic at M1 sizes):
- A1 taut.rs:519 `arcs.iter().find` per prim and relax.rs:209 `anchor_of` → O(k²) per pass; index arcs by prim.
- A2 taut.rs:569-574 insertion loop O(k·adds); build by merge instead.
- A3 relax.rs:61-79 `Stacks::radius`: `position()` inside `pos` → O(m²) per call, called per node per pass (m = members
  of one anchor side, ≤ wires). Cache positions in a `HashMap` per anchor when stacks get deep.
- A4 taut.rs:429-447 drop test (e) queries the bbox of the centre triangle: for a long shortcut this is O(n) candidates
  per anchor → O(k·n) per drop pass. Inherent to the centre-triangle design; acceptable for M1, revisit for Tanmatsu.
- A5 smooth.rs:470-489 net report: `reports.iter().filter` per net (nets×wires), `model.kept.iter()` per wire-less net
  (nets×kept) and `ex.skipped.iter().find(kept.contains)` (nets×skipped×kept_c). Index reports/kept/skipped by net.
- A6 Wall clock covers relax only; CDT/sketch (smooth.rs:273-299) and the self-check rounds (smooth.rs:363-403, up to
  n+1 full `Checker::new` + `model.clone()`) do not consult `deadline`. Polynomial and fast, but "anytime" is
  only true for relax. Add a deadline check per round (return what passed so far, `timed_out: true`).

## B. Tasks 4–6 vs their notes — PASS
- Funnel (sketch.rs:127-184) is the note §3 verbatim: non-strict pops, goal processed on both sides, apex advances
  only when the opposite chain is empty, σ: left chain → Ccw. `sketch_wire` implements §2 steps 1–6 incl. the 2 nm
  retry shifts and the frame-edge rejection; `stack_order` = §4 (first crossing, t / 1−t, ties by id).
- Taut (taut.rs): `chain_geometry` = note §1 (InsideObstacle at start/goal, exempt/concentric drop, terminal σ from
  the neighbour, zero-sweep arcs merged through `push_seg`); `tauten` = §2 (drop pass before add pass, hysteresis
  `PEN_TOL` 1 nm / `SLACK_BAND_NM` 10 µm, add-rule σ/position, sorted + deduped adds). Deviations D1–D4 are in
  (tangent.rs:72-73 point terminals; `query_grow`/`margin` getters; per-layer `orders`).
- Relax (relax.rs) = note §3: conservative initial membership, radius formula 21 §5 with `c(i,j)` = max(clr_i, clr_j,
  min) + ε and 0 for same net, dyn grid remove/insert around every tauten, process order (width desc, net, id),
  `same_chain` 1 nm, failed wire retries from the sketch and contributes no dyn items.
- Review-focus tests exist and assert the right things: 1 `smooth_dirty_hand_wire_falls_back` (InsideObstacle path;
  note that a hand merely *crossing* a foreign inflated circle mid-span is repaired, not failed — that is correct),
  2 `wires_t_junction_on_segment_interior`, 3 `smooth_dangling_end_stays_fixed` + `smooth_junction_endpoints_bit_identical`,
  4 `tauten_terminal_inside_foreign_obstacle_is_error`, 5 `smooth_zero_wall_clock_times_out` + `deadline_sets_timed_out`.
- Determinism: no `HashMap` iteration reaches an output (stacks members are sets, candidates sorted, reports by id).

## C. Known limits (STATE open issue 4)
- C(a) **"SS" after quantisation — DEFERRED (M1.5), constraint wording must be amended.** `quantise_chain`
  (quant.rs:156) turns a degenerate arc (sagitta < 5 nm or chord < 10 nm) into its chord → `S S S` in the emitted
  pattern (smooth.rs:419-425). A *blind* merge of the three segments is NOT a valid fix: a degenerate arc at R ≥ 0.4 mm
  still bends the path by up to 0.57°, and merging 2 mm tangent segments moves copper ≈ 10 µm toward the anchor
  (checked: R 0.4 mm / L 2 mm → 10 000 nm; R 1 mm / L 5 mm → 15 800 nm) — more than ε, i.e. a DRC violation.
  Minimal correct fix = **mitre**: in `quantise_chain`, when `quantise_with` yields a `Seg` for an `Arc` input,
  emit nothing for the arc and instead set the previous `Seg.b` and `prev` to `round_nm(M)`, `M = c + u_mid ·
  R / cos(sweep/2)` (`u_mid` = unit vector to `a.mid()`): the two tangent lines meet at M, which lies ≤ 5 nm
  *outside* the arc circle (= the sagitta, by construction of the degenerate rule), so clearance to the anchor is
  untouched and the chain stays within 5 nm of the exact geometry. Terminals are unaffected: the first `start` and the
  last `end` are still the once-rounded shared points (`prev` chain, 23 §4.4). The pattern becomes `SS` with a
  G0 corner < 0.6°; so the plan constraint should read "pattern contains `SS` only at a mitred degenerate arc" and
  `WireReport` gains `mitred_arcs: usize`. Test `quant_props.rs::degenerate_arc_is_mitred`: prims
  `S (0,0)→T_in`, `A c=(5 mm,−1 mm) R=1 mm Ccw sweep 0.004 rad` (sagitta 2 nm), `S T_out→(10 mm, 0)` built with
  `Arc::from_tangent_points` → 2 `Seg`s, shared vertex within 1 nm of M, `|M − c| − R ∈ [0, 6]` nm, first start
  `(0,0)` and last end `(10 000 000, 0)` bit-identical. Not on eprom (8 arcs, smallest 0.088 rad), DRC-safe as is.
- C(b) stack-order fan weakness — DEFERRED (M1.5): wrong order → crossing chains → self-check → that wire falls
  back (bounded). Not on eprom (no shared wrapped anchor). Refinement already specified in m1-sketch-design §4.
- C(c) drop rule (d) removal — **sound, no action.** Verified numerically (20 000 random p, q, c, R, σ with c on the
  σ side of p→q at ≥ R): the σ-consistent wrap is *always* the long way (> π), never a short arc, so after (b)+(c)
  the arc can only be a negative (2π − backward) wrap or a genuine winding loop around c; the funnel never emits a
  winding path (turn < π per vertex) and the add rule inserts along the existing chain, so dropping is right.
  Residual (documented in the note): (e) checks only centres inside the triangle; an obstacle whose centre is within
  R of c_i but outside the triangle (fine-pitch neighbour) can be jumped if the shortcut clears it — a DRC-clean
  homotopy change. DEFERRED (M2: test the disc of radius R around c_i in (e) as well).
- C(d) T-split kept item replaced only if all its wires are taut (`propagate`, smooth.rs:211) — correct and
  conservative; DEFERRED (M1.5: harness-side split of the kept item so the taut part can be replaced). Not on eprom
  (+5V 1/3 taut shows no sharing).
- C(e) wall parallel runs need foot anchors — DEFERRED (M1.5) per the user's zero-slack ruling; the mid-span
  fail-fast (taut.rs:324) reports the honest cause in one pass.

## D. Zero-slack classifier — **BLOCKING (F1): tighten before the gate test pins it**
T9a's definition (hand items pass `Checker` at margin 0, fail at margin ε) is necessary but not sufficient. KiCad's
walkaround router routinely leaves hand tracks at *exactly* the design clearance somewhere along their length, so
"fails at ε" is true for many hand wires that have a perfectly feasible channel at ε. Loopholes (wire excused
although a taut path exists):
1. Failure cause unrelated to ε: `self-check: …` (sub-nm rounding, crossing chains from C(b)), `sleeve`, `infeasible:
   pass cap`, `timeout`, `shares item … with a fallback wire` — all excused if the hand grazes any foreign item at ε.
2. Judged against the *original* board: a hand tight to a neighbouring hand track that the smoother moved away is
   excused although its channel is now wide.
3. (Not a loophole, verified) edge (check.rs:353/420), hole (check.rs:479) and same-net/tie exemptions are in `Checker`.
Fix spec (smooth.rs, after the self-check rounds, T9a scope): `zero_slack(w) := reason(w) starts with "infeasible:"
or "inside obstacle:" (the two ε-binding classes) && Checker(post-smooth model, margin 0) reports no violation for
w's hand items && Checker(post-smooth model, margin ε) reports ≥ 1`. Post-smooth model = `model.kept` minus
`replaced`; call `check_items` with the emitted `items` followed by the hand items of w and read only violations with
`v.item ≥ items.len()` (hand items are same-net to themselves → exempt). `hand_slack_nm` as in T9a. Tests
(smooth_engine.rs): T9a's `zero_slack_hand_wire_is_classified` unchanged; add `timeout_fallback_is_not_zero_slack`
(same fixture, `wall_clock_s: 0.0` → reason "timeout", `zero_slack == false`, `wires_zero_slack == 0`) and
`shared_item_fallback_is_not_zero_slack` (zero-slack wire T-joined to a second wire of the same net through one kept
item → second wire reason "shares item", `zero_slack == false`). Gate rule stays as ruled: every fallback must be
zero-slack; eprom 22/23 satisfy the tightened rule (reason "infeasible: wall … grazed mid-span").

## E. Outside-pad length — PASS (same thing on both sides)
`hand_outside` (smooth.rs:195) and the emitted `outside_len` (smooth.rs:158) use the same `terminal_skeletons`
(both terminal pads of the wire on the wire's layer), the same convex-distance clipping, clip every piece against
both pads along the whole length (re-entries included), and both polylines start/end at the pad centre; fallback
wires contribute `hand_out` on both sides. Only asymmetry: hand arcs are measured as 10° chords (wires.rs:23 samples),
−0.13 % on the hand side (conservative for the gate); eprom hand has 0 arcs. DEFERRED cosmetic: use `Piece.len`
scaling per hand piece. The gate's `length_outside_pads_nm ≤ hand_outside_pads_nm` is a like-for-like comparison.

## Findings
- F1 BLOCKING — zero-slack classifier loopholes (D); fix spec above, lands in T9a.
- F2 DEFERRED (M1.5) — `SS` from degenerate arcs: mitre fix + constraint wording (C(a)).
- F3 DEFERRED (M1.5) — deadline not consulted outside relax (A6).
- F4 DEFERRED (M2) — O(k²)/O(m²)/nets×kept spots A1–A5; disc test in drop rule (e) (C(c) residual).
- F5 DEFERRED (M1.5) — C(b) fan order, C(d) harness item split, C(e) foot anchors, hand-arc chord length (E).

Cargo test (scratch target, `--release`, main@e4331f9): all 18 suites ok, 0 failed.
