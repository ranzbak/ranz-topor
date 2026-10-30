# 24 — Length, differential pairs, skew and arc meanders

**Purpose.** Compute and control trace length on segment+arc chains; route differential
pairs as constant-gap offset curves; compensate skew; generate arc-based meanders for
length matching.

Conventions: see [20](20-clearance-model.md#conventions-used-in-all-geometry-docs).

---

## 1. Lengths

- Segment: `|B − A|`.
- Arc: `R · Δθ`.

For an arc read from KiCad `(S, M, E)`: compute centre `C` (as KiCad does), then

    σ  = sign(cross(M − S, E − M))                 # math axes
    Δθ = (σ · (atan2(E − C) − atan2(S − C))) mod 2π
    L  = |S − C| · Δθ

Net length = sum over primitives. Integer rounding contributes `≈ ΔR·Δθ` — sub-micron.
KiCad's tuning/length rules may additionally count via height through the stackup and
pad-to-die length; verify what the `length`
DRC constraint includes in the targeted KiCad version before matching against it
([KiCad constraints](../kicad/30-kicad-constraints.md)).

## 2. Differential pairs as offset curves

Centre-to-centre spacing `s = w + gap`. Each trace is the offset of the pair's centre curve
`γ` by `±s/2`:

- offset of a segment = parallel segment shifted `s/2` along its normal;
- offset of arc `(C, R, span)` = **concentric** arc `(C, R ± s/2)` with the **same angular span**.

So a G1 segment+arc chain offsets to G1 chains with an **exactly constant gap** — the key
advantage of curved over 45° routing.

Constraints: inner radius `R − s/2 ≥ w/2` (practically `≥ ~2w`).
Routing model: treat the pair as **one wire of width `2w + gap`** in the topological router;
its stacks then hold the pair's offsets. Split into two traces only at the pad fan-out.

## 3. Skew from turns

An arc of turning angle `Δθ` makes the outer trace longer by `s·|Δθ|`, **independent of R**.
Over the whole pair:

    skew = s · Θ_net,   Θ_net = Σ signed turning angles (positive toward trace P's side)

A 90° left followed by a 90° right cancels. (A sharp/mitred corner of deflection `δ` gives
`2s·tan(δ/2) ≥ s·δ`, so curves are never worse.)

Compensation:
- prefer topologies whose turns cancel;
- add small bumps (semicircular) to the shorter trace **near** where skew accrues, rather
  than one big compensation at the end (general SI practice).

## 4. Arc meanders (serpentines)

Centreline of legs of length `H` joined by semicircles of radius `r`. Each half-period
advances `2r` along the baseline and has length `H + πr`, so

    added length per half-period = H + (π − 2)·r

Constraints:
- leg pitch: `2r − w ≥ s_meander` (often 3w–4w to limit self-coupling);
- `r ≥ R_min`;
- amplitude `H + 2r + w ≤` available channel width (from the free space beside the wire —
  i.e. the slack in the relevant cuts, see [10](../topology/10-triangulation-and-cuts.md)).

Solve for number of half-periods `N` and leg length `H` with `N·(H + (π − 2)r) = deficit`;
put the remainder into one partial bump. Tightly-coupled meanders propagate slightly faster
than their length suggests (rule of thumb: keep spacing ≥ 3–4w).

Meanders change the topology locally (they occupy area): reserve the meander region in the
sketch *before* final relaxation, or tune only after routing in the remaining free space.

## 5. Interaction with KiCad's tuning tools

KiCad 8+ has a length-tuning tool that generates meanders (including round/arc corner styles)
and stores them as generator objects. The plugin can either generate its own meanders
(§4) or leave length tuning to KiCad's tool after routing. Recommended: route minimal-length
first, report per-net length vs. `length` constraints, and leave interactive tuning to KiCad
in early versions ([feasibility study](../02-feasibility-study.md)).

## References

- TopoR description of constant-gap curved diff pairs: https://en.wikipedia.org/wiki/TopoR
