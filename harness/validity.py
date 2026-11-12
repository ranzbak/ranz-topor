"""Test validity gate (M0v): a pattern or board run can only judge the router when the router worked to exactly the
design rules the expected answer was built for (user 2026-11-12: "the test needs to be valid in order to be able to
judge the algorithm").

Every `tools/route_board.sh` run compares, before anything else, the router's EFFECTIVE rules -- the rules in the
model the engine received (`outcome.model["rules"]`: fab floors raised, custom-rule values floored or not modelled,
widths, clearances, via sizes the router may place, clearance regions, edge and hole clearances, `[search]
margin_um`) -- with the rule set the expected answer assumes:

- a pattern declares it in its `expected.json` under ``"rules_assumed"`` (schema below), or
- without a declaration (older patterns, real boards: the hand layout is the reference) it is derived from the board's
  own files: the same rule pipeline with ``[rules] fab_profile = "board"`` (Board Setup, net classes and custom rules
  exactly as KiCad's DRC judges them; KiCad DRC-checked the reference copper against these files).

A declared rule set is also checked against the board files (a generator that declares 0.15 mm but writes 0.20 into
the .kicad_pro builds an invalid test). Any difference makes the run INVALID: `<name>--validity.json`, metrics
``valid`` / ``validity``, the first line of the route_board summary; its faults are then not router faults.

``rules_assumed`` (mm; every key optional: a key not given takes the board files' own value, so ``{"authority":
"board", "options": {...}}`` means "the board's rules plus these options"; the keys given are also checked against
the board files; ``margin`` defaults to 0; the audit applies ``options`` before it resolves the rules):

    {"authority": "board",                       # informational: the rules the answer was built for
     "board": {"min_clearance": 0.15, "min_track_width": 0.15, "min_via_diameter": 0.4, "min_via_drill": 0.2,
               "min_annular": 0.1, "hole_clearance": 0.2, "via_hole_clearance": 0.2, "hole_to_hole": 0.25,
               "edge_clearance": 0.5, "min_connection": 0},
     "nets": {"*": {"clearance": 0.15, "width": 0.15, "vias": [[0.4, 0.2]]}},   # "*" = every net + the default
     "regions": [{"rule": "BGA neckdown", "arg": "U1", "clearance": 0.1}],   # [] = no clearance region
     "margin": 0.0,                                # [search] margin_um in mm: the answer keeps exact clearances
     "not_needed": ["track_width inside"],         # unsupported custom-rule parts the answer does not use
     "options": {"vias.via_in_pad": false}}        # topor.toml settings the answer assumes

"vias" is the list of via sizes ``[diameter, drill]`` the router may place for the net, largest first: a mirror of
`engine/src/vias.rs` ``via_sizes`` (net class via, then the smaller pre-defined and fab-profile vias; ``[vias] size =
"board_min"`` the rule size). "unsupported" lists custom-rule parts the router cannot carry exactly (a track width or
via size tied to a courtyard / rule area, rules it warns about or refuses): each one makes the run INVALID unless the
declaration names it in ``not_needed``.

    python -m harness.validity audit <pattern dir | expected.json> ... [--set section.key=value] [--md]
    python -m harness.validity show <route_board out dir>

Complexity: O(N x V) for N nets and V candidate via sizes; the pipeline replica is one pcbnew load and model export.
"""
from __future__ import annotations

import argparse
import json
import math
import re
import sys
import tempfile
from pathlib import Path

BOARD_FIELDS = ("min_clearance", "min_track_width", "min_via_diameter", "min_via_drill", "min_annular",
                "hole_clearance", "via_hole_clearance", "hole_to_hole", "edge_clearance", "min_connection")
NET_FIELDS = ("clearance", "width", "via_d", "via_drill", "min_width")
DEFAULT_NET = "(default)"
TOL_MM = 1e-6
_GEO = re.compile(r"(intersects\w*courtyard|inside\w*courtyard|insidearea|intersectsarea|enclosedbyarea)", re.I)


def _mm(v) -> float:
    return round(int(v or 0) / 1e6, 6)


def _nm(mm) -> int:
    return int(round(float(mm) * 1e6))


# ------------------------------------------------------------------ via sizes (mirror of engine/src/vias.rs)
def _rule_size(rules: dict, width: int, grid: int, allowed: list[int]) -> tuple[int, int]:
    need = math.ceil(max(width, 0) / math.pi)
    drill = (need + grid - 1) // grid * grid if grid > 0 else need
    drill = max(drill, int(rules.get("min_via_drill") or 0))
    bigger = [a for a in allowed if a >= drill]
    if bigger:
        drill = min(bigger)
    return max(int(rules.get("min_via_diameter") or 0), drill + 2 * int(rules.get("min_annular") or 0)), drill


def via_list(rules: dict, nr: dict, net_code: int | None, opts: dict) -> list[list[float]]:
    """Via sizes ``[d, drill]`` (mm) the engine may place for a net with rule `nr`, largest first: ``via_sizes`` of
    engine/src/vias.rs (net class via or the via fallback, then the smaller pre-defined and ``routing_vias``
    sizes; ``[vias] size = "board_min"``: the one rule size). `opts`: ``via_size``, ``grid_nm``, ``allowed_nm``.
    O(V log V)."""
    allowed = list(opts.get("allowed_nm") or [])
    pairs = rules.get("diff_pairs") or []
    in_pair = net_code is not None and any(int(p.get("p", -1)) == net_code or int(p.get("n", -1)) == net_code
                                           for p in pairs)
    if opts.get("via_size") == "board_min" and not in_pair:
        w = int(nr.get("width") or 0)
        d, dr = _rule_size(rules, w, int(opts.get("grid_nm") or 0), allowed)
        return [[_mm(d), _mm(dr)]]
    dmin, rmin, ann = (int(rules.get(k) or 0) for k in ("min_via_diameter", "min_via_drill", "min_annular"))
    fb = rules.get("via_fallback") or [800_000, 400_000]
    if (not rules.get("predefined_vias") and [nr.get("via_d"), nr.get("via_drill")] == [800_000, 400_000]
            and list(fb) != [800_000, 400_000]):
        first = (max(int(fb[0]), dmin), max(int(fb[1]), rmin))
    else:
        first = (int(nr.get("via_d") or 0), int(nr.get("via_drill") or 0))
    cand = [(int(d), int(r)) for d, r in rules.get("predefined_vias") or []]
    cand += [(max(int(r) + 2 * ann, dmin), int(r)) for r in rules.get("routing_vias") or []]
    rest = sorted({(d, r) for d, r in cand if r >= rmin and d >= dmin and 2 * ann <= d - r and d < first[0]
                   and r < first[1] and (not allowed or r in allowed)}, reverse=True)
    return [[_mm(d), _mm(r)] for d, r in [first, *rest]]


# ------------------------------------------------------------------ canonical rule set
def unsupported(rules: dict) -> list[str]:
    """Custom-rule parts the router cannot carry exactly (each named): a track width / via size / hole size whose
    condition is a courtyard or rule area (the router has one width and via per net, evaluated outside every
    region), constraints it warns about or applies only as a precaution, refused rules, regions naming nothing.
    O(rules)."""
    out = []
    for r in rules.get("custom_rules") or []:
        use, con, cond = r.get("use"), r.get("constraint"), str(r.get("condition") or "")
        if use == "warned":
            out.append(f"custom rule '{r['rule']}': {con} not honoured ({r.get('note') or 'warned'})")
        elif use == "net (precaution)":
            out.append(f"custom rule '{r['rule']}': {con} inside {cond} applied board-wide as a precaution")
        elif con in ("track_width", "via_diameter", "hole_size") and _GEO.search(cond) and use not in (
                "ignored", "not routing", "no routed copper", "zone fill"):
            out.append(f"custom rule '{r['rule']}': {con} inside {cond} not modelled (one {con} per net, "
                       f"outside every region; values {r.get('values')})")
    for w in rules.get("warnings") or []:
        if str(w).startswith("REFUSE:"):
            out.append(str(w))
    for n in rules.get("custom_notes") or []:
        if "depends on the net" in n or "names no footprint" in n:
            out.append(str(n))
    return sorted(dict.fromkeys(out))


def _opts_of(cfg) -> dict:
    if cfg is None:
        return {"via_size": "net_class", "grid_nm": 10_000, "allowed_nm": []}
    return {"via_size": str(cfg.get("vias", "size")), "grid_nm": _nm(cfg.get("vias", "drill_grid_mm") or 0),
            "allowed_nm": [_nm(x) for x in cfg.get("vias", "allowed_drills_mm") or []]}


def rule_set(rules: dict, net_names: dict, cfg=None) -> dict:
    """The canonical rule set (mm) of resolved rules / engine-input model rules. `net_names`: code -> name; `cfg`:
    the run's Config (via policy, margin). O(N x V)."""
    opts = _opts_of(cfg)
    board = {f: _mm(rules.get(f)) for f in BOARD_FIELDS}
    if not rules.get("via_hole_clearance"):  # engine: 0 (older models) = hole_clearance
        board["via_hole_clearance"] = board["hole_clearance"]

    def net(nr: dict, code) -> dict:
        d = {f: _mm(nr.get(f)) for f in NET_FIELDS}
        d["vias"] = via_list(rules, nr, code, opts)
        return d

    nets = {DEFAULT_NET: net(rules.get("default") or {}, None)}
    for code, nr in sorted((rules.get("per_net") or {}).items(), key=lambda kv: int(kv[0])):
        nets[str(net_names.get(int(code), f"#{code}"))] = net(nr, int(code))
    regs = rules.get("clearance_regions")
    if regs is None:  # resolved rules before the model export: the planned regions
        regs = [{"name": r["rule"], "ref": r["arg"], "clearance": r["clearance"]} for r in rules.get("custom_regions") or []]
    regions = sorted({(str(r.get("name") or r.get("rule")), str(r.get("ref") or r.get("arg")), _mm(r["clearance"]))
                      for r in regs})
    margin = float(cfg.get("search", "margin_um") or 0) / 1000.0 if cfg is not None else 0.0
    return {"profile": rules.get("profile"), "board": board, "nets": nets,
            "regions": [{"rule": a, "arg": b, "clearance": c} for a, b, c in regions], "margin": round(margin, 6),
            "unsupported": unsupported(rules),
            "floors_applied": [f"{x['name']} {_mm(x['design']):g} -> {_mm(x['floor']):g}"
                               for x in rules.get("floors_applied") or []],
            "custom_floors": [n for n in rules.get("custom_notes") or [] if str(n).startswith("floor:")]}


def model_rule_set(model: dict, cfg=None) -> dict:
    """:func:`rule_set` of an engine-input model (its ``rules`` and net table). O(N x V)."""
    names = {int(n["code"]): n["name"] for n in model.get("nets", []) if n.get("name") is not None}
    return rule_set(model.get("rules") or {}, names, cfg)


# ------------------------------------------------------------------ the rule pipeline of a run (replica)
def pipeline_model(pc, hand_pc, cfg, *, fab_profile: str | None = None) -> dict:
    """The engine-input model of a route_board run on project copy `pc` (rules from widths of `hand_pc`), as
    harness.route_board + topor.run build it: resolve_rules (``[rules] fab_profile`` of `cfg`, or `fab_profile`),
    export_model, the via fallback, the diff pairs. `pc` / `hand_pc`: harness.copy.ProjectCopy. One pcbnew load
    per board."""
    from harness.model import export_model, load_board
    from harness.rip import record_widths
    from harness.rules import resolve_rules
    from topor import preflight as preflight_mod
    from topor import run as run_mod

    prof = fab_profile or str(cfg.get("rules", "fab_profile"))
    widths = record_widths(load_board(hand_pc))
    dru = pc.dru.read_text() if pc.dru is not None else None
    b = load_board(pc)
    rules = resolve_rules(b, dru_text=dru, widths_before=widths, fab_profile=prof)
    model = export_model(b, rules, name=Path(pc.pcb).stem, dru_text=dru)
    pro = json.loads(pc.pro.read_text()) if pc.pro is not None and pc.pro.is_file() else {}
    if preflight_mod.via_setup_missing(pro, cfg):
        mr, _info = run_mod.apply_via_fallback(model["rules"], cfg)
        model = {**model, "rules": mr}
    from harness.route_board import board_netclasses
    model, _notes = run_mod.attach_diff_pairs(model, rules, Path(pc.pcb), pro, cfg, board_netclasses(pc)[1])
    return model


def board_rule_set(pc, hand_pc, cfg) -> dict:
    """The rules the board's own files give (``[rules] fab_profile = "board"``), as a rule set. O(pipeline)."""
    return model_rule_set(pipeline_model(pc, hand_pc, cfg, fab_profile="board"), cfg)


# ------------------------------------------------------------------ declaration
def find_expected(board_path: Path) -> Path | None:
    """The `expected.json` of a pattern board (``<pattern>/{problem,reference,...}/<name>.kicad_pcb`` or next to
    it), None for a board without one. O(1)."""
    p = Path(board_path)
    if p.suffix != ".kicad_pcb":
        return None
    for d in (p.parent, p.parent.parent):
        e = d / "expected.json"
        if e.is_file():
            return e
    return None


def declared_of(expected: Path | None) -> dict | None:
    """``rules_assumed`` of an expected.json, None without one. O(file)."""
    if expected is None or not Path(expected).is_file():
        return None
    try:
        doc = json.loads(Path(expected).read_text())
    except ValueError:
        return None
    ra = doc.get("rules_assumed")
    return ra if isinstance(ra, dict) else None


# ------------------------------------------------------------------ the expected copper itself
_VIA = re.compile(r"\(via\b(.*?)\n\t\)", re.S)
_SEG = re.compile(r"\((?:segment|arc)\b(.*?)\n\t\)", re.S)
_SIZE = re.compile(r"\(size\s+([-\d.]+)")
_DRILL = re.compile(r"\(drill\s+([-\d.]+)")
_WIDTH = re.compile(r"\(width\s+([-\d.]+)")
_NETNAME = re.compile(r'\(net\s+"((?:[^"\\]|\\.)*)"\)')


def reference_uses(pcb_text: str, eff: dict) -> tuple[list[dict], list[str]]:
    """What the expected copper (`pcb_text`, a reference board) uses that the router may not: a via size not in
    the router's via list of its net (``vias`` of `eff`), a track narrower than the router may draw (the board's
    minimum track width or the net's custom minimum). Returns (differences, notes); a track width other than the
    router's net width is a note (the router draws the net width, necks down to the floor). O(items)."""
    nets = eff.get("nets") or {}
    floor_b = float((eff.get("board") or {}).get("min_track_width") or 0)
    bad_via: dict = {}
    for m in _VIA.finditer(pcb_text):
        body = m.group(1)
        sz, dr, nn = _SIZE.search(body), _DRILL.search(body), _NETNAME.search(body)
        if not (sz and dr and nn):
            continue
        d, r, net = round(float(sz.group(1)), 6), round(float(dr.group(1)), 6), nn.group(1)
        allowed = (nets.get(net) or nets.get(DEFAULT_NET) or {}).get("vias") or []
        if not any(_same([d, r], v) for v in allowed):
            bad_via.setdefault((d, r, json.dumps(allowed)), []).append(net)
    narrow: dict = {}
    other: dict = {}
    for m in _SEG.finditer(pcb_text):
        body = m.group(1)
        w, nn = _WIDTH.search(body), _NETNAME.search(body)
        if not (w and nn):
            continue
        wv, net = round(float(w.group(1)), 6), nn.group(1)
        nr = nets.get(net) or nets.get(DEFAULT_NET) or {}
        floor = max(floor_b, float(nr.get("min_width") or 0))
        if wv < floor - TOL_MM:
            narrow.setdefault((wv, floor), set()).add(net)
        elif not _same(wv, nr.get("width")):
            other.setdefault((wv, nr.get("width")), set()).add(net)
    diffs = [{"rule": "reference via", "expected": [d, r], "effective": json.loads(a), "nets": sorted(set(n)),
              "why": "the expected answer places a via the router may not place"} for (d, r, a), n in sorted(bad_via.items())]
    diffs += [{"rule": "reference track width", "expected": w, "effective": f">= {f:g}", "nets": sorted(n),
               "why": "the expected answer draws narrower than the router may"} for (w, f), n in sorted(narrow.items())]
    notes = [f"reference track width {w:g} mm != the router's net width {_fmt(rw)} mm ({len(n)} net(s): "
             f"{', '.join(sorted(n)[:4])})" for (w, rw), n in sorted(other.items(), key=lambda kv: str(kv[0]))]
    return diffs, notes


# ------------------------------------------------------------------ comparison
def _same(a, b) -> bool:
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return abs(float(a) - float(b)) <= TOL_MM
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(_same(x, y) for x, y in zip(a, b))
    return a == b


def _fmt(v) -> str:
    if isinstance(v, list):
        if v and all(isinstance(x, list) for x in v):
            return "[" + ", ".join("/".join(f"{y:g}" for y in x) for x in v) + "]"
        return "[" + ", ".join(_fmt(x) for x in v) + "]"
    if isinstance(v, float):
        return f"{v:g}"
    return str(v)


def compare(declared: dict, eff: dict, *, cfg=None, what: str = "effective") -> list[dict]:
    """Differences between a declared rule set (keys optional) and a full rule set `eff`: ``[{"rule",
    "expected", "effective", "nets"?, "why"}]``. Net differences are grouped by (field, expected, effective).
    `what` names `eff` in the texts. O(N x fields)."""
    out: list[dict] = []
    floors = " ".join(eff.get("floors_applied") or [])
    prof = eff.get("profile")

    def why(field: str) -> str:
        if field in floors or (field == "via_hole_clearance" and "hole_clearance" in floors):
            return f"fab floor ({prof})"
        return ""

    for k, v in (declared.get("board") or {}).items():
        e = (eff.get("board") or {}).get(k)
        if e is None or not _same(v, e):
            out.append({"rule": f"board.{k}", "expected": v, "effective": e, "why": why(k)})
    grouped: dict = {}
    en = eff.get("nets") or {}
    for pat, dv in (declared.get("nets") or {}).items():
        targets = list(en) if pat == "*" else [pat]
        for name in targets:
            ev = en.get(name)
            if ev is None:
                out.append({"rule": f"nets.{name}", "expected": "present", "effective": "missing", "why": ""})
                continue
            for f, v in dv.items():
                e = ev.get(f)
                if e is None or not _same(v, e):
                    grouped.setdefault((f, json.dumps(v), json.dumps(e)), []).append(name)
    for (f, v, e), names in sorted(grouped.items()):
        v, e = json.loads(v), json.loads(e)
        hint = why({"width": "min_track_width", "clearance": "min_clearance"}.get(f, f))
        if not hint and f == "vias" and prof != "board":
            hint = f"fab profile ({prof}) via floors / alternative drills"
        out.append({"rule": f"net.{f}", "expected": v, "effective": e, "nets": names, "why": hint})
    if "regions" in declared:
        key = lambda r: (str(r.get("arg")), round(float(r.get("clearance") or 0), 6))  # noqa: E731
        dk = sorted({key(r) for r in declared.get("regions") or []})
        ek = sorted({key(r) for r in eff.get("regions") or []})
        if dk != ek:
            out.append({"rule": "regions", "expected": [f"{a} {c:g}" for a, c in dk],
                        "effective": [f"{a} {c:g}" for a, c in ek],
                        "why": "; ".join(eff.get("custom_floors") or []) or ""})
    dm = float(declared.get("margin", 0.0) or 0.0)
    if not _same(dm, float(eff.get("margin") or 0.0)):
        out.append({"rule": "search.margin_um (mm)", "expected": dm, "effective": eff.get("margin"), "why": ""})
    if cfg is not None:
        for k, v in (declared.get("options") or {}).items():
            sec, _, key = k.partition(".")
            try:
                got = cfg.get(sec, key)
            except KeyError:
                got = None
            if not _same(v, got):
                out.append({"rule": f"option {k}", "expected": v, "effective": got, "why": ""})
    return out


def merged_declaration(declared: dict, board: dict | None) -> dict:
    """The full rule set an expected answer assumes: the board files' rule set `board` with the declared fields
    laid over it field by field (``board`` per key; ``nets`` per net and field: the board net, then ``"*"``, then
    the named net; ``regions`` / ``options`` / ``not_needed`` replace when given; ``margin`` declared or 0). A key
    not declared keeps the board files' value, so it is still compared. `board` None: the declaration as is.
    O(N x fields)."""
    if board is None:
        return declared
    dn = declared.get("nets") or {}
    star = dn.get("*") or {}
    nets = {name: {**fields, **star, **(dn.get(name) or {})} for name, fields in (board.get("nets") or {}).items()}
    for name, fields in dn.items():
        if name != "*" and name not in nets:
            nets[name] = dict(fields)
    out = {**board, "board": {**(board.get("board") or {}), **(declared.get("board") or {})}, "nets": nets,
           "margin": float(declared.get("margin", 0.0) or 0.0)}
    for k in ("authority", "regions", "options", "not_needed"):
        if k in declared:
            out[k] = declared[k]
    return out


def judge(declared: dict | None, eff: dict, board: dict | None, *, cfg=None, expected_path=None,
          reference_text: str | None = None) -> dict:
    """The validity verdict of one run. `declared`: ``rules_assumed`` (None: `board` is the declaration);
    `eff`: the run's effective rule set; `board`: the board files' own rule set (None: not computed). A
    difference of `eff` from the declaration, a declaration that differs from the board files, or an
    unsupported custom-rule part not in ``not_needed`` makes it INVALID; with `reference_text` (the expected
    copper of a pattern) so does copper the router may not place (:func:`reference_uses`). O(N x fields + items)."""
    source = f"expected.json rules_assumed ({expected_path})" if declared is not None else \
        "board files (no rules_assumed): Board Setup + net classes + custom rules, [rules] fab_profile = \"board\""
    # a declaration states what it gives; the rest is the board's own rules ({"authority": "board", "options":
    # {...}} = the board files plus these options)
    decl = merged_declaration(declared, board) if declared is not None else board
    diffs = compare(decl, eff, cfg=cfg) if decl is not None else []
    if declared is not None and board is not None:
        for d in compare(declared, board, cfg=None):
            out = dict(d, rule=f"declaration vs board files: {d['rule']}", effective=d["effective"],
                       why="the expected answer assumes rules the board files do not carry")
            diffs.append(out)
    not_needed = [str(x) for x in (declared or {}).get("not_needed") or []]
    notes = []
    if reference_text is not None:
        rd, rn = reference_uses(reference_text, eff)
        diffs += rd
        notes += rn
    for u in eff.get("unsupported") or []:
        if any(s in u for s in not_needed):
            notes.append(f"not modelled, not needed by the expected answer: {u}")
        else:
            diffs.append({"rule": "unsupported custom rule", "expected": "honoured", "effective": u,
                          "why": "the router cannot be given this rule"})
    return {"valid": not diffs, "source": source, "profile": eff.get("profile"), "invalid": diffs, "notes": notes}


def diff_text(d: dict) -> str:
    nets = d.get("nets")
    ns = ""
    if nets:
        ns = f" ({len(nets)} net(s): {', '.join(nets[:4])}{' ...' if len(nets) > 4 else ''})"
    why = f" [{d['why']}]" if d.get("why") else ""
    return f"{d['rule']}: expected {_fmt(d['expected'])}, router {_fmt(d['effective'])}{ns}{why}"


def verdict_line(v: dict) -> str:
    if v["valid"]:
        return f"VALIDITY: valid (router rules = {v['source'].split(' (')[0]}; profile {v['profile']})" + (
            f"; {len(v['notes'])} note(s)" if v["notes"] else "")
    head = "; ".join(diff_text(d) for d in v["invalid"][:4]) + (" ..." if len(v["invalid"]) > 4 else "")
    return (f"VALIDITY: INVALID -- the router ran under other rules than the expected answer ({len(v['invalid'])} "
            f"difference(s); faults of this run are NOT router faults): {head}")


def summary_of(rs: dict) -> str:
    """One-line summary of a rule set: board minimums, the default net, regions, profile. O(1)."""
    b = rs.get("board") or {}
    d = (rs.get("nets") or {}).get(DEFAULT_NET) or {}
    widths = sorted({n.get("width") for n in (rs.get("nets") or {}).values() if n.get("width") is not None})
    clr = sorted({n.get("clearance") for n in (rs.get("nets") or {}).values() if n.get("clearance") is not None})
    vias = sorted({tuple(v) for n in (rs.get("nets") or {}).values() for v in (n.get("vias") or [])}, reverse=True)
    g = lambda k: _fmt(b.get(k)) if k in b else "?"  # noqa: E731
    parts = [f"profile {rs.get('profile')}" if rs.get("profile") else "",
             f"min track {g('min_track_width')} / clr {g('min_clearance')} / via {g('min_via_diameter')}/"
             f"{g('min_via_drill')} / hole clr {g('hole_clearance')} (via {g('via_hole_clearance')}) / edge "
             f"{g('edge_clearance')}",
             f"nets width {_fmt(widths)} clr {_fmt(clr)} vias {_fmt([list(v) for v in vias])}" if widths else
             (f"default width {_fmt(d.get('width'))}" if d else ""),
             ("regions " + ", ".join(f"{r['arg']} {r['clearance']:g}" for r in rs["regions"])) if rs.get("regions")
             else ""]
    return "; ".join(p for p in parts if p)


# ------------------------------------------------------------------ CLI
def _pattern_board(pdir: Path) -> Path | None:
    for sub in ("reference", "problem"):
        d = pdir / sub
        if d.is_dir():
            pcbs = sorted(d.glob("*.kicad_pcb"))
            if pcbs:
                return pcbs[0]
    pcbs = sorted(pdir.glob("*.kicad_pcb"))
    return pcbs[0] if pcbs else None


def options_of(declared: dict | None) -> dict:
    """The topor.toml settings a declaration assumes, as Config changes ``{(section, key): value}``. O(1)."""
    out = {}
    for k, v in ((declared or {}).get("options") or {}).items():
        sec, _, key = str(k).partition(".")
        out[(sec, key)] = v
    return out


def audit_one(pdir: Path, cfg, work: Path) -> dict:
    """Effective rules of a pattern under `cfg` plus the options its declaration assumes (no routing) against its
    declaration / board files. O(pipeline)."""
    from harness.copy import copy_project
    pdir = Path(pdir)
    if pdir.name == "expected.json":
        pdir = pdir.parent
    pcb = _pattern_board(pdir)
    if pcb is None:
        return {"pattern": pdir.name, "error": "no board"}
    pc = copy_project(pcb, work / pdir.name)
    exp = pdir / "expected.json"
    decl = declared_of(exp)
    if options_of(decl):  # the test runs with the options its expected answer assumes
        cfg = cfg.with_values(options_of(decl))
    eff = model_rule_set(pipeline_model(pc, pc, cfg), cfg)
    brd = board_rule_set(pc, pc, cfg)
    ref = pcb if pcb.parent.name == "reference" else None
    v = judge(decl, eff, brd, cfg=cfg, expected_path=exp if decl is not None else None,
              reference_text=ref.read_text() if ref is not None else None)
    return {"pattern": pdir.name, "board": str(pcb), "declared": decl is not None,
            "intended": summary_of({**brd, **decl} if decl is not None else brd), "effective": summary_of(eff),
            "options": {f"{a}.{b}": v for (a, b), v in options_of(decl).items()},
            "valid": v["valid"], "invalid": [diff_text(d) for d in v["invalid"]], "notes": v["notes"],
            "invalid_rules": sorted({d["rule"] for d in v["invalid"]})}


def _kv(s: str):
    k, _, v = s.partition("=")
    sec, _, key = k.partition(".")
    try:
        val = json.loads(v)
    except ValueError:
        val = v
    return (sec, key), val


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    a1 = sub.add_parser("audit", help="effective rules of pattern boards (no routing) vs their declaration")
    a1.add_argument("patterns", nargs="+", type=Path)
    a1.add_argument("--set", action="append", default=[], metavar="SECTION.KEY=VALUE")
    a1.add_argument("--json", type=Path, default=None)
    a1.add_argument("--md", action="store_true", help="markdown table")
    a2 = sub.add_parser("show", help="print the validity of a route_board run")
    a2.add_argument("run_dir", type=Path)
    a = ap.parse_args(argv)
    if a.cmd == "show":
        for f in sorted(a.run_dir.glob("*--validity.json")):
            v = json.loads(f.read_text())["verdict"]
            print(f"{f.name}: {verdict_line(v)}")
            for d in v["invalid"]:
                print("  - " + diff_text(d))
            for n in v["notes"]:
                print("  note: " + n)
        return 0
    from topor.config import Config
    cfg = Config.defaults()
    if a.set:
        cfg = cfg.with_values(dict(_kv(s) for s in a.set))
    rows = []
    with tempfile.TemporaryDirectory(prefix="validity-") as td:
        for p in a.patterns:
            try:
                rows.append(audit_one(p, cfg, Path(td)))
            except Exception as e:  # noqa: BLE001 - one broken pattern must not hide the others
                rows.append({"pattern": Path(p).name, "error": f"{type(e).__name__}: {e}"})
    if a.json:
        a.json.write_text(json.dumps(rows, indent=1))
    for r in rows:
        if "error" in r:
            print(f"| {r['pattern']} | error: {r['error']} | | |" if a.md else f"{r['pattern']}: ERROR {r['error']}")
            continue
        if a.md:
            print(f"| {r['pattern']} | {r['intended']}{'' if r['declared'] else ' (board files)'} | {r['effective']} | "
                  f"{'yes' if r['valid'] else 'NO: ' + '; '.join(r['invalid'])} |")
        else:
            print(f"{r['pattern']}: {'valid' if r['valid'] else 'INVALID'}")
            for d in r["invalid"]:
                print("  - " + d)
    return 0


if __name__ == "__main__":
    sys.exit(main())
