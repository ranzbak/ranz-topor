### Task 10 (S): via-reduction pass (M3 double-crossing equivalent) — `engine/src/route.rs`

**Files:**
- Modify: `engine/src/route.rs` (new `via_reduction_pass`, called after the PathFinder loop and before `zero_slack_pass`), `engine/src/model.rs` (`Options.via_reduction: bool` default true)
- Test: `engine/tests/route_layers.rs`

**Interfaces:**
- Consumes: note §6, `MultiGraph`, `Negotiator.history/site_history`, `metrize`, `Snap`.
- Produces: `fn via_reduction_pass(cx: &Ctx, best: &mut Snap, ..) -> (usize /* vias removed */, usize /* candidates */)`; `RouteResult.via_reduction = "removed N of V vias in C candidates" | "off" | "skipped: timeout"`; `pub const REDUCTION_VIA_FACTOR: f64 = 4.0`, `pub const REDUCTION_LEN_SLACK: f64 = 1.10`. Rule: connections with `vias ≥ 1` ordered `(−vias, id)`; each: remove, re-search with `via_cost × REDUCTION_VIA_FACTOR`, accept iff fewer vias ∧ metrize + self-check pass ∧ `length_outside ≤ old × REDUCTION_LEN_SLACK`, else restore (`Snap` of the multigraph + relaxer per candidate); deadline before every candidate. One pass, `O(m_v · (search + metrize))`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn via_reduction_removes_detourable_via_pair() { /* F->F connection whose iteration-1 path hopped to B.Cu under a wire later ripped: after the pass vias 0, via_reduction starts "removed 2 of 2" */ }
#[test] fn via_reduction_keeps_necessary_vias_and_length_bound() { /* F->B pad pair: vias stays 1; a reduction that would lengthen > 10 % is not taken */ }
#[test] fn via_reduction_off_option() { /* via_reduction false -> "off", vias unchanged */ }
```

- [ ] **Step 2: Run to verify failure** (`--test route_layers`) — Expected: FAIL.
- [ ] **Step 3: Implement**; run all engine suites + clippy — Expected: PASS.
- [ ] **Step 4: Progress PNG** `--task T10` (eprom) and `--board wiccon-2023 --task T10-wiccon`; report vias before/after and `zero_slack_wires`; send to main. If eprom still needs a zero-slack wire: one F diagnosis (why B.Cu is not taken for +5V #24 — expected causes: site fit near J1, via cost vs corridor), then fix task or escalate; never relax `margin_nm`.
- [ ] **Step 5: Commit:** `git commit -m "feat(route): via-reduction pass after convergence (TopoR lifting, 13 §2)"`.

