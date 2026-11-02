# 01 — Annotated bibliography

Grouped by topic. ★ = most important for this project. "No free PDF" means only abstracts /
secondary descriptions were available during research (2026-10-30).

---

## A. Rubber-band / topological routing — foundations

- **R. Cole, A. Siegel**, "River routing every which way, but loose", *FOCS 1984*, pp. 65–73.
  https://doi.org/10.1109/SFCS.1984.715902 — Detailed routing given a homotopy; introduces
  *cuts*; routable iff every cut is safe.
- ★ **C. E. Leiserson, F. M. Maley**, "Algorithms for routing and testing routability of
  planar VLSI layouts", *STOC 1985*, pp. 69–78. — Rubber-band equivalent (RBE), rubber-band
  sketch, cut flow/capacity theorems, spokes; one of the discoveries of the funnel algorithm.
- **F. M. Maley**, *Single-Layer Wire Routing and Compaction*, MIT Press 1990 (PhD MIT 1987).
  https://archive.org/details/singlelayerwirer00fmil — Homotopic routing with polygonal
  metrics; "Algorithm W" (funnel through a corridor).
- **F. M. Maley**, "Testing homotopic routability under polygonal wiring rules",
  *Algorithmica* 15, 1996. https://doi.org/10.1007/BF01942604 — O(N log N) routability test.
- **S. Gao, M. Jerrum, M. Kaufmann, K. Mehlhorn, W. Rülling, C. Storb**, "On continuous
  homotopic one layer routing", *SoCG 1988*; LNCS 333. https://doi.org/10.1007/3-540-50335-8_24
  — Euclidean (curved) case; fan-shaped forbidden regions + funnel on universal cover.

## B. SURF (UC Santa Cruz, W. W.-M. Dai's group)

- **W. W.-M. Dai, R. Kong, J. Jue, M. Sato**, "Rubber band routing and dynamic data
  representation", *ICCAD 1990*. https://ieeexplore.ieee.org/document/129838 — CDT as dynamic
  structure for incremental routability.
- ★ **W. W.-M. Dai, T. Dayan, D. Staepelaere**, "Topological routing in SURF: generating a
  rubber-band sketch", *DAC 1991*, pp. 39–44. https://doi.org/10.1145/127601.127622
- **W. W.-M. Dai, R. Kong, M. Sato**, "Routability of a rubber-band sketch", *DAC 1991*,
  pp. 45–48. https://doi.org/10.1145/127601.127623 — Spoke creation; routable iff ERBS exists.
- **D. Staepelaere, J. Jue, T. Dayan, W. W.-M. Dai**, "SURF: rubber-band routing system for
  multichip modules", *IEEE Design & Test* 10(4), 1993. https://doi.org/10.1109/54.245960
- **T. Dayan, W. W.-M. Dai**, "Force-driven constrained wiring optimization", UCSC-CRL-91-40.
  https://tr.soe.ucsc.edu/research/technical-reports/UCSC-CRL-91-40 — spring model for vias / Steiner points.
- **D. Staepelaere**, "Geometric Transformations for a Rubber-band Sketch", MS thesis
  UCSC-CRL-92-39. https://tr.soe.ucsc.edu/sites/default/files/technical-reports/UCSC-CRL-92-39.pdf
  — free PDF; ERBS → rectilinear/octilinear by plane sweep.
- **T. Dayan, W. W.-M. Dai**, "Layer Assignment for Rubber Band Routing", UCSC-CRL-93-04.
  https://tr.soe.ucsc.edu/research/technical-reports/UCSC-CRL-93-04 — sections, steepest descent, detour cost.
- ★ **T. Dayan**, "Rubber-band Based Topological Router", PhD thesis, UC Santa Cruz 1997,
  ProQuest 9738147. https://www.proquest.com/openview/eed28af8ffaedc3e6806d96f63529658/1 —
  the most complete description; regions, region splitting, net ordering. No free PDF.

## C. Curvilinear routing (closest to TopoR geometry)

- ★ **T. Hama, H. Etoh**, "Curvilinear detailed routing algorithm and its extension to
  wire-spreading and wire-fattening", *ASP-DAC 1998*.
  https://www.cecs.uci.edu/~papers/compendium94-03/papers/1998/aspdac98/pdffiles/06c_3.pdf —
  free PDF; lazy fan-shaped forbidden regions on a CDT + funnel; spreading/fattening.
- **T. Kohira, A. Takahashi**, "An any-angle routing method using quasi-Newton method",
  *ASP-DAC 2012*. https://ieeexplore.ieee.org/document/6164935/ — continuous post-topology polishing incl. length matching.

## D. TopoR (Eremex)

- ★ **O. B. Polubasov**, «Трассировка в САПР TopoR – взгляд изнутри», *Электроника НТБ*
  2010 №6. https://www.electronics.ru/journal/article/112 — most detailed public algorithm
  description (initial routing, cost, rip-up, layer separation, Pareto variants).
- **S. Yu. Luzin, O. B. Polubasov**, «Топологическая трассировка: реальность или миф?»,
  *Chip News* 2002 №5. http://chipnews.ru/html.cgi/arhiv/02_05/9.htm — definitions; metrization
  as the main difficulty; "quasi-triangulation".
- **S. Yu. Luzin, S. Popov, Yu. Popov**, «Гибкая топологическая трассировка в произвольных
  направлениях», *Электроника НТБ* 2013 №1. https://www.electronics.ru/journal/article/3556 —
  conflicts, parallel length matching, BGA escape.
- **EDN**, "Speed and improve PCB routing", 2013. https://www.edn.com/speed-and-improve-pcb-routing/
  — flexible routing, Steiner 120° rule, vendor benchmark claims.
- Wikipedia: https://en.wikipedia.org/wiki/TopoR
- ★ **S. Yu. Luzin**, "Specifics and Advantages of Flexible Topological Any-Angle Routing", Nürnberg
  2015 (10 pp.). https://www.rodina-ru.com/files/eremex/topor/en/Nurnberg.pdf (site certificate
  expired 2026-10) — English; clinch detection rules, local via minimisation on via pairs, double-
  crossing elimination, concurrent delay equalisation, inside-out BGA escape, glass-weave argument.
- Books (not seen, UNVERIFIED contents): Luzin, Lyachek, Petrosyan, Polubasov, *Models and
  Algorithms for Automated Design of Electronic and Computer Equipment* (BHV-Petersburg 2010,
  ISBN 978-5-9775-0576-5); Luzin, Lyachek, Polubasov, *Автоматизация проектирования печатных
  плат. Система топологической трассировки TopoR* (SPbGETU LETI 2005).
- No patents found for TopoR's method (Rospatent not searched directly).
- Further TopoR papers listed on the Wikipedia page (2026-11-02; titles as given there, venues and contents
  UNVERIFIED, not seen):
  - **S. Yu. Luzin, O. B. Polubasov** (1997), on flexible routing ("FreeStyle Route").
  - ★ **O. B. Polubasov** (2001), "Global minimization of the number of interlayer junctions" — global via
    minimisation; basis of TopoR's layer separation.
  - **S. Yu. Luzin, G. S. Petrosyan, O. B. Polubasov** (2008, 2009), BGA routing problems — inside-out escape.
  - ★ **O. B. Polubasov** (2011), "Routing Concepts of a Topological Router CAD System" — probably the most
    complete algorithm description after Polubasov 2010.
  - **A. A. Lysenko, S. Yu. Luzin, O. B. Polubasov** (2011), "Synchronizing Delays in Signal Transmission
    Lines" — length matching.
  - **Karabran, Zyrin** (2013), reducing parasitic coupling between conductors (crosstalk).
- Page claims worth keeping (Wikipedia, partly "citation needed"): routes all connections even when that
  violates design constraints and corrects violations later; necking near narrow pads and in bottlenecks;
  minimum and desired clearance per net; several layout variants optimised simultaneously, the worst
  (length, vias) dropped; lines-only or lines + arcs mode; dedicated BGA strategy.

## E. Shortest paths (computational geometry)

- **D. T. Lee, F. P. Preparata**, *Networks* 14(3), 1984. https://doi.org/10.1002/net.3230140304 — funnel algorithm.
- ★ **J. Hershberger, J. Snoeyink**, "Computing minimum length paths of a given homotopy
  class", *CGTA* 4(2), 1994. https://doi.org/10.1016/0925-7721(94)90010-8
- **A. Efrat, S. Kobourov, A. Lubiw**, "Computing homotopic shortest paths efficiently",
  *CGTA* 35(3), 2006. https://arxiv.org/abs/cs/0204050
- **S. Bespamyatnikh**, "Computing homotopic shortest paths in the plane", *J. Algorithms* 49(2), 2003.
- **S. Cabello, Y. Liu, A. Mantler, J. Snoeyink**, "Testing homotopy for paths in the plane", *DCG* 31(1), 2004.
- **D. Z. Chen, H. Wang**, "Computing shortest paths amid pseudodisks", *SODA 2011*.
- **V. Polishchuk, J. Mitchell**, "Thick non-crossing paths and minimum-cost flows in polygonal domains", *SoCG 2007*. https://doi.org/10.1145/1247069.1247079
- **J. Erickson**, lecture notes. https://jeffe.cs.illinois.edu/teaching/comptop/2023/notes/05-shortest-homotopic.html
- **M. Kallmann**, "Dynamic and robust local clearance triangulations", *ACM TOG* 2014. https://mkallmann.github.io/papers/14-tog-lct.pdf

## F. Via minimisation / layer assignment

- **C.-P. Hsu**, "Minimum-via topological routing", *IEEE TCAD* CAD-2(4), 1983.
- **M. Marek-Sadowska**, *IEEE TCAD* CAD-3(3), 1984. https://doi.org/10.1109/TCAD.1984.1270074
- **M. Stallmann, T. Hughes, W. Liu**, "Unconstrained via minimization for topological multilayer routing", *IEEE TCAD* 9(9), 1990. https://doi.org/10.1109/43.59073
- **N. Naclerio, S. Masuda, K. Nakajima**, "The via minimization problem is NP-complete". https://ieeexplore.ieee.org/document/42135/
- **R. Y. Pinter**, "Optimal layer assignment for interconnect", ISCAS 1982.
- **F. Liers, G. Pardella**, simple planar max-cut. http://e-archive.informatik.uni-koeln.de/630/

## G. Rip-up and negotiation

- **D. Harabor, A. Grastien**, "An Optimal Any-Angle Pathfinding Algorithm" (Anya), *ICAPS 2013*.
  https://cdn.aaai.org/ojs/13609/13609-40-17127-1-2-20201228.pdf — optimal, online any-angle search
  on grids, expanding intervals of states instead of points; no preprocessing.
- **M. Cui, D. Harabor, A. Grastien**, "Compromise-free Pathfinding on a Navigation Mesh"
  (Polyanya), *IJCAI 2017* (details UNVERIFIED) — Anya generalised to convex-polygon navigation
  meshes, i.e. directly to our CDT: exact Euclidean shortest-path length as an **admissible A\*
  heuristic** in the M2 search (point agent; clearance, order and capacity stay in the topology).
- **A. Nash, S. Koenig**, "Any-Angle Path Planning", *AI Magazine* 34(4), 2013.
  https://idm-lab.org/bib/abstracts/papers/aimag13.pdf — overview of Theta*, Lazy Theta*, Field D*,
  A* with post-smoothing: A* variants that propagate along graph edges but allow shortcuts via
  line-of-sight (parent pointers). For us: cost/length estimate inside the M2 topological search
  ([12 §6](topology/12-sketch-generation.md#6-recommended-plan-for-this-plugin)); not a replacement
  for the clearance-aware curved funnel.

- **L. McMurchie, C. Ebeling**, "PathFinder: a negotiation-based performance-driven router for FPGAs", *FPGA 1995*.
- **A. Haas**, "Whole-Route Planning of a Memory Bus Between Two BGAs…", arXiv 2610.35601 (2026). https://arxiv.org/html/2610.35601 — CP-SAT crossing/layer model; negotiated congestion prices.

## H. Any-angle packaging (RDL) routing

- **Chung, Chuang, Chang**, "Any-Angle Routing for Redistribution Layers in 2.5D IC Packages", *DAC 2023*. https://doi.org/10.1109/DAC56929.2023.10247899
- **Chuang et al.**, "RDL routing with dynamic via insertion under irregular via structures", *DAC 2024*. https://doi.org/10.1145/3649329.3657371
- "RDL routing for InFO WLCSP", *ICCAD 2016*. https://doi.org/10.1145/2966986.2967070 (authors UNVERIFIED)

## I. Curves

- **K. M. Bolton**, "Biarc curves", *Computer-Aided Design* 7(2), 1975.
- **D. J. Walton, D. S. Meek**, arc-spline approximation of Bézier spirals, *J. Comput. Appl. Math.* 1996.
- Tutorial: https://raw.org/math/computer-graphics/approximating-bezier-curves-with-circular-arcs/

## J. Open-source implementations

- ★ **Topola** (Rust, MIT/Apache-2.0): https://codeberg.org/mikolaj/topola , https://topola.dev/
- **gEDA PCB toporouter** (C, `src/toporouter.c`): https://github.com/bert/pcb/wiki/Autorouters:-gEDA-pcb-Toporouter
  - Anthony Blake's toporouter pages (archived 2009–2011; GPL-2.0-or-later code): main page
    https://web.archive.org/web/20110227055406/http://anthonix.resnet.scms.waikato.ac.nz/toporouter/ and the detour
    optimisation page http://web.archive.org/web/20100914113959/http://anthonix.resnet.scms.waikato.ac.nz:80/toporouter/detour.html
    (detour optimisation by rip-up, deterministic variants, multiple traces on constraint edges; noted limit: poor results
    with few layers and dense constraints).
- **Salewski Ruby-PCB-Router**: https://github.com/StefanSalewski/Ruby-PCB-Router , http://ssalewski.de/Router.html.en ; Nim port RBR: https://github.com/StefanSalewski/RBR
- **Woody `pcb_topo`**: https://github.com/jeffmhopkins/Woody/issues/47
- **KiCadRoutingTools**: https://github.com/drandyhaas/KiCadRoutingTools
- **Freerouting** (shape-based, not topological): https://github.com/freerouting/freerouting
- **kicad-round-tracks** (corner rounding): https://github.com/mitxela/kicad-round-tracks
- Altium Situs (commercial, topological global + geometric detail): https://www.altium.com/documentation/altium-designer/pcb/routing/situs-topological-autorouter

## K. Benchmarks and datasets

- **T. Lu et al.**, "OmniRouting: A Semantic-Coupled Multimodal Benchmark for Constraint-Aware
  Spatial Reasoning in PCB Routing", arXiv 2608.04434 (Aug 2026), CC BY-NC-ND 4.0.
  https://arxiv.org/abs/2608.04434 — 1,681 industrial schematic-coupled PCB designs (placements,
  netlists, stackups, routing constraints); finds current LLM/LMM routing weak on path planning,
  design rules and connectivity (supports an algorithmic engine). Data/code promised, not yet
  released (2026-10-30); file format UNVERIFIED. Candidate extra benchmark corpus beyond the
  user's boards ([benchmarks](benchmarks/README.md)).

## L. Constraint-aware PCB routers (non-topological)

- ★ **W. Fang, L. Guo, J. Lin, S. Xiong, H. He, J. Xu, J. Chen**, "Obstacle-Aware Length-Matching
  Routing for Any-Direction Traces in Printed Circuit Board", arXiv 2407.19195 (2024; preliminary
  version DAC 2024). https://arxiv.org/abs/2407.19195 — two stages: non-overlapping region per trace,
  then obstacle-aware meandering by dynamic programming (DP O(n²) in the discretisation, pattern
  restore O(n), obstacle queries via segment tree O(log² N)) preserving the original route;
  **MSDTW** (multi-scale dynamic time warping) merges a diff pair into a median trace, meanders it,
  and splits it back. Any-direction segments, no arcs. Basis for our M5 length matching with arc
  meanders ([24 §4](geometry/24-length-diffpairs-tuning.md#4-arc-meanders-serpentines)).

- **T.-C. Lin, D. Merrill, Y.-Y. Wu, C. Holtz, C.-K. Cheng**, "A Unified Printed Circuit Board Routing
  Algorithm With Complicated Constraints and Differential Pairs", *ASP-DAC 2021*, pp. 170–175.
  https://doi.org/10.1145/3394885.3431568 — full-board router (escape + area in one) handling
  coupled diff-pair routing and pad-entry constraints; A*-based on a grid (UNVERIFIED, inferred from
  citing work; full text not accessible). Reference for pad-entry/exit stubs and diff pairs (M4/M5).
- **M. M. Ozdal, M. D. F. Wong**, "A Length-Matching Routing Algorithm for High-Performance Printed
  Circuit Boards", *IEEE TCAD* 2006. https://dl.acm.org/doi/10.1109/TCAD.2006.882584 — classic
  length-matching routing; reference for matched groups / DDR ([12 §3.2](topology/12-sketch-generation.md#32-matched-groups-ddr-byte-lanes-buses)).
- "A Novel Global Routing Algorithm for Printed Circuit Boards Based on Triangular Grid" (2023).
  https://www.researchgate.net/publication/376366184 — triangulation-based PCB global routing,
  close to our routing field (details UNVERIFIED).

## M. Free-space maps with clearance (passages with a width)

The board as a graph of free space whose edges carry a passage width; the basis of the planned exact
capacity model (plan "TopoR-style core", 2026-11-02).

- ★ **M. Kallmann**, "Shortest Paths with Arbitrary Clearance from Navigation Meshes", *SCA 2010* —
  Local Clearance Triangulation (LCT): refine a CDT until per-edge clearance tests are exact; paths of
  any width checked on edges only. The TOG 2014 follow-up (section E) adds dynamic insert/remove.
- **D. Demyen, M. Buro**, "Efficient Triangulation-Based Pathfinding", *AAAI 2006* (TA\*/TRA\*) —
  search with a width over triangles; an abstract graph of triangle corridors on top.
- ★ **R. Geraerts, M. Overmars**, "The Corridor Map Method: a General Framework for Real-Time High-Quality
  Path Planning", *Computer Animation and Virtual Worlds* 18(2), 2007 (venue UNVERIFIED) — corridors
  with a clearance radius, built on the generalised Voronoi diagram (medial axis).
- **R. Geraerts**, "Planning Short Paths with Clearance using Explicit Corridors", *ICRA 2010* —
  explicit corridor map; nodes = junctions of free space, edges = corridors with their narrowest width
  (our "corridor graph": capacity ≈ bottleneck width / (width + clearance)).
- Tile-based gridless routers (rectangular free-space tiles):
  - **J. K. Ousterhout**, "Corner Stitching: A Data-Structuring Technique for VLSI Layout Tools",
    *IEEE TCAD* 3(1), 1984.
  - **A. Margarino, A. Romano, A. De Gloria, F. Curatelli, P. Antognetti**, "A Tile-Expansion Router",
    *IEEE TCAD* 6(4), 1987 (authors/issue UNVERIFIED).
  - **J. Dion, L. M. Monier**, "Contour: A Tile-based Gridless Router", DEC WRL Research Report 95/3,
    1995.
  - **J. Cong, J. Fang, K.-Y. Khoo**, "DUNE — A Multilayer Gridless Routing System", *IEEE TCAD* 20(5),
    2001.

## N. Multicommodity flow and global routing (planning without geometry)

Routing as integer multicommodity flow on a capacitated 3-D graph (area × layers × capacity); solved
fractionally, then rounded and repaired.

- **P. Raghavan, C. D. Thompson**, "Randomized rounding: a technique for provably good algorithms and
  algorithmic proofs", *Combinatorica* 7(4), 1987 — rounding a fractional flow, overflow bounds.
- ★ **C. Albrecht**, "Global routing by new approximation algorithms for multicommodity flow", *IEEE TCAD*
  20(5), 2001.
- ★ **D. Müller, K. Radke, J. Vygen**, "Faster min–max resource sharing in theory and practice",
  *Mathematical Programming Computation* 3(1), 2011 — the global-routing core of BonnRoute; min-max
  objective = no commodity starved.
- **M. Gester, D. Müller, T. Nieberg, C. Panten, C. Schulte, J. Vygen**, "BonnRoute: Algorithms and data
  structures for fast and good VLSI routing", *DAC 2012*; journal version *ACM TODAES* 18(2), 2013.
- **F. Kelly**, proportional fairness (max Σ w·log utility), *European Transactions on
  Telecommunications* 8(1), 1997 — background for the per-tier weighting (title UNVERIFIED).
- **OrthoRoute** (GPU, open source, 2025): PathFinder on a fixed 3-D Manhattan lattice in CUDA, a KiCad
  autorouter by Brian Benchoff (URL/details UNVERIFIED; search "OrthoRoute KiCad"). Reference for GPU
  parallel negotiation; limited to orthogonal routing.

## O. Octilinear drawing and bend minimisation

For the 45° drawing step (few bends, 90° only when needed, bend placement as an LP).

- ★ **R. Tamassia**, "On embedding a graph in the grid with the minimum number of bends", *SIAM J.
  Computing* 16(3), 1987 — bend minimisation as min-cost flow.
- **M. Nöllenburg, A. Wolff**, "Drawing and Labeling High-Quality Metro Maps by Mixed-Integer
  Programming", *IEEE TVCG* 17(5), 2011 — octilinear layout with bend penalties.
- Maley's polygonal wiring rules (section A) cover the routability theorem for 45° (octagonal) metrics.

## P. Machine learning for routing (survey 2026-11-02, from memory)

No open model known to route real boards well. For reference only:

- **DeepPCB** (InstaDeep) and **Quilter**: commercial, reinforcement learning, cloud only; no open weights.
- **Y. He, F. S. Bao**, "Circuit Routing Using Monte Carlo Tree Search and Deep Neural Networks",
  arXiv 2006.13607, 2020. https://arxiv.org/abs/2006.13607 — small grids.
- **H. Liao et al.**, deep reinforcement learning for global routing (chips), 2019–2020 (details
  UNVERIFIED).
- **RouteNet / CircuitNet**: congestion prediction for chip design (open dataset); prediction, not routing.

## Reading order for implementers

1. Hama & Etoh 1998 (free; geometry stage almost directly implementable).
2. Erickson's notes (funnel, homotopy) → Hershberger & Snoeyink.
3. Staepelaere 1992 thesis + UCSC-CRL-93-04 (free; SURF internals).
4. Polubasov 2010 (TopoR's search and optimisation strategy).
5. Topola and gEDA toporouter source code.
6. Dayan 1997 thesis (purchase) when implementing regions and net ordering.

For the TopoR-style core (plan 2026-11-02): Kallmann 2010/2014 (LCT) → Geraerts 2010 (corridors) →
Müller–Radke–Vygen 2011 + BonnRoute (flow) → Tamassia 1987 (bends) → Polubasov 2010/2011 and Luzin 2015
(TopoR internals). Downloaded copies: `docs/literature/` (git-ignored, see its `INDEX.md`).
