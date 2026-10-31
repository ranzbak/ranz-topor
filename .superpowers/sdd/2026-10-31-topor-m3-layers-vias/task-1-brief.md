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

