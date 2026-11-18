#!/usr/bin/env python3
"""exist-term mutants: `exist_term_mut.py list` / `exist_term_mut.py <name> apply|revert <tree>`.
One mutant per changed rule of the round (docs/dev/design/exist-term.md); each must fail
engine/tests/topo_exist_term.rs or the core.rs unit test exist_end_maps_track_ends_inside_own_copper_to_that_copper."""
import sys
from pathlib import Path

C = "engine/src/topo/core.rs"
M = {
    "dang_retain_off": (C, "                    src.retain(|e| e.v == c.a);\n", ""),
    "pre_join_off": ("engine/src/topo/tree.rs", "    for &(a, b) in &st.pre_join {", "    for &(a, b) in st.pre_join.iter().filter(|_| false) {"),
    "turn_cost_off": ("engine/src/topo/init.rs", "        self.turn_cost(d.v, toward) + self.junction_cost(d, toward)", "        self.junction_cost(d, toward)"),
    "lead_draw_off": ("engine/src/topo/draw.rs", "            let Some((q, _)) = k.lead else { continue };", "            let Some((q, _)) = k.lead.filter(|_| false) else { continue };"),
    "lead_off": (C, "lead: dir.map(|d| (d, len))", "lead: dir.map(|d| (d, len)).filter(|_| false)"),
    "kept_end_not_free": ("engine/src/topo/exact.rs", " && !self.is_kept_end(i);", ";"),
    "pad_map_off": (C, "m.pads.iter().position(|p| p.net == net && p.layers", "m.pads.iter().position(|p| false && p.net == net && p.layers"),
    "via_map_off": (C, "m.vias.iter().position(|v| v.net == net && crate::model::via_layers(&v.layers, &all).contains(&layer) && (v.at", "m.vias.iter().position(|v| false && v.net == net && crate::model::via_layers(&v.layers, &all).contains(&layer) && (v.at"),
    "kept_tracks_off": (C, "copper_shapes: true, kept_tracks: true, ..d }", "copper_shapes: true, kept_tracks: false, ..d }"),
    "dead_ent_off": ("engine/src/topo/draw.rs", "                self.add_ent(Ent { core: Core::Poly(ob.poly.clone()), grow: 0.0, mask, net: ob.net, clear: ob.clear, fixed: true, live: false, shape: false });\n", ""),
    "planned_end_off": (C, "            push(&mut out, board::Extra::KeptEnd { net: c.net, layer: k.layer, at, w, s, lead: None }, c.net, at);\n", ""),
}


def main():
    if sys.argv[1] == "check":
        for k, (f, old, _) in M.items():
            print(k, (Path(sys.argv[2]) / f).read_text().count(old))
        return
    if sys.argv[1] == "list":
        print(" ".join(M))
        return
    name, op, tree = sys.argv[1], sys.argv[2], Path(sys.argv[3])
    f, old, new = M[name]
    p = tree / f
    s = p.read_text()
    keep = p.with_name(p.name + ".orig")
    if op == "revert":
        p.write_text(keep.read_text())
        keep.unlink()
        return
    if s.count(old) != 1:
        sys.exit(f"{name}: {op}: pattern found {s.count(old)} times in {f}")
    keep.write_text(s)
    p.write_text(s.replace(old, new))


main()
