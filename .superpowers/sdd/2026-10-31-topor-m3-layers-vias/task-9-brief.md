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

