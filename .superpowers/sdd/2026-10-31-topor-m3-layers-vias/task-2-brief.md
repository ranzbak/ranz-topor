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
- [ ] **Step 4: Run** all engine suites + clippy — Expected: PASS; `search_slots`, `negotiate_loop`, `route_engine` unchanged. Then produce a dcsgonebadge model JSON with `.venv/bin/python -m harness.bench dcsgonebadge <scratch> --rip all --router topor` (writes `<scratch>/dcsgonebadge--model.json`) and run `cargo run --release --example route_probe -- <scratch>/dcsgonebadge--model.json --quiet`: report router seconds (M2: 45 s, 34 s of it in 59 ghost searches; expected a few s) in the task report.
- [ ] **Step 5: Commit:** `git commit -m "perf(search): sleeve-restricted clearance queries (RC3); D2 hard edges, D3 direct check, D7 spot nets, D10 vertex pinch"`.

