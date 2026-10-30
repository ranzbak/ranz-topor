# Topor M1 Taut Smoother Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Execution method (chosen by the user 2026-10-30, 45 §4):** rotating coordinator (O) dispatching one worker per task at the tier in the task title (H/S/F); independent tasks in parallel in isolated worktrees; F only for the design notes, failed-gate diagnosis and the gate review marked below. Every worker report is folded into `docs/progress/STATE.md` (≤ 5 lines each); handover at the M1 gate or at ~50 % context / ~15 round-trips.

**Goal:** Take the hand routing of eprom-to-kernal (a copy; source read-only), keep its topology and pull every trace taut into native KiCad segments + arcs: 26/26 nets converted, 0 new routing DRC violations, total length ≤ the measured hand track length (measured at gate time by `harness.measure`; latest 2026-10-30: 112.9 mm), widths unchanged, deterministic, engine time < 2 s, per-trace fallback to the hand copper when a trace fails its self-check.

**Architecture:** The engine (Rust) grows five modules on top of M0: `obstacle` (board → anchor circles + walls per layer, 20 §2–3), `wires` (hand copper → wires between terminals), `cdt` + `sketch` (constrained Delaunay triangulation via `spade`, crossing sequences of the hand polylines, funnel → anchor sequence with side σ per anchor, stack order per anchor side; 10, 11 §2, 21 §5), `taut` + `relax` (bitangent/arc chains with stack radii, Gauss–Seidel relaxation over all wires with add/drop fix-ups; 21 §2–5, 25 §2), and `smooth` (orchestration, quantisation, `Checker` self-check, per-wire fallback, report). One new PyO3 function `smooth(model_json, options_json) -> str`. The harness gets `smoother.py` (router callable for `run_bench`), `writer.remove_items`, `progress.py` (progress PNGs) and the M1 gate test. The sketch JSON of the M0 outline is dropped: the engine extracts the topology from `model.kept` itself (the harness only needs `rip="none"`).

**Tech Stack:** as M0 (Rust 1.96, pyo3 0.23 abi3-py39, maturin in `.venv`, Python 3.12 + pcbnew SWIG, KiCad 10.0.8 `kicad-cli`, rsvg-convert, ImageMagick) plus the crate **`spade = "2.15.1"`** (CDT; already in `~/.cargo/registry`, builds offline).

**Spec:** `docs/design/45-mvp-plan.md` (§1 M1 gate, §3 T1.1–T1.4, §4–§5a); M0 plan outline `docs/superpowers/plans/2026-10-30-topor-m0-harness.md` (M1 section, Global Constraints, Interfaces); geometry `docs/geometry/20`, `21`, `23`, `25`; topology `docs/topology/10`, `11`, `12 §3.1`; gaps `docs/design/44 §3.1 Must`; state `docs/progress/STATE.md`, `docs/progress/notes/m0-gate-review.md`.

## Global Constraints

All M0 Global Constraints still apply (Linux, `.venv` only, read-only source boards with sha256 checks, no KiCad types in the engine, KiCad-frame integer nm on the wire with one Y flip each way, `ε_build` 5 000 nm, degenerate-arc rule, fixed DRC/render commands, JLCPCB profile precedence, complexity budget, compact ≤ 20-line summaries, conventional commits). In addition for M1:

- New dependency: `spade = "2.15.1"` only (no rayon, no other crates); M1 runs single-threaded and reports `threads_used: 1`.
- Engine version becomes **0.1.0** at the gate (Task 9); wheel rebuilt with `bash tools/build.sh --dev` before every gate/benchmark run.
- Anchor-circle inflation (20 §1–2): `ρ = r_core + c + w/2 + ε_build` with `c = max(clearance(wire net), obstacle own clearance, min_clearance)`; holes use `hole_clearance`, edge uses `edge_clearance`; same-net and net-tie obstacles are exempt exactly as `Checker::is_exempt`.
- Stack radii (21 §5): `R_k = max(r_A + c(A,k) + w_k/2, max_{j<k}[R_j + w_j/2 + c(j,k) + w_k/2])` with every `c` including `ε_build`; same-net pair `c = 0`; stack order comes from the sketch (crossing order on the triangulation edge), never from geometry.
- Terminals: a wire attaches at the **pad centre** (20 §4), at the via centre, at a junction (hand T-vertex, fixed) or at a dangling hand endpoint (fixed); shared endpoints are rounded once and bit-identical (23 §4.4).
- Caps (45 §5a): relaxation iterations `Options.relax_iter_cap` (default 200); sleeve length ≤ `MAX_SLEEVE = 2000` crossings per wire; tauten fix-up passes ≤ `TAUT_PASS_CAP = 64` per wire per iteration; self-check/fallback rounds bounded by the wire count; wall clock `Options.wall_clock_s` with anytime return (`timed_out: true`, best feasible result so far). No all-pairs loops: every neighbour lookup goes through `Grid`.
- Fallback granularity is the **wire**: a wire that fails extraction, tauten or self-check keeps its hand items (they are not listed in `replaced`); net status `taut` (all wires taut), `partial`, `fallback` or `skipped`.
- Output is native arcs only: emitted chains alternate segments and arcs (`pattern` contains `"SS"` only where `quantise_chain` turned a degenerate arc (sagitta < 5 nm or chord < 10 nm) into its chord — DRC-safe, ≤ 5 nm from the exact geometry; the mitre fix is deferred to M1.5, gate review F2; the M1 gate asserts no `"SS"` on eprom), arcs go through `quantise_chain` (degenerate → segment).
- Gate numbers (45 §1 M1, eprom-to-kernal): the user keeps editing the board, so no eprom count or length is pinned; they are **measured at gate time by `.venv/bin/python -m harness.measure --board eprom-to-kernal`** (latest 2026-10-30: 53 segments, 0 arcs, 112.9 mm, widths 0.25 mm x43 / 0.4 mm x10, 17 vias all free, 27 wires, 25 nets with copper, orig DRC 0 with `--refill-zones`, 0 unconnected). Gate criteria (**user rulings 2026-10-30**): every wire taut **or** a zero-slack fallback (wire count = the `harness.measure` engine wire count; a zero-slack fallback is a wire whose hand copper has slack < `ε_build` (5 µm) to a foreign obstacle at design rules, so the channel is infeasible with the build margin — e.g. eprom +5V wires 22/23 against board edge + J1; it keeps its DRC-clean hand copper and is reported as `fallback: zero slack`; no other fallback is allowed); every net with a hand wire `taut` or `partial`/`fallback` only through zero-slack wires; `routing_new == 0`; **length gate = length outside pad copper**: emitted `length_outside_pads_nm` ≤ `hand_outside_pads_nm` (both measured outside the terminal pads' copper, same copy, same run; fallback wires count with their hand length on both sides); total KiCad track+arc length vs the measured hand track length is reported as information only; `width_match` all compared nets, `arcs ≥ 1`, `router_s < 2.0`, two runs byte-identical. eprom is a KiCad-10 project (`.kicad_pro`; netclasses Default/Signal/power with pattern assignments `+5V`,`GND` → power, `OE` and `[DA][0-9]{1,2}` → Signal, resolved through pcbnew's effective netclass). Manufacturer minimums are **floors** (user ruling, `apply_fab_floors=True` default): design values below the JLCPCB profile are raised and reported (`floors: <name> <design> -> <floor> mm`); latest eprom values: edge clearance 0.25 mm (board setup; floor 0.2 mm), clearance 0.15 mm, track width floor-raised 0.15 -> 0.2 mm (nets keep their copper widths), hole clearance floor-raised 0.25 -> 0.35 mm.
- **Progress PNG (user requirement 2026-10-30):** every task whose completion changes the routing outcome on eprom-to-kernal (first taut traces, all nets, arcs instead of segments, fallback working, DRC count down — Tasks 8 and 9 below, and any later fix task) ends with: `.venv/bin/python -m harness.progress --task <TaskId>` → `docs/benchmarks/progress/<YYYY-MM-DD>-m1-<TaskId>.png` (hand left, topor right, zone fills removed, caption `<TaskId>: <taut>/26 converted, DRC routing <n>, <length> mm vs <hand> mm` (hand = copper length of the board under test)), `git add` + commit of the PNG, and the coordinator immediately sends the path plus the one-line numbers to the main session ("main").
- Gate escalation (45 §5a): budgets (length, < 2 s, caps) are hypotheses; one F diagnosis round, then stop and escalate to the user with measurements — never relax a budget silently.
- Commit messages end with the attribution line given by the executing session's system reminder (currently `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`).

## Review Focus

1. A hand track that already violates clearance to a foreign pad (DRC-dirty hand routing): the sleeve passes through an inflated obstacle; tauten must return `Infeasible`/`InsideObstacle` and the wire falls back to its hand copper with a reason — no panic, no emitted violation. → `test_smooth_dirty_hand_wire_falls_back` (Task 8).
2. A hand track ending on the interior of another same-net segment (T-junction without a shared vertex, common in hand routing): extraction must split the segment and create a junction terminal; otherwise the smoothed copper loses the connection. → `wires_t_junction_on_segment_interior` (Task 2).
3. A dangling hand end or a via as a terminal: the emitted chain must end bit-identically at the hand endpoint / via centre so KiCad connectivity (and `track_dangling` counts) are unchanged. → `smooth_dangling_end_stays_fixed` (Task 8).
4. A pad centre lying inside a foreign inflated circle (fine-pitch parts: 0.5 mm pitch with 0.25 mm wire): the start point is inside an obstacle; the wire must fail cleanly (`InsideObstacle`) and fall back, not loop or panic. → `tauten_terminal_inside_foreign_obstacle_is_error` (Task 5).
5. Wall-clock budget hit during relaxation: the engine returns what passed the self-check so far with `timed_out: true` and never hangs. → `smooth_zero_wall_clock_times_out` (Task 8).

## File Structure

```
engine/
  Cargo.toml                  + spade = "2.15.1"; version 0.1.0 (Task 9)
  src/lib.rs                  + pub mod obstacle; pub mod wires; pub mod cdt; pub mod sketch; pub mod taut; pub mod relax; pub mod smooth;
  src/obstacle.rs             ObstacleSet: anchors + walls per layer, inflation, grid queries (T1.1)
  src/wires.rs                Terminal, Wire, extract_wires() from model.kept (T1.2a)
  src/cdt.rs                  Cdt wrapper around spade: build, crossings, reduce (T1.2b)
  src/sketch.rs               funnel(), Sketch (anchor sequence + σ), stack order from crossings (T1.2c, F note)
  src/taut.rs                 chain_geometry(), tauten() with add/drop fix-ups (T1.3a, F note)
  src/relax.rs                Stacks, relax() Gauss–Seidel loop with caps/deadline (T1.3b)
  src/smooth.rs               smooth(): orchestration, quantise, self-check, fallback, SmoothResult (T1.4a)
  src/py.rs                   + smooth(model_json, options_json) -> str
  tests/obstacle_model.rs, wires_extract.rs, cdt_crossings.rs, sketch_funnel.rs, taut_chain.rs, relax_stacks.rs, smooth_engine.rs
docs/progress/notes/m1-sketch-design.md   F design note for Task 4 (≤ 1 page)
docs/progress/notes/m1-taut-design.md     F design note for Tasks 5–6 (≤ 1 page)
harness/
  smoother.py                 smooth_router(model, options) -> dict (items, remove_uuids, report)
  writer.py                   + remove_items(board, uuids) -> int
  bench.py                    router result may be a dict; removes replaced items; summary line; --router flag
  progress.py                 render_progress(task) -> docs/benchmarks/progress/<date>-m1-<task>.png
tests/
  test_smoother.py            fixture-board tests of smoother/bench/writer removal/progress
  gate/test_m1_gate.py        M1 gate (marked gate)
docs/benchmarks/2026-10-30-topor-0.1.0/{README.md, images/, metrics/}
docs/benchmarks/progress/     progress PNGs (Global Constraint)
```

Dependencies: Tasks 1, 2, 3, 7 are independent (parallel worktrees). 4 ← 1, 3. 5 ← 1. 6 ← 4, 5. 8 ← 2, 6, 7. 9 ← 8.

---

### Task 1 (S): Obstacle model — `engine/src/obstacle.rs`

**Files:**
- Create: `engine/src/obstacle.rs`, `engine/tests/obstacle_model.rs`
- Modify: `engine/src/lib.rs` (add `pub mod obstacle;`)

**Interfaces:**
- Consumes: `BoardModel` (math frame, `from_json`), `BoardModel::skeleton(pad, layer) -> Skeleton`, `Rules::net_rule`, `Grid<usize>`, `Aabb`, `Prim`, `Seg`, `Arc`, `dequantise`, `geom::EPS_BUILD_NM`.
- Produces (all nm, math frame):
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObsKind { Pad, Via, Hole, Edge, Shape, Track, Terminal }
#[derive(Clone, Debug)]
pub enum Core { Point(Vec2), Seg(Seg), Arc(Arc) }
pub struct Anchor { pub c: Vec2, pub r_core: f64, pub net: i32, pub kind: ObsKind, pub own_clr: f64, pub tie_nets: Vec<i32>, pub label: String }
pub struct Wall   { pub core: Core, pub r_core: f64, pub net: i32, pub kind: ObsKind, pub own_clr: f64, pub tie_nets: Vec<i32>, pub label: String, pub ends: [usize; 2] /* anchor indices on the same layer */ }
pub struct LayerObstacles { pub anchors: Vec<Anchor>, pub walls: Vec<Wall>, anchor_grid: Grid<usize>, wall_grid: Grid<usize> }
pub struct TerminalObstacle { pub layer: u32, pub at: Vec2, pub net: i32, pub width: i64, pub label: String }
pub struct ObstacleSet { /* per-layer map + rule values */ }
impl ObstacleSet {
    /// `exclude_kept`: uuids of kept items being smoothed (not obstacles). `terminals`: junction/dangling
    /// points of smoothed wires (kind Terminal, r_core = width/2, obstacle for other nets only).
    pub fn build(model: &BoardModel, opts: &Options, exclude_kept: &HashSet<String>, terminals: &[TerminalObstacle]) -> ObstacleSet;
    pub fn layers(&self) -> Vec<u32>;
    pub fn layer(&self, layer: u32) -> &LayerObstacles;
    pub fn exempt(a_net: i32, a_tie: &[i32], net: i32) -> bool;              // (a_net != 0 && a_net == net) || a_tie.contains(&net)
    pub fn clearance(&self, net: i32, kind: ObsKind, obs_net: i32, own_clr: f64) -> f64; // Hole → hole_clearance; Edge → edge_clearance; else max(clr(net), own_clr, min_clearance)
    pub fn rho_anchor(&self, layer: u32, idx: usize, net: i32, width: f64) -> f64; // r_core + clearance + width/2 + margin
    pub fn rho_wall(&self, layer: u32, idx: usize, net: i32, width: f64) -> f64;
    pub fn query_anchors(&self, layer: u32, bb: Aabb) -> Vec<usize>;
    pub fn query_walls(&self, layer: u32, bb: Aabb) -> Vec<usize>;
}
```
  Construction rules: pad skeleton `Point{p,r}` → 1 anchor (`r_core = r`); `Seg{s,r}` → 2 anchors + 1 wall (ends = the two anchors, same `r_core`); `Poly{pts,r}` → one anchor per vertex + one wall per edge; a pad is added on each of `pad.layers` with that layer's shape (per-layer padstack, 44 §3.1); via → Point anchor `r_core = d/2`, net = via net, on `via_layers` (through = all layers), plus a `Hole` anchor `r_core = drill/2` on all layers (net = via net, exempt for own net — the Checker still lacks this, see Deferred); `model.holes` → `Hole` anchor per hole (slot → Seg core wall + 2 anchors) with net = owning pad's net (same lookup as `Checker::new`); outline prims → `Edge` walls (`r_core = 0`, net 0; Arc cores allowed) with endpoint anchors; `copper_shapes` → anchors at all polygon vertices + walls per edge, net = shape net, `tie_nets` from `owner_ref`; `kept` items not in `exclude_kept` → `Track` walls (Seg/Arc core, `r_core = width/2`) with endpoint anchors; `terminals` → `Terminal` anchors. Own clearance per obstacle = `max(local override, clr(obstacle net))` for pads, `clr(net)` otherwise (M0 finding C1). Grid cell = the Checker's formula (`max_w + max_clr + margin`, ≥ 100 000 nm). Complexity O(n) build, O(1) average query.

- [ ] **Step 1: Write the failing tests** in `engine/tests/obstacle_model.rs` (build models with `BoardModel::from_json` on inline JSON like `tests/model_json.rs`; wire net = 3, obstacle net = 2, `rules.default.clearance = 200_000`, `min_clearance = 0`, `hole_clearance = 250_000`, `edge_clearance = 500_000`, margin 5 000; wire width 250 000 unless stated; "≈" = `abs ≤ 1.0` nm)

```rust
#[test] fn circle_pad_one_anchor() { /* circle d=1_600_000 at (10,10) mm, layer 0: anchors.len()==1, walls.is_empty(), rho_anchor(0,0,3,250_000.0) ≈ 1_130_000 */ }
#[test] fn rect_pad_four_corner_anchors_and_walls() { /* rect 1.6×1.6 at (10,10) mm rot 0: 4 anchors at (9.2,9.2),(10.8,9.2),(10.8,10.8),(9.2,10.8) mm (any order), r_core==0, 4 walls, rho ≈ 330_000; rot 90 gives the same corner set */ }
#[test] fn roundrect_anchors_carry_corner_radius() { /* 2.0×1.0 rc 0.25 mm: 4 anchors at centre ± (0.75, 0.25) mm, r_core ≈ 250_000, rho ≈ 580_000 */ }
#[test] fn oval_two_anchors_square_oval_one() { /* oval 1.7×1.0: 2 anchors at (±0.35,0) mm, r_core 500_000, rho ≈ 830_000, 1 wall; oval 1.7×1.7: 1 anchor, rho ≈ 1_180_000 */ }
#[test] fn poly_pad_anchor_per_vertex() { /* trapezoid 4 pts → 4 anchors, 4 walls */ }
#[test] fn same_net_and_net_tie_are_exempt() { /* pad net 3 vs wire net 3 → exempt; pad in tie group with nets [2,4] → exempt for 4 */ }
#[test] fn per_layer_padstack_differs() { /* shapes {0: circle 1.6, 2: rect 1.0×1.0}, layers [0,2] → layer 0: 1 anchor; layer 2: 4 anchors */ }
#[test] fn hole_and_edge_use_their_own_clearance() { /* NPTH d=3 mm → Hole anchor rho ≈ 1_880_000; rect outline 50×40 mm → 4 Edge walls + 4 anchors, rho_wall ≈ 630_000; for width 400_000: 705_000 */ }
#[test] fn pth_hole_belongs_to_pad_net() { /* PTH pad net 2 with drill 1 mm → Hole anchor net 2; exempt for net 2 */ }
#[test] fn via_is_copper_anchor_and_hole_anchor() { /* via d 600_000 drill 300_000 net 2 layers [0,2], board layers [0,1,2] → on layer 1: Via anchor r_core 300_000 and Hole anchor r_core 150_000 */ }
#[test] fn kept_track_is_wall_unless_excluded() { /* kept seg uuid "k1" net 2 width 250_000 → 1 Track wall r_core 125_000 + 2 anchors; with exclude_kept={"k1"} → none */ }
#[test] fn copper_shape_polygon_and_tie_owner() { /* shape triangle net 0 → 3 anchors 3 walls, never exempt; owner_ref of a net tie → tie_nets */ }
#[test] fn terminal_obstacle_for_other_nets_only() { /* TerminalObstacle net 3 width 400_000 → anchor kind Terminal r_core 200_000, exempt for 3, not for 2 */ }
#[test] fn pad_local_clearance_raises_own_clr() { /* local_clearance 300_000 → own_clr 300_000 → rho uses 300_000 */ }
#[test] fn query_returns_only_nearby() { /* 1000 circle pads on a 1 mm grid; query a 2×2 mm box → ≤ 16 anchors */ }
```

- [ ] **Step 2: Run** `cargo test --release -p topor-engine --test obstacle_model` → FAIL (module missing).
- [ ] **Step 3: Implement `obstacle.rs`** per Interfaces (reuse `Checker`'s `via_layers`, hole→pad lookup and `tie_nets_of` logic — move those three helpers into `model.rs` as `pub fn` so both modules share them).
- [ ] **Step 4: Run** `cargo test --release -p topor-engine --test obstacle_model` → PASS; `cargo clippy --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit** `feat(engine): obstacle model — anchor circles and walls per layer (T1.1)`.

---

### Task 2 (S): Wire extraction from hand copper — `engine/src/wires.rs`

**Files:**
- Create: `engine/src/wires.rs`, `engine/tests/wires_extract.rs`
- Modify: `engine/src/lib.rs`

**Interfaces:**
- Consumes: `BoardModel` (`kept`, `pads`, `vias`, `nets`), `Options` (`existing_copper`, `scope.nets`), `copper_gap_skeleton`, `Prim`, `Arc::point_at`, `dequantise`.
- Produces:
```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Terminal { Pad { pad: usize, at: Vec2 }, Via { via: usize, at: Vec2 }, Junction { at: Vec2 }, Dangling { at: Vec2 } }
impl Terminal { pub fn at(&self) -> Vec2; pub fn is_fixed_point(&self) -> bool /* Junction | Dangling */ }
pub struct Wire { pub id: usize, pub net: i32, pub layer: u32, pub width: i64, pub start: Terminal, pub goal: Terminal,
                  pub hand: Vec<Vec2> /* starts at start.at(), ends at goal.at() */, pub kept_uuids: Vec<String>, pub hand_len: f64 }
pub struct Extraction { pub wires: Vec<Wire>, pub skipped: Vec<(String /*uuid*/, String /*reason*/)> }
pub fn extract_wires(model: &BoardModel, opts: &Options) -> Extraction;
```
  Rules: candidates = `kept` items with `net != 0`, `!locked`, net in scope (`scope.nets` by name, all if empty) and `existing_copper == Smooth` (otherwise every item is skipped with reason `"existing_copper fixed"`). Per net and layer build the vertex graph of the candidate items (vertices = integer endpoints; an endpoint lying on the interior of another same-net item (gap ≤ 0 to its centreline, `point_seg`/`point_arc`) splits that item there). Node classes: a vertex inside a same-net pad on that layer (`copper_gap_skeleton(zero-width point, skeleton) <= 0`) belongs to that pad; consecutive in-pad vertices of the same pad form one `Pad` terminal; a vertex at a same-net via position → `Via` terminal; a vertex of degree ≥ 3 → `Junction`; degree 1 → `Dangling`. Wires = maximal chains between terminals; `hand` = terminal point, hand vertices in order (in-pad vertices kept; arcs sampled every ≤ 10° of sweep), terminal point. A chain whose two terminals are the same pad with no junction (loop) is skipped (`"loop"`), its items stay. `hand_len` = Σ hand prim lengths. Wire ids are assigned in order (net code, layer, start terminal x, y) — deterministic.

- [ ] **Step 1: Write the failing tests** in `engine/tests/wires_extract.rs` (pads: circle d 1.6 mm net 5 at P1 (0,0), P2 (10,0), P3 (20,0) mm; kept segs width 250 000 on layer 0; `existing_copper: "smooth"`)

```rust
#[test] fn two_segment_l_into_pad_is_one_wire() { /* segs (0,0)→(9.5,0.3) and (9.5,0.3)→(9.7,0.1) mm: 1 wire, start Pad(P1), goal Pad(P2), hand == [(0,0),(9.5,0.3),(9.7,0.1),(10,0)] mm, hand_len ≈ 10_103_700 nm (abs 10), kept_uuids both */ }
#[test] fn pass_through_pad_splits() { /* (0,0)→(10,0), (10,0)→(20,0): 2 wires, both end at Pad(P2) */ }
#[test] fn wires_t_junction_on_segment_interior() { /* (0,0)→(20,0) and (10,0)→(10,5) with P4 at (10,5): 3 wires, exactly one Junction at (10,0) mm shared by all three */ }
#[test] fn junction_vertex_degree_three() { /* explicit shared vertex at (10,2): 3 wires, Junction at (10,2) */ }
#[test] fn dangling_end_is_terminal() { /* (0,0)→(5,5) only: 1 wire, goal Dangling at (5,5) mm, hand ends there exactly */ }
#[test] fn via_is_terminal() { /* via net 5 at (5,5) d 600_000; seg (0,0)→(5,5): goal Via */ }
#[test] fn locked_net0_and_fixed_are_skipped() { /* locked → skipped "locked"; net 0 → "net 0"; existing_copper Fixed → all skipped, wires empty */ }
#[test] fn scope_nets_filters_by_name() { /* nets {5:"A",6:"B"}, scope.nets=["A"] → only net 5 wires */ }
#[test] fn loop_back_to_same_pad_is_skipped() { /* (0,0)→(3,3)→(0.2,0.1): 0 wires, skipped reason "loop" */ }
#[test] fn hand_arc_is_sampled() { /* kept arc sweep 90° r 5 mm: hand has ≥ 10 points, hand_len ≈ π/2·5 mm (rel 1e-3) */ }
#[test] fn ids_are_deterministic() { /* two extractions on the same model → equal ids/order */ }
```

- [ ] **Step 2: Run** `cargo test --release -p topor-engine --test wires_extract` → FAIL.
- [ ] **Step 3: Implement `wires.rs`** per Interfaces (vertex map keyed by `IVec2`; union of touching items by exact integer equality after splitting; pad containment via `BoardModel::skeleton`).
- [ ] **Step 4: Run** the test → PASS; clippy clean.
- [ ] **Step 5: Commit** `feat(engine): extract wires between terminals from hand copper (T1.2a)`.

---

### Task 3 (S, review F at the gate): CDT and crossing sequences — `engine/src/cdt.rs`

**Files:**
- Create: `engine/src/cdt.rs`, `engine/tests/cdt_crossings.rs`
- Modify: `engine/Cargo.toml` (`spade = "2.15.1"`), `engine/src/lib.rs`

**Interfaces:**
- Consumes: `spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation, HasPosition}`, `spade::LineIntersectionIterator::new(&tri, from, to)` yielding `Intersection::{EdgeIntersection, VertexIntersection, Overlap}` (spade 2.15.1 `intersection_iterator.rs`), `Vec2`, `seg_seg`.
- Produces:
```rust
pub const MAX_SLEEVE: usize = 2000;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing { pub left: usize, pub right: usize, pub t: f64 /* fraction from left to right of the hand crossing point */ }
pub struct Cdt { /* triangulation + input-index <-> handle maps */ }
impl Cdt {
    /// Points are deduplicated (coincident inputs share a vertex). Constraints are index pairs (outline edges).
    pub fn build(points: &[Vec2], constraints: &[(usize, usize)]) -> Result<Cdt, String>;
    pub fn num_vertices(&self) -> usize;
    pub fn vertex_index(&self, input: usize) -> usize;     // canonical index after dedup
    /// Triangulation edges crossed by the polyline, in order, oriented by travel direction (u on the left, v on the right).
    /// Err if a polyline vertex other than the first/last coincides with a triangulation vertex, or > MAX_SLEEVE crossings.
    pub fn crossings(&self, polyline: &[Vec2]) -> Result<Vec<Crossing>, String>;
    /// Cancel adjacent crossings of the same edge in opposite directions (11 §2 step 3), repeatedly.
    pub fn reduce(seq: &[Crossing]) -> Vec<Crossing>;
}
```
  Coordinates stay in nm (f64). `t` is computed from the exact segment–segment intersection point.

- [ ] **Step 1: Write the failing tests** (points in mm × 1e6: A(0,0), B(10,0), C(20,0), D(5,10), E(15,10) → triangles ABD, BDE, BCE)

```rust
#[test] fn build_dedups_coincident_points() { /* points [A, B, A] → num_vertices 2, vertex_index(2) == vertex_index(0) */ }
#[test] fn polyline_crossings_in_order_with_sides() { /* polyline (3,4)→(17,4) mm: 2 crossings: {left: D, right: B, t ≈ 0.6}, {left: E, right: B, t ≈ 0.6} (abs 1e-6) */ }
#[test] fn reverse_direction_swaps_sides() { /* (17,4)→(3,4): first crossing left B, right E */ }
#[test] fn back_and_forth_reduces() { /* (3,4)→(9,4)→(3,4)→(17,4): raw 4 crossings, reduce → 2 equal to the straight case */ }
#[test] fn polyline_starting_on_vertex_is_ok_but_interior_vertex_is_err() { /* A→(17,4): Ok; (3,4)→B→(17,4): Err */ }
#[test] fn constraint_edge_is_kept() { /* square outline as constraints + an interior point: every constraint pair is an edge (spade is_constraint_edge) */ }
#[test] fn sleeve_cap() { /* zig-zag polyline crossing one edge 2001 times → Err containing "MAX_SLEEVE" */ }
```

- [ ] **Step 2: Run** `cargo test --release -p topor-engine --test cdt_crossings` → FAIL.
- [ ] **Step 3: Implement `cdt.rs`**; vertex data = input index; `crossings` walks each polyline segment with spade's line intersection iterator, converting `EdgeIntersection(e)` into `(from, to)` + side by `cross(dir, to - from)`; `t` from `seg_seg` intersection.
- [ ] **Step 4: Run** → PASS; clippy clean; `cargo build --release --features python` still builds (spade is pure Rust).
- [ ] **Step 5: Commit** `feat(engine): constrained Delaunay triangulation and crossing sequences via spade (T1.2b)`.

---

### Task 4 (F design note → S impl): Funnel and sketch — `engine/src/sketch.rs`

**Files:**
- Create: `docs/progress/notes/m1-sketch-design.md` (F, ≤ 1 page), `engine/src/sketch.rs`, `engine/tests/sketch_funnel.rs`
- Modify: `engine/src/lib.rs`

**Interfaces:**
- Consumes: `Cdt`, `Crossing`, `ObstacleSet`/`LayerObstacles`, `Wire`, `Terminal`, `Orient`.
- Produces:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnchorRef { pub anchor: usize /* index into LayerObstacles.anchors; terminals are Terminal anchors */, pub sigma: Orient }
pub struct Sketch { pub wire: usize, pub seq: Vec<AnchorRef> /* interior anchors in path order */, pub crossings: Vec<Crossing> /* reduced */ }
/// Funnel (11 §2) over portals (left, right) from start to goal; returns interior vertices with σ
/// (left turn → obstacle on the left → Ccw; right turn → Cw). O(k) amortised.
pub fn funnel(start: Vec2, goal: Vec2, portals: &[(Vec2, Vec2)]) -> Vec<(usize /* portal index */, bool /* true = left vertex */, Orient)>;
/// Per layer: CDT points = anchor centres ∪ wire terminal points (dedup); constraints = Edge walls.
pub struct LayerSketch { pub cdt: Cdt, pub point_to_anchor: Vec<Option<usize>> }
pub fn build_layer_cdt(obs: &LayerObstacles, terminals: &[Vec2]) -> Result<LayerSketch, String>;
pub fn sketch_wire(wire: &Wire, ls: &LayerSketch) -> Result<Sketch, String>;   // crossings → reduce → funnel → AnchorRefs
/// Stack order per (anchor, σ): wires sorted by the crossing position `t` (distance from the anchor along the first
/// sleeve edge incident to it), ties by wire id. Wire W is a member candidate of (A, σ) iff its reduced crossings contain
/// an edge incident to A with A on side σ (left ⇒ Ccw, right ⇒ Cw).
pub fn stack_order(sketches: &[Sketch], ls: &LayerSketch) -> HashMap<AnchorRef, Vec<usize>>;
```

- [ ] **Step 1 (F): Write `docs/progress/notes/m1-sketch-design.md`**: the portal convention, the σ rule, how a hand path touching a terminal vertex is handled (allowed at ends only), the crossing-`t` stack-order rule and its known weakness (two wires entering a fan on different edges), what the swept-region test in Task 5 relies on. ≤ 1 page, formulas by section reference only (11 §2, 21 §5).
- [ ] **Step 2: Write the failing tests** in `engine/tests/sketch_funnel.rs`

```rust
#[test] fn funnel_straight_when_portal_is_wide() { /* start (0,0), goal (10,2) mm, portal ((5,3),(5,0.5)) → empty */ }
#[test] fn funnel_bends_right_around_right_vertex() { /* portal ((5,3),(5,1.5)) → [(0, false, Cw)] */ }
#[test] fn funnel_bends_left_around_left_vertex() { /* goal (10,-2), portal ((5,-1.5),(5,-3)) → [(0, true, Ccw)] */ }
#[test] fn funnel_two_bends_zigzag() { /* portals ((4,1.0),(4,-5)), ((8,5),(8,-1.0)), goal (12,0): [(0,true,Ccw),(1,false,Cw)] */ }
#[test] fn sketch_wire_around_one_pad() { /* obstacles: circle pad net 2 at (5,0.5) mm; wire net 3 from (0,0) to (10,0) with hand [(0,0),(5,-1),(10,0)]: seq == [AnchorRef{anchor: that pad, sigma: Ccw}] */ }
#[test] fn sketch_wire_hand_on_other_side_gives_cw() { /* hand [(0,0),(5,2),(10,0)] → Cw */ }
#[test] fn stack_order_inner_first() { /* two wires both passing below the pad, hand y = -1 (wire 0) and y = -2 (wire 1): stack_order[(pad,Ccw)] == [0, 1]; swapping hand paths swaps the order */ }
#[test] fn sketch_is_deterministic() { /* same inputs twice → equal seq */ }
```

- [ ] **Step 3: Run** `cargo test --release -p topor-engine --test sketch_funnel` → FAIL.
- [ ] **Step 4 (S): Implement `sketch.rs`** per the note and Interfaces (simple stupid funnel with portal pairs; `build_layer_cdt` dedups terminal points against anchor centres via `Cdt::build`).
- [ ] **Step 5: Run** → PASS; clippy clean.
- [ ] **Step 6: Commit** `feat(engine): funnel sketch with sides and stack order from crossings (T1.2c)`.

---

### Task 5 (F design note → S impl): Single-wire taut chain — `engine/src/taut.rs`

**Files:**
- Create: `docs/progress/notes/m1-taut-design.md` (F, ≤ 1 page; also covers Task 6), `engine/src/taut.rs`, `engine/tests/taut_chain.rs`
- Modify: `engine/src/lib.rs`

**Interfaces:**
- Consumes: `bitangent(c1, s1, c2, s2) -> Result<Seg, GeomError>` (a terminal is `Circle { r: 0.0 }`), `Arc::from_tangent_points(c, r, sigma, t_in, t_out)`, `dist_prim`, `point_seg`, `ObstacleSet` queries + `rho_*`, `AnchorRef`, `Grid`.
- Produces:
```rust
pub const TAUT_PASS_CAP: u32 = 64;
pub const SLACK_BAND_NM: f64 = 10_000.0;   // 25 §2 ε-band: drop only when slack exceeds it
#[derive(Clone, Debug, PartialEq)]
pub enum TautError { InsideObstacle(String), Infeasible(String), Cap, Sleeve(String) }
pub struct Chain { pub prims: Vec<Prim> /* alternating Seg/Arc, G1 */, pub seq: Vec<AnchorRef>, pub length: f64 }
impl Chain { pub fn pattern(&self) -> String /* "SAS…" */ }
/// Dynamic obstacle = a primitive of another wire's current chain.
pub struct DynObstacle { pub prim: Prim, pub width: f64, pub net: i32, pub wire: usize,
                         pub anchor: Option<AnchorRef> /* arcs */, pub adjacent: [Option<AnchorRef>; 2] /* segs: anchors at both ends */ }
pub trait Env {
    fn layer(&self) -> &LayerObstacles;
    fn rho_anchor(&self, a: usize) -> f64;                 // this wire's inflation incl. stack radius (Task 6)
    fn rho_wall(&self, w: usize) -> f64;
    fn exempt_anchor(&self, a: usize) -> bool; fn exempt_wall(&self, w: usize) -> bool;
    fn dyn_near(&self, bb: Aabb) -> Vec<&DynObstacle>;      // other wires' prims, same-net excluded
    fn dyn_clearance(&self, d: &DynObstacle) -> f64;         // c(j,k) + ε + w_j/2 + w_k/2 (centreline requirement)
    fn stack_radius(&self, a: AnchorRef) -> f64;             // radius for this wire at (anchor, σ), ≥ rho_anchor
}
/// Geometry of a fixed anchor sequence (21 §2–3); Err(Infeasible) when a needed inner tangent does not exist,
/// Err(InsideObstacle) when start/goal lies inside an inflated non-exempt obstacle.
pub fn chain_geometry(start: Vec2, goal: Vec2, seq: &[AnchorRef], env: &dyn Env) -> Result<Chain, TautError>;
/// Fix-up loop (21 §4, 11 §4, 25 §2 step 3): repeat ≤ TAUT_PASS_CAP: (a) drop slack anchors — the neighbour bitangent keeps the
/// anchor centre on side σ at distance ≥ R + SLACK_BAND and the centre triangle (p, a, q) contains no non-exempt core and the
/// shortcut violates nothing; (b) add anchors for every primitive that penetrates a non-member obstacle (static anchor → that
/// anchor; wall interior → its nearer endpoint anchor; dyn arc → its anchor; dyn seg → its adjacent anchors), with
/// σ = sign(cross(t, C − P)) (P closest point on the prim, t its tangent there). Stops when a pass changes nothing.
pub fn tauten(start: Vec2, goal: Vec2, seq: Vec<AnchorRef>, env: &dyn Env) -> Result<Chain, TautError>;
```

- [ ] **Step 1 (F): Write `docs/progress/notes/m1-taut-design.md`**: the slack test (incl. the centre-triangle approximation of the swept region and why the self-check + fallback bounds its risk), the add rule and σ rule, how dyn obstacles map to anchors (arc → its anchor; seg → the two adjacent anchors so the stack formula emerges), the ε-band, termination argument under the caps, and the relaxation loop of Task 6 (processing order, membership = wires whose chain currently has an arc at (A, σ) with sweep > 0, order fixed by `stack_order`). ≤ 1 page.
- [ ] **Step 2: Write the failing tests** in `engine/tests/taut_chain.rs` with a test `Env` (static obstacles from `ObstacleSet`, no dyn obstacles unless stated; wire net 3, width 250 000, clearance 200 000, margin 5 000)

```rust
#[test] fn straight_when_nothing_blocks() { /* start (0,0), goal (10,0) mm, pad at (5,3): tauten(seq=[]) → prims == [Seg], length ≈ 10_000_000 (abs 1) */ }
#[test] fn wraps_one_circle_with_exact_tangents() { /* circle pad d 800_000 (r_core 400_000) at (5,0.5) mm, seq [(pad, Ccw)]: pattern "SAS"; arc.c ≈ (5e6, 5e5), arc.r ≈ 730_000, tangent points ≈ (4_966_350, -229_220) and (5_033_650, -229_220) (abs 200 nm); length ≈ 10_010_580 (abs 200) */ }
#[test] fn slack_anchor_is_dropped() { /* same pad moved to (5,3): seq [(pad, Ccw)] → result seq empty, 1 Seg */ }
#[test] fn penetrating_obstacle_is_added() { /* pad at (5,0.5), seq [] (as if the funnel missed it): the add rule inserts (pad, Ccw) (centre above the +x travel direction) and the result equals the exact-tangent case */ }
#[test] fn wall_clip_wraps_endpoint() { /* Edge wall from (-5,-0.9) to (4,-0.9) mm, start (-3,0.5), goal (10,0.5), seq []: wall rho 630_000 reaches y=-0.27 → straight, 1 Seg. Wall moved to y=-0.1: the straight line violates (0.6 < 0.63) → result wraps the wall endpoint anchor at (4,-0.1) with Cw, pattern "SAS", length > 13_000_000 */ }
#[test] fn infeasible_gap_is_error() { /* two circle pads r_core 400_000 at (5,0.55) and (5,-0.55) (gap 0.3 mm < 0.25+0.4): seq [(top, Ccw),(bottom, Cw)] → Err(Infeasible) */ }
#[test] fn tauten_terminal_inside_foreign_obstacle_is_error() { /* pad net 2 d 800_000 at (0.3, 0): start (0,0) → Err(InsideObstacle(label contains "pad")) */ }
#[test] fn exempt_anchor_is_never_kept() { /* same-net pad at (5,0.5), seq [(pad, Ccw)] → straight, seq empty */ }
#[test] fn dyn_arc_adds_its_anchor_with_stack_radius() { /* dyn obstacle: another wire's arc at the pad (5,0.5) r 730_000 w 250_000 net 2; env.stack_radius((pad,Ccw)) = 1_185_000 (= 0.73 + 0.125 + 0.205 + 0.125); seq [] → result wraps pad Ccw at r ≈ 1_185_000 */ }
#[test] fn pass_cap_returns_err_cap() { /* Env whose rho alternates each query (adversarial) → Err(Cap) within TAUT_PASS_CAP passes, no hang */ }
#[test] fn chain_is_g1_and_alternating() { /* exact-tangent case: consecutive prims share endpoints (abs 1 nm) and tangent directions (angle < 1e-6 rad); pattern has no "SS"/"AA" */ }
```

- [ ] **Step 3: Run** `cargo test --release -p topor-engine --test taut_chain` → FAIL.
- [ ] **Step 4 (S): Implement `taut.rs`** per the note; `chain_geometry` is 21 §2–3 verbatim (`bitangent` per consecutive pair with their σ, `Arc::from_tangent_points` between the incoming and outgoing tangent points, zero-sweep arcs omitted from `prims` but kept in `seq` until the drop test removes them).
- [ ] **Step 5: Run** → PASS; clippy clean.
- [ ] **Step 6: Commit** `feat(engine): taut tangent-arc chain with add/drop fix-ups (T1.3a)`.

---

### Task 6 (S, from the F note of Task 5): Stacks and relaxation — `engine/src/relax.rs`

**Files:**
- Create: `engine/src/relax.rs`, `engine/tests/relax_stacks.rs`
- Modify: `engine/src/lib.rs`

**Interfaces:**
- Consumes: `Wire`, `Sketch`, `stack_order`, `tauten`, `Env`, `DynObstacle`, `ObstacleSet`, `Grid`, `Options.relax_iter_cap`, `std::time::Instant`.
- Produces:
```rust
pub struct Stacks { pub order: HashMap<AnchorRef, Vec<usize>>, members: HashMap<AnchorRef, BTreeSet<usize>> }
impl Stacks {
    pub fn new(order: HashMap<AnchorRef, Vec<usize>>) -> Stacks;
    /// 21 §5 with members only (wires inside `wire` in `order` that are current members); base = obs.rho_anchor(...).
    pub fn radius(&self, a: AnchorRef, wire: usize, wires: &[Wire], obs: &ObstacleSet) -> f64;
    pub fn set_members(&mut self, wire: usize, arcs: &[AnchorRef]);   // replaces this wire's memberships
}
pub struct RelaxResult { pub chains: Vec<Result<Chain, TautError>>, pub iterations: u32, pub converged: bool, pub timed_out: bool }
/// Gauss–Seidel: each iteration processes wires in order (width desc, net, id); each wire is tautened against static obstacles
/// and the current chains of all other wires (per-layer Grid of DynObstacle, updated per wire via remove/insert);
/// then its stack memberships are refreshed. Stops when an iteration changes no chain (prims equal within 1 nm),
/// at `relax_iter_cap`, or at `deadline` (timed_out). O(Σ prims) per iteration.
pub fn relax(wires: &[Wire], sketches: &[Sketch], obs: &ObstacleSet, opts: &Options, deadline: Instant) -> RelaxResult;
```

- [ ] **Step 1: Write the failing tests** in `engine/tests/relax_stacks.rs` (helper builds `ObstacleSet` + wires + sketches on layer 0; clearance 200 000, margin 5 000)

```rust
#[test] fn two_wires_same_side_get_stack_radii() { /* pad r_core 400_000 at (5,0.5) mm; wire 0 w 250_000 hand y=-1, wire 1 w 400_000 hand y=-2, both (0,y)→(10,y) terminals at their own y: arc radii ≈ 730_000 and 1_260_000 (abs 2); copper gap between the two chains (min over prim pairs of copper_gap) ≥ 205_000 - 1 */ }
#[test] fn radius_formula_with_mixed_clearance() { /* rules per_net: wire 1 clearance 300_000: R_1 = max(0.4+0.3+0.2+0.005, 0.73+0.125+0.305+0.2) = 1_360_000 */ }
#[test] fn inner_wire_leaving_stack_shrinks_outer() { /* after wire 0's hand path moves far away (slack), one relax run: wire 1 arc radius ≈ 730_000 + (0.4-0.25)/2 = 805_000 */ }
#[test] fn parallel_runs_keep_clearance() { /* two wires between two shared pads (both wrap pad A then pad B on the same side): min copper gap along the straight runs ≈ 205_000 (abs 1_000) */ }
#[test] fn converges_and_is_deterministic() { /* 6 wires around 3 pads: converged == true within relax_iter_cap; two runs equal; iterations ≤ 10 */ }
#[test] fn relax_iter_cap_respected() { /* relax_iter_cap = 1 → iterations == 1, converged false allowed */ }
#[test] fn deadline_sets_timed_out() { /* deadline = now → timed_out == true, returns without panic in < 50 ms */ }
#[test] fn failed_wire_does_not_block_others() { /* one wire Infeasible, the other taut: chains[0].is_err(), chains[1].is_ok() */ }
```

- [ ] **Step 2: Run** `cargo test --release -p topor-engine --test relax_stacks` → FAIL.
- [ ] **Step 3: Implement `relax.rs`** per the Task 5 note (the `Env` impl for one wire borrows `Stacks`, the dyn grid and the wire's net/width).
- [ ] **Step 4: Run** → PASS; clippy clean.
- [ ] **Step 5: Commit** `feat(engine): stack radii and Gauss–Seidel relaxation with caps (T1.3b)`.

---

### Task 7 (S): Harness integration — smoother router, item removal, bench, progress PNG

**Files:**
- Create: `harness/smoother.py`, `harness/progress.py`, `tests/test_smoother.py`
- Modify: `harness/writer.py` (add `remove_items`), `harness/bench.py` (`Router` result normalisation, removal, summary, `--router` flag), `docs/benchmarks/README.md` (progress folder note)

**Interfaces:**
- Consumes: `topor_engine.smooth(model_json, options_json) -> str` (Task 8; tests here use a fake router), `run_bench`, `writer.write_items/save`, `render.{render_png,caption,compose_compare}`, `copper_metrics`, `BENCH_BOARDS`.
- Produces:
```python
# harness/smoother.py
def smooth_router(model: dict, options: dict) -> dict:
    """{"items": [...], "remove_uuids": [...], "report": <engine report dict>}; forces existing_copper="smooth"."""
# harness/writer.py
def remove_items(board, uuids: list[str]) -> int:   # board.Delete (never Remove, M0 ruling); returns the count removed
# harness/bench.py
Router = Callable[[dict, dict], "list[dict] | dict"]
# run_bench(...) result gains: "replaced": int, "router_report": dict | None, "smooth": {"nets_taut": n, "nets": N, "wires_taut": w, "wires": W, "length_mm": x, "hand_mm": y, "iterations": i, "timed_out": b} | None
# summary: extra line "smooth: taut <n>/<N> nets, <w>/<W> wires, <length> mm (hand <y>), iters <i>"; argparse --router {none,smoother}
# harness/progress.py
def render_progress(task: str, *, board: str = "eprom-to-kernal", date: str | None = None, out_dir: Path = ROOT / "docs/benchmarks/progress") -> dict:
    """run_bench(board, tmp, router=smooth_router, rip="none"); compare PNG with caption
    f"{task}: {nets_taut}/{nets} converted, DRC routing {routing_new}, {length_mm:.1f} mm vs {hand_mm:.1f} mm" → out_dir/<date>-m1-<task>.png;
    prints the path and the caption line; returns the bench result."""
# CLI: .venv/bin/python -m harness.progress --task T1.4a [--board eprom-to-kernal]
```

- [ ] **Step 1: Write the failing tests** in `tests/test_smoother.py` (fixture board; `fake_router` returns 3 items for net SIG plus `remove_uuids` of one existing fixture track)

```python
def test_remove_items_by_uuid(fixture_board):            # write 3 items, save, reload, remove 2 → returns 2; track count −2; unknown uuid ignored
def test_run_bench_accepts_dict_router(fixture_board, tmp_path):  # run_bench(..., router=fake_router, rip="none", pcb_override=fixture pcb): result["replaced"] == 1, routed_items == 3, summary contains "smooth:" only when report present
def test_run_bench_list_router_still_works(fixture_board, tmp_path):  # M0 behaviour unchanged: list result, replaced == 0
def test_smooth_router_forces_smooth_option(monkeypatch):  # monkeypatch topor_engine.smooth to capture options → existing_copper == "smooth"; returned dict keys
def test_progress_png_caption_and_path(fixture_board, tmp_path, monkeypatch):  # render_progress("T1.4a", board="fixture", date="2026-10-30", out_dir=tmp_path) with a fake router → file tmp_path/"2026-10-30-m1-T1.4a.png" exists; size == (2*(w+40), h+40) of the hand render; caption text asserted via the returned dict ["caption"]
```

- [ ] **Step 2: Run** `.venv/bin/python -m pytest tests/test_smoother.py -v` → FAIL.
- [ ] **Step 3: Implement** the four files per Interfaces (`run_bench`: `pcb_override` already allows fixtures; add `"fixture"` handling only through `pcb_override`).
- [ ] **Step 4: Run** → PASS; `pytest` default suite still green.
- [ ] **Step 5: Commit** `feat(harness): smoother router hook, item removal, bench summary and progress PNG (T1.4b)`.

---

### Task 8 (S): `smooth()` orchestration, self-check, fallback, PyO3 surface — `engine/src/smooth.rs`

**Files:**
- Create: `engine/src/smooth.rs`, `engine/tests/smooth_engine.rs`
- Modify: `engine/src/lib.rs`, `engine/src/py.rs` (+ `smooth`), `tests/test_engine_surface.py` (+ surface test)

**Interfaces:**
- Consumes: `extract_wires`, `ObstacleSet::build`, `build_layer_cdt`, `sketch_wire`, `stack_order`, `relax`, `quantise_chain`, `flip_track`, `Checker::new/check_items`, `Item`, `TrackItem`.
- Produces:
```rust
#[derive(Serialize)] pub struct WireReport { pub id: usize, pub net: i32, pub layer: u32, pub width: i64, pub status: String /* taut|fallback */, pub reason: String, pub pattern: String, pub anchors: usize, pub length_nm: i64, pub hand_length_nm: i64 }
#[derive(Serialize)] pub struct NetReport { pub net: i32, pub name: String, pub status: String /* taut|partial|fallback|skipped */, pub wires: usize, pub taut: usize, pub length_nm: i64, pub hand_length_nm: i64 }
#[derive(Serialize)] pub struct SmoothResult { pub items: Vec<Item> /* KiCad frame */, pub replaced: Vec<String>, pub nets: Vec<NetReport>, pub wires: Vec<WireReport>,
    pub wires_total: usize, pub wires_taut: usize, pub nets_total: usize, pub nets_taut: usize, pub total_length_nm: i64, pub hand_length_nm: i64,
    pub iterations: u32, pub converged: bool, pub timed_out: bool, pub self_check_rounds: u32, pub runtime_s: f64, pub threads_used: usize /* 1 */ }
pub fn smooth(model: &BoardModel, opts: &Options) -> SmoothResult;
// py.rs: #[pyfunction] fn smooth(py, model_json: String, options_json: String) -> PyResult<String>  (JSON of SmoothResult)
```
  Pipeline: extract → ObstacleSet (exclude all extracted wires' kept uuids; terminals of Junction/Dangling kind as `TerminalObstacle`) → per layer CDT + sketches (a wire whose sketch fails → `fallback`, reason) → `relax` with deadline `now + wall_clock_s` → quantise each Ok chain (`quantise_chain`, flip to KiCad frame, width = wire width, net, layer; group `None`) → self-check round: `Checker::new(model with kept minus the uuids of all currently-taut wires, opts)` + `check_items(all items)`; every item with a violation marks its wire `fallback` (reason `"self-check: <against>"`); repeat the round with the shrunk taut set until no new failures (bounded by wire count) → `replaced` = kept uuids of taut wires. `skip_self_check` emits everything (debug). Net status from its wires; nets without candidate wires are `skipped`. `total_length_nm` = Σ emitted prim lengths; `hand_length_nm` = Σ hand lengths of all extracted wires. Deterministic: no HashMap iteration order reaches the output (sort by id).

- [ ] **Step 1: Write the failing tests** in `engine/tests/smooth_engine.rs` (inline JSON models in the KiCad frame as the harness emits them; `options {"existing_copper":"smooth","margin_nm":5000}`)

```rust
#[test] fn smooth_l_route_around_pad_becomes_arc_chain() { /* P1 net 5 (0,0), P2 net 5 (10,0), foreign pad net 2 d 800_000 at (5,0.5) mm, hand L below it: items pattern "SAS", wires_taut 1, replaced == hand uuids, total_length_nm < hand_length_nm, status taut */ }
#[test] fn smooth_output_items_pass_checker() { /* same: Checker on (model minus replaced) reports 0 violations for the items */ }
#[test] fn smooth_dirty_hand_wire_falls_back() { /* hand path 0.1 mm from the foreign pad, start inside its inflated circle: wire status "fallback", reason starts with "infeasible" or "inside", items empty, replaced empty, net status "fallback" */ }
#[test] fn smooth_dangling_end_stays_fixed() { /* seg (0,0)→(5,5) with P1 only: emitted chain's last point == (5_000_000, 5_000_000) exactly (KiCad frame) */ }
#[test] fn smooth_junction_endpoints_bit_identical() { /* T on a vertex: the three chains touching the junction end at the identical IVec2 */ }
#[test] fn smooth_fixed_copper_is_skipped() { /* existing_copper fixed (default) → items empty, every net "skipped" */ }
#[test] fn smooth_zero_wall_clock_times_out() { /* wall_clock_s 0.0 → timed_out true, items empty or only self-check-clean wires, runtime_s < 0.5 */ }
#[test] fn smooth_is_deterministic() { /* serde_json of two runs equal */ }
#[test] fn smooth_self_check_fallback_restores_obstacle_for_others() { /* wire A fails the check (forced by an obstacle only the checker knows: a keepout no_tracks over its chain); wire B's chain passed near A's hand copper: after the second round B is re-checked against A's restored hand items → self_check_rounds == 2 */ }
```

```python
# tests/test_engine_surface.py
def test_smooth_surface_round_trip(engine):   # engine.smooth(json.dumps(_model_with_track()), json.dumps({"existing_copper": "smooth"})) parses; keys items, replaced, nets, wires, runtime_s, threads_used == 1
def test_smooth_rejects_bad_json(engine):     # ValueError on "{" 
```

- [ ] **Step 2: Run** `cargo test --release -p topor-engine --test smooth_engine` → FAIL; `.venv/bin/python -m pytest tests/test_engine_surface.py -k smooth` → FAIL.
- [ ] **Step 3: Implement `smooth.rs`** and the `py.rs` function per Interfaces; register in `topor_engine`.
- [ ] **Step 4: Run** both → PASS; clippy clean; `bash tools/build.sh --dev`; `pytest` default suite green.
- [ ] **Step 5: First eprom result** — `.venv/bin/python -m harness.progress --task T1.4a` (Task 7 tool). Expected: a line `T1.4a: <n>/26 converted, DRC routing <v>, <L> mm vs <hand> mm` and `docs/benchmarks/progress/2026-10-30-m1-T1.4a.png`. Any `n < 26`, `v > 0` or `L > hand` is recorded in STATE.md as the starting point for Task 9 — not a failure of this task.
- [ ] **Step 6: Commit** `feat(engine): smooth() with self-check and per-wire fallback; progress PNG T1.4a` (includes the PNG). Coordinator sends the PNG path + the caption line to main.

---

### Task 9 (S, gate review F): M1 gate, benchmark run 0.1.0, images

**Files:**
- Create: `tests/gate/test_m1_gate.py`, `docs/benchmarks/2026-10-30-topor-0.1.0/README.md`, `.../images/eprom-to-kernal-{hand,topor,compare}.png`, `.../metrics/eprom-to-kernal--{metrics,drc-orig,drc-routed}.json` (+ wiccon/dcsgone informational runs)
- Modify: `engine/Cargo.toml` (version `0.1.0`), `docs/benchmarks/README.md` (runs table row), `docs/progress/STATE.md` (coordinator)

**Interfaces:**
- Consumes: `run_bench(board, out_dir, router=smooth_router, rip="none", options={"existing_copper": "smooth"})`, `BENCH_BOARDS`, `sha256_tree/assert_unchanged`, `png_size` (from `tests/gate/test_m0_gate.py`), `engine.smooth`.

- [ ] **Step 1: Write the gate test** `tests/gate/test_m1_gate.py`

```python
# Edit-proof: derive from a copy of the board in the same run (harness.measure.copper_facts), pin nothing absolute.
EPROM = None  # {segments, hand_mm, wires} measured at gate time

@pytest.mark.gate
def test_m1_gate_eprom_taut_smoother(engine, tmp_path):
    src = BENCH_BOARDS["eprom-to-kernal"]; hashes = sha256_tree(src.parent)
    r = run_bench("eprom-to-kernal", tmp_path, router=smooth_router, rip="none", options={"existing_copper": "smooth"})
    s, rep = r["smooth"], r["router_report"]
    facts = measured_hand_facts()  # copper_facts on a copy + engine wire count, same run
    assert s["nets_taut"] == s["nets"] and s["wires_taut"] == s["wires"] == facts["wires"]
    assert r["routed_drc"]["routing_new"] == 0, r["routed_drc"]
    length = r["routed_all"]["track_mm"] + r["routed_all"]["arc_mm"]
    assert length <= facts["track_mm"] and abs(s["length_mm"] - length) < 0.05
    assert r["routed_all"]["arcs"] >= 1 and all("SS" not in w["pattern"] for w in rep["wires"])
    assert r["width_match"]["compared"] == r["width_match"]["matched"] > 0
    assert r["replaced"] == facts["segments"] + facts["arcs"] and r["removed"]["removed_tracks"] == 0
    assert r["router_s"] < 2.0 and rep["threads_used"] == 1 and not rep["timed_out"]
    model_json = (tmp_path / "eprom-to-kernal--model.json").read_text(); o = json.dumps({"existing_copper": "smooth"})
    assert engine.smooth(model_json, o) == engine.smooth(model_json, o)
    assert png_size(tmp_path / "eprom-to-kernal-compare.png")[0] > 0
    assert_unchanged(src.parent, hashes)
    print(f"GATE M1 eprom: taut {s['nets_taut']}/{s['nets']} wires {s['wires_taut']}/{s['wires']} len {length:.3f} (hand {facts['track_mm']}) "
          f"drc_new {r['routed_drc']['routing_new']} arcs {r['routed_all']['arcs']} router_s {r['router_s']} iters {rep['iterations']}")

@pytest.mark.gate
@pytest.mark.parametrize("board", ["wiccon-2023", "dcsgonebadge"])
def test_m1_smoother_other_boards_complete(board, tmp_path):
    """Informational (not an M1 criterion): the smoother must complete; fallback keeps the board DRC-equal."""
    r = run_bench(board, tmp_path, router=smooth_router, rip="none", options={"existing_copper": "smooth", "wall_clock_s": 30})
    print(f"GATE-INFO {board}: {r['smooth']} drc_new={r['routed_drc']['routing_new']} router_s={r['router_s']}")
    assert r["routed_drc"]["routing_new"] == 0
```

- [ ] **Step 2: Run** `.venv/bin/python -m pytest -m gate tests/gate/test_m1_gate.py -v -s` → expected PASS; if any assertion fails: one F diagnosis note (`docs/progress/notes/m1-gate-diagnosis.md`) → fix task(s) each ending with a progress PNG → re-run; a second failure of the same budget escalates to the user (45 §5a).
- [ ] **Step 3: Version 0.1.0** — set `engine/Cargo.toml` version to `0.1.0`, `bash tools/build.sh --dev`, `pytest tests/test_scaffold.py tests/test_engine_surface.py` green (version test).
- [ ] **Step 4: Benchmark run** — `bash tools/bench.sh eprom-to-kernal scratch/bench-0.1.0/eprom --rip none --router smoother` and the same for `wiccon-2023`, `dcsgonebadge`; copy `*-hand.png`, `*-topor.png`, `*-compare.png` into `docs/benchmarks/2026-10-30-topor-0.1.0/images/` and the `--metrics.json`/`--drc-*.json` into `metrics/`; write `README.md` in the 0.0.1 format: metrics table (nets taut, wires, segs/arcs, length vs the measured hand length, DRC routing_new, runtime/router_s), the eprom compare image embedded, observations (edge-clearance pads pre-existing: 12; `track_dangling` 1 from the `+5V` stub kept), raw data link; add the run row to `docs/benchmarks/README.md`.
- [ ] **Step 5: Progress PNG** — `.venv/bin/python -m harness.progress --task T1.4-gate` → `docs/benchmarks/progress/2026-10-30-m1-T1.4-gate.png`; coordinator sends path + numbers to main.
- [ ] **Step 6: Full verification** — `cargo test --release`, `cargo clippy --all-targets -- -D warnings`, `.venv/bin/python -m pytest` (default) and `-m "gate or scaling"`; all green; record the numbers in STATE.md.
- [ ] **Step 7: Commit** `feat: M1 taut smoother gate and benchmark 0.1.0 (eprom hand vs topor)`; then F gate review (complexity of the new loops: every loop cap named above present; no all-pairs; stack/relax O(k) per wire per iteration) → note `docs/progress/notes/m1-gate-review.md`; conditions fixed before handover.

---

## Deferred (first tasks of M2, recorded for STATE.md)

1. **F-C2** (M0 review): the `Checker` must model existing vias' drill holes and new vias' holes (`hole_to_hole`, track vs via hole) — not needed for M1 (no vias emitted; eprom has none; the `ObstacleSet` already adds via-hole anchors for tauten). First M2 task.
2. Pad hull edges and locked tracks as CDT constraint edges (10 §2) — M1 uses outline constraints only; the fix-up loop + self-check cover the rest.
3. Steiner/junction relocation (25 §3), spreading (25 §4), pad exit stubs (20 §4), curved funnel with lazy regions (11 §5) if profiling demands, multithreading.

## Self-review notes

- Spec coverage: T1.1 → Task 1; T1.2 → Tasks 2–4; T1.3 → Tasks 5–6; T1.4 → Tasks 7–9; 44 Must gaps (copper graphics, net ties, per-layer padstacks, local overrides) → Task 1 tests; 12 §3.1 order → wider-first processing in Task 6; 23 §4 quantisation → Task 8 via `quantise_chain`; benchmark images → Task 9 + progress PNGs.
- Wire count (latest 2026-10-30: 27 wires, 0 skipped, hand length 121.463 mm measured pad-centre to pad-centre vs 112.886 mm of copper; always re-measured at gate time by `harness.measure`, which runs `engine/examples/extract_wires.rs` on the model JSON of a copy) is not pinned; the replaced items are the hand segments/arcs (vias are terminals, never replaced).
- Known risk for the length criterion: the hand routing is DRC-clean against the board-setup edge clearance, so the smoother has no forced inward moves; if `length ≤ measured hand track length` fails while everything else passes, that is a user ruling, not a budget relaxation.
