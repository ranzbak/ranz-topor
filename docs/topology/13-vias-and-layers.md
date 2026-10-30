# 13 — Layer assignment, via minimisation and via placement

**Purpose.** Turn a sketch with crossings into a multi-layer topology: assign wire pieces to
layers so crossings are on different layers, with as few vias as possible, then place the
vias in continuous space.

Related: [12](12-sketch-generation.md), [25 §3 via placement](../geometry/25-relaxation-and-reflow.md#3-vias-and-steiner-points-in-continuous-space).

---

## 1. Problem variants and complexity

- **Constrained via minimisation (CVM):** topology fixed, choose layers for segments.
  - **2 layers, junction degree ≤ 3:** reduces to **planar max-cut** → polynomial
    (Pinter 1982; Chen, Kajitani & Chan 1983; Liers & Pardella give a simple ≈ O(n^1.5 log n)
    planar max-cut). This matches TopoR's claim of an exact 2-layer minimum.
  - General (more layers / higher degree): NP-complete (Naclerio, Masuda & Nakajima).
- **Unconstrained (topological) via minimisation (UVM):** topology also free.
  Hsu 1983; Marek-Sadowska 1984; Stallmann, Hughes & Liu 1990 (most variants NP-hard,
  O(kn²) for switchbox/channel).

## 2. Practical approaches

**SURF (Dayan & Dai, UCSC-CRL-93-04):**
1. Split each 2-pin branch into 5–10 *sections*; vias only at section boundaries.
2. Initial: peel a maximal non-conflicting net set per layer.
3. Steepest descent: assign the free net with the smallest cost increase, then repeatedly
   reassign whichever net improves cost most. `Cost = K_v·vias + estimated length`, with
   length estimated by **pairwise detours** between same-layer components (continuous
   conflict measure, better than yes/no crossing).
4. Each net's best assignment = shortest path in a layered graph of (layer, section point).

**TopoR (Polubasov 2010):**
- Global "расслоение" (layer separation): never increases vias; exact for 2 layers,
  heuristic beyond; random tie-breaking adds topological diversity; capped at ~50% runtime.
- "Всплытие" (segment lifting): per conductor, move its segments (and those of wires it
  crosses) to any layer to remove vias.

**Modern:** Haas (arXiv 2610.35601, 2026) models crossings, order and layer changes for a
BGA-to-BGA bus in CP-SAT and beats human via counts — exact models are practical for local
subproblems.

## 3. Recommended plan

- 2-layer MVP: build the crossing graph from the sketch, solve layer assignment as planar
  max-cut (or a simple greedy + local search first; swap in max-cut when tests exist).
- >2 layers: SURF-style section steepest descent; optionally CP-SAT (OR-Tools) for dense
  local regions.

## 4. Placing vias

At the topological level a via is "somewhere between these crossings". Placement:

1. Candidate region = triangles along the wire where the via disk fits:
   `F_via = board \ ∪ (O ⊕ D(r_via + c(O, via)))` over **all layers the via spans**, plus
   hole-to-hole clearance and keepouts.
2. Within the region, length-optimal position by Weiszfeld / force balance, then projection
   ([25 §3](../geometry/25-relaxation-and-reflow.md#3-vias-and-steiner-points-in-continuous-space)).
3. Insert the via as a new vertex in every spanned layer's CDT (incremental CDT needed),
   and as an anchor with its own stacks.

**Dense fine-pitch regions (BGA/QFN):** most triangles cannot hold a via → separate fan-out
(Luzin 2015: escape **inside out** — the number of available channels does not shrink per layer,
so a 784-ball BGA needs 7 layers instead of 10; routing outside in loses 8 channels per layer.)
pre-pass (dog-bone vias at pad-grid centres; TopoR escapes BGAs from the centre outwards).

## 5. KiCad constraints for vias

Via diameter/drill from the netclass (or `via_diameter` / `hole_size` custom rules); via
types: through, blind/buried, micro (layer pairs must be valid for the stackup); annular
ring minimum; hole-to-hole; via-in-pad policy. See
[KiCad constraints](../kicad/30-kicad-constraints.md).

## References

- R. Y. Pinter, "Optimal layer assignment for interconnect", ISCAS 1982.
- Chen, Kajitani, Chan, "A graph-theoretic via minimization algorithm for two-layer printed circuit boards", IEEE TCAS 1983.
- Liers & Pardella, simple planar max-cut: http://e-archive.informatik.uni-koeln.de/630/
- C.-P. Hsu, "Minimum-via topological routing", IEEE TCAD CAD-2(4), 1983.
- M. Marek-Sadowska, IEEE TCAD CAD-3(3), 1984. https://doi.org/10.1109/TCAD.1984.1270074
- Stallmann, Hughes, Liu, IEEE TCAD 9(9), 1990. https://doi.org/10.1109/43.59073
- Naclerio, Masuda, Nakajima, "The via minimization problem is NP-complete", IEEE Trans. Computers 1989.
- Dayan & Dai, UCSC-CRL-93-04. https://tr.soe.ucsc.edu/research/technical-reports/UCSC-CRL-93-04
- Polubasov 2010. https://www.electronics.ru/journal/article/112
- Haas, arXiv 2610.35601 (2026). https://arxiv.org/html/2610.35601
