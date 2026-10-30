# 25 — Relaxation ("rubber-banding"), via placement and re-flow

**Purpose.** Turn a topological sketch into final geometry by iterated tightening, place vias
and Steiner points in continuous space, spread wires into spare room, and re-flow when
components move — all *without changing topology*.

Conventions: see [20](20-clearance-model.md#conventions-used-in-all-geometry-docs).

---

## 1. State

- **Topological state (discrete, frozen during relaxation):** per wire, its anchor sequence
  with σ per anchor; per anchor side, the stack order.
- **Geometric state (continuous):** stack radii, tangent chains, positions of vias and
  Steiner points.

## 2. Relaxation loop

One iteration:

1. For each anchor side, compute the stack radii `R_k` from the current stack membership
   ([21 §5](21-tangent-arc-paths.md#5-many-wires-around-one-obstacle-stacks-and-spokes)).
2. For each wire, recompute its taut chain with those radii ([21 §4](21-tangent-arc-paths.md#4-pulling-a-wire-taut-along-its-sleeve)).
3. Drop anchors whose slack exceeds ε; add anchors where a chain penetrates a non-member
   obstacle core by more than ε.
4. Update stack membership (wires taut at, or within reach of, each anchor).
5. Move vias and Steiner points one step toward force balance (§3).

Stop when membership no longer changes and the largest point displacement is below a
tolerance.

- **Convergence:** every step does not increase total length while staying feasible; length
  is bounded below → converges to a local optimum (monotone descent).
- **Oscillation** happens if geometry is allowed to *reorder* wires (two wires swapping the
  inner track each iteration). Fix: wire order is part of the topology; change it only by an
  explicit topological move with hysteresis. Anchor add/drop flip-flops are damped by the
  ε-band above.
- **Gauss–Seidel alternative:** route each wire as a shortest homotopic path treating all
  other wires' current geometry as obstacles, iterate. Simpler, but order-dependent and slower;
  the stack formulation solves all wires on a shared anchor at once.

## 3. Vias and Steiner points in continuous space

**Feasible region for a via centre:**

    F_via = board \ ∪ ( O ⊕ D(r_via + c(O, via)) )

over other-net copper on **every layer the via spans** (through via: all copper layers),
plus hole-to-hole clearance to other drills, keepouts, and same-net pads if via-in-pad is
not allowed. Also respect annular ring and via size from the netclass.

**Length-optimal position.** The gradient of total length with respect to an endpoint `p` of
taut paths is minus the sum of unit directions of the attached segments:

    ∇L(p) = −Σ_i u_i         (u_i = unit vector from p along attached segment i)

Optimum = force balance `Σ u_i = 0` (Fermat–Weber). Heuristic Weiszfeld iteration (exact only for point anchors — with disks the tangent points move) toward the last taut
anchors `a_i`:

    p ← Σ (a_i / |p − a_i|) / Σ (1 / |p − a_i|)

then project into `F_via`. With exactly two attached segments, every point on the straight
line between the two anchors is optimal — use the freedom for secondary goals: maximise
minimum clearance (move toward the channel's medial axis), keep near the pad (fan-out), or
align with neighbours.

For a Steiner point joining 3 branches, the same condition yields the classic 120° junction.

**Fan-out (dog-bone) feasibility** for BGA pitch `p`, pad diameter `d_p`, via diameter `d_v`,
via at the centre of four pads:

    p/√2 − d_p/2 − d_v/2 ≥ c
    e.g. p = 0.8, d_p = 0.4, c = 0.1  →  d_v ≤ 0.531 mm

## 4. Using spare room (don't leave everything at minimum clearance)

Tight wires at minimum clearance hurt yield and crosstalk. Options that keep topology and G1:

- **Global spacing scale** `λ ≥ 1` multiplying all wire–wire clearances; binary-search the
  largest `λ` for which every cut capacity check ([10](../topology/10-triangulation-and-cuts.md))
  still passes.
- **Local**: in each channel (cut) with spare width `S` and `n` wires, add `S/(n+1)` to each gap.

Both only change stack radii. Note: spreading increases length slightly — make it a user
option ("minimum length" vs "balanced spacing").

## 5. Re-flow when a component moves (interactive / incremental mode)

Keep the topology; a moving pad cannot pass through wires — it pushes them:

1. Move in small steps.
2. Per step, find wires intersecting the pad's swept inflated hull.
3. Insert the pad's anchor into those wires with σ = the side they were on.
4. Run relaxation.

Triangulation changes during motion are handled kinetically (edge flips update the crossing
sequences) or by re-triangulating and re-deriving sleeves from the old geometry. This mirrors
TopoR's behaviour of re-shaping wires immediately when components, vias or junctions move.

In a KiCad plugin this is realistic as a batch "re-flow selected nets after I moved parts"
command, not a live drag (the plugin cannot hook KiCad's interactive move tool; see
[KiCad constraints](../kicad/30-kicad-constraints.md)).

## References

- Weiszfeld algorithm / Fermat–Weber point: standard (E. Weiszfeld, 1937).
- TopoR behaviour: https://en.wikipedia.org/wiki/TopoR ; https://www.edn.com/speed-and-improve-pcb-routing/
- SURF rubber-band sketch and spokes: see [01 bibliography](../01-bibliography.md).
