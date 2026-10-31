# Task 1 report: persistent checker item index

Status: DONE (scaling pytest not run: needs the wheel; coordinator runs it after merge).

## Changes
- engine/src/check.rs: `ItemIndex` (new/insert/remove/len/is_empty; ids reuse smallest free slot via min-heap; grids per layer + wildcard key for vias without listed layers), `Checker::cell()`, `Checker::check_against(idx, items, skip)`. `check_items` now = empty index, check then insert per item. Obstacle gained `hole: Option<Track>` (via drill, hole-to-hole target), `required()` takes the obstacle clearance as parameter (index items resolve it from the checker's per-net clearance).
- engine/src/route.rs: `MetrState` gets `idx`, `item_ids`, `check_visited`; `unindex()`; indexed = wires whose res is Ok. Self-check round: pending wires are checked with `check_against` against the index and earlier pending wires (temporarily inserted, removed at round end), violations charged to the later wire (as before). Passing wires are indexed at the end; chain change / drop_wire un-index. New `RouteResult.check_candidates_visited` (cumulative to the best snapshot).
- Tests: engine/tests/check_index.rs (5), route_engine.rs `route_self_check_rounds_scale_with_changed_wires` (Rust synth, 40 nets: routed 40/40, rounds 1, visited 1591 < 16000).

## Decisions / deviations
- ItemIndex stores obstacles only (no separate `items` Vec: unused).
- skip is supported by check_against but route passes `&[]`: pending wires are not pre-indexed, so no self-hits, and the old "charge the later wire" semantics are kept exactly.
- A via with empty `layers` now occupies every layer both as indexed obstacle and as checked item (before: no layer, i.e. unchecked); needed by the brief's via test. Indexed via drills now count for hole-to-hole (check_items via/via previously ignored earlier drills).

## Results
- `cargo test --release`: all suites ok; `cargo clippy --all-targets -- -D warnings` clean.
- Not run: `pytest tests/scaling` (needs wheel).
