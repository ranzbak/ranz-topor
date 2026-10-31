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

