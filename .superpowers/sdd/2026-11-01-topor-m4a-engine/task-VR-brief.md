### Task VR (opus, rotation 2): `[search] variants` — K deterministic net-order variants, keep the best (Ruling A-C10, DG R2.3 (a), R2.4 item 4)

Read rotation-2-common.md (incl. "Round 2 update"), ledger Ruling A-C10, docs/progress/notes/m4-dcsgone-diagnosis.md R2.3 (a) and the
prototype `decompose::order_variant` in docs/progress/notes/m4-dcsgone-dg-prototypes.patch (env-selected — the real one is an Option).
Implement: `Options.variants: u32` (default 1 = today, exact) + `[search] variants` Setting; the variant set is a deterministic
list of orderings, v0 (today's order) always first, then DG's most diverse (v3 width desc + seeded shuffle, v5 nets grouped in a
seeded net order, v7 previous-run failures first is not available inside one run — pick a deterministic replacement, e.g. "first-pass
failures of v0 first" when v0 has run, or v1 longer-first; justify). Run the K variants in parallel (rayon; each variant gets
threads/K worker threads, ≥ 1; with threads < K run them sequentially), each with the whole deadline share you choose (document it:
e.g. each variant gets the full remaining wall clock when run in parallel; sequential runs split it), keep the best by (routed desc,
failed asc, unconnected/pour-lost asc, vias asc, outside length asc, variant index asc). The result must be byte-identical for threads
1/2/4 and across repeats for every K (a variant's result must not depend on its thread count — check how the engine's threads affect a
single run today; it is byte-identical across threads per the gate). Cancel/deadline respected per variant; anytime result = best
committed so far. RouteResult gains `variants: [{index, routed, failed, vias, length}]` + chosen index (report fields, plugin run report
line). Big-O: K × one run.
Measure K ∈ {1, 2, 4} on eprom, wiccon, dcsgone (and nametag, wedding as info) at the gate options AND state the quiet-machine runtime
estimate for dcsgone with K = 4 inside 600 s (measure single-variant time at threads 1 when the load allows; else extrapolate and say so).
Default stays 1 unless measured as a gain inside the 600 s budget (A-C10); the coordinator re-decides after the FR/FF/FG merges — make the
code ready for that. Tests: variants_one_is_identical_to_today, variants_threads_byte_identical, variants_pick_is_lexicographic,
variants_respect_cancel. PNG `--task VR --board dcsgonebadge` with the best K. Commit `feat(route): deterministic net-order variants`.
Report task-VR-report.md.
