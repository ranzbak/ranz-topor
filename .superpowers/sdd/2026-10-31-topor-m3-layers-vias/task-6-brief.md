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

