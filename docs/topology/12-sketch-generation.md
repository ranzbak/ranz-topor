# 12 — Sketch generation: net decomposition, ordering, rip-up and optimisation

**Purpose.** Produce a complete, routable topology (the *rubber-band sketch*) for all nets:
which edges each wire crosses, in what order, on which layer — before any coordinates.

Related: [10](10-triangulation-and-cuts.md), [13 vias and layers](13-vias-and-layers.md).

---

## 1. Net decomposition

- Split each multi-pin net into 2-pin connections with a minimum spanning tree (MST) over
  pad positions (rectilinear or Euclidean distance).
- Later: Steiner points. TopoR adds branch points where the angle between branches is
  below 120° and moves them to equilibrium by forces from nearby wires; four-terminal subnets
  are checked and rebuilt if not minimal. SURF moves Steiner points and vias with a spring
  model (Dayan & Dai, UCSC-CRL-91-40).
- MVP: connect each pad to the nearest pad of the partial tree (no T-junction onto a wire).

## 2. Initial routing (sequential)

TopoR (Polubasov 2010):
- Order: **wider conductors first, then shorter** among equal width.
- Per conductor: shortest triangulation path that respects clearance (capacity), then
  among those the one with fewest crossings. At most two conductors may meet at a point.
- No layer assignment yet — crossings are allowed and become vias later
  ([13](13-vias-and-layers.md)).

Dayan's thesis introduces a net-ordering algorithm that beats "shortest first"; SURF's
layer-assignment report orders branches by *pairwise detour* and moves "hot" branches (those
that split a region into disconnected parts) to the end, with a lemma guaranteeing the planar
router can then complete.

## 3. Removing order dependence

1. **Negotiated congestion (PathFinder, McMurchie & Ebeling, FPGA 1995):**
   `cost = (base + history) × present_overuse`; reroute all nets each iteration; history
   grows on persistently overused edges. Drives conflicts out without depending on order.
   Used by Woody and (as "negotiated congestion prices") by Haas's bus planner.
2. **Double-crossing removal** (TopoR): two equal-width wires crossing twice can swap their
   segments between the crossings → remove both crossings. Repeat until none remain.
3. **Pin swapping** between functionally equivalent pins (only if the schematic allows —
   for KiCad, generally *off* unless the user enables it; back-annotation needed).

### 3.1 Net priority (signal importance)

PCB practice: route the **highest-speed / most critical** signals first (RF, high-speed diff
pairs, clocks), then descend in frequency; slow GPIO last. **Wide power traces** are the
exception: they don't need the shortest path, but they need *room* (width), so they are
placed early, right after the critical tier — consistent with TopoR's "wider first" rule.
Suggested tiers:

| Tier | Nets | What priority buys |
|---|---|---|
| 1 | RF, high-speed diff pairs, clocks, length/skew-constrained nets | shortest path, fewest vias, best layer |
| 2 | wide power / high-current traces (not served by a plane) | guaranteed corridor width |
| 3 | medium-speed buses | normal |
| 4 | slow signals, GPIO, LEDs | yield space; may detour |

In a topological router, priority acts at several levels, not only routing order:

- **Order** — tier 1 first in the initial sequential pass (order dependence is real, see §2).
- **Cost weight** — in negotiated congestion (§3) multiply each net's length/via cost by its
  priority weight, so congestion pushes *low*-priority nets aside instead of tier-1 nets.
  This keeps priority meaningful even after order dependence is removed.
- **Stack position** — around every anchor the innermost wire is the shortest
  ([21 §5](../geometry/21-tangent-arc-paths.md#5-many-wires-around-one-obstacle-stacks-and-spokes));
  when the topology leaves the order free, give the inner positions to higher priority.
- **Vias and layers** — higher via cost for tier 1 (each via is an impedance discontinuity);
  prefer layers adjacent to an unbroken reference plane; forbid crossing plane splits.
- **Rip-up protection** — tier-1 nets are ripped up last (or only with user consent).
- **Spreading** — extra spacing from §4 of [25](../geometry/25-relaxation-and-reflow.md#4-using-spare-room-dont-leave-everything-at-minimum-clearance) goes first
  to gaps next to tier-1 nets (crosstalk).

**Where priorities come from in KiCad.** KiCad has no routing-priority attribute. The netclass
`priority` field exists but only resolves which netclass wins when a net is in several (rule
precedence), so it is not a routing priority. Feasible sources:
1. a plugin setting table: netclass (or net pattern) → tier, stored with the project;
2. inference as default: diff pairs, nets with `length`/`skew` rules, impedance-named netclasses,
   RF/CLK name patterns → tier 1; netclasses with track width above the default → tier 2;
3. a naming convention for netclasses (e.g. a `HS_` / `PWR_` prefix) as a user-visible override.

### 3.2 Matched groups (DDR byte lanes, buses)

DDR and similar parallel interfaces need **groups** of nets that are routed alike:

| Group (DDR example) | Same layer | Length / skew | Reference |
|---|---|---|---|
| Byte lane: DQ0–7 + DM0 + DQS0 pair | yes, whole lane on one layer | DQ/DM within ± tolerance of DQS | DQS pair |
| Address / command / control | per group, fly-by | matched to CK within tolerance | CK pair |
| Clock CK, strobes DQS | — | intra-pair skew (diff pair) | — |

Per group the router needs these **labels**: group id, layer lock (one named layer, or "same
layer as the rest of the group"), target length or reference net, tolerance, equal via count,
and priority tier (always tier 1, [§3.1](#31-net-priority-signal-importance)).

**Routing order:** matched groups are high-speed by definition and are routed **first** —
whole groups at once (with their reference nets: DQS, CK), corridor and meander space
reserved, before any other net is placed. Within tier 1 the order is: matched groups →
other diff pairs / clocks / RF → remaining tier-1 nets. Rip-up never breaks a group apart:
a group is ripped and rerouted as a unit.

**Where the labels come from in KiCad**
- KiCad custom rules already express much of this, conditioned on netclass or net-name patterns:
  `length` (min/opt/max), `skew` (max, between nets matched by the same rule), `via_count`
  (max), and `disallow track` on all other layers to force a layer — so the group definition
  can live in `.kicad_dru`, where KiCad's DRC also checks it (guide rails,
  [KiCad constraints §4.1](../kicad/30-kicad-constraints.md#41-kicad-drc-as-guide-rails-not-only-a-final-gate)).
  Exact expression syntax for group skew per rule is UNVERIFIED in detail.
- Netclass per byte lane (e.g. `DDR_BL0`) as the group id — readable through the API.
- Plugin settings table for what rules don't capture (reference net, tolerance in ps vs mm).
- Inference as a suggestion only: net names `DQ[0-7]`, `DM0`, `DQS0_P/N` → lane 0, etc.

**How it changes the algorithm**
1. **Layer assignment** ([13](13-vias-and-layers.md)): a group is assigned as one unit; its
   wires may change layer only at the escape vias at both ends, with equal via count.
2. **Bundle topology**: route the group as a bundle through one corridor with a fixed crossing
   order (no wire of another net between them) — essentially one wide "wire", like a diff pair.
3. **Turn skew inside a bundle**: when a bundle of pitch `p` turns by `Θ`, wire `k` positions
   further out is longer by `k·p·Θ` — the diff-pair rule of
   [24 §3](../geometry/24-length-diffpairs-tuning.md#3-skew-from-turns) generalised. Opposite
   turns cancel; topologies with net turning near zero need least tuning.
4. **Length matching**: the minimal-length router makes the *shortest* wire set the pace;
   the others get arc meanders ([24 §4](../geometry/24-length-diffpairs-tuning.md#4-arc-meanders-serpentines))
   in space **reserved before final relaxation**, otherwise the taut neighbours leave no room.
5. **Verification**: KiCad DRC `length` / `skew` / `via_count` violations map back to groups.
6. **Swap groups (free ordering within a lane)**: in DDR3 the data bits of one byte lane
   (DQ0–7) may be connected in whatever order is convenient — the lane is a bundle of 8 wires
   whose bit order does not matter (DQS pair and DM stay fixed). Labelled per lane as a *swap
   group* (user marks it; inference from names only as a suggestion). The router then picks the
   order: sort the pins at both ends by their position across the bundle and pair them in that
   order, which gives a **crossing-free bundle in O(n log n)** — no search over permutations,
   fewer vias, and naturally equal-length neighbours. Consequences: the pad nets change, so the
   plugin applies the swap on the board and outputs a swap list; the schematic must follow
   (KiCad's update-schematic-from-PCB back-annotation of net changes is UNVERIFIED — fallback:
   report the swap list for the user to apply). Some controllers/training modes restrict which
   bits may move (e.g. a bit used for write-levelling feedback) — UNVERIFIED per device, so
   the user can pin individual bits.
   The same mechanism covers **any bus whose bit order is free** (FPGA/MCU GPIO banks, LED
   matrix lines, connector pins the designer leaves open): route the bus as one bundle — a single
   wide wire through the topology, like a diff pair — and resolve which bit lands on which pin
   only at the two ends.

Feasibility: high effort (same class as length matching in the feasibility study), but
the theory is straightforward. The hard part is reserving meander space in dense
regions. Commercial routers handle DDR mostly with dedicated bus/length-tuning tools after
topology, and TopoR tunes lengths for a whole group in parallel (Luzin et al. 2013).

## 4. Rip-up and reroute / optimisation (TopoR's scheme)

- Path cost `S = Σ kᵢ·xᵢ`: length, vias, DRC violations visible; ~100 hidden terms
  (crossings, wedging between another wire's segments, joins to planar pads or branch points…).
- Coefficients per conductor, **deliberately fluctuating** to vary the cost landscape.
- Per-conductor **reroute counter** prevents oscillation.
- Each full board topology is a node in a solution graph; each reroute an edge; steepest
  descent between nodes.
- **Multi-variant / Pareto:** combine `F = F1·sinφ + F2·cosφ` (F1 = cheap vias, F2 = expensive
  vias); keep all solutions on the convex hull of (length, vias); user picks. Random restarts
  from stored variants escape local minima.

gEDA toporouter used "ROAR" rip-up plus detour optimisation with pairwise net scoring.

## 5. Topological conflicts to detect

From TopoR (Luzin, Popov & Popov 2013): **loops, half-loops and "clinches"** detected from
triangulation edges — wires that wrap uselessly around features, or pinch between two others.
Detect by reduced crossing sequences (a loop = the sequence re-crosses the same edge set
around a vertex).

## 6. Recommended plan for this plugin

1. MST decomposition; order wide-first, then short-first.
2. Per connection: A* over the per-layer routing graph ([10 §3](10-triangulation-and-cuts.md#3-routing-graph))
   with cost = estimated length + capacity overuse penalty + crossing penalty (via cost).
3. PathFinder iterations until no capacity overuse (or iteration cap).
4. Double-crossing removal.
5. Layer assignment ([13](13-vias-and-layers.md)) → metrization → exact check → rip-up of
   offending connections with raised history costs.
6. Optional later: Pareto archive over via cost.

## References

- Polubasov, «Трассировка в САПР TopoR – взгляд изнутри», Электроника НТБ 2010 №6. https://www.electronics.ru/journal/article/112
- Luzin, Popov, Popov, «Гибкая топологическая трассировка в произвольных направлениях», Электроника НТБ 2013 №1. https://www.electronics.ru/journal/article/3556
- EDN, "Speed and improve PCB routing", 2013. https://www.edn.com/speed-and-improve-pcb-routing/
- McMurchie & Ebeling, "PathFinder: a negotiation-based performance-driven router for FPGAs", FPGA 1995.
- Dayan & Dai, "Force-driven constrained wiring optimization", UCSC-CRL-91-40. https://tr.soe.ucsc.edu/research/technical-reports/UCSC-CRL-91-40
- Dayan & Dai, "Layer Assignment for Rubber Band Routing", UCSC-CRL-93-04. https://tr.soe.ucsc.edu/research/technical-reports/UCSC-CRL-93-04
- gEDA toporouter: https://github.com/bert/pcb/wiki/Autorouters:-gEDA-pcb-Toporouter
