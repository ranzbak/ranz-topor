"""Stage M1 = 6.3 "compass": one table per run, per violation class, of what the plan predicted, what the drawing
produced and what KiCad's DRC finds on the written board; per connection its detour and its vias planned vs drawn.

The engine writes the predicted side (``RouteResult.compass``, engine/src/topo/compass.rs): per class the plan's own
count and the located items (cuts, edges, via sites, crossings, traversals through foreign copper, forced paths)
with their connections. This module classifies every drawn violation (``RouteResult.violations``) against those
items, joins KiCad's DRC items by place, and builds the table. Pure Python, no pcbnew.

Why: TopoR shows the predicted criteria of every variant in its variant table (TopoR 6.3 manual p. 72: Length of the
unsmoothed connections, "usually exceeds the length of smoothed-out connections by 10 to 15 percent", Vias, Violation,
Shrinkages; p. 73: the four optimisation criteria) and checks the copper by DRC after routing (p. 183: it "routes all
connections, even where this causes constraint violations"). A drawn violation in a class the plan predicts is a fault of
that built step or of its realisation; a drawn count in a class the plan cannot predict yet ("not predicted") is a
missing method step; a drawing loss is a fault of the drawing; "KiCad only" is our checker missing what KiCad sees.
Classes: docs/dev/inflated-sketch.md §6.1 and §6.3.

Classification of one drawn violation (conn c, layer L, place p = the middle of the stretch below the rule; reach r =
max(``reach_mm``, 4 x its width, about two pitches) + half the stretch's length): the first that matches of
  via site (its class is via_site and the plan has a via of c without a site), unlayerable (a crossing of c no layer
  separates, within r), foreign copper (a traversal of c through foreign copper on L, within r), planner over-full (an
  over-full cut / edge-layer on L within r that c crosses; or one c does not cross: sub-class "nearby"), forced path
  (c was placed without a legal path), pad exit (a planned pad exit of c within r; 5.10), drawing loss at a planned
  neck (a cut on L within r that fits only necked), model gap (the other capacity model's over-full item of c on L
  within r: with ``[capacity] model = "cuts"`` the per-edge packer the drawing uses), pad exit not predicted (its class
  is pad_exit), else drawing loss (sub-class: the engine's own class). The violation rows add up to the drawn
  violations; the shrinkage row compares the plan's necked stretches with the drawn ones (a shrinkage is no violation,
  TopoR 6.3 manual p. 72). A KiCad DRC item joins the nearest drawn violation whose stretch it lies within
  ``drc_radius_mm`` of (distance to the middle minus half the stretch). O(violations x items of the connection + DRC
  items x violations).
"""
from __future__ import annotations

import json
import math
from collections import Counter, defaultdict
from pathlib import Path

MM = 1_000_000
DETOUR_FLAG = 1.5  # the user's no-detours mark (memory no-detours): a flag in the table, never a gate

ORDER = ("planner_overfull", "shrinkage", "via_site", "unlayerable", "foreign_copper", "forced_path", "pad_exit",
         "drawing_loss", "model_gap", "open", "kicad_only")
LABELS = {
    "planner_overfull": "planner: over-full",
    "shrinkage": "planner: shrinkage",
    "via_site": "planner: via site",
    "unlayerable": "planner: unlayerable crossing",
    "foreign_copper": "planner: foreign copper",
    "forced_path": "planner: forced path",
    "pad_exit": "pad exit",
    "drawing_loss": "drawing loss",
    "model_gap": "model gap",
    "open": "open (no copper)",
    "kicad_only": "KiCad only (our checker silent)",
}
# Classes that are violations in the plan's measure (their sum = the plan's predicted violations).
VIOLATION_CLASSES = ("planner_overfull", "via_site", "unlayerable", "foreign_copper", "forced_path", "pad_exit")


def _seg_dist(p, a, b) -> float:
    ax, ay = a
    bx, by = b
    px, py = p
    dx, dy = bx - ax, by - ay
    l2 = dx * dx + dy * dy
    t = 0.0 if l2 <= 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / l2))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def _near(item: dict, at, layer: str, r: float) -> bool:
    """Item on the violation's layer (or any layer) with a segment within `r` nm of `at` (no segments: anywhere)."""
    ls = item.get("layers") or []
    if ls and layer not in ls and layer != "all layers":
        return False
    segs = item.get("segs") or []
    return not segs or any(_seg_dist(at, s[0], s[1]) <= r for s in segs)


def classify_violation(v: dict, mine: list, others: list, reach_nm: float) -> tuple:
    """(class, sub-class, index of the matched item in `mine` or None) of one drawn violation (see the module doc).
    `mine`: the compass items of its connection; `others`: the over-full items of other connections only."""
    at, layer, vc = v.get("at") or [0, 0], v.get("layer", ""), v.get("class") or ""

    def first(cls, need_near=True):
        for i, it in enumerate(mine):
            if it["class"] == cls and (not need_near or _near(it, at, layer, reach_nm)):
                return i
        return None

    if vc == "via_site" and (i := first("via_site", need_near=False)) is not None:
        return "via_site", "", i
    for cls, sub in (("unlayerable", ""), ("foreign_copper", ""), ("planner_overfull", "crosses it")):
        if (i := first(cls)) is not None:
            return cls, sub, i
    if any(it["class"] == "planner_overfull" and _near(it, at, layer, reach_nm) for it in others):
        return "planner_overfull", "nearby (the wire does not cross it)", None
    if (i := first("forced_path", need_near=False)) is not None:
        return "forced_path", "", i
    if (i := first("pad_exit")) is not None:
        return "pad_exit", "", i
    if (i := first("shrinkage")) is not None:
        return "drawing_loss", "at a planned neck (the cut fits only necked)", i
    if (i := first("model_ref")) is not None:
        return "model_gap", "the drawing's per-edge packer: over-full; the plan's cut test: fits", i
    if vc == "pad_exit":
        return "pad_exit", "not predicted (no pad-exit plan object)", None
    return "drawing_loss", vc, None


def _drc_points(v) -> list:
    """Item positions (nm) of a DRC violation (a toporlib.drc.Violation or its JSON dict)."""
    items = v.items if hasattr(v, "items") and not isinstance(v, dict) else (v.get("items") or [])
    out = []
    for it in items:
        p = it.get("pos") or {}
        if "x" in p and "y" in p:
            out.append((p["x"] * MM, p["y"] * MM))
    return out


def _drc_type(v) -> str:
    return v.type if hasattr(v, "type") and not isinstance(v, dict) else v.get("type", "")


def build(route: dict, drc_new=None, unconnected_new: int | None = None, *, reach_mm: float = 0.5,
          drc_radius_mm: float = 0.5, model_gap: dict | None = None) -> dict:
    """The compass of one run: `route` = the engine result dict (``result.json["route"]`` or the bare RouteResult),
    `drc_new` = KiCad's new routing violations of the written board (``toporlib.drc.new_routing``; Violation objects
    or JSON dicts; None = not run), `unconnected_new` = KiCad's new unconnected items. Returns a JSON-able dict:
    ``rows`` (per class), ``violations`` (per drawn violation its class), ``connections`` (per connection),
    ``flags``, ``notes``. Never raises on a result without a compass (``available`` False). `model_gap`: violation
    index -> sub-class text for violations an outside cut test found over-full (``harness.predict_vs_drawn --cuts``
    with ``[capacity] model = "edges"``): such a drawing loss becomes a model gap."""
    cp = route.get("compass")
    conns = route.get("connections") or []
    viols = route.get("violations") or []
    width = {c.get("id"): c.get("width") or 0 for c in conns}
    out = {"available": cp is not None, "model": (cp or {}).get("model"), "loop": (cp or {}).get("opt_loop"),
           "round": (cp or {}).get("round"), "notes": list((cp or {}).get("notes") or [])}
    items = list((cp or {}).get("items") or [])
    by_conn = defaultdict(list)
    over_by_conn = defaultdict(set)
    for k, it in enumerate(items):
        it = {**it, "_k": k}
        for c in it.get("conns") or []:
            by_conn[c].append(it)
            if it["class"] == "planner_overfull":
                over_by_conn[c].add(k)
    overfull = [dict(it, _k=k) for k, it in enumerate(items) if it["class"] == "planner_overfull"]
    drawn_rows = []
    matched_items = set()
    for i, v in enumerate(viols):
        c = v.get("conn", -1)
        reach = max(reach_mm * MM, 4.0 * (width.get(c) or 0)) + 0.5 * (v.get("length_nm") or 0)
        mine = by_conn.get(c, [])
        others = [it for it in overfull if it["_k"] not in over_by_conn.get(c, set())]
        cls, sub, j = classify_violation(v, mine, others, reach) if cp is not None else ("drawing_loss", v.get("class") or "", None)
        if j is not None:
            matched_items.add(mine[j]["_k"])
        if model_gap and cls == "drawing_loss" and i in model_gap:
            cls, sub = "model_gap", model_gap[i]
        drawn_rows.append({"i": i, "conn": c, "name": v.get("name"), "from": v.get("from"), "to": v.get("to"),
                           "layer": v.get("layer"), "at_mm": [round(v["at"][0] / MM, 3), round(v["at"][1] / MM, 3)],
                           "kind": v.get("kind"), "engine_class": v.get("class"), "class": cls, "sub": sub,
                           "half_len_mm": round(0.5 * (v.get("length_nm") or 0) / MM, 3)})
    # KiCad's DRC items: the class of the nearest drawn violation within drc_radius_mm, else "KiCad only".
    kicad = Counter()
    kicad_only = Counter()
    if drc_new is not None:
        rad = drc_radius_mm * MM
        for dv in drc_new:
            pts = _drc_points(dv)
            best = None
            for r in drawn_rows:
                at = (r["at_mm"][0] * MM, r["at_mm"][1] * MM)
                d = min((math.hypot(px - at[0], py - at[1]) for px, py in pts), default=math.inf) - r["half_len_mm"] * MM
                if d <= rad and (best is None or d < best[0]):
                    best = (d, r["class"])
            if best is None:
                kicad["kicad_only"] += 1
                kicad_only[_drc_type(dv)] += 1
            else:
                kicad[best[1]] += 1
    pred = (cp or {}).get("predicted") or {}
    drawn = Counter(r["class"] for r in drawn_rows)
    drawn_conns = defaultdict(set)
    for r in drawn_rows:
        drawn_conns[r["class"]].add(r["conn"])
    pred_conns = defaultdict(set)
    pred_items = Counter()
    for it in items:
        pred_items[it["class"]] += 1
        pred_conns[it["class"]].update(it.get("conns") or [])
    clean = Counter()
    for k, it in enumerate(items):
        if k not in matched_items:
            clean[it["class"]] += 1
    open_n = sum(1 for c in conns if c.get("status") not in ("routed", "ignored"))
    rows = []
    for cls in ORDER:
        if cls == "open":
            p, pc = None, None
        elif cls in ("drawing_loss", "model_gap", "kicad_only"):
            p, pc = 0, 0  # the plan never predicts its drawing's failure
        else:
            p = pred.get(cls) if cp is not None else None
            pc = len(pred_conns[cls]) if p is not None else None
        if cls == "open":
            d = open_n
        elif cls == "shrinkage":
            d = (cp or {}).get("drawn_shrinkages")
        else:
            d = None if cls == "kicad_only" else drawn.get(cls, 0)
        dc = None if cls in ("open", "kicad_only", "shrinkage") else len(drawn_conns[cls])
        if cls == "open":
            k = unconnected_new
        elif cls == "shrinkage":
            k = None  # KiCad has no shrinkage check (a neck below the board minimum is a track_width violation)
        else:
            k = kicad.get(cls, 0) if drc_new is not None else None
        located = pred_items.get(cls, 0) > 0 and cls != "shrinkage"
        rows.append({"class": cls, "label": LABELS[cls], "predicted": p, "predicted_conns": pc,
                     "predicted_clean": (clean.get(cls, 0) if located else None) if p else (0 if p == 0 else None),
                     "drawn": d, "drawn_conns": dc, "kicad": k})
    flags = []
    for r in rows:
        if r["predicted"] is None and r["drawn"] and r["class"] not in ("open",):
            flags.append(f"{r['label']}: drawn {r['drawn']}, not predicted by the plan (missing method step)")
        elif r["class"] == "drawing_loss" and r["drawn"]:
            flags.append(f"drawing loss: {r['drawn']} violations where the plan said fits (drawing fault)")
        elif r["class"] == "model_gap" and r["drawn"]:
            flags.append(f"model gap: {r['drawn']} violations where the plan's and the drawing's capacity models disagree")
        elif r["class"] == "kicad_only" and r["kicad"]:
            flags.append(f"KiCad only: {r['kicad']} DRC items no engine violation is near ({dict(kicad_only)}): checker gap")
    out.update({
        "rows": rows,
        "predicted_violations": (cp or {}).get("predicted_violations"),
        "loop_predicted": (cp or {}).get("loop_predicted"),
        "drawn_violations": len(viols),
        "drawn_shrinkages": (cp or {}).get("drawn_shrinkages"),
        "kicad_routing_new": None if drc_new is None else len(list(drc_new)),
        "kicad_only_by_type": dict(kicad_only),
        "violations": drawn_rows,
        "connections": connection_rows(route),
        "flags": flags,
    })
    if cp is not None and out["loop_predicted"] is not None and out["loop_predicted"] != out["predicted_violations"]:
        out["notes"].append(f"the TopoR loop's pick predicted {out['loop_predicted']} violations, the plan handed to "
                            f"the drawing {out['predicted_violations']}: the plan changed between the pick and the drawing")
    return out


def connection_rows(route: dict) -> list:
    """Per connection: drawn length, detour ratio against the engine's reference (``ref_len_nm``, else the ghost) and
    against the plan's own taut length, vias planned vs drawn, violations, the flags. A diff pair's N connection takes
    the plan entry of its P connection (the pair is one lead wire in the plan). O(connections)."""
    cp = route.get("compass") or {}
    plan = {c["conn"]: c for c in cp.get("conns") or []}
    # a coupled pair is one lead wire in the plan (the P connection): the N connection reads its plan entry
    for dp in route.get("diff_pairs") or []:
        if dp.get("conn_p") in plan and dp.get("conn_n") not in plan:
            plan[dp["conn_n"]] = plan[dp["conn_p"]]
    nviol = Counter(v.get("conn") for v in route.get("violations") or [])
    rows = []
    for c in route.get("connections") or []:
        cid = c.get("id")
        if c.get("status") == "ignored":
            continue
        length = c.get("length_nm") if c.get("status") == "routed" else None
        ref = c.get("ref_len_nm") or c.get("ghost_len_nm")
        pl = plan.get(cid)
        taut = pl.get("taut_nm") if pl else None
        ratio = round(length / ref, 3) if length and ref and ref > 0 else None
        ratio_plan = round(length / taut, 3) if length and taut and taut > 0 else None
        pv = pl.get("vias") if pl else None
        dv = c.get("vias") if c.get("status") == "routed" else None
        flags = []
        if ratio is not None and ratio > DETOUR_FLAG:
            flags.append("detour")
        if ratio_plan is not None and ratio_plan > DETOUR_FLAG:
            flags.append("drawn longer than planned")
        if pv is not None and dv is not None and pv != dv:
            flags.append("vias planned != drawn")
        if nviol.get(cid):
            flags.append("violations")
        if c.get("status") != "routed":
            flags.append(c.get("status") or "open")
        rows.append({"conn": cid, "net": c.get("name"), "from": (c.get("from") or {}).get("label"),
                     "to": (c.get("to") or {}).get("label"), "status": c.get("status"),
                     "length_mm": None if length is None else round(length / MM, 3),
                     "ref_mm": None if not ref else round(ref / MM, 3),
                     "planned_mm": None if not taut else round(taut / MM, 3),
                     "detour": ratio, "detour_vs_plan": ratio_plan, "vias_planned": pv, "vias_drawn": dv,
                     "violations": nviol.get(cid, 0), "flags": flags})
    return rows


def _n(x) -> str:
    return "n/a" if x is None else str(x)


def table_md(cp: dict, *, max_conns: int = 15) -> list:
    """The compass as markdown lines (report.md section, harness/predict_vs_drawn.py). O(rows)."""
    if not cp or not cp.get("rows"):
        return ["Compass: not available (this engine result carries no plan; the topological core writes it)."]
    lines = [f"Plan model `{cp.get('model')}`, loop `{cp.get('loop')}`, cycle {cp.get('round')}. Predicted = what the "
             "plan handed to the drawing predicts (items: cuts, edges, vias, crossings; conns = connections in them); "
             "drawn = rule violations of the written copper by our checker, classified against the plan; KiCad = new "
             "routing DRC items of the written board, by the class of the nearest drawn violation (0.5 mm).", "",
             "| class | predicted (conns) | predicted, drawn clean | drawn (conns) | KiCad DRC |",
             "|---|---|---|---|---|"]
    for r in cp["rows"]:
        pc = f" ({r['predicted_conns']})" if r.get("predicted_conns") else ""
        dc = f" ({r['drawn_conns']})" if r.get("drawn_conns") else ""
        p = "not predicted" if r["predicted"] is None and r["class"] not in ("open",) else _n(r["predicted"])
        if r["class"] == "open":
            p = "-"
        lines.append(f"| {r['label']} | {p}{pc} | {_n(r['predicted_clean'])} | {_n(r['drawn'])}{dc} | {_n(r['kicad'])} |")
    lines += ["", f"Totals: plan predicts {_n(cp.get('predicted_violations'))} violations"
              + (f" (TopoR loop pick: {cp['loop_predicted']})" if cp.get("loop_predicted") is not None else "")
              + f"; drawn {cp.get('drawn_violations')} violations; KiCad {_n(cp.get('kicad_routing_new'))} new routing "
              "items. Shrinkage row: necked stretches planned / drawn (no violation)."]
    for f in cp.get("flags") or []:
        lines.append(f"- {f}")
    for n in cp.get("notes") or []:
        lines.append(f"- note: {n}")
    sub = Counter((v["class"], v["sub"]) for v in cp.get("violations") or [] if v["sub"])
    if sub:
        lines += ["", "Sub-classes: " + "; ".join(f"{LABELS.get(c, c)} / {s}: {k}" for (c, s), k in sub.most_common())]
    rows = [c for c in cp.get("connections") or [] if c["flags"]]
    det = [c for c in cp.get("connections") or [] if c.get("detour") is not None]
    gt = sum(1 for c in det if c["detour"] > DETOUR_FLAG)
    vmis = sum(1 for c in cp.get("connections") or [] if "vias planned != drawn" in c["flags"])
    lines += ["", f"Per connection: {len(det)} with a detour reference, {gt} with detour > {DETOUR_FLAG}; "
              f"{vmis} with vias planned != drawn; {len(rows)} flagged in all."]
    if rows:
        rows = sorted(rows, key=lambda c: (-(c["violations"] or 0), -(c["detour"] or 0), c["conn"] or 0))[:max_conns]
        lines += ["", "| conn | net | from -> to | length mm | detour | vs plan | vias plan/drawn | violations | flags |",
                  "|---|---|---|---|---|---|---|---|---|"]
        for c in rows:
            lines.append(f"| {c['conn']} | {c['net']} | {c['from']} -> {c['to']} | {_n(c['length_mm'])} | "
                         f"{_n(c['detour'])} | {_n(c['detour_vs_plan'])} | {_n(c['vias_planned'])}/{_n(c['vias_drawn'])} | "
                         f"{c['violations']} | {', '.join(c['flags'])} |")
    return lines


def summary_line(cp: dict) -> str | None:
    """One line for the run summary: predicted / drawn / KiCad per class with a non-zero entry. O(rows)."""
    if not cp or not cp.get("rows"):
        return None
    parts = []
    for r in cp["rows"]:
        vals = (r["predicted"], r["drawn"], r["kicad"])
        if any(v for v in vals):
            p = "-" if r["predicted"] is None else r["predicted"]
            parts.append(f"{r['class']} {p}/{_n(r['drawn'])}/{_n(r['kicad'])}")
    return "compass (predicted/drawn/KiCad): " + (", ".join(parts) if parts else "all zero")


def columns(cp: dict | None) -> dict:
    """Flat columns for a scorecard (score_runs / scorecard): ``p_<class>``, ``d_<class>``, ``k_<class>`` plus the
    per-connection counts. Missing compass: {}. O(rows + connections)."""
    if not cp or not cp.get("rows"):
        return {}
    out = {}
    for r in cp["rows"]:
        out[f"p_{r['class']}"] = r["predicted"]
        out[f"d_{r['class']}"] = r["drawn"]
        out[f"k_{r['class']}"] = r["kicad"]
    cs = cp.get("connections") or []
    out["detour_gt_1_5"] = sum(1 for c in cs if (c.get("detour") or 0) > DETOUR_FLAG)
    out["vias_plan_ne_drawn"] = sum(1 for c in cs if "vias planned != drawn" in c["flags"])
    return out


def drc_from_json(path) -> object:
    """A toporlib.drc.DrcReport from a kicad-cli DRC JSON file (no DRC run). O(size)."""
    from toporlib.drc import DrcReport, Violation  # noqa: PLC0415
    doc = json.loads(Path(path).read_text())
    vs = [Violation(type=v["type"], severity=v.get("severity", ""), description=v.get("description", ""),
                    items=list(v.get("items", []))) for v in doc.get("violations", [])]
    return DrcReport(path=Path(path), exit_code=0, seconds=0.0, violations=vs,
                     unconnected=list(doc.get("unconnected_items", [])), by_type=dict(Counter(v.type for v in vs)))
