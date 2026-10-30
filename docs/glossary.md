# Glossary

| Term | Meaning |
|---|---|
| **Anchor / anchor circle** | A circle (centre, radius) around which wires bend: pad centre, pad hull corner, via, Steiner point. Every arc of a taut wire is centred on an anchor. See [20](geometry/20-clearance-model.md). |
| **Arc spline / biarc** | Curve made of circular arcs; a biarc is two tangent-continuous arcs matching end points and tangents. Used because KiCad tracks only support segments and arcs. |
| **CDT** | Constrained Delaunay triangulation — Delaunay triangulation forced to contain given edges (board outline, keepouts). |
| **Crossing sequence** | Ordered list of triangulation edges a wire crosses; reduced form identifies its homotopy class. |
| **Cut** | Segment between two features; flow (Σ width + spacing of crossing wires) must not exceed its capacity (length minus feature radii). |
| **CVM / UVM** | Constrained / unconstrained via minimisation: layer assignment with topology fixed / free. |
| **DRC** | Design rule check (KiCad: `kicad-cli pcb drc` or the GUI). |
| **DRCEpsilon** | KiCad tolerance (default 0.5 µm) subtracted from required clearance in DRC; do not rely on it. |
| **Disturbance** | Narrow passage caused by a nearby non-adjacent feature that triangle-edge capacity checks miss (Kallmann). |
| **ERBS** | Extended rubber-band sketch — sketch with spokes that satisfies widths/spacings (SURF). |
| **Funnel algorithm** | Linear-time shortest path through a sleeve of triangles (Lee–Preparata, Chazelle, Leiserson–Maley). |
| **G1** | Tangent-continuous curve (no corners). |
| **Homotopy (class)** | Wires are homotopic if one deforms into the other without crossing an obstacle; the class = "which side of each obstacle". |
| **LCT** | Local Clearance Triangulation (Kallmann 2014): refined CDT on which local clearance checks are exact. |
| **IPC API** | KiCad's protobuf/NNG plugin API (KiCad 9+); replaces the SWIG bindings. |
| **IU** | KiCad internal unit = 1 nm (integer). |
| **Metrization** | Converting a topology into concrete geometry (TopoR term «метризация»). |
| **Netclass** | KiCad grouping of nets sharing clearance, track width, via size, diff-pair rules. |
| **PathFinder** | Negotiated-congestion rip-up/reroute scheme (McMurchie & Ebeling 1995): cost = (base + history) × present overuse. |
| **PCM** | KiCad Plugin and Content Manager. |
| **Quasi-triangulation** | TopoR/FreeStyle Router's metrization structure: Delaunay-like with segment "vertices". |
| **RBE** | Rubber-band equivalent — shortest path homotopic to a given wire. |
| **Region** | Angular sector around a vertex between attached wires (Dayan); routing graph nodes in region formulation. |
| **Rubber-band sketch (RBS)** | Topological routing where each wire is its taut zero-width representative. |
| **σ (sigma)** | Wrap orientation: +1 = anchor on wire's left (CCW, math axes), −1 = right. |
| **Sketch** | Complete topological routing of all wires (crossing orders, layers), before geometry. |
| **Sleeve** | Sequence of triangles crossed by a wire (topologically a disk). |
| **Spoke** | Segment radiating from a feature along which wires are spaced (SURF); equals the stack radii around an anchor. |
| **Stack** | Ordered set of wires wrapping one side of an anchor, at nested radii. |
| **Steiner point** | Branch point of a multi-pin net; optimal junction angles are 120°. |
| **SWIG bindings** | Legacy `pcbnew` Python module; deprecated in KiCad 9, removal planned for 11. |
| **Theta\*** | Any-angle variant of A* search (used by Topola on its navmesh). |
| **Taut chain** | Shortest path in a class: tangent segments + arcs on anchor circles. |
| **Teardrop** | Fillet widening a trace where it enters a pad/via; native in KiCad 7+. |
| **Universal cover** | Space in which homotopy classes become ordinary paths; underlying theory of the funnel on sleeves. |
