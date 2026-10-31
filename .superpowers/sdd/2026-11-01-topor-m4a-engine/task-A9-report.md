# Task A9 report: priority tiers, do-not-touch, ignored pads, pour_served

Status: DONE_WITH_CONCERNS (see Concerns).

## Engine (commit 20be763)
- model.rs: Options.net_priority (BTreeMap<String,u8>), exclude_nets (Vec<String>), ignore_pads (Vec<PadRef>), pour_served (BTreeMap<String,bool>); defaults empty.
- decompose.rs: tier = table value 1..4 else inference (wide 2 / else 3); out-of-range -> warning + inference. Excluded nets: NetInfo{excluded:true}, no connections. pour_served override: false = pour not used (pads routed as signal net), true = only pour-completion edges, no MST among pour islands, no connections for a net without usable pour (status pour_served).
- route.rs: NetRouteReport.status "excluded"; ignore_pads: pads removed from a model clone (no terminal, no obstacle), one ConnReport per ignored pad, status "ignored", reason "pad outside board", NOT counted in connections_total.
- Tier meaning = plugin priority 1:1 (1 first/cheapest weight 0.5/via x2, 3 normal, 4 weight 1.25/via x0.75; plugin only produces 1..3).
- Tests engine/tests/route_priority.rs: priority_table_orders_and_weights, tier_4_pays_lower_via_cost, excluded_net_copper_is_fixed_obstacle, ignored_pad_skipped_and_reported, pour_served_override_both_directions. Written after the implementation (not seen failing first: they could not compile before the fields existed).
- Big-O in doc comments (decompose: + O(log N + |exclude|) per net; drop_ignored_pads O(P log I)).

## Plugin (second commit)
- run.build_options passes net_priority, exclude_nets, ignore_pads, pour_served (booleans only); empty values are omitted.
- schema.py: "No effect in 0.4.0" tails removed (priority, pour_served, ignore_pads); ENGINE_INTERNAL gets exclude_nets/ignore_pads/pour_served. docs/user/configuration.md regenerated, --check clean. operate.md updated.
- report.py: "Excluded nets (do not touch)" line; ignored connections already appear in the connection table (status != routed).
- tests/plugin/test_run_flow.py: test_a9_options_reach_the_engine_and_excluded_net_stays (options JSON inspected, kept copper of excluded net not ripped / not routed, report lines), test_a9_build_options_defaults_are_left_out. test_config_docs.py adjusted: only detour_reference keeps the no-effect marker.
- scope.py workaround (listing all other nets in scope.nets) left untouched; redundant now but harmless.

## Verification
- cargo test --release: all green; clippy -D warnings clean.
- Plugin subset (config_docs, config, priority, report, scope, messages, help_doc, preflight, app): 127 passed; run_flow -k a9: 2 passed.
- Byte-identity without the new options (route JSON minus runtime_s/threads_used, harness run_bench rip=all threads=2, old shared-venv engine vs this build): eprom 2dcde8ce40237b0c = 2dcde8ce40237b0c, wiccon d24bb79b58c7cbeb = d24bb79b58c7cbeb. eprom 27/27, router 0.34 s.
- No progress PNG: A9 does not change outcomes without options.

## Concerns
- Full default `pytest -q` did not finish in this session (the run hung, output empty; machine shared with other workers, kicad-cli stuck); only the subset above was run. Coordinator should run the full suite.
- connections_total excludes ignored connections (documented); a consumer comparing len(connections) to it will see a difference.
- Judgement calls: pour_served semantics above; reason string fixed to "pad outside board".
