### Task A9 (S): priority tiers, do-not-touch nets, ignored pads — `decompose.rs`, `model.rs`, `route.rs`

**Interfaces:** `Options.net_priority: BTreeMap<String, u8>`, `exclude_nets: Vec<String>`, `ignore_pads: Vec<PadRef>`; `Connection.tier` = table value (1..4) else M3 inference; excluded nets never decomposed, their copper fixed (`NetRouteReport.status == "excluded"`); ignored pads → no terminal/obstacle, `ConnReport.status == "ignored"`, reason `"pad outside board"`.

- [ ] **Step 1:** `priority_table_orders_and_weights`, `tier_4_pays_lower_via_cost`, `excluded_net_copper_is_fixed_obstacle`, `ignored_pad_skipped_and_reported` (`engine/tests/route_priority.rs`). FAIL → implement → PASS. eprom/wiccon byte-identical without options. Commit `feat(route): priority tiers from options, do-not-touch nets, ignored pads`.


## Coordinator additions (binding; Ruling A-C4/A-C5)
- Engine (as above) PLUS `[nets] pour_served` per-net override: `Options.pour_served: BTreeMap<String, bool>` (true = treat the net as
  pour-served even if auto says no — connections to pads inside its pour are served by pour completion / via-to-pour; false = route it
  as a signal net even if it has a pour). Default empty = M3 auto behaviour (eprom GND pour_served, wiccon unchanged). Test both
  directions on a fixture.
- Priority semantics (46 §2.1 + 46 row 5): the plugin already resolves the order marker `@1–@3` > netclass `topor_p1..p3` >
  `[nets] priority` and power-by-name (plugin/topor/priority.py, B7) and reports them; it does NOT pass them to the engine yet.
  Wire `run.py` → engine Options: `net_priority` (net name → tier 1..4 as the plugin computes; check the tier mapping the plugin
  uses vs the engine's `Connection.tier` meaning and state it in a doc comment), `exclude_nets` (do_not_touch, patterns already
  expanded by the plugin to names), `ignore_pads` (whole-board accepted PAD_OUTSIDE_BOARD pads; PadRef format = whatever the plugin
  stores in topor.toml `ignore_pads` — match it exactly, footprint ref + pad number). Remove the "(No effect in 0.4.0 …)" doc-string
  tails of the keys you make effective (C34), regenerate docs/user/configuration.md, `--check` clean; update docs/user/operate.md
  sentences that say these are not applied. Run report lists excluded / ignored connections (statuses "excluded"/"ignored").
- Tests: plugin run-flow test on FakeBoard proving the engine receives net_priority / exclude_nets / ignore_pads / pour_served
  (inspect the options JSON passed to route_async) + one end-to-end where an excluded net's copper stays and is not routed.
- Determinism: BTreeMap only; eprom + wiccon byte-identical WITHOUT the new options (route JSON minus runtime_s/threads_used).
- Commits `feat(route): priority tiers from options, do-not-touch nets, ignored pads` + `feat(plugin): pass priorities, do-not-touch,
  ignored pads and pour_served to the engine`.
