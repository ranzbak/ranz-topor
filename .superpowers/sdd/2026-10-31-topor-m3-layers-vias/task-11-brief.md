### Task 11 (S): rayon — thread pool, parallel batch searches with deterministic commit, parallel self-check and builds — `engine/Cargo.toml`, `route.rs`, `negotiate.rs`, `check.rs`

**Files:**
- Modify: `engine/Cargo.toml` (`rayon = "1.10"`), `engine/src/route.rs` (`route` wraps `route_inner` in a pool), `engine/src/negotiate.rs` (`iteration` batch), `engine/src/check.rs` (`check_against` parallel over items), `engine/src/multigraph.rs` (`build` parallel over layers)
- Test: `engine/tests/route_threads.rs`, `engine/tests/negotiate_loop.rs`, `tests/test_engine_surface.py`

**Interfaces:**
- Consumes: `Options.threads`, `Options::effective_threads()`, note §7, `RouteGraph.edge_version`, `MultiGraph.site_version`.
- Produces: `route()` = `rayon::ThreadPoolBuilder::new().num_threads(opts.effective_threads()).build()` → `pool.install(|| route_inner(model, opts))`; on build error: sequential with warning `"thread pool unavailable: 1 thread"`; `RouteResult.threads_used` = pool size. `Negotiator::iteration` batch mode: (1) remove every active connection and pending rips from `mg` (snapshot), (2) `active_in_order.par_iter().map(search).collect::<Vec<_>>()` (deterministic order), (3) commit in order: a `Found` path whose edges' `edge_version` and sites' `site_version` all equal the snapshot's is inserted; otherwise the connection is re-searched sequentially against the current graph (counted in `IterOutcome.resolved_sequentially`); `Blocked`/`Timeout` as M2. `check_against` maps items in parallel and concatenates reports in item order. `MultiGraph::build` builds layers in parallel. Metrization sequential. `threads == 1` runs the identical batch code on a 1-thread pool.

- [ ] **Step 1: Write the failing tests**

```rust
// route_threads.rs
#[test] fn route_threads_byte_identical() { /* 12-net mixed 2-layer model: serde_json of route() with threads 1, 2, 4 identical after removing runtime_s and threads_used; threads_used == requested */ }
#[test] fn threads_zero_uses_available_parallelism_minus_one() { /* threads 0 -> threads_used == effective_threads() >= 1 */ }
#[test] fn parallel_self_check_matches_sequential() { /* 200 random items: check_against on a 4-thread pool == on 1 thread */ }
// negotiate_loop.rs
#[test] fn iteration_batch_commit_invalidates_shared_edge() { /* two connections through one cap-1 channel searched in a batch: the second's snapshot path is invalidated (edge_version changed), resolved_sequentially == 1, result equals the sequential M2 outcome (one detours), identical with 1 and 4 threads */ }
```

- [ ] **Step 2: Run to verify failure** — Expected: FAIL (no rayon, no batch).
- [ ] **Step 3: Implement**; `tests/test_engine_surface.py::test_route_threads_option` (`threads` 2 → `threads_used == 2`).
- [ ] **Step 4: Run** all engine suites + clippy, rebuild wheel, pytest surface — Expected: PASS. Timing table in the report: eprom and wiccon `router_s` at threads 1 / 4 (`python -m harness.bench <board> <scratch> --rip all --router topor --threads N`). Gate checks `.venv/bin/pytest tests/gate/test_m2_gate.py -m gate -q -s` with the threads assertion temporarily noted (fixed in Task 12).
- [ ] **Step 5: Progress PNG** `--task T11` (eprom) — the batch semantics may change the routing (outcome-affecting); send to main with the thread timing line.
- [ ] **Step 6: Commit:** `git commit -m "feat(engine): rayon thread pool, parallel batch searches with deterministic commit, parallel self-check (31 §6.1)"`.

