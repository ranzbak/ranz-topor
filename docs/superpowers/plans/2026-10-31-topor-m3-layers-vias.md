# Topor M3 Two Layers + Vias Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Execution method (user, 45 §4, as in M1/M2):** rotating coordinator (O) dispatching one worker per task at the tier in the task title (H/S/F); independent tasks in parallel in isolated worktrees (R3: workers never touch the shared `.venv`); F only for the design note, failed-gate diagnosis and the gate review. Every worker report is folded into `docs/progress/STATE.md` (≤ 5 lines); SDD ledger `.superpowers/sdd/2026-10-31-topor-m3-layers-vias/progress.md`; handover at the M3 gate or at ~50 % context / ~15 round-trips. R1: work on main, **never push**.

**Goal:** Route the ripped benchmark boards on both copper layers with through vias from the JLCPCB drill list: eprom-to-kernal stays clean (27/27, `routing_new` 0, unconnected 0, outside-pad length ≤ hand) and no longer needs the zero-slack +5V corridor because B.Cu is available; wiccon-2023 routes ≥ 90 % of its connections with 0 new DRC violations in < 10 s on 4 threads; output is byte-identical across runs and thread counts.

**Architecture:** M3 keeps the M2 planar slot invariant **per layer** and adds vias as a layer change at pre-placed **via sites** — points that are CDT vertices of every copper layer (one candidate per face of the union triangulation where the default via fits on both layers, plus escape sites next to SMD terminals; fit judged by the exact `Checker`, so keepouts, NPTH, hole-to-hole and pours are respected). The slot A* state gains a layer; a via is "reach site `s` as a goal on layer L, leave it as a start on layer L′" at cost `via_cost`, so each connection chooses its layers by shortest path in the layered graph (13 §2, SURF step 4). An active site becomes an anchor on every layer (capacity, metrization, self-check) and is exclusive; PathFinder negotiates sites like edges. Because wires never cross inside a layer, TopoR's double-crossing removal has nothing to remove; its M3 equivalent is a bounded **via-reduction pass** (TopoR "segment lifting", 13 §2) after convergence. Pour-served pads the pour does not reach get a stub to a via that lands inside the pour (12 §1). Rayon parallelises the searches of one PathFinder iteration against a frozen graph snapshot with a deterministic in-order commit (31 §6.1), the self-check and the per-layer builds; metrization stays sequential Gauss–Seidel. Three M2 carry-overs come first because the via work depends on them: a persistent checker item index, the RC3 sleeve-restricted search queries, and the D1–D10 fixes.

**Tech Stack:** as M2 (Rust 1.96, `spade = "2.15.1"`, pyo3 0.23 abi3-py39, maturin in `.venv`, Python 3.12 + pcbnew SWIG, KiCad 10.0.8 `kicad-cli`, rsvg-convert, ImageMagick) plus **one new crate: `rayon = "1.10"`** (listed per 45 §4 brief rule 5).

**Spec:** `docs/design/45-mvp-plan.md` (§1 M3 gate, §3 T3.1–T3.3, §4–§5a); `docs/topology/13-vias-and-layers.md` (§1–§4), `12` (§1 pour-served, §3 PathFinder, §3.1 tiers, §6 steps 4–5); `docs/geometry/25 §3`; `docs/kicad/30 §4.1, §5.1, §5.2` (via profile, drill list), `31 §6.1` (threads); `docs/progress/STATE.md` (M2 DONE header + M3 carry-over list), `docs/progress/notes/m2-gate-review.md` (RC3, D1–D10), `m2-metrize-design.md` (Relaxer, SlotNesting), `m2-route-diagnosis.md` (§2: why eprom needs B.Cu); M2 plan `docs/superpowers/plans/2026-10-31-topor-m2-router.md` (names reused verbatim).

## Global Constraints

All M0/M1/M2 Global Constraints and rulings still apply (Linux, `.venv` only, read-only source boards with sha256 checks in every gate test, no KiCad types in the engine, KiCad-frame integer nm on the wire with one Y flip each way, `EPS_BUILD_NM` 5 000 / `margin_nm` 5 000, degenerate-arc rule, fab floors, length gate = outside pad copper, R6/R6b/R8 zero-slack retry, progress PNGs, compact ≤ 20-line summaries, conventional commits). In addition for M3:

- **Layers:** routable layers = every `model.layers[].routable` copper layer (eprom/wiccon: F.Cu id 0, B.Cu id 31); a terminal may start/end on any routable layer where it has copper (`on_layer`), THT pads on both; a connection with no common routable layer at either terminal fails `"terminal not on any routable layer"` (no search). Layer tie-break: lower layer id first.
- **Vias (30 §5.2, 13 §5):** through vias only (`ViaItem.layers == []`); sizes per net from `via_sizes(rules, net)`: the net's `(via_d, via_drill)` first (netclass or JLCPCB 0.6/0.3 mm), then every smaller `rules.routing_vias` drill `≥ rules.min_via_drill` with `d = max(drill + 2·min_annular, min_via_diameter)` — never a drill outside `routing_vias` (0.25/0.30/0.40/0.50), never 0.20 or 0.15. A via site is a vertex of every routable layer's CDT; `MIN_SITE_PITCH_NM = 100_000`; a site is usable by a connection iff it is unowned, not blocked by an active neighbour and its `max_d ≥` the net's smallest size. `Options.via_cost_nm` default `2_000_000` (length-equivalent), scaled by `via_weight(tier) = {1: 2.0, 2: 1.0, 3: 1.0, 4: 0.75}`; `Options.max_vias_per_connection` default `4` (a path needing more is `Blocked`). Via items carry `net`, `at`, `d`, `drill`, `wire: Some(conn)`; the KiCad writer makes `VIATYPE_THROUGH`.
- **Caps (45 §5a):** PathFinder `pathfinder_iter_cap` 50, `STALL_ITERS` 8, `STUCK_REPEATS` 3 unchanged; A* expansions per search `≤ 8·Σ_L(E_L + X_L) + 4·S + 64` (S = site count); via sites `O(F + 4·T)` by construction (F faces of the union CDT, T SMD terminals), no cap needed; site activation `O(deg + neighbours)`; via-reduction pass ≤ one re-search + one metrization per connection with vias, one pass; block probe ≤ `PROBE_STEP_CAP` 64 steps with the deadline per step (D9); self-check per round `O(changed × q)` (persistent index); relax caps as M2; wall clock checked as M2 plus before every batch and every via-reduction candidate. No exponential/factorial stage, no order permutations, no all-pairs board loops. **Every new component states its Big-O in its task and the gate review checks it.**
- **Determinism (user):** same model + options ⇒ byte-identical JSON (minus `runtime_s`) for `threads ∈ {1, 2, 4}` and across runs: parallel searches read a frozen snapshot, results are committed in `order()`, a path invalidated by an earlier commit is re-searched sequentially, every reduction is `collect`ed in index order; A* key `(f, h, layer_idx, edge, slot, dir, id)`; `threads_used = opts.effective_threads()`.
- **Budgets (user, 45 §5a):** eprom `router_s < 2.0`; wiccon `router_s < 10.0` with `threads=4`; scaling 1-layer and 2-layer synth within the M2 ratio bounds (`t100/t10 ≤ 40`, `t1000/t100 ≤ 30`); budgets are hypotheses — one F diagnosis round, then escalate to the user with numbers; never relax silently.
- **M3 gate numbers (user, 45 §1 M3):** eprom from `rip="all"`, `threads=4` (bench default): connections routed = all (27 at the last measurement; measured in-run), nets 25/25 (GND pour-served), `partial == failed == 0`, `routing_new == 0`, `unconnected_items == 0` after `--refill-zones`, `width_match 25/25`, `arcs ≥ 1`, no `SS`, `length_outside_pads_nm ≤ hand`, `router_s < 2.0`, `iterations ≤ 50`, `timed_out == false`, `threads_used == 4`; **zero_slack_wires target 0** (user: "aim" — printed and reviewed, see Open question 4). wiccon-2023: `routed ≥ 0.9·connections_total`, `routing_new == 0`, `router_s < 10.0`, `timed_out == false`, `unconnected_items` printed. Both: two runs and `threads 1/2/4` byte-identical. Scaling: existing test + 2-layer variant. M1 and M2 gate tests stay green (M2's `threads_used == 1` assertion becomes `== r["threads"]`, Task 12).
- **Progress PNG (user):** every task whose completion changes a benchmark outcome ends with `.venv/bin/python -m harness.progress --task <TaskId> --router topor [--board wiccon-2023]` → `docs/benchmarks/progress/<YYYY-MM-DD>-m3-<TaskId>.png` (hand left, topor right, **both copper layers rendered and colour-coded**, caption with vias and per-layer length), committed, path + one-line numbers sent to main.
- **Engine version 0.3.0** at the gate (Cargo.toml, `plugin.json`, `SHIPPED_VERSION`); wheel rebuilt with `VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python` before every pytest run that uses the engine; `bash tools/build.sh --dev` before the gate/benchmark; `cargo clippy --all-targets --manifest-path engine/Cargo.toml -- -D warnings` clean.
- Commit messages end with the attribution line given by the executing session's system reminder.

## Review Focus

1. A connection whose pads have copper only on B.Cu (wiccon: 52 of 60 connections failed "terminal not on layer F.Cu" in M2) must route on B.Cu **without a via**; a mixed F/B pair needs exactly one via. → `route_b_cu_pads_route_without_via`, `route_mixed_layers_uses_one_via` (Task 9).
2. Two nets wanting the same via site (or two sites closer than hole-to-hole): the site is exclusive, the neighbour is blocked, the second net takes another site or detours, deterministically, within the iteration cap. → `negotiate_site_contention_two_nets` (Task 9).
3. A candidate site inside a `no_vias` keepout, within `hole_clearance` of an NPTH or within `hole_to_hole` of an existing via drill must be rejected by the fit test (`max_d == 0`). → `via_sites_respect_keepout_npth_and_via_holes` (Task 7).
4. A pour-served net with a pad the pour does not reach and **no** site inside the pour: the pour connection fails `"no via site inside pour"` after one search, nothing else is affected, no loop. → `route_pour_completion_without_site_fails_cleanly` (Task 9).
5. A batch-searched path invalidated by an earlier commit (shared edge) must be re-searched sequentially and give the same result with 1 and 4 threads. → `iteration_batch_commit_invalidates_shared_edge`, `route_threads_byte_identical` (Task 11).

## File Structure

```
engine/
  Cargo.toml                 + rayon = "1.10" (Task 11); version 0.3.0 (Task 12)
  src/lib.rs                 + pub mod vias; pub mod multigraph;
  src/check.rs               ItemIndex, Checker::check_against (Task 1); fit helper via_fits (Task 7)
  src/search.rs              sleeve-restricted clearance_bend, D2/D3/D7/D10 (Task 2); layered state, via move, multi-goal, Leg/SlotPath (Task 8)
  src/graph.rs               D1 terminal clr/r (Task 3); VertexInfo.site, set_site, edge_version (Task 7)
  src/route.rs               D6 stubs from last_reached, D9 probe cap/deadline (Task 3); per-layer Ctx, legs → wires, via items, reports (Task 9); via-reduction pass (Task 10); rayon pool + parallel self-check/builds (Task 11)
  src/negotiate.rs           record_best removed, D4/D5 tests (Task 3); per-layer history, site history, site rip-up (Task 9); batch iteration with commit rule (Task 11)
  src/decompose.rs           decompose(model, opts, layers), TermRef.layers, TermKind::Pour, pour completion connections (Task 6)
  src/model.rs               PourFill, Pour.fills, Options.via_cost_nm / max_vias_per_connection, ViaItem.wire (Task 4 engine part)
  src/obstacle.rs            Anchor.enabled, site anchors on every layer, ObstacleSet::set_site (Task 7)
  src/vias.rs                ViaSize, via_sizes, ViaSite, via_sites (Task 7)
  src/multigraph.rs          LayerCtx, MultiGraph: per-layer graphs, site ownership/blocking, pour membership, insert/remove per connection (Task 7)
  tests/check_index.rs, search_sleeve.rs (+ search_slots.rs), graph_capacity.rs, route_engine.rs, negotiate_loop.rs,
        decompose_nets.rs, via_sites.rs, multigraph.rs, search_layers.rs, route_layers.rs, route_threads.rs
docs/progress/notes/m3-layers-vias-design.md   F design note (Task 5, ≤ 2 pages)
docs/progress/notes/m3-gate-review.md          F gate review (Task 12)
harness/
  model.py                   _export_pours: fills (filled polygon outer rings per layer) (Task 4)
  rules.py                   no change (routing_vias / drill_list already exported); via sizes are engine-side
  fixtures.py                make_fixture_board(..., b_pads=False, pour_gap_pad=False) (Task 4)
  router.py                  drc_spots: via items → one spot per spanned layer (Task 4)
  bench.py                   summary: vias, per-layer length; `--threads` already exists (Task 4)
  progress.py                _render_progress_m3: name -m3-, caption vias/per-layer mm/threads (Task 4)
  render.py                  unchanged (F.Cu,B.Cu,Edge.Cuts already rendered); colour test only (Task 4)
tests/
  test_router.py             pours fills, 2-layer fixture, via spots, m3 progress, colour test (Task 4)
  scaling/synth.py           synth_model(n_pairs, seed, jitter_nm=0, layers=1) (Task 12)
  scaling/test_route_scaling.py   + 2-layer variant (Task 12)
  gate/test_m3_gate.py       M3 gate (Task 12); gate/test_m2_gate.py threads assertion (Task 12)
docs/benchmarks/<date>-topor-0.3.0/{README.md, images/, metrics/}   (Task 12)
```

**Dependencies and parallelism:** Tasks 1, 2, 3, 4, 5 are independent (five parallel worktrees; 2 and 3 split `search.rs` vs `graph.rs`/`route.rs`/`negotiate.rs` so they do not collide). 6 ← 4 (engine `Pour.fills`). 7 ← 1, 3. 8 ← 2, 5, 7. 9 ← 6, 8. 10 ← 9. 11 ← 10. 12 ← 11. Tasks 6 and 7 run in parallel.

**Model tiers:** Task 1 S · Task 2 S · Task 3 S (D4/D5/D8 sub-steps H) · Task 4 S · Task 5 **F** · Task 6 S · Task 7 S (review F at the gate) · Task 8 S from the note (review F at the gate) · Task 9 S · Task 10 S · Task 11 S · Task 12 S + **F** gate review. Any task failing its acceptance twice → F diagnosis note (45 §5).

---

### Task 1 (S): persistent checker item index — `engine/src/check.rs`, `engine/src/route.rs`

**Files:**
- Modify: `engine/src/check.rs` (`check_items` near line 449), `engine/src/route.rs` (`metrize` self-check rounds, `MetrState`)
- Test: `engine/tests/check_index.rs`, `engine/tests/route_engine.rs` (one test)

**Interfaces:**
- Consumes: `Checker::new(&BoardModel, &Options)`, `Checker::check_items(&[Item]) -> CheckReport`, `Item`, `Violation { item, against, layer, gap, required }`, `Grid<usize>`.
- Produces:

```rust
pub struct ItemIndex { /* items: Vec<Option<Item>>, obstacles: Vec<Option<Obstacle>>, grids: HashMap<u32, Grid<usize>>, free: Vec<usize> */ }
impl ItemIndex {
    pub fn new(cell: f64) -> ItemIndex;                 // cell = Checker::cell()
    pub fn insert(&mut self, item: &Item) -> usize;      // id; label "item:<id>"
    pub fn remove(&mut self, id: usize);                 // O(cells of the item)
    pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool;
}
impl Checker {
    pub fn cell(&self) -> f64;
    /// `items` against the board obstacles and against every indexed item except the ids in `skip`
    /// (sorted) and except each other (callers index what they keep). Violation.against for an
    /// indexed item is "item:<id>".
    pub fn check_against(&self, idx: &ItemIndex, items: &[Item], skip: &[usize]) -> CheckReport;
}
```

`check_items(items)` keeps its semantics and is re-expressed as: empty index, insert as the walk proceeds. `MetrState` gains `idx: ItemIndex` and `item_ids: Vec<Vec<usize>>` per wire; a self-check round checks only the wires in `pending` with `skip = own ids`, then re-inserts the changed wires' items; **complexity per round `O(changed × q)`** instead of `O(|emitted| × q)`. Determinism: ids are allocated from `free` (sorted) then by push; violations sorted as before.

- [ ] **Step 1: Write the failing tests** (`engine/tests/check_index.rs`, model as `check_clearance.rs`: 2 layers, nets 1/2, clearance 200 000, width 250 000)

```rust
#[test] fn check_against_sees_indexed_items_of_other_nets() {
    // idx: net 2 seg y = 5 mm x 0..10; check net 1 seg y = 5.3 mm: gap 0.3-0.25 = 0.05 < 0.205 -> 1 violation against "item:0"
}
#[test] fn check_against_skips_listed_ids_and_same_net() { /* skip [0] -> 0 violations; same net -> 0 */ }
#[test] fn remove_frees_the_slot_and_the_grid() { /* insert a, remove a, check the former conflict -> 0 violations; insert b -> id 0 reused */ }
#[test] fn via_item_in_index_blocks_tracks_on_every_layer_and_drills() {
    // idx: ViaItem net 2 at (5,5) d 0.6 drill 0.3 layers []; net 1 seg layer 31 y = 5.35 -> copper violation;
    // ViaItem net 1 at (5.4,5) drill 0.3 -> hole_to_hole violation (gap 0.1 < 0.255), against "item:0"
}
#[test] fn check_items_equals_check_against_incremental() { /* 20 random items: check_items(all) == walk with check_against + insert (same violations, same order) */ }
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test check_index` — Expected: FAIL (missing symbols).
- [ ] **Step 3: Implement** `ItemIndex`, `check_against`, `cell()`; rewrite `check_items` on top of them (the `seen`/`seen_grids` locals become an `ItemIndex`); switch `metrize` (`route.rs` self-check rounds) to the persistent index: insert items of every metrized wire once, replace on chain change (`remove` old ids, `insert` new), check only `pending` wires with `skip = own ids`.
- [ ] **Step 4: Write the failing route test** (`engine/tests/route_engine.rs`): `route_self_check_rounds_scale_with_changed_wires` — synth-like model with 40 two-pad nets (`tests/scaling/synth.py` layout rebuilt in Rust, or a JSON fixture under `engine/tests/fixtures/`): `RouteResult.self_check_rounds ≤ 3` and `RouteResult.check_candidates_visited` (new field, Σ `CheckReport.candidates_visited`) `< 40 × 40 × 10`.
- [ ] **Step 5: Run** all engine suites + clippy; `smooth_engine` and `route_engine` unchanged otherwise — Expected: PASS. Then `.venv/bin/pytest tests/scaling -m scaling -q -s`: n=1000 self-check share drops (report the `SCALING` line; the ratios must still pass).
- [ ] **Step 6: Commit:** `git commit -m "perf(check): persistent item index, incremental self-check (M2 carry-over)"`.

### Task 2 (S): RC3 sleeve-restricted search queries + D2, D3, D7, D10 — `engine/src/search.rs`

**Files:**
- Modify: `engine/src/search.rs` (`clearance_bend` :557, `bend_ok` :676, `los` :751, `step_cost` :834, `direct` :1151, `hard` :529)
- Test: `engine/tests/search_sleeve.rs` (new), `engine/tests/search_slots.rs`

**Interfaces:**
- Consumes: `RouteGraph::verts_near(bb)`, `LayerObstacles::query_walls(bb)`, `Node { apex, since, .. }`, `LOS_PORTAL_CAP = 32`, `Anchor.only_nets`, `VertexInfo.owner/nets`.
- Produces: `clearance_bend(a, t, portals: &[usize])` queries `≤ portals.len() + 1` boxes — the sub-chords between consecutive portal crossing points (`seg_closest(chord, portal)` parameters, sorted), each grown by `ρ` — and merges the candidates into one sorted, dedup'd `Vec<usize>`; **per expansion `O((C + 1) · local)`**, no query spans the chord's full bbox. D2: `hard(e)` is true when `cap(e) < w + ε + 2·max(c, c_e)` (statically unfit, O(1)) — never pushed, also not in ghost mode's soft cost (ghost keeps `GHOST_BLOCK_NM` for non-passable, and `hard` edges are skipped as impassable). D3: `direct()` calls `clearance_bend(p, q, &[])` and returns `None` when it reports a bend. D7: a spot vertex (`owner` starts with `"spot:"`) blocks only the nets in its anchor's `only_nets` (`vi.nets` holds them; empty = all). D10: `bend_ok` also tests the five bend samples against foreign **vertices** from `verts_near(wbb)` (`need = vj.r + vj.clr.max(c) + w/2 + ε`) and point walls (`Core::Point`) that are not vertices of the graph.
- Search stats: `SlotPath.expansions` unchanged; new `pub struct SearchStats { pub expansions: usize, pub candidates: usize }` returned via `SearchOutcome::Found(SlotPath)` → `SlotPath.candidates` (sum of query candidates), used by the tests below.

- [ ] **Step 1: Write the failing tests** (`engine/tests/search_sleeve.rs`; graphs from the `graph_capacity.rs` builders)

```rust
#[test] fn sleeve_query_matches_full_query_first_bend() {
    // proptest, 200 cases: random board 60x40 with 8..30 foreign round pads, random a/t and 0..6 portal edges crossed by the chord:
    // clearance_bend_full (old bbox query, kept as a private test helper under #[cfg(test)]) and the sleeve version return the same (bend, vertex)
}
#[test] fn ghost_candidates_bounded_on_long_board() {
    // 200 pads in a 10x20 grid (pitch 3 mm) on a 40x70 board, unroutable connection (walled off): ghost() returns Some,
    // path.candidates <= expansions * (LOS_PORTAL_CAP + 1) * 64
}
#[test] fn statically_unfit_edge_is_hard() { /* two pads 1.2 mm apart (cap 0.2): search never crosses that edge; !fits even when empty */ }
#[test] fn direct_path_rejects_third_vertex_inside_band() { /* pads (10,10),(20,10) adjacent, foreign pad at (15,10.6): Found with crossings non-empty (not direct) */ }
#[test] fn spot_vertex_blocks_only_listed_net() { /* spot only_nets [1]: net 2 search passes through its band; net 1 bends */ }
#[test] fn bend_pinched_against_foreign_vertex_is_rejected() { /* pad r 0.8 with a foreign pad centre 1.3 mm beyond it on the wrap side: inner class or Blocked */ }
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test search_sleeve` — Expected: FAIL.
- [ ] **Step 3: Implement** per the Interfaces (the portal list for `clearance_bend` is the `since` chain of `los`; `direct` passes `&[]`).
- [ ] **Step 4: Run** all engine suites + clippy — Expected: PASS; `search_slots`, `negotiate_loop`, `route_engine` unchanged. Then produce a private-badge-b model JSON with `.venv/bin/python -m harness.bench private-badge-b <scratch> --rip all --router topor` (writes `<scratch>/private-badge-b--model.json`) and run `cargo run --release --example route_probe -- <scratch>/private-badge-b--model.json --quiet`: report router seconds (M2: 45 s, 34 s of it in 59 ghost searches; expected a few s) in the task report.
- [ ] **Step 5: Commit:** `git commit -m "perf(search): sleeve-restricted clearance queries (RC3); D2 hard edges, D3 direct check, D7 spot nets, D10 vertex pinch"`.

### Task 3 (S; D4/D5/D8 steps H): D1, D6, D9 fixes, D4/D5 tests, D8, dead code — `graph.rs`, `route.rs`, `negotiate.rs`

**Files:**
- Modify: `engine/src/graph.rs` (`build` :173–290, terminal vertex info :213), `engine/src/route.rs` (`collect_blocks`/`probe_path` :639–779, `partial_pass` :782–955), `engine/src/negotiate.rs` (`record_best`/`best`, `IterOutcome`)
- Test: `engine/tests/graph_capacity.rs`, `engine/tests/route_engine.rs`, `engine/tests/negotiate_loop.rs`

**Interfaces:**
- Consumes: `TerminalInfo { at, net, owner }`, `GraphRules`, `Rules.per_net`, `Connection.width/clearance`, `BlockInfo.last_reached`, `probe_path`, `Negotiator`.
- Produces: D1 — `TerminalInfo` gains `pub clr: f64, pub r: f64` (clearance of the terminal's net rule `max(net clearance, min_clearance)`; `r = width/2` for `Copper`/`Dangling` junction terminals, 0 for pad/via centres) and `RouteGraph::build` uses them for `VertexInfo.clr/r`. D6 — forward partial stubs start at the crossing point of `last_reached` (reverse stubs at the goal as before) with the ghost crossings between. D9 — `pub const PROBE_STEP_CAP: usize = 64`; `probe_path` checks `Instant::now() >= deadline` before every face step and returns `None` on timeout; `collect_blocks` probes only connections whose `BlockInfo.edge.is_none()` or whose path failed metrization (as now) and skips when timed out. D8 — the clinches summary prints `len(regular)` and `len(u)` separately. Dead code — `Negotiator::record_best`/`best` removed (route.rs owns `Snap`). D4 — `negotiate_contended_channel_routes_both` asserts the history switch: after the loop `history[channel edge] > 0` and the detouring connection's iteration-1 path used the channel (record `IterOutcome.paths` of iteration 1). D5 — `route_split_vertex_bend_fails_connection_once`: a Routing-constraint split vertex on the only corridor → that connection `failed` with reason starting `"infeasible"` or `"split vertex"`, `iterations ≤ 5`, others routed.

- [ ] **Step 1: Write the failing tests:** `graph_capacity.rs::terminal_vertex_uses_net_clearance_and_junction_radius` (terminal net clearance 0.3 → `verts[t].clr == 300_000`; junction width 0.4 → `r == 200_000`; `cap` of the incident edge shrinks accordingly); `route_engine.rs::partial_stub_starts_at_last_reached` (wall fixture of M2 Task 10 with a foreign pad between start and wall: the forward stub's first item starts within 1 nm of the `last_reached` crossing point, not at the pad centre); `route_engine.rs::block_probe_respects_deadline_and_step_cap` (`Options { wall_clock_s: 0.05, partial_routes: true, .. }` on a 30-connection model with 10 walled-off connections: `route()` returns within 1.0 s wall time, `timed_out` true, no panic; with `wall_clock_s: 10.0` every probe reports `blocking` and `probe_steps ≤ PROBE_STEP_CAP` per connection, exposed as `ConnReport.probe_steps: Option<usize>`); D4 and D5 tests as above.
- [ ] **Step 2: Run to verify failure** (`--test graph_capacity --test route_engine --test negotiate_loop`) — Expected: FAIL.
- [ ] **Step 3: Implement** D1, D6, D9, D8, remove `record_best`/`best`.
- [ ] **Step 4: Run** all engine suites + clippy — Expected: PASS. Run `.venv/bin/pytest tests/gate/test_m2_gate.py -m gate -q -s` (eprom; D1 changes capacities): must still pass; if the eprom numbers change, progress PNG `--task T3-D1` and report.
- [ ] **Step 5: Commit:** `git commit -m "fix(route): D1 terminal clearance, D6 stubs from last_reached, D9 probe deadline/cap, D4/D5 tests, D8, drop record_best"`.

### Task 4 (S): harness and model preparation — pour fills, 2-layer fixture, via spots, M3 progress, layer colours

**Files:**
- Modify: `harness/model.py` (`_export_pours` :444), `harness/fixtures.py`, `harness/router.py` (`drc_spots`, `_layer_of`), `harness/bench.py` (`_route_summary` :55, summary print :131), `harness/progress.py`, `engine/src/model.rs`
- Test: `tests/test_router.py`, `tests/test_model_export.py`, `engine/tests/model_json.rs`

**Interfaces:**
- Produces (engine `model.rs`):

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct PourFill { pub layer: u32, pub outer: Vec<Pt> }        // KiCad frame on the wire, Y flipped by from_json like copper_shapes
// Pour: + pub fills: Vec<PourFill>
// Options: + pub via_cost_nm: i64 (default 2_000_000), pub max_vias_per_connection: u32 (default 4)
// ViaItem: + #[serde(default, skip_serializing_if = "Option::is_none")] pub wire: Option<usize>
```

- Produces (harness): `model["pours"][i]["fills"] = [{"layer": id, "outer": [[x, y], ...]}, ...]` — for every layer of the zone, every outline `k` of `z.GetFilledPolysList(layer)` (SWIG: `SHAPE_POLY_SET.OutlineCount()`, `.Outline(k)`, `SHAPE_LINE_CHAIN.PointCount()/CPoint(i)` — UNVERIFIED names; fall back to `z.Outline()` if the filled list is unavailable and record a warning in the model) as outer rings only (holes dropped, as M0 copper shapes), points nm; `make_fixture_board(..., b_pads=False, pour_gap_pad=False)`: `b_pads=True` makes SIG pads `"2"`, `"3"` SMD on **B.Cu** (layer set B.Cu only) so SIG needs a via between pad 1 (F.Cu SMD) and pad 2, and pads 2–3 route on B.Cu; `pour_gap_pad=True` adds a GND SMD pad `"9"` on F.Cu at `(10, 30)` that the B.Cu GND pour cannot reach (pour completion fixture). `drc_spots`: a via item named by a violation yields one spot per copper layer of the board (through via) with `r = clearance_nm + d`. `_route_summary` adds `vias`, `length_by_layer_mm: {name: mm}`, `threads_used`; summary line `  route: ... vias V, F.Cu X / B.Cu Y mm`. `progress.render_progress(task, router="topor", board=...)` → `_render_progress_m3`: file `<date>-m3-<task>.png`, caption line 1 `"{task}: {routed}/{connections} connections, {nets_routed}/{nets} nets, partial {p}, failed {f}, vias {v}, arcs {a}, zero slack {z}, iters {i}"`, line 2 `"DRC routing {n}, unconnected {u}, outside pads {x:.1f} vs hand {h:.1f} mm, F.Cu {f:.1f} / B.Cu {b:.1f} mm, router {s:.2f} s on {t} threads"`; `main` gains `--threads` (default 4, passed to `run_bench`). Rendering: `render_png` already renders `F.Cu,B.Cu,Edge.Cuts` with KiCad's theme colours (F.Cu red, B.Cu blue); the test below pins that B.Cu copper is visibly distinct.

- [ ] **Step 1: Write the failing tests**

```python
def test_export_pours_have_fill_outlines(fixture_board):      # pours[0]["fills"][0]["layer"] == B_Cu, >= 4 points, bbox inside the outline
def test_fixture_b_pads_puts_sig_pads_on_b_cu(tmp_path):     # pads 2,3 layers == [B_Cu]; model pads reflect it
def test_fixture_pour_gap_pad_is_not_reached(tmp_path):      # pad "9" of GND not in pours[0]["pads"]
def test_drc_spots_via_item_spots_every_layer():             # fake violation naming a via uuid -> 2 spots (layers 0 and 31), r == clearance + d
def test_route_summary_has_vias_and_layer_lengths():         # fake report with 2 vias, items on both layers -> vias 2, length_by_layer_mm keys F.Cu/B.Cu
def test_progress_m3_caption_and_path(engine, fixture_board, tmp_path):  # name ends "-m3-T4.png"; caption line 1 contains "vias "; line 2 contains "B.Cu"
def test_render_b_cu_copper_is_distinct(tmp_path):           # board with one B.Cu track vs same with one F.Cu track vs none: pairwise image RMSE > 0.002 (harness.render compare helper of test_render_metrics)
```

`engine/tests/model_json.rs::pour_fills_and_via_options_round_trip` — JSON with `fills` parses, Y flipped; `Options::from_json("{}")` gives `via_cost_nm 2_000_000`, `max_vias_per_connection 4`.

- [ ] **Step 2: Run** `.venv/bin/pytest tests/test_router.py tests/test_model_export.py -q` and `cargo test --test model_json` — Expected: FAIL.
- [ ] **Step 3: Implement** engine model types, `_export_pours` fills, fixture params, `drc_spots`, summary, progress M3, CLI `--threads`.
- [ ] **Step 4: Run** the suites again + `tests/test_render_metrics.py` + clippy — Expected: PASS (rebuild the wheel first).
- [ ] **Step 5: Commit:** `git commit -m "feat(harness): pour fill outlines, 2-layer fixture, via DRC spots, M3 progress caption and layer colours"`.

### Task 5 (F): design note — via sites, layered slot search, site negotiation, via reduction, parallel batch — `docs/progress/notes/m3-layers-vias-design.md`

**Files:**
- Create: `docs/progress/notes/m3-layers-vias-design.md` (≤ 2 pages)

**Interfaces:**
- Consumes: this plan's Tasks 7–11 Interfaces, `m2-search-design.md` (state `(edge, slot, dir)`, Rule T, start/goal rules, P1 spokes, `los`), `m2-metrize-design.md` §1–2, `m2-route-diagnosis.md` §2, 13 §1–§4, 12 §1/§3/§3.1/§6, 25 §3, 31 §6.1.
- Produces the note Tasks 7–11 implement, fixing: (1) **site generation**: union CDT (anchor points of all routable layers + outline, no constraints needed) → one candidate per inner face at the incentre when the inradius ≥ `d_default/2 + c_max + ε`; escape sites per SMD terminal pad at `hull_r + c_pad + d/2 + ε` along `±x̂, ±ŷ` of the pad rotation; dedup at `MIN_SITE_PITCH_NM`; fit by `Checker` with net 0 (foreign to all) for sizes largest→smallest; sort by `(x, y)`; complexity `O(F + 4T)` candidates × `O(q)` each. (2) **Layered state**: `(layer_idx, edge, slot, dir)` plus `AtSite(site, layer_idx)`; a site vertex is unowned ⇒ passable like a split vertex (`r = 0`, `clr = 0`, no Rule T strands of its own); the via move = goal rules of M2 into `AtSite(s, L)` (Rule T against the strands around the site vertex on L), then start rules from `AtSite(s, L′)`; cost `via_cost × via_weight(tier)`; at most `max_vias_per_connection` moves; start set = the terminal vertex on every layer where the terminal has copper, goal set likewise; **pour goals** = `AtSite(s, L_pour)` for sites with `in_pour[s] ∋ pour` and free same-net vias listed by the pour (vertices on `L_pour`); `h` = Euclidean to the nearest goal (admissible); expansion cap `8·Σ_L(E_L + X_L) + 4·S + 64`; ghost search: sites and layers free, `GHOST_BLOCK_NM` on non-passable edges and owned/blocked sites. (3) **Site occupancy**: exclusive owner; neighbour blocking rule `|st| < d_s/2 + max_d_t/2 + max(c_s, hole_to_hole)`; activation effects on both layers (`set_site` → `VertexInfo { r = d/2, clr = c_net, nets = [net], owner = "via:<conn>:<site>" }`, incident `cap`/`c_e` recomputed) and on the obstacle anchors (`Anchor.enabled`, `r_core = d/2`, `net`); overuse created by activation is ordinary edge overuse. (4) **Negotiation**: history per `(layer, edge)` and per site (`HIST_SITE = HIST_BLOCK`); a `Blocked` connection whose ghost uses an owned site rips that owner (as the M2 strand rip-up, `MAX_RIP`); `penalise_*` unchanged otherwise; best iteration key `(routed, −vias, Σ est_len)`. (5) **Metrization**: one `Wire` per leg (`Wire.layer = leg.layer`, terminals `Terminal::Via { at }` at sites), `SlotNesting` per layer from that layer's graph, via anchors exempt for their owner (`Members.has`), `ViaItem`s emitted per active site, self-check over tracks + vias. (6) **Via-reduction pass**: after convergence (or stall), connections with `vias ≥ 1` in order of `(−vias, id)`: remove from the multigraph, re-search with `via_cost × 4` and the current history, accept iff `vias` fewer **and** metrization + self-check pass **and** `length_outside ≤ old × 1.10`, else restore the old path — one pass, `O(m_v · (search + metrize))`. (7) **Parallel batch** (Task 11): snapshot = the graph after removing every active connection and pending rips; searches `par_iter` in `order()`; commit in order with the `edge_version`/site-version check; invalidated ⇒ sequential re-search; proof sketch that the result is independent of the thread count. (8) **Complexity table** per 45 §5a for (1)–(7) and the tests that pin each rule (Tasks 7–11). Decide and justify: discrete sites instead of continuous placement (25 §3) for M3 — the graph's vertex/edge/face indices and slot occupancy would need remapping after an incremental CDT insert/remove (spade 2.15.1 supports `ConstrainedDelaunayTriangulation::remove`, but `RouteGraph` does not) — continuous refinement deferred (M4/M5).

- [ ] **Step 1 (F):** Read the inputs above and `search.rs` (`Cyc`, `push_goal`, `goal_ok`, `los`), `graph.rs` (`insert_path`, `strand`, `crossing_lr`), `relax.rs` (`Relaxer`, `slot_nesting`). Write the note with sections (1)–(8); every constant named in this plan appears with its default.
- [ ] **Step 2:** Self-check the note against Review Focus 1–5, the Global Constraint caps and determinism; name the counter-example where a via move at a site adjacent to a terminal would violate Rule T and how the goal rule prevents it.
- [ ] **Step 3: Commit:** `git commit -m "docs(notes): M3 via sites, layered slot search, site negotiation and parallel batch design (F)"`.

### Task 6 (S): decomposition over layers and pour completion — `engine/src/decompose.rs`

**Files:**
- Modify: `engine/src/decompose.rs` (`decompose` :222, `Connection`, `TermRef`, `TermKind`, `terminal_of` :587), `engine/src/route.rs` (call sites compile: `decompose(model, opts, &[layer])`, `on_layer` reads `TermRef.layers`)
- Test: `engine/tests/decompose_nets.rs`

**Interfaces:**
- Consumes: `BoardModel { pads, vias, kept, pours (+fills), rules }`, `Options.scope`, `via_layers`, `point_in_poly` (make `crate::check::point_in_poly` `pub(crate)`), `Pour.pads/vias/fills`.
- Produces:

```rust
pub enum TermKind { Pad(usize), Via(usize), Copper(String), Dangling(String), Pour(usize /* index into model.pours */) }
pub struct TermRef { pub kind: TermKind, pub label: String /* "pour:<uuid>" for Pour */, pub at: Vec2, pub layers: Vec<u32> /* routable copper layers of the terminal; Pour: its fill layers */ }
pub struct Connection { pub id, pub net, pub width, pub clearance, pub tier, pub from: TermRef, pub to: TermRef, pub est_len: f64 }   // `layer` removed
pub fn decompose(model: &BoardModel, opts: &Options, layers: &[u32]) -> Decomposition;   // layers = routable copper layers, sorted
```

Rules: clusters as M2 (union on any shared copper layer); the pour is a cluster node joining its listed pads/vias. **Pour completion (12 §1):** in a net with ≥ 1 pour, every cluster that does not contain a pour gets exactly one connection `cluster → Pour` (attach port = the cluster port nearest the pour's fill outlines; `est_len` = point-to-polygon distance on the pour's layer, 0 when inside; `to.at` = that nearest outline point, `to.layers` = the pour's fill layers); no MST between non-pour clusters of such a net. `pour_served` ⇔ pour present and 0 connections. A net whose pour has no `fills` (older model JSON) behaves as M2 (warning `"pour <uuid> has no fill outline"`). Nets without pours: MST as M2. `terminal_of(Pour)` → `Terminal::Via { at }` (the via lands in the pour). Complexity unchanged (per-net union-find + MST; point-to-polygon `O(P)` per cluster).

- [ ] **Step 1: Write the failing tests** (`engine/tests/decompose_nets.rs`)

```rust
#[test] fn terminals_carry_their_copper_layers() { /* THT pad -> layers [0, 31]; SMD B.Cu pad -> [31]; via -> [0, 31] */ }
#[test] fn pour_completion_connects_unreached_cluster_to_pour() {
    // GND pour on B.Cu fill = rect (0,0)-(30,30), lists pads A (THT); pad C SMD F.Cu at (40,15) unreached: 1 conn from C to Pour,
    // to.label "pour:<uuid>", to.layers [31], est_len == 10 mm, to.at == (30,15); pour_served false; NetInfo.connections == [0]
}
#[test] fn pour_served_net_without_gaps_has_no_connections() { /* all pads listed -> 0 conns, pour_served true (M2 behaviour kept) */ }
#[test] fn two_unreached_clusters_each_connect_to_the_pour_not_each_other() { /* pads C, D unreached and 5 mm apart: 2 conns, both to Pour */ }
#[test] fn pour_without_fills_falls_back_to_m2() { /* fills [] -> warning, no pour connections */ }
#[test] fn decompose_layers_argument_filters_terminal_layers() { /* layers [0] only: SMD B.Cu pad -> layers [] */ }
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test decompose_nets` — Expected: FAIL.
- [ ] **Step 3: Implement**; adapt `route.rs` minimally (single-layer behaviour unchanged: `decompose(model, opts, &[layer])`, early failure when `from.layers`/`to.layers` lacks `layer`, `TermKind::Pour` connections fail `"pour completion needs vias (Task 9)"` until Task 9).
- [ ] **Step 4: Run** all engine suites + clippy — Expected: PASS (`route_engine` unchanged).
- [ ] **Step 5: Commit:** `git commit -m "feat(decompose): terminal layers, pour completion connections (12 §1)"`.

### Task 7 (S, review F at the gate): via sizes, via sites, multigraph — `engine/src/vias.rs`, `engine/src/multigraph.rs`, `graph.rs`, `obstacle.rs`

**Files:**
- Create: `engine/src/vias.rs`, `engine/src/multigraph.rs`; Modify: `engine/src/graph.rs`, `engine/src/obstacle.rs`, `engine/src/check.rs` (`via_fits`), `engine/src/lib.rs`
- Test: `engine/tests/via_sites.rs`, `engine/tests/multigraph.rs`, `engine/tests/graph_capacity.rs`

**Interfaces:**
- Consumes: `Checker::check_items` (Task 1 code path), `Cdt::build`, `Cdt::topology`, `build_layer_cdt(obs, terminals, Routing)`, `RouteGraph::build`, `LayerObstacles.anchors`, `ObstacleSet`, `Rules { routing_vias, min_via_drill, min_via_diameter, min_annular, hole_to_hole, default.{via_d, via_drill}, per_net }`, `Pour.fills`, `point_in_poly`.
- Produces:

```rust
// vias.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)] pub struct ViaSize { pub d: i64, pub drill: i64 }
pub fn via_sizes(rules: &Rules, net: i32) -> Vec<ViaSize>;      // Global Constraints "Vias"; largest first; never empty (net default)
#[derive(Clone, Debug)] pub enum SiteOrigin { Face, Escape(usize /* pad index */) }
#[derive(Clone, Debug)] pub struct ViaSite { pub at: Vec2, pub max_d: i64, pub max_drill: i64, pub origin: SiteOrigin }
pub const MIN_SITE_PITCH_NM: f64 = 100_000.0;
pub fn via_sites(model: &BoardModel, obs: &ObstacleSet, checker: &Checker, layers: &[u32], escape_pads: &[usize], sizes: &[ViaSize], eps: f64) -> Vec<ViaSite>;  // sorted by (x, y); sites with max_d == 0 dropped
// check.rs
impl Checker { pub fn via_fits(&self, at: Vec2, size: ViaSize) -> bool; }   // ViaItem net 0, layers [], 0 violations
// obstacle.rs
// Anchor: + pub enabled: bool (default true); ObstacleSet::exempt_anchor returns true when !enabled
impl ObstacleSet { pub fn add_site_anchors(&mut self, sites: &[ViaSite], layers: &[u32]) -> Vec<Vec<usize>>;   // [site][layer idx] -> anchor index; kind Via, enabled false, r_core = max_d/2, net 0, label "site:<i>"
                   pub fn set_site(&mut self, site_anchors: &[usize] /* per layer */, active: Option<(i32, ViaSize)>); }
// graph.rs
// VertexInfo: + pub site: Option<usize>
// RouteGraph: + pub edge_version: Vec<u64>;  pub fn set_site(&mut self, v: usize, active: Option<(i32, f64 /* r */, f64 /* clr */, String /* owner */)>) -> Vec<usize> /* edges recomputed; bumps their version */
// multigraph.rs
pub struct LayerCtx { pub layer: u32, pub ls: LayerSketch, pub g: RouteGraph }
pub struct MultiGraph { pub layers: Vec<LayerCtx>, pub sites: Vec<ViaSite>, pub site_vertex: Vec<Vec<usize>>, pub site_anchor: Vec<Vec<usize>>,
                        pub owner: Vec<Option<(usize, ViaSize)>>, pub blocked: Vec<u32>, pub in_pour: Vec<Vec<usize>>, pub site_version: Vec<u64>, /* private: site grid, hole_to_hole */ }
impl MultiGraph {
    pub fn build(model: &BoardModel, obs: &mut ObstacleSet, sites: Vec<ViaSite>, layers: &[u32], terminals: &[TerminalInfo], rules: &GraphRules) -> Result<MultiGraph, String>;  // per layer: build_layer_cdt(points = terminals on that layer ∪ all sites, Routing) + RouteGraph::build; sites must be CDT vertices (else Err)
    pub fn layer_index(&self, layer: u32) -> Option<usize>;
    pub fn site_free(&self, s: usize, net: i32, size: ViaSize) -> bool;   // owner None, blocked 0, max_d >= size.d
    pub fn activate(&mut self, s: usize, conn: usize, net: i32, size: ViaSize, clr: f64, obs: &mut ObstacleSet);   // set_site on every layer + anchors; blocked += 1 on neighbours within d/2 + max_d_t/2 + max(clr, hole_to_hole)
    pub fn release(&mut self, s: usize, obs: &mut ObstacleSet);
    pub fn sites_near(&self, bb: Aabb) -> Vec<usize>;                        // sorted
    pub fn vertex_of(&self, layer_idx: usize, p: Vec2) -> Option<usize>;
}
```

Site generation (note §1): candidates from the union CDT faces (incentre) and escape sites of `escape_pads`; fit = `via_fits` for `sizes` largest→smallest (`max_d`/`max_drill` = first that fits; 0 = none); complexity `O((F + 4T) · q)`. `in_pour[s]` = pours whose fill on any layer contains `at` (`O(S · ΣP)` once). Activation `O(deg_L · layers + neighbours)`.

- [ ] **Step 1: Write the failing tests**

```rust
// via_sites.rs
#[test] fn via_sizes_default_then_smaller_routing_vias() { /* JLCPCB rules, net default 0.6/0.3, min_annular 0.1, min_via_drill 0.25: [(600k,300k),(450k,250k)]; netclass via 0.8/0.4 -> [(800k,400k),(600k,300k),(450k,250k)]; never drill 200k */ }
#[test] fn sites_fill_open_board_and_avoid_pads() { /* 50x40 empty board with 4 pads: sites >= 4, every site >= 0.3+0.2+0.005 mm from every pad edge (via_fits), sorted by (x,y) */ }
#[test] fn escape_sites_next_to_smd_terminal() { /* SMD rect pad 1x2 mm at (20,20) rot 0: 4 escape sites at distance hull_r + 0.2 + 0.3 + 0.005 along ±x/±y; a side blocked by a foreign pad is dropped */ }
#[test] fn via_sites_respect_keepout_npth_and_via_holes() { /* no_vias keepout rect, NPTH d 1.0, existing via: candidates inside/near them have max_d 0 (absent) */ }
#[test] fn site_pitch_dedup_and_determinism() { /* two candidates 0.05 mm apart -> one; two runs equal */ }
// multigraph.rs
#[test] fn sites_are_vertices_of_every_layer() { /* 2 layers: site_vertex[s][l] position == sites[s].at; VertexInfo.site == Some(s); r == 0, clr == 0 before activation */ }
#[test] fn activate_shrinks_capacity_and_blocks_neighbours() {
    // site s between two pads 2.6 mm apart: cap(e) 1.6 before; activate(s, conn 0, net 1, 0.6/0.3, clr 0.2) -> cap of edges incident to s drop by 0.3,
    // c_e == 0.2, edge_version bumped, anchors enabled on both layers; a site 0.4 mm away: blocked == 1; release -> restored, blocked 0
}
#[test] fn in_pour_membership_from_fills() { /* pour fill rect on B.Cu: sites inside have in_pour == [0], outside [] */ }
#[test] fn graph_set_site_recomputes_only_incident_edges() { /* graph_capacity.rs: versions of non-incident edges unchanged */ }
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test via_sites --test multigraph --test graph_capacity` — Expected: FAIL.
- [ ] **Step 3: Implement** per the Interfaces and note §1/§3.
- [ ] **Step 4: Run** all engine suites + clippy — Expected: PASS (`route_engine` still single-layer: `route()` untouched).
- [ ] **Step 5: Commit:** `git commit -m "feat(vias): via sizes from the drill list, via sites with exact fit, multigraph with site ownership (T3.2)"`.

### Task 8 (S from the Task 5 note, review F at the gate): layered slot search with via moves and pour goals — `engine/src/search.rs`

**Files:**
- Modify: `engine/src/search.rs` (`State`, `Node`, `push_state`, `push_goal`, `run`, `search`, `ghost`, `direct`, `strand_blockers`), `engine/src/negotiate.rs`/`route.rs` (compile against the new `SlotPath` through a `SlotPath::single_layer()` adapter until Task 9)
- Test: `engine/tests/search_layers.rs`, `engine/tests/search_slots.rs` (unchanged tests pass through the adapter)

**Interfaces:**
- Consumes: `MultiGraph` (Task 7), `Connection`/`TermRef.layers` (Task 6), note §2, Task 2's `clearance_bend`.
- Produces:

```rust
#[derive(Clone, Debug)] pub struct Leg { pub layer_idx: usize, pub start: usize, pub goal: usize, pub crossings: Vec<(usize, usize)>, pub faces: Vec<usize>, pub est_len: f64 }
#[derive(Clone, Debug)] pub struct SlotPath { pub conn: usize, pub legs: Vec<Leg>, pub sites: Vec<(usize, ViaSize)> /* legs.len() - 1 */, pub est_len: f64 /* Σ legs */, pub cost: f64 /* incl. via cost */, pub expansions: usize, pub candidates: usize }
impl SlotPath { pub fn vias(&self) -> usize; pub fn crossings_on(&self, layer_idx: usize) -> Vec<(usize, usize)>; }
pub struct Goal { pub vertices: Vec<(usize, usize)> /* (layer_idx, vertex) */, pub pour_sites: Vec<usize>, pub pour_layer: Option<usize> }
pub struct SearchParams<'a> { pub history: &'a [Vec<f64>] /* [layer_idx][edge] */, pub site_history: &'a [f64], pub weight: f64, pub present_overuse: f64, pub expansion_cap: usize, pub deadline: Instant, pub eps: Option<f64>, pub via_cost: f64, pub max_vias: u32 }
pub fn search(mg: &MultiGraph, obs: &ObstacleSet, conn: &Connection, start: &[(usize, usize)], goal: &Goal, p: &SearchParams) -> SearchOutcome;   // SearchOutcome::Blocked.blocked_edge: Option<(usize layer_idx, usize edge)>
pub fn ghost(mg: &MultiGraph, obs: &ObstacleSet, conn: &Connection, start: &[(usize, usize)], goal: &Goal, p: &SearchParams) -> Option<(SlotPath, Option<(usize, usize)>, Option<(usize, usize)>)>;
pub fn default_expansion_cap(mg: &MultiGraph) -> usize;   // 8·Σ(E+X) + 4·S + 64
pub fn via_weight(tier: u8) -> f64;
```

States and rules per note §2; a via move chooses the largest `ViaSize` of `via_sizes(rules, net)` for which `site_free(s, net, size)`; the state key for closed-set/heap includes `layer_idx` and `vias_used`; `direct()` applies per layer. Complexity `O(V′ log V′)`, `V′ = Σ_L 2(E_L + X_L) + 2S`, per expansion as Task 2.

- [ ] **Step 1: Write the failing tests** (`engine/tests/search_layers.rs`; 2-layer boards 50 × 40 built with the Task 7 helpers)

```rust
#[test] fn tht_pads_route_on_either_layer_without_via() { /* THT pads (10,10),(40,10), foreign SMD F.Cu pad row blocking F.Cu only: Found, legs.len() == 1, legs[0].layer_idx == 1 (B.Cu), vias() == 0 */ }
#[test] fn smd_f_to_smd_b_needs_exactly_one_via() { /* pad F.Cu (10,10), pad B.Cu (40,10): Found, legs 2, sites 1, site at a free site, cost == est_len + 2 mm */ }
#[test] fn via_cost_prefers_detour_when_cheaper() { /* detour on F.Cu 1 mm longer than via route: Found with vias 0; via_cost 0.5 mm -> vias 1 */ }
#[test] fn owned_and_blocked_sites_are_not_used() { /* only site between the pads owned by conn 7: Blocked (ghost Some) with max_vias 4 */ }
#[test] fn max_vias_cap_blocks() { /* path needs 2 vias, max_vias 1 -> Blocked */ }
#[test] fn pour_goal_reaches_nearest_site_inside_pour() { /* SMD F.Cu pad, GND pour fill rect on B.Cu 5 mm away: Found, last leg on B.Cu with est_len 0, sites[0].0 in in_pour */ }
#[test] fn via_move_respects_rule_t_at_site() { /* foreign wire wrapping the site vertex on L: the move is rejected (Blocked or another site), never a crossing */ }
#[test] fn layered_search_is_deterministic_and_capped() { /* two runs equal; expansion_cap 1 -> Blocked/Timeout without panic */ }
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test search_layers` — Expected: FAIL.
- [ ] **Step 3: Implement** per note §2; keep `search_slots.rs` green through `SlotPath::single_layer` (one leg on layer 0).
- [ ] **Step 4: Run** all engine suites + clippy — Expected: PASS.
- [ ] **Step 5: Commit:** `git commit -m "feat(search): layered slot A* with via moves at sites and pour goals (T3.1)"`.

### Task 9 (S): negotiation over layers and sites, `route()` orchestration, via items, reports — `negotiate.rs`, `route.rs`, `py.rs`

**Files:**
- Modify: `engine/src/negotiate.rs`, `engine/src/route.rs` (`route`, `Ctx`, `MetrState`, `metrize`, `path_crossings`, `partial_pass`, `zero_slack_pass`, reports), `engine/src/relax.rs` (`slot_nesting` per layer graph — signature unchanged, called per layer), `engine/src/py.rs` (no signature change)
- Test: `engine/tests/route_layers.rs`, `engine/tests/negotiate_loop.rs`, `tests/test_engine_surface.py`, `tests/test_router.py`

**Interfaces:**
- Consumes: Tasks 6–8, `Relaxer`, `SlotNesting`, `ItemIndex` (Task 1), `Checker`.
- Produces:

```rust
// negotiate.rs
pub const HIST_SITE: f64 = HIST_BLOCK;
pub struct Negotiator { pub history: Vec<Vec<f64>> /* [layer_idx][edge] */, pub site_history: Vec<f64>, pub bumps: Vec<u32>, pub iteration: u32, /* cur, pending_rip */ }
impl Negotiator {
    pub fn new(mg: &MultiGraph, conns: usize) -> Negotiator;
    pub fn iteration(&mut self, mg: &mut MultiGraph, obs: &mut ObstacleSet, conns: &[Connection], active: &[usize], starts: &[Vec<(usize, usize)>], goals: &[Goal], opts: &Options, deadline: Instant) -> IterOutcome;
    pub fn penalise_failed(&mut self, mg: &MultiGraph, conn: usize); pub fn penalise_blocked(&mut self, conn: usize, b: &BlockInfo); pub fn penalise_overuse(&mut self, mg: &MultiGraph) -> usize;
}
pub fn detect_clinches(mg: &MultiGraph, conns: &[Connection], paths: &[Option<SlotPath>], starts: &[Vec<(usize, usize)>]) -> ClinchReport;   // per layer, summed
// route.rs
pub struct ConnReport { /* M2 fields + */ pub vias: usize, pub layers: Vec<String>, pub via_sites: Vec<[i64; 2]> /* KiCad nm */ }
pub struct NetRouteReport { /* + */ pub vias: usize }
pub struct RouteResult { /* M2 fields + check_candidates_visited (Task 1) + */ pub vias: usize, pub length_by_layer_nm: BTreeMap<String, i64>, pub sites: usize, pub via_reduction: String /* Task 10 */ }
pub fn route(model: &BoardModel, opts: &Options) -> RouteResult;   // unchanged signature
```

Orchestration: routable layers = all routable copper layers (sorted); `decompose(model, opts, &layers)`; early failures `"terminal not on any routable layer"`; `ObstacleSet::build` + `add_site_anchors`; `via_sites` with `escape_pads` = SMD terminal pads of live connections, `sizes = via_sizes(rules, 0)`; `MultiGraph::build`; `starts[i]`/`goals[i]` from `TermRef.layers` (Pour → `Goal { pour_sites: in_pour matches on the pour layer, pour_layer }`); loop as M2 with `Negotiator::iteration`; on `Found`, `mg.insert_path(conn, &path)` activates the path's sites (`activate`) and inserts each leg into its layer graph; `MetrState` wires = one per **leg** (`wire_of: Vec<(conn, leg)>`, `conn_wires: Vec<Vec<usize>>`), legs' terminals `Terminal::Via { at: site.at }`, `Wire.layer = layers[leg.layer_idx]`; metrize per layer (`slot_nesting(&mg.layers[l].g, ..)`), emitted items = tracks of all legs + one `ViaItem { kind: "via", net, at: kicad(site.at), d, drill, layers: [], group: None, wire: Some(conn) }` per site, self-checked together via `ItemIndex`; a connection fails if any leg fails. `length_outside_pads_nm` = Σ legs (`outside_len` with the pad skeletons of the real terminals only; vias contribute nothing). Reports as above; `nets_total` counts nets with ≥ 1 connection (pour-completion nets included). Partial stubs: from the ghost's first leg (D6 rule). Zero-slack retry (R6): unchanged, operates on the leg's layer. Complexity: as M2 per layer; via activation per `insert_path` `O(vias · (deg + neighbours))`.

- [ ] **Step 1: Write the failing engine tests** (`engine/tests/route_layers.rs`; 2-layer models, `Options { wall_clock_s: 10.0, ..Default }`)

```rust
#[test] fn route_b_cu_pads_route_without_via() { /* net 1 two SMD B.Cu pads: routed 1, vias 0, items all layer 31, layers == ["B.Cu"] */ }
#[test] fn route_mixed_layers_uses_one_via() { /* SMD F.Cu + SMD B.Cu: routed, vias 1, items: tracks on both layers + 1 ViaItem (d 600_000, drill 300_000, layers []), Checker margin 0 clean, via_sites.len() == 1 */ }
#[test] fn route_tht_board_prefers_f_cu_on_ties() { /* two THT pads, nothing in the way: layers == ["F.Cu"] */ }
#[test] fn route_wall_on_f_cu_goes_under_on_b_cu() { /* M2 wall fixture (locked F.Cu GND track): VCC routed with 2 vias or on B.Cu (THT) — partial/failed 0 */ }
#[test] fn negotiate_site_contention_two_nets() { /* two F->B connections whose only cheap sites are one site and its blocked neighbour: both routed, vias 1 each, different sites, deterministic, iterations <= 50 */ }
#[test] fn route_pour_completion_lands_via_in_pour() { /* pour_gap fixture model: GND pour conn routed, 1 via inside the fill polygon, ConnReport.to.kind == "pour", net status routed */ }
#[test] fn route_pour_completion_without_site_fails_cleanly() { /* pour fill 0.5 mm wide: failed "no via site inside pour", iterations <= 2, other nets routed */ }
#[test] fn route_terminal_on_no_routable_layer_fails() { /* pad with layers [] : failed "terminal not on any routable layer" */ }
#[test] fn route_two_layers_deterministic() { /* 6 mixed nets, two runs byte-identical minus runtime_s */ }
```

`negotiate_loop.rs`: `site_rip_up_frees_owner_for_blocked_connection` (ghost of a blocked connection uses an owned site → owner ripped next iteration, `site_history > 0`, both routed within the cap); M2 tests pass through the single-layer adapter.

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test route_layers --test negotiate_loop` — Expected: FAIL.
- [ ] **Step 3: Implement** per the Interfaces and note §3–§5; `tests/test_engine_surface.py::test_route_surface_round_trip` extended: keys `vias`, `length_by_layer_nm`, `sites`.
- [ ] **Step 4: Run** all engine suites, clippy, rebuild the wheel, `.venv/bin/pytest tests/test_engine_surface.py tests/test_router.py -q` — Expected: PASS; `test_router.py::test_route_router_on_fixture` extended with `b_pads=True`: `routed == connections_total`, `routing_new == 0`, `vias >= 1`, via written as `PCB_VIA` through.
- [ ] **Step 5: First 2-layer runs:** `.venv/bin/python -m harness.progress --task T9 --router topor` (eprom) and `--board wiccon-2023 --task T9-wiccon`; commit both PNGs; send paths + numbers (routed, vias, routing_new, unconnected, outside vs hand, zero_slack_wires, router_s) to main; record in STATE.
- [ ] **Step 6: Commit:** `git commit -m "feat(route): two-layer routing with vias, site negotiation, pour completion, via items (T3.1/T3.2)"`.

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

### Task 12 (S, gate review F): M3 gate, 2-layer scaling, version 0.3.0, benchmark 0.3.0

**Files:**
- Create: `tests/gate/test_m3_gate.py`, `docs/benchmarks/<date>-topor-0.3.0/{README.md, images/, metrics/}`, `docs/progress/notes/m3-gate-review.md` (F)
- Modify: `tests/gate/test_m2_gate.py` (`threads_used == r["threads"]`), `tests/scaling/synth.py`, `tests/scaling/test_route_scaling.py`, `engine/Cargo.toml`, `plugin/plugin.json`, `plugin/topor_plugin.py` (0.3.0), `docs/benchmarks/README.md`, `docs/progress/STATE.md`

**Interfaces:**
- Consumes: `run_bench(board, out, router=route_router, rip="all", threads=4, options={...})`, `measured_hand_facts`, `sha256_tree`/`assert_unchanged`, `png_size`, `topor_engine.route`.
- Produces: `synth_model(n_pairs: int, seed: int, jitter_nm: int = 0, layers: int = 1) -> dict` — `layers == 2`: `layers` list gains B.Cu (id 31, routable), the second pad of every pair is SMD on B.Cu only (every connection needs one via); scaling test parametrised over `layers ∈ {1, 2}` with the M2 bounds and `vias == routed` asserted for `layers == 2`.

- [ ] **Step 1: Version 0.3.0** in `engine/Cargo.toml`, `plugin/plugin.json`, `SHIPPED_VERSION`; `bash tools/build.sh --dev`; `tests/test_engine_surface.py::test_version_matches_cargo` green.
- [ ] **Step 2: Scaling:** add the `layers` parameter and the 2-layer variant; run `.venv/bin/pytest tests/scaling -m scaling -q -s` — Expected: PASS for both variants (on failure: the one F diagnosis round of 45 §5a, then escalate).
- [ ] **Step 3: Write the gate test** (`tests/gate/test_m3_gate.py`)

```python
@pytest.mark.gate
def test_m3_gate_eprom_two_layers(engine, tmp_path):
    src = BENCH_BOARDS["eprom-to-kernal"]; hashes = sha256_tree(src.parent); facts = measured_hand_facts(tmp_path)
    r = run_bench("eprom-to-kernal", tmp_path / "bench", router=route_router, rip="all", threads=4)
    rep, rt, drc, hs = r["router_report"], r["route"], r["routed_drc"], r["hand_stats"]
    assert rep["nets_total"] == rep["nets_routed"] == 25 and next(n for n in rep["nets"] if n["name"] == "GND")["status"] == "pour_served"
    assert rep["connections_total"] == rep["routed"] == facts["wires"] and rep["partial"] == rep["failed"] == 0
    assert drc["routing_new"] == 0 and drc["incomplete"]["unconnected_items"] == 0, drc
    assert r["width_match"]["compared"] == r["width_match"]["matched"] == 25
    ra = r["routed_all"]; assert ra["arcs"] >= 1 and all("SS" not in c["pattern"] for c in rep["connections"])
    assert rep["length_outside_pads_nm"] <= hs["hand_outside_pads_nm"]
    assert r["router_s"] < 2.0 and rep["threads_used"] == 4 and not rep["timed_out"] and rep["iterations"] <= 50
    assert not rep["zero_slack_wires"] or r["zero_slack"]["status"] == "kept"      # target: zero_slack_wires == 0 (printed; Open question 4)
    model_json = (tmp_path / "bench" / "eprom-to-kernal--model.json").read_text()
    runs = {t: json.loads(engine.route(model_json, json.dumps({"threads": t}))) for t in (1, 2, 4)}
    runs["again"] = json.loads(engine.route(model_json, json.dumps({"threads": 4})))
    for x in runs.values(): x.pop("runtime_s"); x.pop("threads_used")
    assert len({json.dumps(x, sort_keys=True) for x in runs.values()}) == 1
    assert png_size(tmp_path / "bench" / "eprom-to-kernal-compare.png")[0] > 0
    assert_unchanged(src.parent, hashes)
    print(f"GATE M3 eprom: routed {rep['routed']}/{rep['connections_total']} nets {rep['nets_routed']}/{rep['nets_total']} vias {rep['vias']} "
          f"layers {rep['length_by_layer_nm']} outside-pads {rep['length_outside_pads_nm']/1e6:.3f} (hand {hs['hand_outside_pads_nm']/1e6:.3f}) mm "
          f"drc_new {drc['routing_new']} unconnected {drc['incomplete']['unconnected_items']} arcs {ra['arcs']} router_s {r['router_s']} "
          f"iters {rep['iterations']} zero_slack_wires {len(rep['zero_slack_wires'])} via_reduction '{rep['via_reduction']}'")

@pytest.mark.gate
def test_m3_gate_wiccon(engine, tmp_path):
    src = BENCH_BOARDS["wiccon-2023"]; hashes = sha256_tree(src.parent)
    r = run_bench("wiccon-2023", tmp_path, router=route_router, rip="all", threads=4, options={"partial_routes": False})
    rep, rt, drc = r["router_report"], r["route"], r["routed_drc"]
    assert rep["routed"] >= 0.9 * rep["connections_total"], (rep["routed"], rep["connections_total"])
    assert drc["routing_new"] == 0 and r["router_s"] < 10.0 and not rep["timed_out"]
    model_json = (tmp_path / "wiccon-2023--model.json").read_text()
    runs = [json.loads(engine.route(model_json, json.dumps({"threads": t}))) for t in (1, 4)]
    for x in runs: x.pop("runtime_s"); x.pop("threads_used")
    assert json.dumps(runs[0], sort_keys=True) == json.dumps(runs[1], sort_keys=True)
    assert_unchanged(src.parent, hashes)
    print(f"GATE M3 wiccon: routed {rep['routed']}/{rep['connections_total']} vias {rep['vias']} drc_new {drc['routing_new']} "
          f"unconnected {drc['incomplete']['unconnected_items']} router_s {r['router_s']} iters {rep['iterations']}")

@pytest.mark.gate
@pytest.mark.parametrize("board", ["private-badge-b"])
def test_m3_router_other_boards_complete(board, tmp_path):   # informational, as M2 (wall_clock_s 60, routing_new 0, router_s <= 65)
```

- [ ] **Step 4: Run** `.venv/bin/python -m harness.measure --board eprom-to-kernal` (re-measure), then `.venv/bin/pytest tests/gate -m "gate or not gate" -q -s` (M0–M3 gates; M2's threads assertion updated) — Expected: PASS. On an M3 criterion failure: one F diagnosis round (`m3-eprom-diagnosis.md` / `m3-wiccon-diagnosis.md`), fix tasks with progress PNGs, re-run; never relax a budget; if still failing, escalate to the user with the numbers.
- [ ] **Step 5: Benchmark 0.3.0:** `tools/bench.sh <board> docs/benchmarks/<date>-topor-0.3.0/work --rip all --router topor --threads 4` for eprom-to-kernal, wiccon-2023, private-badge-b, and nametag-sao / wedding-gift via `--pcb` on scratch copies (`wall_clock_s` 60 for the informational boards); copy `*-hand.png`, `*-topor.png`, `*-compare.png` to `images/`, `--metrics.json`/`--drc-*.json` to `metrics/`; `README.md` after the 0.2.0 template with new columns **vias**, **F.Cu / B.Cu mm**, **threads**, `via_reduction`; the gate criteria table for eprom and wiccon; observations per board; run row in `docs/benchmarks/README.md`; the `work/` directory is not committed. Progress PNGs `--task T12-gate` (eprom) and `--task T12-gate-wiccon`; send to main.
- [ ] **Step 6: Commit:** `git commit -m "feat: M3 gate, 2-layer scaling, benchmark 0.3.0, version 0.3.0"`.
- [ ] **Step 7 (F): Gate review** `docs/progress/notes/m3-gate-review.md`: complexity of every new loop (site generation, multigraph build/activation, layered search per expansion after RC3, batch commit, via-reduction pass, pour goals, persistent index, probe cap) against 45 §5a with file:line; Tasks 7–9 vs the design note; Review Focus tests 1–5 present and asserting the specified facts; determinism (no `HashMap` order reaches an output; batch commit proof); D1–D10 closure; findings BLOCKING / DEFERRED (M4). Fix BLOCKING items (S fix task + progress PNG), then STATE.md handover: M3 DONE header with numbers, carry-overs for M4.

## Deferred to M4 (recorded for STATE.md, with reasons)

1. **Dangling-end G1 continuation** — M4 "finishing hand-started traces" (45 §3); M3 routes from dangling ends as fixed terminals (unchanged from M2).
2. **Continuous via placement / Weiszfeld refinement (25 §3, T3.2 wording "in continuous space")** — M3 places vias at discrete sites that are CDT vertices; moving a via needs incremental CDT insert/remove plus remapping of `RouteGraph` indices and slot occupancy (10 §5); deferred to M4 together with incremental CDT (also needed by the DRC loop when spots arrive mid-run). Reason: scope; the site lattice already yields DRC-clean vias from the drill list.
3. **Planar max-cut layer assignment (13 §1, 45 §5a row "Layer assignment")** — not needed: the layered search assigns layers per connection as the shortest path in the layered graph (SURF step 4, 13 §2) and SMD pads are layer-specific obstacles that the crossing-first model does not represent; max-cut / SURF steepest descent return for > 2 layers in M5. **TopoR double-crossing removal (12 §3.2)** — no crossings exist in the per-layer slot model; its M3 equivalent is the via-reduction pass (Task 10).
4. **Multi-variant search (31 §6.1 "N complete routings with different seeds")** — M3 uses threads for batch searches / self-check / builds; variants (Pareto over via cost, 12 §4) are M5 (`Options.seed` stays unused).
5. **Priority tiers from a plugin table, tier-1 inference, via cost per tier beyond `via_weight`** — M4 (12 §3.1); M3 infers tier 2/3 as M2.
6. **Re-sketch only changed wires** (STATE carry-over) — B2 already re-sketches only the `work` set; the remaining cost is `path_crossings` per changed wire (O(k)); no task. **Zero-slack pass searches** (STATE carry-over) — bounded by R8 (≤ 4 candidates); unchanged in M3; revisit if the eprom gate no longer needs the pass (Open question 4).
7. **Nametag-sao / wedding-gift informational quality** (wedding 20/43 nets, nametag 30 iterations at 44 s) — re-measured in the 0.3.0 benchmark; improvements (Steiner, exit stubs) are M4 items.
8. **B.Cu legs cutting pours** (eprom GND pour on B.Cu): no pour-cut cost in M3; DRC (`unconnected_items`, starved thermals) judges and the spot loop re-routes; a pour-aware layer cost is an M4 item if the benchmarks show islands.

## Self-review notes

- Spec coverage: 45 T3.1 (layer assignment / via minimisation) → Tasks 5, 8, 10; T3.2 (via placement, drill list, CDT with vias) → Tasks 4, 7, 9; T3.3 gate → Task 12; 13 §4 feasible region → `via_fits` (Task 7); 12 §1 pour completion → Tasks 4, 6, 8, 9; 12 §3.1 via cost per tier → `via_weight` (Task 8); 12 §6 steps 4–5 → Tasks 9–10; 30 §5.2 drill list / 0.20 avoided → `via_sizes` (Task 7); 30 §4.1 DRC spots for vias → Task 4; 31 §6.1 threads → Task 11. STATE M3 carry-overs: vias/layers → 6–10; rayon → 11; G1 → deferred 1; RC3 → 2; persistent item index → 1; zero-slack pass searches → deferred 6; re-sketch only changed wires → deferred 6; D1 → 3; D2/D3/D7/D10 → 2; D4/D5/D6/D8/D9 → 3; `record_best` → 3; wedding/nametag → deferred 7.
- Type consistency: `SlotPath { legs, sites }` (Task 8) is what `MultiGraph::insert_path` (Task 7/9), `Negotiator` (Task 9), `MetrState` legs → `Wire`s (Task 9) and the reduction pass (Task 10) consume; `TermRef.layers` (Task 6) feeds `starts`/`goals` (Task 9); `ViaSize` (Task 7) appears in `SlotPath.sites`, `MultiGraph.owner`, `ViaItem` (d/drill); `ItemIndex` (Task 1) is used by `metrize` (Tasks 1, 9) and `check_against` is parallelised in Task 11; `edge_version` (Task 7) is the batch commit key (Task 11); `PourFill`/`in_pour` (Tasks 4, 7) feed `Goal.pour_sites` (Task 8).
- Proportion: interfaces, tests and values only; the layered search rules live in the F note (Task 5), as the M2 search rules did.
- Risk for the eprom "no zero-slack" target: it depends on the +5V link U2.28→J1.2 taking B.Cu (THT pads, no via needed) — the search finds it only if the B.Cu corridor under J1 is free of the 17 GND stitching vias' bands; if not, Task 10 Step 4 escalates with the measurement rather than touching `margin_nm`.
