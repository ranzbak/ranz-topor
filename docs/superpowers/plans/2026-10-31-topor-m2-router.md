# Topor M2 Single-Layer Topological Router Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Execution method (user, 45 §4, as in M1):** rotating coordinator (O) dispatching one worker per task at the tier in the task title (H/S/F); independent tasks in parallel in isolated worktrees; F only for the two design/diagnosis notes, failed-gate diagnosis and the gate review. Every worker report is folded into `docs/progress/STATE.md` (≤ 5 lines); SDD ledger `.superpowers/sdd/2026-10-31-topor-m2-router/progress.md`; handover at the M2 gate or at ~50 % context / ~15 round-trips.

**Goal:** Route the ripped eprom-to-kernal board (all signal/+5V tracks removed; pads, GND pour, 17 free GND stitching vias and zones kept) on F.Cu with topor's own topological router: 25/25 non-pour nets connected, 0 unconnected after zone refill, 0 routing DRC, netclass widths (power 0.4 mm, Signal 0.25 mm), native arcs, length outside pad copper ≤ hand (76.598 mm measured 2026-10-30), deterministic, < 2 s engine time, inside the complexity budget, `partial_routes` and `scope` options honoured.

**Architecture:** The engine grows five modules on top of M1: `graph` (per-layer routing graph on the CDT: faces, edges with cut capacity, ordered crossing slots — 10 §3–4), `decompose` (same-net copper clusters incl. pours → MST connections → scope filter → order; 12 §1, §3.1), `search` (planar slot A* per connection with a Lazy-Theta*/Polyanya length estimate and a slot-free "ghost" search for blocked connections; 12 §6), `negotiate` (PathFinder iterations with history costs and priority bumps, clinch detection; 12 §3, §5) and `route` (orchestration: hand-off of the found sketches to the M1 metrization `relax`, quantisation, `Checker` self-check with per-connection feedback, partial stubs, report). Single layer: wires never cross, so crossings are excluded by construction in the slot search (double-crossing removal is therefore not needed until M3 turns crossings into vias). Two carry-overs come first: via holes as `Checker` obstacles (F-C2) and robust CDT constraint insertion (split/merge) that currently fails every wire on wiccon-2023 and wedding-gift. One new PyO3 function `route(model_json, options_json) -> str` plus `wire_stats(model_json) -> str`; the harness gets `router.py`, `bench --router topor` with a DRC loop based on "DRC spots", `progress` for M2, and the M2 gate.

**Tech Stack:** as M1 (Rust 1.96, `spade = "2.15.1"`, pyo3 0.23 abi3-py39, maturin in `.venv`, Python 3.12 + pcbnew SWIG, KiCad 10.0.8 `kicad-cli`, rsvg-convert, ImageMagick). No new crates (no rayon: M2 is single-threaded, `threads_used: 1`; parallel rounds are an M3 item).

**Spec:** `docs/design/45-mvp-plan.md` (§1 M2 gate, §3 T2.1–T2.4, §4–§5a); M0 plan outline `docs/superpowers/plans/2026-10-30-topor-m0-harness.md` (§ "M2 — Single-layer router", Options/Scope); M1 plan `2026-10-30-topor-m1-smoother.md` (Global Constraints, rulings); topology `docs/topology/10` (CDT, cut capacity), `11` (funnel), `12` (§1, §3/3.1, §5, §6); gaps `docs/design/44 §3.1 Must`; DRC `docs/kicad/30 §4.1`; state `docs/progress/STATE.md`, `docs/progress/notes/m1-gate-review.md`.

## Global Constraints

All M0/M1 Global Constraints still apply (Linux, `.venv` only, read-only source boards with sha256 checks, no KiCad types in the engine, KiCad-frame integer nm on the wire with one Y flip each way, `EPS_BUILD_NM` 5 000, degenerate-arc rule, fixed DRC/render commands, JLCPCB profile precedence, compact ≤ 20-line summaries, conventional commits). In addition for M2:

- **Rulings carried from M1 (user, 2026-10-30):** length gate = **length outside pad copper** (`length_outside_pads_nm ≤ hand_outside_pads_nm`, both measured on the same copy in the same run; hand side from the unripped board through `wire_stats`); manufacturer minimums are **floors** (`apply_fab_floors=True`: eprom track width 0.15 → 0.2 mm, hole clearance 0.25 → 0.35 mm, edge clearance 0.25 mm from board setup); zero-slack fallback does not apply to `route()` (no hand copper is replaced — a connection is `routed`, `partial` or `failed`); **progress PNGs** per task that changes the eprom outcome (see below).
- **M2 gate numbers (45 §1 M2, user 2026-10-31):** eprom-to-kernal from `rip="all"`: nets routed **25/25** (GND is pour-served and reported as such, not counted), connections routed = all (27 at the 2026-10-30 measurement = the M1 wire count; measured at gate time, not pinned), `routing_new == 0`, `unconnected_items == 0` after `--refill-zones`, every emitted net's median width equals the hand width (`width_match compared == matched == 25`), `arcs ≥ 1`, no `"SS"` pattern, `length_outside_pads_nm ≤ hand_outside_pads_nm` (hand 76.598 mm on 2026-10-30), `router_s < 2.0`, `threads_used == 1`, `timed_out == false`, `iterations ≤ 50`, `partial == 0`, two runs byte-identical (report `runtime_s` excluded).
- **Caps (45 §5a):** PathFinder iterations `Options.pathfinder_iter_cap` (default 50); A* expansions per search ≤ `8·(E + X) + 64` (E edges, X crossings on the layer); relax `relax_iter_cap` 200 / `TAUT_PASS_CAP` 64 / `MAX_SLEEVE` 2000 as in M1; self-check rounds ≤ connections + 1; KiCad DRC loop `drc_loop_cap` (default 5, harness); wall clock `Options.wall_clock_s` checked before every connection search, before every relax call and every self-check round — on timeout `route()` returns the best completed iteration (`timed_out: true`). No exponential/factorial stage, no routing-order permutations (bumps are a bounded priority adjustment), no all-pairs loops over board items (per-net cluster MST is O(ports²) per net, bounded by the net's own size).
- **Determinism:** no `HashMap` iteration reaches an output or an order; A* ties broken by `(f, h, edge, slot)`; connection order is a total order; same model + options ⇒ byte-identical JSON.
- **Capacity model (10 §4, conservative):** vertex radius `r_v` = anchor `r_core` (0 for terminals, split vertices, frame corners); vertex clearance `c_v` = Pad/Via/Track/Shape/Terminal `max(own_clr, min_clearance)`, Hole `hole_clearance`, Edge `edge_clearance`, Keepout/frame 0; edge `cap(e) = |pq| − r_p − r_q`, `c_e = max(c_p, c_q)`; a connection of width `w`, clearance `c` fits edge `e` iff `Σ_{i∈X(e)} (w_i + ε + max(c_i, c)) + w + ε + 2·max(c, c_e) ≤ cap(e)` (ε = `margin_nm`). Overuse `(need_all(e) − cap(e)).max(0)` with `need_all` = Σ widths + ε each + gaps between consecutive crossings `max(c_i, c_{i+1})` + end gaps `max(c_first, c_e)`, `max(c_last, c_e)`.
- **Constraint edges (10 §2):** `ConstraintSet::EdgeOnly` (M1 smoother, unchanged behaviour) or `ConstraintSet::Routing` = outline + pad anchor hulls + kept tracks/arcs (chords) + copper-shape polygons + track keepouts; inserted with `add_constraint_and_split` (spade ≥ 2.6), collinear duplicates merged. A constraint edge is passable only for its owner nets (pad: pad net + tie nets; kept item/shape: its net + tie nets; outline, keepout: none). Arc walls use the chord (as M1).
- **Terminals:** pad centre (`Terminal::Pad`), via centre (`Terminal::Via`), a kept item endpoint (`Terminal::Junction`, fixed) or a dangling hand end (`Terminal::Dangling`, fixed; G1 continuation deferred to M4 — reported as `g1_angle_deg` only). Shared endpoints rounded once (23 §4.4).
- **Widths and clearances:** per connection from `rules.net_rule(net)` (`width`, `clearance`); priority tier inferred: 2 if `width > rules.default.width`, else 3 (12 §3.1; plugin table is M4). Order key `(tier, −width, est_len, net, id)` with failure bumps first.
- **`existing_copper`:** `fixed` (default) and `rip` (harness ripped before export) behave identically in `route()`; `smooth` is not supported by `route()` and adds `warnings: ["existing_copper=smooth ignored by route(); treated as fixed"]`.
- **Progress PNG (user requirement):** every task whose completion changes the eprom routing outcome (first end-to-end run, partial routes, gate, any fix task) ends with `.venv/bin/python -m harness.progress --task <TaskId> --router topor` → `docs/benchmarks/progress/<YYYY-MM-DD>-m2-<TaskId>.png`, `git add` + commit, and the coordinator immediately sends the path plus the one-line numbers to the main session.
- **Engine version 0.2.0** at the gate (Cargo.toml, `plugin.json`, `SHIPPED_VERSION`); wheel rebuilt with `VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python` before every pytest run that uses the engine, and `bash tools/build.sh --dev` before the gate/benchmark.
- **M1 gate stays green:** `tests/gate/test_m1_gate.py` runs at the M2 gate (regression); `cargo clippy --all-targets --manifest-path engine/Cargo.toml -- -D warnings` clean.
- **Gate escalation (45 §5a):** budgets (length, < 2 s, caps) are hypotheses; one F diagnosis round, then stop and escalate to the user with measurements — never relax a budget silently.
- Commit messages end with the attribution line given by the executing session's system reminder.

## Review Focus

1. A connection whose pad has no copper on the routing layer (SMD pad on B.Cu — wiccon-2023 informational run): `route()` must report it `failed: "terminal not on layer F.Cu"` without searching, and nothing else on the board may be affected. → `route_pad_off_layer_fails_cleanly` (Task 8).
2. Two same-net pads that are adjacent CDT vertices (the common straight track): the slot path has zero crossings and must produce a direct `S` chain, not "no path". → `search_adjacent_terminals_zero_crossings` (Task 6).
3. A pad centre inside a foreign inflated circle (fine pitch): the search may succeed but `chain_geometry` returns `InsideObstacle`; the connection must fail with that reason and not block the remaining connections or loop in negotiation. → `route_terminal_inside_foreign_obstacle_fails_once` (Task 8).
4. Wall-clock exhaustion during the first PathFinder iteration: `route()` returns `timed_out: true`, every connection `failed: "timeout"`, no items, no hang. → `route_zero_wall_clock_times_out` (Task 8).
5. One channel with capacity for exactly one wire wanted by two nets: negotiation must route both (one detours), deterministically, within the iteration cap. → `negotiate_contended_channel_routes_both` (Task 7).

## File Structure

```
engine/
  Cargo.toml                  version 0.2.0 (Task 11); no new dependencies
  src/lib.rs                  + pub mod graph; pub mod decompose; pub mod search; pub mod negotiate; pub mod route;
  src/check.rs                via drill holes as Hole obstacles (Task 1)
  src/cdt.rs                  add_constraint_and_split build, topology() (Task 2, 3)
  src/sketch.rs               ConstraintSet, build_layer_cdt(obs, terminals, cons), sketch_from_crossings (Task 2, 8)
  src/obstacle.rs             Anchor.only_nets (DRC spots) (Task 9)
  src/model.rs                Pour / BoardModel.pours, Options.drc_spots, DrcSpot (Task 4, 9)
  src/graph.rs                RouteGraph: faces (inside/outside), edges, capacity, slots (Task 3)
  src/decompose.rs            clusters (union-find), connections (MST), scope, order (Task 4)
  src/search.rs               slot A* + ghost search (Task 5 note → Task 6)
  src/negotiate.rs            Negotiator: history, bumps, best iteration, clinch detection (Task 7)
  src/route.rs                route(): orchestration, metrization hand-off, self-check, partial stubs, RouteResult (Task 8, 10)
  src/py.rs                   + route(model_json, options_json), wire_stats(model_json)
  tests/check_via_holes.rs, cdt_constraints.rs, graph_capacity.rs, decompose_nets.rs, search_slots.rs,
        negotiate_loop.rs, route_engine.rs, route_scaling.rs
docs/progress/notes/m2-cdt-diagnosis.md     F diagnosis (Task 2, ≤ 1 page)
docs/progress/notes/m2-search-design.md     F design note (Task 5, ≤ 2 pages)
harness/
  model.py                    + _export_pours(board) → model["pours"] (Task 4)
  fixtures.py                 + vcc_stub, wall parameters (Task 4, 10)
  router.py                   route_router(model, options) -> dict; drc_spots(...) (Task 9)
  bench.py                    --router topor; rules + pre-rip model + hand stats; DRC loop with spots; summary (Task 9)
  progress.py                 --router topor, m2 file name and caption (Task 9)
tests/
  test_router.py              fixture-board tests of router.py / bench / progress / pours / partial (Task 4, 9, 10)
  scaling/synth.py            synth_model(n_pairs, seed) -> dict (model JSON without pcbnew) (Task 11)
  scaling/test_route_scaling.py   marked scaling (Task 11)
  gate/test_m2_gate.py        M2 gate (marked gate) (Task 11)
docs/benchmarks/<date>-topor-0.2.0/{README.md, images/, metrics/}   (Task 11)
docs/benchmarks/progress/     progress PNGs (Global Constraint)
```

Dependencies: Tasks 1, 2, 4, 5 are independent (parallel worktrees). 3 ← 2. 6 ← 3, 5. 7 ← 6. 8 ← 4, 7. 9 ← 8. 10 ← 9. 11 ← 10.

---

### Task 1 (S): F-C2 — via drill holes as `Checker` obstacles — `engine/src/check.rs`

**Files:**
- Modify: `engine/src/check.rs` (`Checker::new`, the `model.vias` loop near line 240)
- Test: `engine/tests/check_via_holes.rs`

**Interfaces:**
- Consumes: `Checker::new(&BoardModel, &Options)`, `check_items(&[Item]) -> CheckReport` (`Violation.against` labels), `model.vias[].drill`, `Rules.hole_clearance`, `Rules.hole_to_hole`.
- Produces: for every via with `drill > 0` an obstacle `Kind::Hole`, label `viahole:<uuid>`, net = via net, `own_clr 0`, on **all** copper layers (`model.all_layer_ids()`), geometry `zero_seg(centre, drill)`; tracks of other nets need `hole_clearance + margin`; new `ViaItem` drills need `hole_to_hole + margin` (the existing drill branch in `check_items` already handles `Kind::Hole`). Same-net exempt as pad PTH holes.

- [ ] **Step 1: Write the failing tests** (`engine/tests/check_via_holes.rs`; model JSON like `engine/tests/check_clearance.rs`: 2 layers 0/31, nets 1 `SIG`, 2 `GND`, rules `clearance 200_000`, `hole_clearance 500_000`, `hole_to_hole 250_000`, `min_clearance 0`, one via `uuid "v1"`, net 2, `at [5_000_000, 5_000_000]`, `d 600_000`, `drill 300_000`, layers `[0, 31]`, no pads, no outline)

```rust
#[test] fn track_near_via_copper_ok_but_inside_hole_clearance() {
    // SIG seg y = 5.65 mm, x 0..10 mm, w 0.25: copper gap 0.65-0.3-0.125 = 0.225 >= 0.2+0.005;
    // hole gap 0.65-0.15-0.125 = 0.375 < 0.5+0.005 -> exactly one violation, against "viahole:v1"
    let rep = checker().check_items(&[seg(1, 0, 250_000, [0, 5_650_000], [10_000_000, 5_650_000])]);
    assert_eq!(rep.violations.len(), 1);
    assert_eq!(rep.violations[0].against, "viahole:v1");
    assert_eq!(rep.violations[0].gap, 375_000);
    assert_eq!(rep.violations[0].required, 505_000);
}
#[test] fn same_net_track_over_via_hole_is_exempt() { /* net 2 seg through (5,5): 0 violations */ }
#[test] fn new_via_drill_vs_existing_via_hole_to_hole() {
    // ViaItem net 1 at (5.6, 5.0) d 0.6 drill 0.3: hole gap 0.6-0.15-0.15 = 0.3 >= 0.25+0.005 -> ok;
    // at (5.5, 5.0): gap 0.2 < 0.255 -> violation against "viahole:v1" (plus the copper one: net 1 vs 2)
    // assert the hole_to_hole violation has gap 200_000 and required 255_000
}
#[test] fn via_hole_is_checked_on_layer_without_via_copper() {
    // via layers [0] only (blind): SIG seg on layer 31 at y = 5.65 -> 1 violation against "viahole:v1"
}
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test check_via_holes` — Expected: FAIL (0 violations where 1 expected).
- [ ] **Step 3: Implement** in `Checker::new`: after adding each via's copper obstacle, when `v.drill > 0` call `c.add(&all, Obstacle { geo: Geo::Track(zero_seg(v.at.to_vec2(), v.drill as f64)), net: v.net, kind: Kind::Hole, label: format!("viahole:{}", v.uuid), own_clr: 0.0, tie_nets: Vec::new() })`.
- [ ] **Step 4: Run** the new suite and `cargo test --release --manifest-path engine/Cargo.toml` (all suites) — Expected: PASS; M1 `smooth_engine` unchanged (eprom has no vias near tracks within 0.35 mm — if `smooth_engine` fails, report, do not weaken).
- [ ] **Step 5: Commit:** `git commit -m "fix(check): via drill holes as hole obstacles on every layer (F-C2)"`.

### Task 2 (F diagnosis note → S impl): robust CDT constraints — `engine/src/cdt.rs`, `engine/src/sketch.rs`

**Files:**
- Create: `docs/progress/notes/m2-cdt-diagnosis.md` (F, ≤ 1 page), `engine/examples/cdt_probe.rs`
- Modify: `engine/src/cdt.rs` (`Cdt::build`), `engine/src/sketch.rs` (`build_layer_cdt`), `engine/src/smooth.rs` (call site)
- Test: `engine/tests/cdt_constraints.rs`, `engine/tests/sketch_funnel.rs` (existing, must stay green)

**Interfaces:**
- Consumes: spade 2.15.1 `ConstrainedDelaunayTriangulation::add_constraint_and_split(from, to, |pos| V{pos})`; `ObstacleSet` walls (`Wall.kind`, `Wall.ends`, `Wall.label`).
- Produces: `Cdt::build(points: &[Vec2], constraints: &[(usize, usize)]) -> Result<Cdt, String>` never fails on crossing or overlapping constraints (only on non-finite points); `Cdt::num_inputs() -> usize`; `Cdt::is_split_vertex(v: usize) -> bool` (canonical id created by splitting); `Cdt::constraint_edges() -> Vec<(usize, usize)>` (canonical ids, `a < b`, sorted); `pub enum ConstraintSet { EdgeOnly, Routing }`; `build_layer_cdt(obs: &LayerObstacles, terminals: &[Vec2], cons: ConstraintSet) -> Result<LayerSketch, String>` where `Routing` adds walls of kind `Pad`, `Track`, `Shape`, `Keepout` as constraints; `LayerSketch.point_to_anchor` has `None` for split vertices; new `LayerSketch.constraint_owner: HashMap<(usize, usize), (ObsKind, i32, Vec<i32>, String)>` keyed by canonical `(min, max)` vertex pair → `(kind, net, tie_nets, label)` of the wall, filled for every constraint piece (split pieces inherit the owner of the wall they came from; an edge shared by two walls keeps the first in wall order). `smooth()` passes `ConstraintSet::EdgeOnly`.

- [ ] **Step 1 (F): Diagnose.** Write `engine/examples/cdt_probe.rs` (arg: model JSON path; for every layer with anchors: build `ObstacleSet` (`Options::default()`, no exclusions) and call `build_layer_cdt`; on `Err` print the two constraint index pairs, their endpoint coordinates (mm), wall labels and kinds, and whether the pair is crossing, collinear-overlapping or touching at an interior point). Run it on wiccon-2023 and wedding-gift model JSONs produced by `python -m harness.bench <board> <dir> --rip none` (wedding-gift via `--pcb` on a scratch copy as in the 0.1.0 README). Write `docs/progress/notes/m2-cdt-diagnosis.md`: the offending pairs per board, the geometric cause (expected: footprint-contributed Edge.Cuts overlapping the board outline, or arc chords of a round outline crossing), and the fix rule below confirmed or amended. Commit note + example: `docs(notes): m2 CDT constraint diagnosis`.
- [ ] **Step 2 (S): Write the failing tests** (`engine/tests/cdt_constraints.rs`)

```rust
#[test] fn crossing_constraints_are_split() {
    // points (0,0),(10,0),(5,-5),(5,5) mm; constraints (0,1),(2,3) -> Ok; num_vertices 5;
    // split vertex at (5,0) within 1 nm; constraint_edges().len() == 4; is_split_vertex(4)
}
#[test] fn collinear_overlapping_constraints_do_not_fail() {
    // (0,0),(10,0),(4,0),(6,0): constraints (0,1),(2,3) -> Ok; constraint_edges() == {(0,2),(2,3),(3,1)} canonical
}
#[test] fn duplicate_constraint_is_idempotent() { /* (0,1) twice -> 1 constraint edge */ }
#[test] fn routing_constraints_include_pad_hulls_and_kept_tracks() {
    // ObstacleSet from a model with one rect pad 1x2 mm net 1 at (10,10), one kept seg net 2 (20,10)-(30,10) w 0.25,
    // outline 50x40: build_layer_cdt(.., Routing): every pad hull wall and the kept wall is a constraint
    // (is_constraint over their end anchors' CDT indices); constraint_owner[(a,b)].3 == "pad:U1:1" / "kept:<uuid>";
    // with EdgeOnly the same pairs are not constraints
}
#[test] fn layer_with_crossing_outline_builds() { /* two crossing outline segments: build_layer_cdt Ok */ }
```

- [ ] **Step 3: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test cdt_constraints` — Expected: FAIL (`Err("constraint ... crosses another constraint")`, missing symbols).
- [ ] **Step 4: Implement.** `Cdt::build`: insert all points (dedup as now), then for each constraint `(a, b)` with `ca != cb`: `tri.add_constraint_and_split(fh(ca), fh(cb), |p| V { pos: p })` (collinear overlaps and duplicates are handled by spade's split: already-present constraint edges are returned, not duplicated); keep `map` (inputs → canonical) and record `n_inputs`. `constraint_edges()`: iterate `tri.undirected_edges()` filtered by `is_constraint_edge`, map to canonical `(min, max)`, sort. `build_layer_cdt`: constraints from walls filtered by `cons` (`EdgeOnly`: `Edge`; `Routing`: `Edge | Pad | Track | Shape | Keepout`); after the build, fill `constraint_owner` by walking each wall's chord with `LineIntersectionIterator::new_from_handles` and assigning every `EdgeOverlap` piece the wall's `(kind, net, tie_nets, label)` (first writer wins); `point_to_anchor` sized `num_vertices()`.
- [ ] **Step 5: Run** `cargo test --release --manifest-path engine/Cargo.toml` (all; `sketch_funnel`, `smooth_engine` unchanged) and clippy — Expected: PASS. Then re-run `python -m harness.bench wiccon-2023 <scratch> --rip none --router smoother`: the fallback reason `sketch: cdt: constraint ... crosses` must be gone (0 wires with it; report the new taut count and `routing_new`, which must stay 0).
- [ ] **Step 6: Commit:** `git commit -m "fix(cdt): split crossing/overlapping constraints; ConstraintSet for routing"`.

### Task 3 (S, review F at the gate): routing graph with cut capacity and slots — `engine/src/graph.rs`

**Files:**
- Create: `engine/src/graph.rs`; Modify: `engine/src/cdt.rs` (`topology()`), `engine/src/lib.rs`
- Test: `engine/tests/graph_capacity.rs`

**Interfaces:**
- Consumes: `Cdt` (Task 2), `LayerSketch { cdt, point_to_anchor, constraint_owner }`, `LayerObstacles { anchors[i].{c, r_core, net, kind, own_clr, tie_nets, label} }`, `model.outline` (`Vec<TrackPrim>` → `dequantise`), `Rules { min_clearance, edge_clearance, hole_clearance }`, `Options.margin_nm`.
- Produces:

```rust
// cdt.rs
pub struct Topology { pub edges: Vec<(usize, usize, bool)>,   // canonical (a<b), is_constraint; sorted
                      pub faces: Vec<[usize; 3]> }             // ccw vertex triples of inner faces, sorted by (v0,v1,v2) after rotation to min first
impl Cdt { pub fn topology(&self) -> Topology; }
// graph.rs
pub struct VertexInfo { pub anchor: Option<usize>, pub owner: Option<String>, pub nets: Vec<i32>, pub r: f64, pub clr: f64 }
pub struct CrossEntry { pub conn: usize, pub width: f64, pub clr: f64 }
pub struct GEdge { pub a: usize, pub b: usize, pub faces: [Option<usize>; 2], pub constraint: bool,
                   pub owner_nets: Vec<i32>, pub cap: f64, pub c_e: f64, pub cross: Vec<CrossEntry> } // cross ordered a -> b
pub struct Face { pub v: [usize; 3], pub e: [usize; 3], pub inside: bool }
pub struct GraphRules { pub eps: f64, pub min_clearance: f64, pub edge_clearance: f64, pub hole_clearance: f64 }
/// Terminal vertices that are not anchors: (position, net, owner label) — pad centres of non-round pads, junctions.
pub struct TerminalInfo { pub at: Vec2, pub net: i32, pub owner: String }
pub struct RouteGraph { pub verts: Vec<VertexInfo>, pub edges: Vec<GEdge>, pub faces: Vec<Face>, /* private: edge_of, faces_of_vertex, occupancy: Vec<Vec<(usize edge, usize slot)>> per conn */ }
impl RouteGraph {
    pub fn build(ls: &LayerSketch, obs: &LayerObstacles, outline: &[Prim], terminals: &[TerminalInfo], rules: &GraphRules) -> RouteGraph;
    pub fn edge(&self, a: usize, b: usize) -> Option<usize>;
    pub fn faces_of_vertex(&self, v: usize) -> &[usize];
    pub fn passable(&self, e: usize, net: i32) -> bool;          // both faces inside; constraint ⇒ net ∈ owner_nets
    pub fn fits(&self, e: usize, w: f64, c: f64) -> bool;        // Global Constraints formula
    pub fn overuse(&self, e: usize) -> f64;
    pub fn slot_count(&self, e: usize) -> usize;                  // cross.len()
    pub fn insert(&mut self, conn: usize, e: usize, slot: usize, entry: CrossEntry);   // shifts later slots
    pub fn remove_conn(&mut self, conn: usize);                   // O(occupancy) via stored (edge, slot) list
    pub fn occupancy(&self, conn: usize) -> &[(usize, usize)];
}
```

`inside`: a face is inside iff its centroid is inside the outline by ray-casting parity against the outline chords (empty outline ⇒ all inside). Vertex `nets` = anchor net plus `tie_nets`; terminal vertices get `owner`/`nets` from `terminals` by exact position; `owner_nets` of a constraint edge from `constraint_owner` (net + tie nets; outline/keepout ⇒ empty).

- [ ] **Step 1: Write the failing tests** (`engine/tests/graph_capacity.rs`; helper `two_pads(d_mm)`: nets 1, 2 (clearance 200_000 each), round pads `d 1.0 mm` at `(10, 10)` and `(10 + d, 10)`, outline `50 × 40` mm, `eps 5_000`; `e` = the edge between the two pad anchors; wire `w 250_000`, `c 200_000`)

```rust
#[test] fn capacity_two_round_pads() {
    let (g, e) = two_pads(2.6);                 // cap = 2.6 - 0.5 - 0.5 = 1.6 mm
    assert!((g.edges[e].cap - 1_600_000.0).abs() < 1.0);
    // N wires fit iff N*(0.25+0.005) + (N+1)*0.2 <= 1.6: N <= 3
    let mut g = g; let mut n = 0;
    while g.fits(e, 250_000.0, 200_000.0) { g.insert(n, e, n, CrossEntry { conn: n, width: 250_000.0, clr: 200_000.0 }); n += 1; }
    assert_eq!(n, 3);
    assert!(g.overuse(e) < 1.0);                // need_all = 3*0.255 + 4*0.2 = 1.565 <= 1.6
    g.insert(3, e, 3, CrossEntry { conn: 3, width: 250_000.0, clr: 200_000.0 });
    assert!((g.overuse(e) - 420_000.0).abs() < 1.0);   // 4*0.255 + 5*0.2 - 1.6 = 0.42
}
#[test] fn capacity_boundary_single_wire() { /* d 2.0 -> exactly 1; d 1.65 -> 0 (0.655 > 0.65); d 1.66 -> 1 */ }
#[test] fn mixed_clearance_uses_max_gap() { /* existing conn c 0.25, new c 0.2 on cap 1.0: load 0.25+0.005+0.25 + 0.25+0.005 + 2*0.2 = 1.16 > 1.0 -> !fits */ }
#[test] fn edge_vertex_uses_edge_clearance() { /* pad (r 0.5, c 0.2) to outline vertex: c_e = 0.25 when edge_clearance 250_000 */ }
#[test] fn pad_hull_constraint_passable_only_for_owner_net() { /* rect pad net 1: hull edge passable(e,1) && !passable(e,2) */ }
#[test] fn faces_outside_outline_are_impassable() { /* edge between frame corner and outline vertex: !passable for any net */ }
#[test] fn remove_conn_restores_slots() { /* insert 3, remove conn 1 -> slot_count 2, cross order [0, 2], occupancy(1).is_empty() */ }
#[test] fn topology_is_sorted_and_complete() { /* 4 points square: 5 edges, 2 faces, Euler V - E + F(inner) = 1 */ }
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test graph_capacity` — Expected: FAIL (module missing).
- [ ] **Step 3: Implement** `Cdt::topology()` (spade `inner_faces()` / `undirected_edges()`, canonical ids, sorted) and `graph.rs` per the Interfaces; `fits`/`overuse` exactly per Global Constraints; `insert` shifts the stored slots of later entries on that edge (O(cross.len())).
- [ ] **Step 4: Run** the suite + clippy — Expected: PASS.
- [ ] **Step 5: Commit:** `git commit -m "feat(graph): per-layer routing graph with cut capacity and crossing slots (T2.1)"`.

### Task 4 (S): pours export, net decomposition, scope, order — `harness/model.py`, `engine/src/decompose.rs`

**Files:**
- Modify: `harness/model.py` (`export_model`, new `_export_pours`), `harness/fixtures.py` (`vcc_stub` param), `engine/src/model.rs` (`Pour`, `BoardModel.pours`), `engine/src/lib.rs`
- Create: `engine/src/decompose.rs`
- Test: `engine/tests/decompose_nets.rs`, `tests/test_router.py` (pours part), `tests/test_model_export.py` (one schema test)

**Interfaces:**
- Consumes: `model.pads[].{net, ref_, number, layers, at}`, `model.vias`, `model.kept` (`prim`, `net`, `layer`, `uuid`), `model.dangling_ends`, `model.rules`, `Options.scope` (`Scope::selects_pad`, nets/netclasses/area resolved here), `Options.existing_copper`.
- Produces (harness): `model["pours"] = [{"uuid", "net", "layers": [ids], "pads": ["REF.NUM", ...], "vias": [uuid, ...]}]` for every copper zone (not rule area, net ≠ 0) after `pcbnew.ZONE_FILLER(board).Fill(board.Zones())` and `board.GetConnectivity().Build(board)`: pads from `conn.GetConnectedPads(zone)`, vias from `conn.GetConnectedTracks(zone)` filtered `GetClass() == "PCB_VIA"` (UNVERIFIED SWIG return type — fall back to `GetConnectedItems(zone, [pcbnew.PCB_VIA_T])`); sorted. `make_fixture_board(..., vcc_stub=False)`: when true adds a **locked** VCC F.Cu track 0.25 mm from `(20, 20)` (pad 1 centre) to `(27, 24)`.
- Produces (engine):

```rust
// model.rs
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct Pour { pub uuid: String, pub net: i32, pub layers: Vec<u32>, pub pads: Vec<String>, pub vias: Vec<String> }
// BoardModel: pub pours: Vec<Pour>
// decompose.rs
#[derive(Clone, Debug, PartialEq, Serialize)] pub enum TermKind { Pad(usize), Via(usize), Copper(String), Dangling(String) }
#[derive(Clone, Debug, Serialize)] pub struct TermRef { pub kind: TermKind, pub label: String, pub at: Vec2 }  // math frame; label "U1.3" / via uuid / kept uuid
#[derive(Clone, Debug)] pub struct Connection { pub id: usize, pub net: i32, pub layer: u32, pub width: i64, pub clearance: i64, pub tier: u8, pub from: TermRef, pub to: TermRef, pub est_len: f64 }
#[derive(Clone, Debug, Serialize)] pub struct NetInfo { pub net: i32, pub name: String, pub pads: usize, pub clusters: usize, pub pour_served: bool, pub in_scope: bool, pub connections: Vec<usize> }
pub struct Decomposition { pub connections: Vec<Connection>, pub nets: Vec<NetInfo>, pub skipped: Vec<(i32, String)> }
pub fn decompose(model: &BoardModel, opts: &Options, layer: u32) -> Decomposition;
pub fn order(conns: &[Connection], bumps: &[u32]) -> Vec<usize>;   // key (−bump, tier, −width, est_len, net, id)
pub fn terminal_of(t: &TermRef) -> crate::wires::Terminal;          // Pad→Pad{pad,at}, Via→Via, Copper/Dangling→Junction{at}/Dangling{at}
```

Algorithm (12 §1): per net, union-find over nodes = pads + vias + kept items + pours; union pad↔kept item when the item's copper touches the pad polygon on a shared layer (`copper_gap_skeleton ≤ 0`), kept↔kept when touching (`copper_gap ≤ 0`, same layer) or sharing a via, via↔kept when the item's centreline passes within `d/2` of the via centre, pour↔listed pads/vias. Cluster ports = pad centres, via centres, kept item endpoints. Scope: selected terminal set `S` from `scope.pads/footprints` (`selects_pad`), `scope.nets` (names), `scope.netclasses` (`rules.per_net[net].netclass`), `scope.area` (pad centre inside polygon, KiCad frame already flipped); whole board ⇒ all. Connections = Prim's MST over the clusters of the selected terminals (edge weight = min port–port Euclidean distance; attach ports = the minimising pair); if the scope leaves exactly one selected terminal in a net, connect it to the nearest other cluster of the net. `pour_served` ⇔ the net has ≥ 1 pour, ≥ 2 pads+vias and 0 connections. `in_scope=false` nets contribute no connections. `unrouted_only` is a no-op for `fixed` copper (clusters already exclude connected pads) and is accepted. Width/clearance from `rules.net_rule(net)`; `tier` per Global Constraints; `est_len` = port distance; `layer` = the routable layer passed in.

- [ ] **Step 1: Write the failing engine tests** (`engine/tests/decompose_nets.rs`; model JSON builders)

```rust
#[test] fn two_pad_net_is_one_connection() { /* net 1 pads (0,0),(10,0): 1 conn, from.label "U1.1", to.label "U1.2", est_len 10 mm */ }
#[test] fn four_pad_net_is_mst_of_three() { /* pads (0,0),(10,0),(20,0),(10,30): 3 conns = MST edges {1-2, 2-3, 2-4}, sorted by id */ }
#[test] fn partial_trace_cluster_attaches_at_track_end() {
    // pads (0,0),(30,0); kept seg net 1 (0,0)-(12,0): clusters 2, 1 conn: from kind Copper(uuid) at (12,0), to Pad (30,0)
}
#[test] fn complete_net_yields_zero_connections() { /* kept seg joins both pads: 0 conns, NetInfo.clusters 1 */ }
#[test] fn pour_served_net() { /* pads A,B + pour listing both: 0 conns, pour_served true; pour listing only A: 1 conn, pour_served false */ }
#[test] fn scope_pads_restricts_to_those_pads() { /* 3-pad net, scope pads ["U1.1","U1.2"]: 1 conn 1-2 */ }
#[test] fn scope_single_pad_connects_to_nearest_cluster() { /* scope ["U1.3"] -> 1 conn with U1.3 and the nearest pad */ }
#[test] fn scope_nets_and_netclasses() { /* two nets, scope nets ["A"]: only A's conns; scope netclasses ["power"] via rules.per_net */ }
#[test] fn order_is_tier_width_length_then_net() {
    // conns: (Signal 0.25, 5 mm, net 3), (power 0.4, 20 mm, net 1), (Signal 0.25, 2 mm, net 2):
    // order == [power, Signal 2 mm, Signal 5 mm]; with bumps [0,0,1] -> [Signal 5 mm, power, Signal 2 mm]
}
#[test] fn dangling_end_is_a_terminal() { /* kept seg (0,0)-(5,0) with dangling end at (5,0): conn from TermKind::Dangling(item uuid) at (5,0) */ }
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test decompose_nets` — Expected: FAIL.
- [ ] **Step 3: Implement** `Pour` in `model.rs` and `decompose.rs` per the Interfaces (union-find with path compression; all lists sorted; `BTreeMap` by net).
- [ ] **Step 4: Run** engine tests + clippy — Expected: PASS.
- [ ] **Step 5: Write the failing harness tests** (`tests/test_router.py`)

```python
def test_export_pours_lists_connected_pads_and_vias(fixture_board):
    b = load_board(fixture_board); m = export_model(b, resolve_rules(b, dru_text=None, widths_before=record_widths(b)), name="fx")
    pours = m["pours"]; assert len(pours) == 1 and pours[0]["net"] == gnd_code(m)
    assert pours[0]["pads"] == ["U1.4"] and len(pours[0]["vias"]) == 1 and pours[0]["layers"] == [int(pcbnew.B_Cu)]
def test_fixture_vcc_stub_is_locked_and_touches_pad1(tmp_path): ...  # one extra locked VCC track, start (20,20)
```

- [ ] **Step 6: Run** `.venv/bin/pytest tests/test_router.py -q` — Expected: FAIL (no `pours`), then implement `_export_pours` and the fixture parameter; run again — Expected: PASS; `tests/test_model_export.py` still green.
- [ ] **Step 7: Commit:** `git commit -m "feat(decompose): copper clusters, pours, MST connections, scope and order (T2.2)"`.

### Task 5 (F): design note — slot search, negotiation, partial stubs — `docs/progress/notes/m2-search-design.md`

**Files:**
- Create: `docs/progress/notes/m2-search-design.md` (≤ 2 pages)

**Interfaces:**
- Consumes: Task 3 `RouteGraph` API, Task 4 `Connection`, `LayerObstacles` (anchor circles for line-of-sight), 12 §3, §5, §6; Nash & Koenig 2013 (Lazy Theta*), Cui/Harabor/Grastien 2017 (Polyanya).
- Produces: the note that Tasks 6, 7 and 10 implement, fixing: (1) the **slot state** `(edge, slot)` and the transition rule inside a triangle: a wire entering edge `e_in` at slot `s` and leaving through `e_out` (sharing vertex `v`) gets `slot_out` = number of entries on `e_out` that belong to connections also crossing `e_in` on the `v` side of `s` (corner-`v` group nesting), and is blocked when a wire on the `v` side of `s` on `e_in` leaves through the third edge (it would have to be crossed); start: a connection leaving its start vertex `p` takes slot 0 (nearest `p`) on edges incident to `p` and slot `|corner-a group|` on the opposite edge `(a, b)` of a start face; goal symmetric; (2) **cost**: `g` with the Lazy-Theta*-style parent shortcut (line-of-sight test against anchor circles inflated by `r_core + c + w/2 + ε` through `LayerObstacles::query_anchors`) or Polyanya intervals — F chooses, with the reason, and the exact `h` (Euclidean to goal, admissible) and the expansion cap `8·(E + X) + 64`; edge cost `len_est × (1 + hist(e)) × present(e)` with `present(e) = 1` if `fits` else `PRESENT_OVERUSE (4.0) × (1 + overuse/cap)`; priority weight multiplies the `(hist, present − 1)` terms: `{1: 0.5, 2: 0.75, 3: 1.0, 4: 1.25}`; (3) the **ghost search** for blocked connections (slot-free, outline-respecting, non-passable or non-fitting edges cost `+ GHOST_BLOCK_NM = 1e10`), its `blocked_edge` = first such edge and `last_reached` = the crossing before it; (4) **negotiation**: iteration = route every connection in `order(conns, bumps)` (each first removed from the graph), then metrization feedback (Task 8) → `penalise(conn)`: `hist += HIST_FAIL (1.0)` on the failing connection's edges, `bump += 1`; blocked connections: `hist += HIST_BLOCK (0.5)` on the ghost path's edges, `bump += 1`; after each iteration overused edges get `hist += HIST_OVERUSE (0.5)`; convergence = no failures, no blocked, no overuse; best iteration = max routed, then min Σ est_len; (5) **clinch detection** (12 §5, Luzin 2015): regular clinch = connection with terminal vertex `p` crossing an edge incident to `p` at a slot > 0 with a foreign entry below it (structurally impossible in the slot model — detector kept as an invariant); U-clinch = constraint-free edge `(p, q)` whose two vertices belong to the same net `N`, crossed by ≥ 1 foreign entry, while the `N` connection joining their clusters has ≥ 1 crossing; diagnostic only (report counts), no feedback (the note gives the counter-example where a U-clinch is necessary); (6) **partial stubs**: the stub endpoint `P` on the segment from the last reached crossing point `Q` (or the start terminal) toward the ghost crossing point `P0` of the blocked edge = farthest point such that the straight track `Q→P` passes `Checker` at margin ε (bisection, 24 steps); forward stub = ghost crossings before the blocked edge, reverse stub = ghost crossings after it, reversed; `blocking` = midpoint of the blocked edge. Each rule in the note must come with its complexity (45 §5a) and the test it is pinned by (Tasks 6, 7, 10).

- [ ] **Step 1 (F):** Read Tasks 3, 4, 6, 7, 8, 10 of this plan, 12 §3/§5/§6, 10 §3–4, 11 §2, `m1-sketch-design.md` §3–4, `graph.rs`. Write the note with the six sections above (decide the Lazy Theta* vs Polyanya question; default is Lazy Theta* unless Polyanya's exactness is needed for the length gate — say why).
- [ ] **Step 2:** Self-check the note against Review Focus 2, 4, 5 and the complexity table; every constant named above appears with its default value in the note.
- [ ] **Step 3: Commit:** `git commit -m "docs(notes): M2 slot search, negotiation and partial-stub design (F)"`.

### Task 6 (S, from the Task 5 note): slot A* and ghost search — `engine/src/search.rs`

**Files:**
- Create: `engine/src/search.rs`; Modify: `engine/src/lib.rs`
- Test: `engine/tests/search_slots.rs`

**Interfaces:**
- Consumes: `RouteGraph` (Task 3), `Connection` (Task 4), `LayerObstacles::query_anchors`, note §1–3.
- Produces:

```rust
#[derive(Clone, Debug)] pub struct SlotPath { pub conn: usize, pub crossings: Vec<(usize, usize)> /* (edge, slot) in travel order */, pub faces: Vec<usize>, pub est_len: f64, pub expansions: usize }
pub enum SearchOutcome { Found(SlotPath), Blocked { ghost: Option<SlotPath>, blocked_edge: Option<usize>, last_reached: Option<(usize, usize)> }, Timeout }
pub struct SearchParams<'a> { pub history: &'a [f64], pub weight: f64, pub present_overuse: f64, pub expansion_cap: usize, pub deadline: Instant }
pub fn search(g: &RouteGraph, obs: &LayerObstacles, conn: &Connection, start_v: usize, goal_v: usize, p: &SearchParams) -> SearchOutcome;
pub fn ghost(g: &RouteGraph, obs: &LayerObstacles, conn: &Connection, start_v: usize, goal_v: usize, p: &SearchParams) -> Option<(SlotPath, Option<usize>, Option<(usize, usize)>)>;
```

- [ ] **Step 1: Write the failing tests** (`engine/tests/search_slots.rs`; graphs from Task 3 builders: empty 50 × 40 board, round pads `d 1.0`, clearance 0.2, wires 0.25, ε 5 µm)

```rust
#[test] fn search_adjacent_terminals_zero_crossings() { /* pads (10,10),(20,10) adjacent CDT vertices: Found, crossings.is_empty(), est_len ≈ 10 mm */ }
#[test] fn search_goes_around_a_foreign_pad() {
    // net 1 pads (5,10),(35,10); net 2 pad (20,10): Found; path crosses edges on one side of (20,10); est_len in [30.0, 30.6] mm
}
#[test] fn search_respects_capacity() {
    // channel of two pad rows 2.0 mm apart (cap for exactly 1 wire): after insert of one wire, search for a second
    // -> Found with a path not using the channel edge (or Blocked if no other way), never a path crossing a non-fitting edge
}
#[test] fn slot_transition_nests_corner_groups() {
    // conn A routed around pad X on its left; conn B same start/goal region: B's slot on every edge of the shared sleeve is
    // outside A (slot 1) and consistent on consecutive edges (invariant checked by RouteGraph::insert order)
}
#[test] fn search_blocked_by_wall_reports_ghost() {
    // kept locked seg net 9 across the board between the two pads: Blocked { ghost: Some(_), blocked_edge: Some(e) } with
    // e a constraint edge owned by "kept:<uuid>"; last_reached is the crossing before e
}
#[test] fn search_expansion_cap_and_deadline() { /* expansion_cap 1 -> Blocked/Timeout, never panics; deadline in the past -> Timeout */ }
#[test] fn search_is_deterministic() { /* two runs equal SlotPath */ }
```

- [ ] **Step 2: Run to verify failure:** `cargo test --release --manifest-path engine/Cargo.toml --test search_slots` — Expected: FAIL.
- [ ] **Step 3: Implement** per the note (binary heap keyed `(f, h, edge, slot)` with `total_cmp`; closed set per `(edge, slot)`; parent pointers; the slot transition and start/goal rules of note §1; `g` per note §2; ghost per note §3).
- [ ] **Step 4: Run** the suite + clippy — Expected: PASS.
- [ ] **Step 5: Commit:** `git commit -m "feat(search): planar slot A* with Lazy-Theta* length estimate and ghost search (T2.3a)"`.

### Task 7 (S, from the Task 5 note): negotiation loop and clinch detection — `engine/src/negotiate.rs`

**Files:**
- Create: `engine/src/negotiate.rs`; Modify: `engine/src/lib.rs`
- Test: `engine/tests/negotiate_loop.rs`

**Interfaces:**
- Consumes: Tasks 3, 4, 6; note §4–5.
- Produces:

```rust
pub const HIST_FAIL: f64 = 1.0; pub const HIST_BLOCK: f64 = 0.5; pub const HIST_OVERUSE: f64 = 0.5; pub const PRESENT_OVERUSE: f64 = 4.0;
#[derive(Clone, Debug, Default, Serialize)] pub struct ClinchReport { pub regular: Vec<(usize, usize)>, pub u: Vec<(usize, usize)> } // (conn, edge)
#[derive(Clone, Debug)] pub struct BlockInfo { pub edge: Option<usize>, pub ghost: Option<SlotPath>, pub last_reached: Option<(usize, usize)> }
pub struct IterOutcome { pub paths: Vec<Option<SlotPath>>, pub blocked: BTreeMap<usize, BlockInfo>, pub overuse_edges: usize, pub timed_out: bool }
pub struct Negotiator { pub history: Vec<f64>, pub bumps: Vec<u32>, pub iteration: u32, /* best: Option<(usize routed, f64 len, Vec<Option<SlotPath>>)> */ }
impl Negotiator {
    pub fn new(edges: usize, conns: usize) -> Negotiator;
    /// One PathFinder iteration over all `active` connections (others keep their paths in `g`), in `order(conns, bumps)`.
    pub fn iteration(&mut self, g: &mut RouteGraph, obs: &LayerObstacles, conns: &[Connection], active: &[usize], verts: &[(usize, usize)], opts: &Options, deadline: Instant) -> IterOutcome;
    pub fn penalise_failed(&mut self, g: &RouteGraph, conn: usize);          // HIST_FAIL on occupancy edges, bump
    pub fn penalise_blocked(&mut self, conn: usize, b: &BlockInfo);         // HIST_BLOCK on ghost edges, bump
    pub fn penalise_overuse(&mut self, g: &RouteGraph) -> usize;            // returns overused edge count
    pub fn record_best(&mut self, paths: &[Option<SlotPath>], routed: usize, len: f64);
    pub fn best(&self) -> Option<&Vec<Option<SlotPath>>>;
}
pub fn detect_clinches(g: &RouteGraph, conns: &[Connection], paths: &[Option<SlotPath>], verts: &[(usize, usize)]) -> ClinchReport;
```

- [ ] **Step 1: Write the failing tests** (`engine/tests/negotiate_loop.rs`)

```rust
#[test] fn negotiate_contended_channel_routes_both() {
    // two pad rows forming one channel with capacity 1 (rows 2.0 mm apart, 3 pads each, 2.54 pitch) between the left and right
    // halves of a 50x40 board plus a free detour above the rows; nets A and B each pad-left to pad-right through the channel:
    // after <= 50 iterations both Found, exactly one uses a channel edge, overuse_edges == 0; repeating gives identical paths
}
#[test] fn blocked_connection_is_bumped_first_next_iteration() { /* bumps[k] == 1 after penalise_blocked; order() puts k first */ }
#[test] fn overuse_raises_history_on_overused_edges_only() { /* insert 4 on cap-3 edge: penalise_overuse == 1, history[e] == 0.5, others 0 */ }
#[test] fn best_iteration_keeps_max_routed() { /* record_best(2, 10) then (1, 1): best() has 2 routed */ }
#[test] fn u_clinch_detected_and_regular_absent() {
    // net N pads (0,0),(10,0); net F pads (5,3),(5,-3): routed: F crosses the N edge, N detours -> u.len() == 1, regular.is_empty()
}
#[test] fn iteration_respects_deadline() { /* deadline now: timed_out true, no panic */ }
```

- [ ] **Step 2: Run to verify failure** — `cargo test --release --manifest-path engine/Cargo.toml --test negotiate_loop` — Expected: FAIL.
- [ ] **Step 3: Implement** per the note §4–5; `iteration` removes each active connection from `g` before its search and inserts the found path (`RouteGraph::insert` per crossing); deadline checked before every search.
- [ ] **Step 4: Run** the suite + clippy — Expected: PASS.
- [ ] **Step 5: Commit:** `git commit -m "feat(negotiate): PathFinder history costs, bumps, best iteration, clinch detection (T2.3b)"`.

### Task 8 (S): `route()` orchestration, metrization hand-off, self-check, PyO3 — `engine/src/route.rs`

**Files:**
- Create: `engine/src/route.rs`; Modify: `engine/src/sketch.rs` (`sketch_from_crossings`), `engine/src/smooth.rs` (make `outside_len`, `terminal_skeletons`, `hand_outside`, `deadline_after` `pub(crate)`), `engine/src/py.rs`, `engine/src/lib.rs`
- Test: `engine/tests/route_engine.rs`, `tests/test_engine_surface.py` (surface round-trip)

**Interfaces:**
- Consumes: `decompose` (Task 4), `build_layer_cdt(.., ConstraintSet::Routing)` (Task 2), `RouteGraph::build` (Task 3), `Negotiator` (Task 7), M1 `relax(wires, sketches, obs, orders, opts, deadline) -> RelaxResult`, `stack_order`, `funnel`, `quantise_chain`, `Checker`, `ObstacleSet::build(model, opts, &exclude_kept = ∅, &terminals)` (junction terminals of copper attach points as `TerminalObstacle`s, as in `smooth`).
- Produces:

```rust
// sketch.rs
pub fn sketch_from_crossings(wire: usize, start: Vec2, goal: Vec2, crossings: Vec<Crossing>, ls: &LayerSketch) -> Sketch;  // steps 5–6 of sketch_wire, shared by sketch_wire
// route.rs
#[derive(Clone, Debug, Serialize)] pub struct ConnReport { pub id: usize, pub net: i32, pub name: String, pub layer: u32, pub width: i64,
    pub status: String /* routed | partial | failed */, pub reason: String, pub from: TermOut, pub to: TermOut,
    pub length_nm: i64, pub length_outside_pads_nm: i64, pub pattern: String, pub anchors: usize, pub crossings: usize,
    pub blocking: Option<[i64; 2]> /* KiCad frame nm */, pub g1_angle_deg: Option<f64>, pub iteration: u32 }
#[derive(Clone, Debug, Serialize)] pub struct TermOut { pub kind: String /* pad|via|copper|dangling */, pub label: String, pub at: [i64; 2] }
#[derive(Clone, Debug, Serialize)] pub struct NetRouteReport { pub net: i32, pub name: String, pub status: String /* routed | partial | failed | pour_served | complete | out_of_scope */,
    pub connections: usize, pub routed: usize, pub partial: usize, pub failed: usize, pub length_nm: i64, pub length_outside_pads_nm: i64 }
#[derive(Clone, Debug, Serialize)] pub struct RouteResult { pub items: Vec<Item>, pub connections: Vec<ConnReport>, pub nets: Vec<NetRouteReport>,
    pub connections_total: usize, pub routed: usize, pub partial: usize, pub failed: usize, pub nets_total: usize, pub nets_routed: usize,
    pub total_length_nm: i64, pub length_outside_pads_nm: i64, pub iterations: u32, pub converged: bool, pub overuse_edges: usize,
    pub clinches: ClinchReport, pub relax_iterations: u32, pub self_check_rounds: u32, pub timed_out: bool, pub runtime_s: f64,
    pub threads_used: usize, pub warnings: Vec<String> }
#[derive(Clone, Debug, Serialize)] pub struct WireStats { pub wires: usize, pub skipped: usize, pub hand_length_nm: i64, pub hand_outside_pads_nm: i64 }
pub fn route(model: &BoardModel, opts: &Options) -> RouteResult;
pub fn wire_stats(model: &BoardModel) -> WireStats;   // extract_wires + hand_outside on every wire (unripped board)
// py.rs: #[pyfunction] route(model_json, options_json) -> String;  wire_stats(model_json) -> String
```

Orchestration of `route()`: routable layer = the first `layers[].routable` with id 0 (F.Cu) — M2 routes on F.Cu only (`warnings` lists other routable layers as unused); `decompose`; connections whose terminal pad lacks the layer → `failed: "terminal not on layer F.Cu"`; `ObstacleSet::build`; `build_layer_cdt(obs.layer(l), terminals = every connection's `from.at`/`to.at` (dedup), Routing)`; `RouteGraph::build`; start/goal vertices via `LayerSketch.vertex_of` (make a `pub fn vertex_at(&self, p: Vec2) -> Option<usize>`). Loop `it in 1..=pathfinder_iter_cap`: `Negotiator::iteration` over the active set (iteration 1: all; later: failed/blocked/penalised ones — others keep their paths); build `Wire`s (`id = conn id`, `hand = [start, goal]`, `kept_uuids = []`, `hand_len = est_len`) and `Sketch`es via `sketch_from_crossings` with `Crossing { left, right, t }` from each `(edge, slot)`: orientation by travel (the face sequence), `t = (slot + 1) / (slot_count + 1)` measured from `left`; `orders = stack_order`; `relax` for all routed wires; `quantise_chain`; self-check rounds exactly as `smooth.rs` (margin 0, `Checker` over `model` + emitted items, fallback granularity = connection); a connection failing relax or self-check → `penalise_failed`, status pending; iteration result recorded with `record_best(routed = metrized ∧ checked)`; stop when the iteration has no failures/blocked/overuse (converged) or at the cap or deadline. Final report from the best iteration; `length_outside_pads_nm` via `outside_len` with both terminal pads' skeletons (`Copper`/`Via` terminals contribute no skeleton); `nets_total` = nets with ≥ 1 connection in scope; items carry `wire: Some(conn id)`.

- [ ] **Step 1: Write the failing tests** (`engine/tests/route_engine.rs`; models built in JSON; `Options { wall_clock_s: 10.0, ..Default }`)

```rust
#[test] fn route_two_pads_straight() { /* 1 net, 2 round pads 10 mm apart on 50x40: routed 1, items 1 seg, pattern "S", length_nm == 10_000_000 ± 2, no SS */ }
#[test] fn route_around_foreign_pad_has_arc() { /* net 1 (5,10)-(35,10), net 2 pad at (20,10): routed, pattern contains 'A', Checker margin 0 clean */ }
#[test] fn route_four_nets_grid_deterministic() { /* 4 two-pad nets interleaved: routed 4, two runs byte-identical JSON minus runtime_s */ }
#[test] fn route_pad_off_layer_fails_cleanly() { /* pad with layers [31] only: that conn failed "terminal not on layer F.Cu", others routed */ }
#[test] fn route_terminal_inside_foreign_obstacle_fails_once() { /* 0.5 mm pitch pads: failing conn reason starts "inside obstacle:", iterations <= 2, others routed */ }
#[test] fn route_zero_wall_clock_times_out() { /* wall_clock_s 0.0: timed_out, items empty, every conn failed "timeout" */ }
#[test] fn route_pour_served_net_not_counted() { /* GND pour listing its two pads: nets[GND].status "pour_served", nets_total excludes it */ }
#[test] fn route_scope_pads_only() { /* scope pads ["U1.1","U1.2"] on a 3-net board: connections_total 1 */ }
#[test] fn route_smooth_option_warns() { /* existing_copper Smooth: warnings[0] starts "existing_copper=smooth ignored" */ }
#[test] fn wire_stats_on_hand_board() { /* kept copper pad to pad 10 mm, pads d 1.0: wires 1, hand_outside_pads_nm == 9_000_000 ± 2 */ }
```

- [ ] **Step 2: Run to verify failure** — `cargo test --release --manifest-path engine/Cargo.toml --test route_engine` — Expected: FAIL.
- [ ] **Step 3: Implement** `sketch_from_crossings`, `route.rs` and the PyO3 functions per the Interfaces; `tests/test_engine_surface.py::test_route_surface_round_trip` (fixture model → `engine.route` JSON has keys `items`, `connections`, `nets`, `threads_used == 1`).
- [ ] **Step 4: Run** all engine suites, clippy, rebuild the wheel, `.venv/bin/pytest tests/test_engine_surface.py -q` — Expected: PASS.
- [ ] **Step 5: Commit:** `git commit -m "feat(route): route() orchestration with metrization hand-off, self-check and PyO3 surface (T2.4a)"`.

### Task 9 (S): harness integration — `harness/router.py`, bench, DRC spots, progress

**Files:**
- Create: `harness/router.py`; Modify: `harness/bench.py`, `harness/progress.py`, `engine/src/model.rs` (`DrcSpot`, `Options.drc_spots`), `engine/src/obstacle.rs` (`Anchor.only_nets`, spots as anchors), `engine/src/taut.rs`/`relax.rs` (exempt rule), `engine/src/route.rs` (spots → CDT vertices)
- Test: `tests/test_router.py`, `engine/tests/obstacle_model.rs` (one spot test)

**Interfaces:**
- Consumes: `topor_engine.route`, `topor_engine.wire_stats`, `writer.write_items` (groups), `drc.new_routing`, `drc.classify`, `render`, `progress.render_progress`.
- Produces (engine): `#[serde(default)] pub struct DrcSpot { pub layer: u32, pub at: Pt, pub r: i64, pub net: i32 }`, `Options.drc_spots: Vec<DrcSpot>` (default empty); `Anchor.only_nets: Vec<i32>` (empty = applies to every net); `ObstacleSet::build` adds one anchor per spot (`kind: Keepout`, `r_core: r`, `net: 0`, `own_clr: 0`, `only_nets: [net]`, label `spot:<i>`); `ObstacleSet::exempt`-style checks in `taut`/`relax`/`graph` treat an anchor with non-empty `only_nets` as exempt for every net not listed; `RouteGraph` vertex `nets` for a spot = `[]` with `owner = Some("spot:i")` (its edges get `c_v = 0`, `r = r`).
- Produces (harness): `route_router(model: dict, options: dict) -> dict` → `{"items": [...], "remove_uuids": [], "report": <RouteResult dict>}`; `drc_spots(violations, created: dict, model: dict, reach_nm: float, clearance_nm: float) -> list[dict]`: for each new routing violation, the net = `created[uuid]["net"]` of a named written item, else (zone necks) the net of the written item whose centreline passes within `reach_nm` of the named position (as `drc_pins`), spot `at` = the lowest non-zone item position (nm, KiCad frame), `r = reach_nm` for `connection_width` violations else `clearance_nm + max width`, `layer` = the written item's layer; `run_bench` → `--router topor`: `resolve_rules` moved before the rip and reused; `pre_model = export_model(b, rules, name=board)` before the rip; `result["hand_stats"] = json.loads(topor_engine.wire_stats(json.dumps(pre_model)))` when the engine imports (else `None`); DRC loop for dict routers whose report has `"connections"`: accumulate `options["drc_spots"]` (dedup by `(layer, at)`), rerun from the pre-route copy while new spots appear, ≤ `drc_loop_cap` rounds; `result["route"] = _route_summary(report)` = `{connections, routed, partial, failed, nets, nets_routed, length_mm, outside_mm, hand_outside_mm, iterations, timed_out, clinches}`; summary line `  route: routed R/C connections, N/Nn nets, partial P, failed F, outside pads X mm (hand H), iters I, clinches U` and `  partial=<n>` on its own line when > 0 (M0 outline); `progress.render_progress(task, router="topor")` → `docs/benchmarks/progress/<date>-m2-<task>.png`, caption line 1 `"{task}: {routed}/{connections} connections, {nets_routed}/{nets} nets, partial {p}, failed {f}, arcs {arcs}"`, line 2 `"DRC routing {n}, unconnected {u}, outside pads {x:.1f} vs hand {h:.1f} mm, KiCad {k:.1f} vs hand {hand:.1f} mm, router {s:.2f} s"`; CLI `--router {smoother,topor}` (default smoother keeps M1 behaviour).

- [ ] **Step 1: Write the failing engine test** (`engine/tests/obstacle_model.rs`): `drc_spot_is_an_obstacle_only_for_its_net` — `Options { drc_spots: [DrcSpot { layer 0, at (10,10) mm, r 400_000, net 1 }] }`: anchor with label `spot:0`, `only_nets == [1]`; a net-1 taut chain from (5,10) to (15,10) (via `route` on a model with pads of net 1 at those points) wraps the spot (pattern contains `A`), a net-2 chain over the same line is straight.
- [ ] **Step 2: Run to verify failure**, implement `DrcSpot`/`only_nets`/exempt rule/spot anchors, run engine tests + clippy — Expected: PASS. Rebuild the wheel.
- [ ] **Step 3: Write the failing harness tests** (`tests/test_router.py`)

```python
def test_route_router_on_fixture(engine, fixture_board, tmp_path):  # fixture_board is routable: SIG 3 pads + locked track, VCC 2 pads
    r = bench.run_bench("fixture", tmp_path, router=route_router, rip="all", pcb_override=fixture_board.pcb)
    rep = r["router_report"]; assert rep["connections_total"] >= 3 and r["routed_drc"]["routing_new"] == 0
    assert r["hand_stats"]["wires"] >= 1 and r["route"]["routed"] == rep["routed"]
def test_drc_spots_from_violation_names_written_item(): ...   # fake violation naming a created uuid -> one spot, net, r == clearance + width
def test_drc_spots_zone_neck_uses_reach(): ...               # violation type connection_width naming a via position -> r == reach
def test_bench_drc_loop_reroutes_with_spots(engine, fixture_board, tmp_path, monkeypatch):  # monkeypatched run_drc returns one new violation in round 1 -> round 2 options carry drc_spots, rounds == 2
def test_progress_m2_caption_and_path(engine, fixture_board, tmp_path, monkeypatch): # png name ends "-m2-T9.png"; caption line 1 starts "T9: "
def test_bench_summary_prints_partial_line(capsys): ...      # fake router report with partial 2 -> "  partial=2" in output
```

- [ ] **Step 4: Run** `.venv/bin/pytest tests/test_router.py tests/test_smoother.py -q` — Expected: FAIL, then implement `router.py`, bench, progress; run again — Expected: PASS (`test_smoother.py` unchanged).
- [ ] **Step 5: First end-to-end eprom run:** `.venv/bin/python -m harness.bench eprom-to-kernal <scratch> --rip all --router topor` and `.venv/bin/python -m harness.progress --task T9 --router topor`; commit the PNG; send path + numbers to main. Record `routed/27`, `routing_new`, `unconnected`, outside-pad length vs hand, `router_s` in STATE.
- [ ] **Step 6: Commit:** `git commit -m "feat(harness): topor router, DRC spots loop, hand stats, M2 progress PNG (T2.4b)"`.

### Task 10 (S): `partial_routes` option — `engine/src/route.rs`, fixture wall

**Files:**
- Modify: `engine/src/route.rs`, `harness/fixtures.py` (`wall` param)
- Test: `engine/tests/route_engine.rs` (two tests), `tests/test_router.py` (two tests)

**Interfaces:**
- Consumes: `Options.partial_routes` (default false), `BlockInfo` of the best iteration, note §6, `Checker` (margin ε), `relax`, `writer` groups.
- Produces: for every `failed` connection with a `BlockInfo { ghost: Some(_) }` and `partial_routes == true`: two stub `Wire`s (from `from` to `Junction{P_f}`, from `to` to `Junction{P_r}`) metrized together with the routed wires in one extra `relax` + self-check pass (a stub failing self-check is dropped: connection stays `failed`, reason `"partial stub failed self-check"`); emitted items carry `group: Some("topor-partial")` and `wire: Some(conn id)`; `ConnReport.status = "partial"`, `blocking = Some(midpoint of the blocked edge, KiCad frame nm)`; `RouteResult.partial` counts them; `partial_routes == false` emits nothing for failed connections (status `failed`, `blocking` still reported). `make_fixture_board(..., wall=False)`: when true adds two GND round THT pads `d 1.0 mm` at `(1.2, 24)` and `(48.8, 24)` (numbers `"7"`, `"8"`) and a **locked** GND F.Cu track 0.25 mm between their centres (the remaining gap to the board edge is 0.7 mm: a 0.25 mm VCC wire needs 0.2 + 0.25 + 0.5 = 0.95 mm, so VCC pads 1 `(20,20)` and 6 `(35,28)` are walled off).

- [ ] **Step 1: Write the failing engine tests** (`engine/tests/route_engine.rs`)

```rust
#[test] fn partial_off_emits_nothing_for_blocked_connection() { /* wall model: failed 1, items.is_empty(), blocking Some(p) with |p.y - 24 mm| <= 1 mm */ }
#[test] fn partial_on_emits_two_checked_stubs() {
    // partial_routes true: partial 1, items >= 2 all group "topor-partial", wire Some(0); far ends: distance to the wall centreline
    // in [455_000, 460_000] nm; Checker at margin 0 over items: 0 violations; status "partial"; blocking within 1 mm of the wall
}
```

- [ ] **Step 2: Run to verify failure**, implement per note §6, run engine tests + clippy — Expected: PASS. Rebuild the wheel.
- [ ] **Step 3: Write the failing harness tests** (`tests/test_router.py`, fixture `make_fixture_board(path, pads=("1","6"), wall=True, locked_track=False, arc=False)`)

```python
def test_partial_routes_off_leaves_net_unconnected(engine, tmp_path):
    r = bench.run_bench("wall", tmp_path, router=route_router, rip="all", pcb_override=pcb, options={"partial_routes": False})
    assert r["routed_items"] == 0 and r["router_report"]["connections"][0]["status"] == "failed"
    assert r["routed_drc"]["incomplete"]["unconnected_items"] == 1 and r["routed_drc"]["routing_new"] == 0
def test_partial_routes_on_writes_grouped_stubs(engine, tmp_path):
    r = bench.run_bench("wall", tmp_path, router=route_router, rip="all", pcb_override=pcb, options={"partial_routes": True})
    assert r["routed_items"] >= 2; b = load_board(...); grp = [g for g in b.Groups() if g.GetName() == "topor-partial"]; assert len(grp) == 1
    assert r["routed_drc"]["incomplete"] == {"track_dangling": 2, "via_dangling": 0, "unconnected_items": 1} and r["routed_drc"]["routing_new"] == 0
    assert abs(r["router_report"]["connections"][0]["blocking"][1] / 1e6 - 24.0) <= 1.0 and r["route"]["partial"] == 1
```

- [ ] **Step 4: Run** `.venv/bin/pytest tests/test_router.py -q` — Expected: PASS. Progress PNG `--task T10` (eprom outcome unchanged unless a connection was failing — still required: the option is part of the outcome); send to main.
- [ ] **Step 5: Commit:** `git commit -m "feat(route): partial_routes stubs toward the blocking point, grouped topor-partial (T2.4c)"`.

### Task 11 (S, gate review F): M2 gate, scaling test, version 0.2.0, benchmark 0.2.0

**Files:**
- Create: `tests/gate/test_m2_gate.py`, `tests/scaling/synth.py`, `tests/scaling/test_route_scaling.py`, `docs/benchmarks/<date>-topor-0.2.0/{README.md, images/, metrics/}`, `docs/progress/notes/m2-gate-review.md` (F)
- Modify: `engine/Cargo.toml`, `plugin/plugin.json`, `plugin/topor_plugin.py` (0.2.0), `docs/benchmarks/README.md` (run row, progress paragraph), `docs/progress/STATE.md`

**Interfaces:**
- Consumes: `run_bench(board, out, router=route_router, rip="all", options={...})`, `harness.measure`, `copy_project`/`sha256_tree`/`assert_unchanged`, `tests/gate/test_m0_gate.py::png_size`, `topor_engine.route`.
- Produces: `synth_model(n_pairs: int, seed: int) -> dict` — model JSON (no pcbnew): `n_pairs` two-pad nets, round pads `d 1.0 mm`, placed on a `ceil(sqrt(2·n))` grid at 3 mm pitch inside a rectangular outline with 2 mm margin, pad pairs paired by a seeded shuffle of grid cells with distance ≤ 9 mm (so every connection is short and routable), rules clearance 0.2, width 0.25, edge 0.25; gate test marked `gate`; scaling test marked `scaling`.

- [ ] **Step 1: Version 0.2.0** in `engine/Cargo.toml`, `plugin/plugin.json`, `SHIPPED_VERSION`; `bash tools/build.sh --dev`; `tests/test_engine_surface.py::test_version_matches_cargo` green.
- [ ] **Step 2: Write the scaling test** (`tests/scaling/test_route_scaling.py`, marker `scaling`): for `n in (10, 100, 1000)`: `t_n` = best of 3 `engine.route(synth_model(n, 1), {"wall_clock_s": 120})` with `routed == n` asserted for 10 and 100 and `routed >= 0.95·n` for 1000; assert the growth stays within `2 × (m log m)`: `(1000 ln 1000)/(100 ln 100) = 15.0` and `(100 ln 100)/(10 ln 10) = 20.0`, so `t_1000 / t_100 ≤ 30.0` and `t_100 / t_10 ≤ 40.0` (guard `t_10 ≥ 1 ms` by using `max(t, 0.001)`); `iterations ≤ 50`, `timed_out == false`. Run: `.venv/bin/pytest tests/scaling/test_route_scaling.py -m scaling -q` — Expected: PASS (if it fails, that is the one F diagnosis round of 45 §5a: measure where time goes, report, escalate).
- [ ] **Step 3: Write the gate test** (`tests/gate/test_m2_gate.py`)

```python
@pytest.mark.gate
def test_m2_gate_eprom_router(engine, tmp_path):
    src = BENCH_BOARDS["eprom-to-kernal"]; hashes = sha256_tree(src.parent); facts = measured_hand_facts(tmp_path)  # from test_m1_gate
    r = run_bench("eprom-to-kernal", tmp_path / "bench", router=route_router, rip="all")
    rep, rt, drc = r["router_report"], r["route"], r["routed_drc"]
    assert r["removed"]["removed_tracks"] == facts["segments"] and r["kept"]["kept_free_vias"] == 17
    assert rep["nets_total"] == rep["nets_routed"] == 25
    assert next(n for n in rep["nets"] if n["name"] == "GND")["status"] == "pour_served"
    assert rep["connections_total"] == rep["routed"] == facts["wires"] and rep["partial"] == rep["failed"] == 0
    assert drc["routing_new"] == 0 and drc["incomplete"]["unconnected_items"] == 0
    assert r["width_match"]["compared"] == r["width_match"]["matched"] == 25
    ra = r["routed_all"]; assert ra["arcs"] >= 1 and all("SS" not in c["pattern"] for c in rep["connections"])
    assert rep["length_outside_pads_nm"] <= r["hand_stats"]["hand_outside_pads_nm"]        # ruling: outside pad copper
    assert r["router_s"] < 2.0 and rep["threads_used"] == 1 and not rep["timed_out"] and rep["iterations"] <= 50
    model_json = (tmp_path / "bench" / "eprom-to-kernal--model.json").read_text()
    runs = [json.loads(engine.route(model_json, "{}")) for _ in range(2)]
    for x in runs: x.pop("runtime_s")
    assert json.dumps(runs[0], sort_keys=True) == json.dumps(runs[1], sort_keys=True)
    assert png_size(tmp_path / "bench" / "eprom-to-kernal-compare.png")[0] > 0
    assert_unchanged(src.parent, hashes)
    print(f"GATE M2 eprom: routed {rep['routed']}/{rep['connections_total']} nets {rep['nets_routed']}/{rep['nets_total']} outside-pads "
          f"{rep['length_outside_pads_nm']/1e6:.3f} (hand {r['hand_stats']['hand_outside_pads_nm']/1e6:.3f}) mm KiCad {ra['track_mm']+ra['arc_mm']:.3f} "
          f"(hand {facts['track_mm']}) info drc_new {drc['routing_new']} unconnected {drc['incomplete']['unconnected_items']} arcs {ra['arcs']} "
          f"router_s {r['router_s']} iters {rep['iterations']} clinches {rt['clinches']}")

@pytest.mark.gate
@pytest.mark.parametrize("board", ["wiccon-2023", "dcsgonebadge"])
def test_m2_router_other_boards_complete(board, tmp_path):  # informational: must complete, no new routing DRC
    r = run_bench(board, tmp_path, router=route_router, rip="all", options={"wall_clock_s": 60, "partial_routes": False})
    print(f"GATE-INFO {board}: routed {r['route']['routed']}/{r['route']['connections']} failed {r['route']['failed']} drc_new={r['routed_drc']['routing_new']} router_s={r['router_s']}")
    assert r["routed_drc"]["routing_new"] == 0
    assert r["router_s"] <= 65.0            # anytime: wall_clock_s 60 plus the final metrization/self-check pass
    assert_unchanged(BENCH_BOARDS[board].parent, hashes)   # hashes taken before run_bench, as in the eprom test
```

- [ ] **Step 4: Run** `.venv/bin/python -m harness.measure --board eprom-to-kernal` (re-measure; the user may have edited the board), then `.venv/bin/pytest tests/gate -m "gate or not gate" -q` (M2 + M1 gates) — Expected: PASS. If an M2 criterion fails: one F diagnosis round (note `m2-eprom-diagnosis.md`), fix tasks, re-run; never relax a budget.
- [ ] **Step 5: Benchmark 0.2.0:** `tools/bench.sh <board> docs/benchmarks/<date>-topor-0.2.0/work --rip all --router topor` for eprom-to-kernal, wiccon-2023, dcsgonebadge, and nametag-sao / wedding-gift via `--pcb` on scratch copies (`wall_clock_s` 60 for the informational boards); copy `*-hand.png`, `*-topor.png`, `*-compare.png` to `images/`, `--metrics.json`/`--drc-*.json` to `metrics/`; write `README.md` after the 0.1.0 template: metrics table (routed/connections, nets, partial, failed, segs/arcs, outside-pad length vs hand, KiCad track+arc vs hand (info), DRC routing_new, unconnected, router_s/runtime, iterations, clinches), gate criteria, eprom compare image, observations per board (single-layer runs leave many connections unrouted on the 2-layer boards — expected, informational); add the run row to `docs/benchmarks/README.md`. Progress PNG `--task T11-gate`; send to main.
- [ ] **Step 6: Commit:** `git commit -m "feat: M2 gate, scaling test, benchmark 0.2.0, version 0.2.0"`.
- [ ] **Step 7 (F): Gate review** `docs/progress/notes/m2-gate-review.md`: complexity of every new loop (graph build, slot search, negotiation, decomposition MST, clinch detection, partial bisection, self-check rounds) against 45 §5a with file:line; Tasks 6–7 vs the design note; Review Focus tests present and asserting the right things; determinism; findings as BLOCKING / DEFERRED (M2.5 / M3). Fix BLOCKING items (S fix task + progress PNG), then STATE.md handover: M2 DONE header, numbers, carry-overs for M3 (vias/layers; rayon; dangling G1 → M4).

## Deferred (recorded for STATE.md, not in this plan)

1. M1.5 items from the M1 gate review: F2 `SS` mitre for degenerate arcs (spec in `m1-gate-review.md` C(a); becomes a fix task only if the M2 gate's no-`SS` assertion fails), F3 deadline in `smooth` outside relax (new M2 loops consult the deadline by Global Constraint), F4 O(k²)/nets×kept spots A1–A5 and the drop-rule (e) disc test, F5 stack-order fan, harness T-split, wall foot anchors, hand-arc chord length; zero-slack check O(emitted × fallback) → grid query; nametag-sao smoother fallbacks (`self-check: kept` 13, `hand vertex on CDT vertex` 8).
2. Dangling hand ends: routed from as fixed terminals in M2; the G1 lead-in (virtual tangent anchor, `g1_angle_deg < 0.5°` fixture test of the M0 outline) is M4 "finishing hand-started traces" (45 §3).
3. Double-crossing removal (12 §3.2): no-op in a planar slot search; needed in M3 when crossings become vias. Parallel routing rounds (rayon, `effective_threads`): M3 (wiccon < 10 s on 4 threads is an M3 target).
4. Incremental CDT insert/remove (10 §5): M3 (vias); M2 builds the per-layer CDT once per `route()` call. Non-adjacent "disturbance" cuts (10 §4 caveat): covered by metrization + self-check + rip-up (fallback 4), LCT refinement deferred.
5. Pour completion by stub + via for pads the pour does not reach (12 §1): M3. Priority tiers from a plugin table and tier-1 inference (RF/clock/diff pairs): M4.
6. Routing on B.Cu / layer choice: M3 (M2 warns about unused routable layers).

## Self-review notes

- Spec coverage: T2.1 → Tasks 2–3; T2.2 → Task 4; T2.3 → Tasks 5–7; T2.4 → Tasks 8–11; carry-overs F-C2 → Task 1, CDT failure → Task 2; 12 §1 pour-served → Task 4; 12 §3.1 order/priority weights → Tasks 4–5; 12 §5 clinches → Task 7; 12 §6 plan steps 1–3, 5 → Tasks 4, 6, 7, 8 (step 4 double-crossing removal and the layer-assignment part of step 5 deferred, see above); 10 §4 capacity → Task 3; 30 §4.1 DRC loop → Task 9 (spots), 44 Must gaps (net ties, copper graphics, keepouts, locked copper as clusters/obstacles, scope) → Tasks 2–4; `partial_routes` → Task 10; scope options → Task 4; M0-outline engine surface `route()` with per-connection status/blocking/length/threads/timeout → Task 8; benchmark + progress PNGs → Tasks 9–11.
- Type consistency: `SlotPath.crossings: Vec<(edge, slot)>` (Task 6) is what `RouteGraph::insert` (Task 3), `Negotiator` (Task 7) and `route` (Task 8, `Crossing.t = (slot+1)/(slot_count+1)`) consume; `TermRef`/`Connection` (Task 4) feed `search` (Task 6) and `ConnReport` (Task 8); `BlockInfo` (Task 7) feeds the stubs (Task 10); `DrcSpot` (Task 9) is an `Options` field, so `route()` of Task 8 compiles before Task 9 adds it.
- Risk for the length criterion: eprom's hand +5V wires run at exactly the edge clearance (zero slack); the router must find another corridor for them with the 5 µm margin, and the total outside-pad length must still be ≤ 76.598 mm. If every other criterion passes and only the length fails, that is a user ruling (45 §5a), not a budget relaxation.
