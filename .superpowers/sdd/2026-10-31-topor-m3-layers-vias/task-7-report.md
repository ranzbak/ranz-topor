# Task 7 report: via sizes, via sites, multigraph

Status: DONE_WITH_CONCERNS (deviations below). Merged `main` first (fast-forward of Tasks 1-5).

## Changes
- `engine/src/vias.rs` (new): `ViaSize`, `via_sizes`, `SiteOrigin`, `ViaSite`, `MIN_SITE_PITCH_NM`, `via_sites` (Big-O in doc comments).
- `engine/src/multigraph.rs` (new): `LayerCtx`, `MultiGraph` (`build`, `build_on`, `layer_index`, `site_free`, `activate`, `release`, `sites_near`, `vertex_of`, `blocked_by`, `sites_of`, `alloc_legs`, `remove_conn`, `conns_of_legs`).
- `engine/src/graph.rs`: `VertexInfo.site`, dormant-anchor vertex (r 0, clr 0, nets [], owner None, site Some), `edge_version` + `clock`, `set_site`, bump rules (P4) in `insert`/`insert_path`/`remove_conn`; `chord`/`inside_outline` now `pub(crate)`.
- `engine/src/obstacle.rs`: `Anchor.enabled` and `Anchor.site`, `exempt_anchor` true when disabled, `add_site_anchors`, `set_site`.
- `engine/src/check.rs`: `Checker::via_fits`. `lib.rs`: modules.
- Tests: `tests/via_sites.rs` (5), `tests/multigraph.rs` (4), `tests/graph_capacity.rs` (+2).

## Verification
`cargo test --release --manifest-path engine/Cargo.toml`: all suites pass (0 failures); `cargo clippy --all-targets -- -D warnings` clean. TDD note: tests were written together with the implementation (one test failed first on the via_sizes expectation, see 1).

## Deviations / decisions (note beats brief where they differ)
1. `via_sizes` netclass 0.8/0.4 case: brief comment expects `[(800k,400k),(600k,300k),(450k,250k)]`; the note formula `d = max(drill + 2*min_annular, min_via_diameter)` gives `(500k,300k)` for drill 0.30 (0.3 + 0.2). Implemented the note; test pins `[(800,400),(500,300),(450,250)]`. I also skip a smaller drill whose computed `d` is not smaller than the net's own d (keeps the list sorted by d).
2. P2 (leg ids): fields `wire_of: Vec<Option<(conn, leg)>>`, `conn_wires` live in `MultiGraph`; `alloc_legs(conn, n)` (smallest free ids first), `remove_conn(conn, obs)` (releases sites, removes legs from every layer graph, frees ids), `conns_of_legs`. `insert_path(conn, &SlotPath)` is NOT implemented (needs `SlotPath`, Task 8): hook = `alloc_legs` + `layers[li].g.insert_path(k, ..)` + `activate`.
3. `TerminalInfo` has no layer, but the brief wants "terminals on that layer". `MultiGraph::build` (brief signature) puts every terminal on every layer; I added `build_on(.., terminals: &[(TerminalInfo, Vec<u32>)], ..)` for per-layer terminals (avoids changing `TerminalInfo`, which Tasks 3/6 construct). Task 9 should call `build_on`.
4. `Anchor` got an extra `site: Option<usize>` (so `RouteGraph::build` knows the site index without parsing labels).
5. `ObstacleSet::set_site(site_anchors, active)` takes no clearance/label (signature per brief): the active anchor gets `own_clr = net clearance` (>= min_clearance) and keeps label `site:<i>`; `activate`'s `clr` goes to the graph vertex only. Needs the per-owner label `via:<conn>:<s>` on anchors? Not provided; Task 9 may extend.
6. `edge_version`/`clock` are per layer graph (`RouteGraph.clock`), not a global MultiGraph clock; Task 11's `t0` must therefore be a per-layer clock snapshot (or switch to a global one). `MultiGraph` has its own clock for `site_version`.
7. `site_free` also checks `max_drill >= size.drill` (note §2 iii).
8. P4 bumps: set_site bumps all edges of all faces around the vertex (including edges opposite it), so "non-incident" in the brief's test is read as "not in a face around the vertex"; test pins that.
9. `RouteGraph::rho_max` only grows on `set_site` (upper bound, not lowered on release).
10. `in_pour` uses the note's rule (point in fill ring and ring boundary >= max_d/2); `O(S * sum ring sizes)`.
11. Parallel per-layer build (rayon) left to Task 11.
12. `cargo fmt` was applied to the new/changed files only (reverted its unrelated edits to `relax.rs`, `tests/check_index.rs`).
