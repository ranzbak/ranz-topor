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

