# Task 8 report: layered slot A* with via moves and pour goals

Status: DONE_WITH_CONCERNS (deviations below). Branch: worktree `agent-a544e50ced1c1c9ea` (merged `worktree-agent-a141224bcf3a2052b`, fast-forward, clean).

## What changed
- `engine/src/search.rs` (rewritten around a layered core `Layered`; per-layer geometry stays in `Search`):
  - New types `Leg`, `SlotPath{legs,sites,est_len,cost,..}` (+ `vias()`, `crossings_on()`, `single_layer()`), `Goal`, new `SearchParams` (history `[layer][edge]`, `site_history`, `via_cost` final per P8, `max_vias`), `SearchOutcome::Blocked{ghost, blocked_edge:(layer,edge), blocked_site, last_reached:(leg,idx)}` (P5), `via_weight` (0|1 → 2.0, 2|3 → 1.0, 4 → 0.75), `default_expansion_cap(mg) = 8·Σ(E+X) + 4S + 64`, ghost cap `8·ΣE + 4S + 64`.
  - States: `Edge(L,e,slot,dir)`, `AtSite(s)`, roots (one per start), goal nodes; closed/best key `(layer, edge, slot, dir, vias_used)`; heap key `(f, h, layer, edge, slot, dir, id)`.
  - Via move (note §2): `vias < max_vias`, site not in the ancestor chain (`last_site` links, O(vias)), largest `via_sizes(rules, net)` with `site_free`, M2 `goal_ok` at the site vertex, `los` to `site.at`; cost `Δ + via_cost·(1 + ω·site_history[s])`. On pop of an AtSite node: pour site ⇒ Found (P1, `sites.len()==legs.len()`), else M2 start rules on every other layer (plus goal vertices of those faces). Roots also try via moves to site vertices in their faces (zero-crossing first leg).
  - `direct()` per layer in index order; `h` = min(dist to goal vertices, dist to bbox of the pour sites).
  - Ghost: no strands/slots/ownership, vias capped; owned/blocked sites cost `GHOST_BLOCK_NM`; returns `(path, blocked_edge, blocked_site, last_reached)` = first soft-blocked element in travel order.
  - `leg_strand_blockers(g, &Leg)` and `strand_blockers(mg, &ghost) -> leg ids` (sorted).
  - `pub mod planar`: M2 single-layer API (`search`, `ghost`, `strand_blockers`, `default_expansion_cap`, `SearchParams`, `SearchOutcome`, `PlanarPath`) running the same core on one `RouteGraph` with no sites; `SlotPath::single_layer(&PlanarPath, start, goal)` is the adapter.
- `engine/src/multigraph.rs`: `pub rules: Rules` (cloned at build, needed for `via_sizes` in the search) and `MultiGraph::insert_path(conn, &SlotPath, obs) -> leg ids` (alloc_legs, per-leg `RouteGraph::insert_path`, then `activate` per site). The note's §3 assigns it here, so it is implemented.
- `negotiate.rs`, `route.rs`, `examples/route_probe.rs`: import change only (`search::planar::{.., PlanarPath as SlotPath, ..}`), no logic change.
- Tests: new `engine/tests/search_layers.rs` (10 tests: the 8 brief tests + `layers_stay_planar_with_via_on_foreign_wire` + `insert_path_activates_sites_and_remove_conn_frees_them`); `search_slots.rs`, `search_sleeve.rs`, `negotiate_loop.rs` only change their `use` lines.

## Decisions / differences between note and brief
- Followed the note where it differs: ghost returns a 4-tuple (P5); pour goal reached by the via itself (P1, test expects 1 leg / 1 site); `via_cost` is final (P8).
- `SlotPath.candidates` keeps the Task 2 meaning (clearance-query candidates, `search_sleeve` bounds it); the note's "AtSite nodes pushed" was not adopted.
- Pour heuristic uses the bbox of the pour **sites** (the `Goal` struct carries no fill outlines): still admissible, ≥ the note's fill-bbox distance.
- Planar entry points live in `search::planar` instead of the old top-level names (the brief's `search`/`ghost` signatures are the layered ones). Existing tests alias `PlanarPath as SlotPath`, so test bodies are unchanged.
- `est_len` counts the string through pad/vertex centres (no pad radius), so a detour around a pad is not longer in `est_len`; the via-vs-detour test therefore uses a kept-track wall with a gap.
- The planar search treats a non-fitting (unfit) edge as a costly crossing, not a hard block (existing M2 behaviour), so walls in tests use kept tracks (constraint edges); the THT test keeps the brief's foreign pad row (B.Cu wins on cost).

## Tests
- `cargo test --release --manifest-path engine/Cargo.toml`: all suites pass (search_layers 10, search_slots 10, search_sleeve 6, negotiate_loop 7, route_engine 21 incl. eprom tests, multigraph 4, via_sites 5, ...).
- `cargo clippy --all-targets -- -D warnings`: clean.
- Big-O is in the doc comments (module, `search`, `ghost`, `Layered::run`, `try_via`, `leave_vertex`, `insert_path`).

## Concerns
- Rule T test (`via_move_respects_rule_t_at_site`) builds the note's counter-example by hand-inserting the foreign strand (`RouteGraph::insert_path`) and asserts the direct hop x→s is gone and the layers stay planar; no end-to-end routed counter-example yet (Task 9).
- `route.rs`/`negotiate.rs` still single-layer (Task 9); `cargo fmt` would reformat unrelated files (`decompose.rs`, `relax.rs`, tests), I reverted those and formatted only mine.
- Python harness/venv untouched (not rebuilt).
