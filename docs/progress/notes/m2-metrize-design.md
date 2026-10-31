# M2 metrization design note — B1 (slot-faithful metrization) and B2 (incremental metrization)

F, 2026-10-31, after the gate review (`m2-gate-review.md` §1.2, §1.5 RC1/RC2, §6). Ruling R9: no budget or cap
changes; the exact collinear grid stays in the fixture. Lines refer to HEAD b7cb377 before the B1 change.

## 0. What the slot path says and what metrization must not contradict

A routed connection is a list of `(edge, slot)` crossings in travel order (`graph.rs` occupancy, `GEdge::cross`
ordered `a -> b`). On edge `(a, b)` the slot index `s` is exact topology: `cross[0..s]` are the wires between
vertex `a` and this wire, `cross[s+1..]` those between it and `b`. The face the path enters from fixes which
vertex is on its left (`route.rs:259-287 path_crossings`). Everything the metrization derives — the funnel bends,
the stack radii, the anchors the fix-up loop adds — must be a function of that data, never of a geometric guess
that may fall on the other side. Three places broke this (review §1.2):

| | Where | Was | Must be |
| --- | --- | --- | --- |
| (a) | `relax.rs:77-92 Stacks::new` | member of `(A, σ)` = every wire whose crossings touch an edge incident to `A` | nested inside wire `k` at `(A, σ)` = the wires in slots between `A` and `k` on the fan edges of `A` that `k` crosses |
| (b) | `taut.rs:418-430 penetrations` | a penetrated dyn segment adds *both* adjacent anchors, however far; own terminal pads could be candidates | add an adjacent anchor only when the conflict is at that anchor; never the wire's own terminal; otherwise leave the conflict to the other wire |
| (c) | `sketch.rs:235,255 funnel` | a portal vertex exactly on the string is popped (`>= 0`, `<= 0`): no bend, `tauten` re-adds it with a geometric σ | a touched vertex is a bend on its portal side (strict `> 0` / `< 0`); σ of a re-added anchor comes from the slot side when known |

## 1. B1 design

### 1.1 Data: `SlotNesting` (per layer), built in `route.rs` next to `path_crossings`

```rust
pub struct SlotNesting {
    /// (wire k, (anchor, σ)) -> per fan edge of the anchor crossed by k with the anchor on side σ,
    /// the wires nested between the anchor and k, innermost first.
    pub inner: HashMap<(usize, AnchorRef), Vec<Vec<usize>>>,
    /// (wire k, anchor) -> side on which k's slot path passes the anchor (absent when k passes it on
    /// both sides or never crosses one of its fan edges).
    pub sides: HashMap<(usize, usize), Orient>,
}
```
Built by `slot_nesting(g, conn_of: &[usize] /* graph conn id of wire k */, verts)`: for every wire `k`, every
crossing `(e, s)` with `GEdge { a, b, cross }` (`graph.rs:44-59`), left/right from the entered face as in
`path_crossings`: if `verts[left].anchor = Some(A)` then `inner[(k, (A, Ccw))].push(cross[..s] mapped to wire ids
nearest-first)` and `sides[(k, A)] = Ccw`; if `verts[right].anchor = Some(B)`, `inner[(k, (B, Cw))].push(cross[s+1..]
reversed)`, `sides[(k, B)] = Cw`. Entries whose `conn` is not a live wire (plain `insert`, `k: None`) are skipped.
Cost O(Σ occupancy · n_max) per metrize call, once.

### 1.2 (a) Stacks from slots — `relax.rs`

`Stacks` gets a second mode, `Stacks::from_slots(nesting)`. `radius_zs(a, k)` in slot mode = `max` over the fan
edge lists `L` of `inner[(k, a)]` of the 21 §5 fold over `L ++ [k]` (`R_j = max(ρ_j, max_{i<j}(R_i + w_i/2 + c(i,j) +
w_j/2))`); an anchor without an entry gives `ρ_k`. Membership keeps the 21 §5 rule: an inner wire `j` counts while
its current chain bends at the anchor (either σ; `Stacks::bends`, filled by `set_members` after every tauten), and
counts everywhere before its first tauten (conservative start). A static membership was tried first and rejected:
the tangent-arc model gives an anchor one radius, so a wire nested inside `k` on *some* fan edge of a header pin
would inflate `k`'s ring in the pin gap too (eprom J1, iteration 1: A13 `infeasible: pad:J1:1 <-> pad:J1:2`, the
negotiation then drifts to a 91 mm solution). With the dynamic rule an inner wire that runs straight past the anchor
leaves the ring at `ρ_k`; a residual arc-vs-segment conflict goes through the add rule (1.3) on the inner wire's turn.
The hand-wire mode (`Stacks::new(order)`, smoother `smooth.rs:363-368`, partial stubs `route.rs:854`) is unchanged.
Per-edge lists are consistent subsets of one nesting order (planarity), so the max over edges is the exact radius
when wires nest in different sectors of the same anchor; one list is O(n_max²), n_max = edge capacity.

### 1.3 (b) Penetrations — `taut.rs`

`Members` carries the chain's terminal positions; `has(a)` is true for an anchor centred on `start` or `goal`, so
no rule can ever insert the wire's own terminal (same-net pads were already exempt; this makes the invariant
explicit and net-independent). Dyn obstacles:
- arc of wire `j` at `(A, σ_j)`: add `A` (unchanged) with σ = `env.slot_side(A)` if known, else `sigma_for`;
- segment of wire `j` with adjacent anchors `E`: add `E` only when the penetration is *at* `E`, i.e. the distance
  from `E`'s centre to this wire's primitive is `< stack_radius(E) + query_grow()`; a mid-span crossing adds nothing.
  The other wire sees the same crossing from its side (its segment against this wire's arc, or against this wire's
  segment near one of *this* wire's anchors) and resolves it there — the M1 F5 fan rule: the wire that is inside
  bends, the outer one keeps its stack radius.
`tauten` therefore can return `Ok` with an unresolved dyn conflict; Gauss–Seidel settles it on the other wire's turn,
and the self-check keeps the DRC guarantee.

### 1.4 (c) Funnel ties — `sketch.rs`

`funnel` pops a chain vertex only when the new vertex lies strictly beyond it (`cross3 > 0` / `< 0`); a vertex on the
string stays and becomes a bend when the apex advances over it (the advance tests already treat `cross3 == 0` as "advance").
Effect: start / pad / goal collinear with the pad listed as the right portal vertex → `seq = [(pad, Cw)]`, as the left
vertex → `[(pad, Ccw)]`. The hand sketch (`sketch_wire`) sees the same funnel; a hand polyline through a pad centre is
still an error (`hand vertex on CDT vertex`), ties from a shifted hand keep the shifted side.

### 1.5 Invariants, determinism, complexity

- I1: `seq` of a routed sketch bends at every CDT vertex the string touches, on the portal side.
- I2: the wires that can enter `stack_radius(k, A, σ)` are fixed by the slot paths of the live wires; which of them
  count follows their current chains (Gauss–Seidel state), as in the hand mode.
- I3: `penetrations` never returns an anchor at the wire's own terminal, nor an adjacent anchor farther than
  `stack_radius + query_grow` from the conflict.
- Determinism: `inner` lists come from slot order; `sides`/`inner` are `HashMap`s consumed by lookup only; `max`
  over per-edge lists is order-independent.
- Complexity vs 45 §5a: metrization stays O(k · q) per wire per pass (one extra `stack_radius` per dyn segment
  conflict, one lookup per anchor); `SlotNesting` build is O(Σ occupancy · n_max); radii O(n_max²) per anchor.
  No cap changed (`relax_iter_cap`, `TAUT_PASS_CAP`, `STALL_ITERS`, wall clock).

### 1.6 Tests (cargo, `engine/tests/`)

- `taut_chain.rs` (i): a dyn segment crossed mid-span with adjacent anchors 3+ mm away → `tauten` adds neither;
  a dyn arc wrapping the wire's own start pad → the start pad is never in `seq`.
- `relax_stacks.rs` (ii): two wires both listed at `(A, Ccw)`, wire 1 not in wire 0's slot → slot mode gives wire 0
  radius `ρ`, the old mode gives the stacked radius; wire 0 inside wire 1 → wire 1 stacked; max over fan edges;
  membership follows the inner wire's bends; `relax_slots` end to end. `graph_capacity.rs`: `slot_nesting` reads
  the slot order and skips unknown connections.
- `sketch_funnel.rs` (iii): `funnel` ties on the right / left / a pin row, `sketch_from_crossings` with the pad as
  left or right portal vertex → bend on that side; `taut_chain.rs`: the re-added tie anchor takes the slot side and
  the arc lies on that side of the chord.
- Measured (this change alone, no cap touched): eprom 27/27, 25/25, outside 67.095 mm (was 67.034; ≤ 67.1), stall
  after 16 iterations as before; synth seed 1 n=30 30/30 in 0.007 s (was 23/30 in 22.9 s), n=100 100/100 in 0.16 s
  (was 75/100 in 26.8 s), relax 2–3 iterations per metrize call, iteration-1 metrization failures 25 → 1.

## 2. B2 design — incremental metrization (for another worker)

### 2.1 Problem (`route.rs:1420-1423`, `relax.rs:385-438`)

Every PathFinder iteration calls `metrize(&cx, &g, &routed)` with *all* routed connections: sketches rebuilt,
`relax_zs` starts every chain from `Err` and runs Gauss–Seidel over all `m` wires until no chain moves. At n=1000
the deadline strikes inside the first call (`best == None`, every connection `timeout`).

### 2.2 Design

Keep a `MetrState` across PathFinder iterations (owned by `route()` beside `neg`, `route.rs:1408`):
```rust
struct MetrState {
    wires: Vec<Wire>, sketches: Vec<Sketch>, live_of: Vec<usize>,   // wire k <-> live conn
    relaxer: Relaxer,                                               // relax.rs, see 2.3
    res: Vec<Option<Res>>,                                          // last result per wire
}
```
Per iteration, input = `routed` (all live conns with a path) and `changed` = conns searched this iteration
(`active`, `route.rs:1418`) ∪ `out.ripped` ∪ conns removed since the last call. Steps:
1. Wires whose path changed: re-sketch (`path_crossings` + `unanchored_bends` + `sketch_from_crossings`, as
   `route.rs:308-343`), rebuild their `SlotNesting` entries. Wires sharing a fan edge with a changed wire also get
   new `inner` lists (their slot index may have shifted): recompute `slot_nesting` for the whole set (O(Σ occupancy),
   cheap) and mark a wire *dirty* when any of its `inner` lists differs from before.
2. `relaxer.run(dirty ∪ changed, deadline)`: Gauss–Seidel restricted to a work set `W`; a wire leaving `W` after a
   tauten that changed its chain adds its dyn neighbours (`DynLayer.grid.query(bbox of old ∪ new chain grown by
   query_grow)`) to `W`. Stop when `W` is empty, at `relax_iter_cap` sweeps, or at the deadline. Unchanged wires keep
   their chains and dyn obstacles. Cost per iteration O(Σ_{W} k · passes · q) instead of O(m · …).
3. Self-check only the wires whose chain changed plus the ones whose items were violated last time (self-check
   rounds as `route.rs:409-452`, items of changed wires against the whole emitted set via `check_items`).
4. **First call** (`iteration == 1`, or `MetrState` empty): one complete Gauss–Seidel pass in relax order
   (`relax.rs:374-381`) over all wires *before* any convergence sweeps; if the deadline strikes afterwards,
   `timed_out` still returns the completed chains, so `best` exists (anytime promise of 45 §5a).

### 2.3 API sketch (`relax.rs`)

```rust
pub struct Relaxer { stacks: HashMap<u32, Stacks>, dyns: HashMap<u32, DynLayer>, chains: Vec<Result<Chain, TautError>>, seqs: Vec<Vec<AnchorRef>> }
impl Relaxer {
    pub fn new(wires, sketches, obs, nesting: &HashMap<u32, SlotNesting>) -> Relaxer;   // chains = Err("not relaxed")
    pub fn replace(&mut self, k, wire, sketch, nesting_k);   // path changed: drop chain/dyn, reset seq
    pub fn remove(&mut self, k);                             // conn left the graph
    pub fn run(&mut self, work: &[usize], opts, deadline, zs) -> RelaxResult;   // 2.2 step 2 (+ step 4 when all chains are Err)
}
```
`relax_slots` (B1) becomes `Relaxer::new` + `run(all)`. `zero_slack_pass` (`route.rs:1010-1233`) keeps calling the
full `metrize_zs` (≤ 4 candidates × 3, bounded) or uses a cloned `Relaxer`.

### 2.4 Invariants, determinism, expected numbers

- The work set is a `BTreeSet<usize>`; sweeps visit it in relax order; neighbour queries are sorted (`dyn_near`).
- A wire whose path did not change and whose `inner` lists did not change and whose neighbours did not move is not
  re-tautened — its chain is unchanged by construction (tauten is a pure function of its inputs).
- n=1000 (review §1.1: 0.1–0.15 ms per tauten): first complete pass ≈ 0.15 s, convergence sweeps 2–5 → iteration 1
  < 1 s; later iterations ≈ |W| tautens. Scaling target (t1000 ≤ ~m log m × t100) attainable if the search stays at
  its measured 6 µs per expansion.
- Tests: incremental == full (same chains byte-identical for the gate board), first-call anytime (deadline after the
  first pass → every wire has a result), neighbour propagation (moving one wire re-tautens its dyn neighbour only).
