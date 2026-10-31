# Task 5 (F) report — design note `docs/progress/notes/m3-layers-vias-design.md`

Status: DONE (docs only; no code, no .venv). Worktree `.claude/worktrees/agent-a4060a1ca85093560`, branch
`worktree-agent-a4060a1ca85093560`, commit `a0db5a4` on top of `1087a9f`:
`docs(notes): M3 via sites, layered slot search, site negotiation and parallel batch design (F)`.
Attribution line used: the executing session's system reminder (`Claude Fable 5.1`), as the plan's Global Constraints
instruct (the worker contract's `Opus 5.5` line conflicts with it). This report lives at the worktree copy of the ledger
path because the shared-checkout path is not writable from an isolated worktree.

## What the note fixes (sections 1–8, every plan constant with its default)

1. **Site generation**: union CDT (anchor centres of all routable layers + outline, no constraints) → incentre candidate per
   inner face with inradius ≥ `d_default/2 + c_max + ε` (cull only); 4 escape sites per SMD terminal pad at `hull_r + c_pad +
   d/2 + ε` along `±x̂, ±ŷ` of the pad rotation; `via_fits` = `check_items` of a net-0 `ViaItem` with a margin-ε checker
   (pads, NPTH, existing via drills via `viahole:*` Kind::Hole, `no_vias` keepouts, edge all reject); dedup at
   `MIN_SITE_PITCH_NM = 100_000` (escape sites first, then faces, each in `(x, y)` order); output sorted `(x, y)`;
   `in_pour` requires point-in-outer-ring and boundary distance ≥ `max_d/2`. Complexity `O(N log N + (F+4T)·n_sizes·q)`.
2. **Layered state** `(L, e, slot, dir)` + `AtSite(s, L)`, key includes `vias_used`; heap key `(f, h, layer_idx, edge, slot,
   dir, id)`; one root per start layer; via move = M2 goal rule (`Cyc::goal_ok`) on arrival + M2 start rules on departure,
   cost `via_cost·(1 + ω·site_history[s])`, largest fitting size of `via_sizes(rules, net)`; `h` = Euclidean to the nearest
   goal vertex / pour-fill bbox (admissible); caps `8·Σ_L(E_L+X_L) + 4S + 64` (ghost `8·Σ_L E_L + 4S + 64`); ghost soft-blocks
   owned/blocked sites at `GHOST_BLOCK_NM`. Rule T counter-example (Step 2): terminal `p` adjacent to site `s`, A's spoke
   `p → (s,x)`, B entering through `(p,x)` wanting `s` — `goal_ok` rejects (A has a port on each arc); without it B's via
   sits inside A's spoke and cuts A.
3. **Site occupancy**: exclusive `owner`, `blocked` counts with `blocks_of`/`blocked_by`, neighbour rule `|st| < d_s/2 +
   max_d_t/2 + max(c_s, hole_to_hole)`; `set_site` effects on `VertexInfo`/incident `cap`,`c_e`/`rho_max`; anchors
   `enabled`, `r_core = d/2`, net; overuse from activation is ordinary edge overuse.
4. **Negotiation**: `history[L][e]`, `site_history[s]` (`HIST_SITE = HIST_BLOCK = 0.5`), site blockers ripped as strand
   blockers (`MAX_RIP 3`), `penalise_*` otherwise unchanged, best key `(routed ↓, vias ↑, Σ est_len ↑)`, Review Focus 2 and
   4 walked through.
5. **Metrization**: one `Wire` per leg (`Terminal::Site` at sites), `SlotNesting` per layer, one `Relaxer` over both
   layers, own vias exempt through `Members.terms` + same net, `ViaItem` per site attached to the leg ending there,
   self-check over tracks + vias with `ItemIndex`, `length_outside_pads_nm` from real terminals only.
6. **Via-reduction pass**: after convergence/stall/stuck (not timeout), `(−vias, id)` order, re-search with `via_cost ×
   REDUCTION_VIA_FACTOR 4.0` and current history, accept iff fewer vias ∧ metrize + self-check pass (no other connection
   regresses, `overuse_edges == 0`) ∧ `length_outside ≤ old × REDUCTION_LEN_SLACK 1.10`, else restore; one pass.
7. **Parallel batch**: snapshot after removing active + pending rips, `par_iter` in `order()`, in-order commit with the
   `edge_version`/`site_version` check on the touched set `T(p)` plus an activated-site proximity test, invalidated ⇒
   sequential re-search, `Blocked` final (monotone restriction), proof sketch of thread-count independence.
8. **Complexity table** for (1)–(7) with caps and the pinning tests; determinism checklist.

Decision recorded with justification: discrete sites (M3-R3), continuous Weiszfeld refinement deferred to M4.

## Plan amendments (carry into the Task 7–11 dispatches)

- **P1** Task 8: a pour goal is reached by the via itself — `SlotPath.sites.len() == legs.len()` for a pour path (no
  zero-length leg on the pour layer); `vias() = sites.len()`; test `pour_goal_reaches_nearest_site_inside_pour` expects
  `legs 1, sites 1, sites[0].0 ∈ in_pour`; `Goal.pour_layer` informational; listed same-net pour vias are plain `(L, v)` goals.
- **P2** Tasks 7/9: per-layer `RouteGraph` paths are indexed by **leg id** (a path may have two legs on one layer, e.g.
  F→B→F); `wire_of`/`conn_wires` live in `MultiGraph` (not `MetrState`); `MultiGraph::insert_path` allocates and returns
  leg ids from a sorted free list; `strand_blockers`/`overused_conns` results map through `wire_of`.
- **P3** Task 9 orchestration order: `ObstacleSet::build` → `via_sites` → `MultiGraph::build` (which calls
  `add_site_anchors` before `build_layer_cdt`; sites enter each CDT as anchor points so `point_to_anchor` resolves them;
  `RouteGraph::build` maps a disabled anchor to `r 0, clr 0, nets [], owner None, site Some(s)`); `via_sites` gets `sizes`
  = union of `via_sizes(rules, net)` over live nets (descending), not `via_sizes(rules, 0)`; the fit checker is a margin-ε
  instance (the self-check keeps margin 0).
- **P4** Tasks 7/11: `edge_version` is bumped on every edge of every face in a path's `faces` list (+ `along` edges, + the
  faces around a `set_site` vertex), not only on crossed edges — otherwise a spoke through the third edge of a face blocks a
  batch candidate without invalidating it; batch validity additionally rejects a path when a site activated/released since
  the freeze lies within `rho_max + w/2 + ε` of the path's face bbox (the `los` clearance test reads vertices just outside
  the sleeve).
- **P5** Tasks 8/9: ghost result / `BlockInfo` gain `site: Option<usize>` (the first soft-blocked element may be a site);
  `blocked_edge: Option<(layer_idx, edge)>`, `last_reached: Option<(leg, crossing idx)>`.
- **P6** Task 9: new `Terminal::Site { site, at }` for leg ends at sites (`Terminal::Via { via }` keeps indexing `model.vias`;
  `usize::MAX` sentinels avoided).
- **P7** Tasks 9/10: `Snap` holds `mg: MultiGraph` + `MetrState`; obstacle anchors are re-synced from `mg.owner` on restore
  (`sync_anchors`, `O(S)`); `Ctx` drops `obs` (passed explicitly, `&mut` only for activation); `ObstacleSet` is never cloned.
- **P8** Task 8: `SearchParams.via_cost` is the final per-via cost (caller multiplies `via_cost_nm · via_weight(tier)`;
  reduction pass `× REDUCTION_VIA_FACTOR`); `via_weight(0) = via_weight(1) = 2.0`.

## Self-check (Step 2)

Review Focus 1–5 each mapped to a rule and its test (§2 roots/goals per layer → RF1; §3 exclusivity + §4 rip → RF2; §1 fit
with net 0 and margin-ε checker → RF3; §4 permanent failure before any search → RF4; §7 `T(p)` validity + re-search → RF5).
Caps: every loop bounded (expansion caps, `max_vias 4`, `MAX_RIP 3`, iteration cap 50, one reduction pass, `PROBE_STEP_CAP
64`, deadline before every batch and reduction candidate). Determinism: sorted sites/neighbour lists/blockers/free list,
total heap order, `collect` in index order, no `HashMap` iteration reaches an output; thread-count independence argued in §7.
Rule T counter-example given in §2.

## Concerns

- Known limitation (recorded in §1): `in_pour` uses the fill's outer ring with a `max_d/2` inset; a via inside a fill island
  hole (around foreign copper with a pour clearance larger than track clearance) could still be judged "in pour" — KiCad
  `--refill-zones` DRC `unconnected_items` is the gate's judge. Fix if it shows up: carry fill holes in `PourFill` (Task 4).
- The batch semantics differ from M2 sequential (later-ordered active connections do not see earlier-ordered ones' old
  paths); the Task 11 test `iteration_batch_commit_invalidates_shared_edge` still ends as M2, but eprom numbers may shift at
  Task 11 (progress PNG planned there).
- Coverage of large open faces is one site per face; if eprom's +5V #24 needs a via where no face site is, the Task 10 F
  diagnosis is the planned place to look (escape sites next to J1 pads exist by construction).
