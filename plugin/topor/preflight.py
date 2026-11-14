"""Pre-flight checks (B6, 46 §1.1): every judgement the run is about to make on the user's board, as findings
with a cause and a fix from messages.py.

``preflight(model, rules, cfg, sel, orig_drc, cfg_findings, *, pcb_text, pro)`` is pure (no I/O besides the
kicad-cli presence check) and never mutates its inputs. ``orig_drc`` is None before the original DRC ran (the
run calls it once without, adds ``preexisting_drc`` after). Codes emitted here: PCB_FORMAT_OLD,
KICAD_CLI_MISSING, LAYER_ROLES_NEEDED, PLANE_NO_ZONE (M4-R7), LAYER_TYPES, VIA_NOT_SET_UP,
DRILL_OUTSIDE_FILTER, VIA_IN_PAD_UNPROTECTED, PAD_OUTSIDE_BOARD, PREEXISTING_DRC, TRACK_ANGLE_RULE,
LOCKED_COPPER, DANGLING_ENDS, DO_NOT_TOUCH, SCOPE_EMPTY, NON_DEFAULTS, DEFAULTS_USED, GROUPS_UNAVAILABLE.
(POWER_BY_NAME / MARKER_* are emitted by the scope/priority code, B7.)

Via sizes "not set up" (ruling C33) = no ``board.design_settings.via_dimensions`` entry with a diameter > 0 (the
first entry is KiCad's ``{0, 0}`` placeholder) and no net class setting ``via_diameter`` / ``via_drill`` other
than KiCad's factory 0.8 / 0.4 mm in the ``.kicad_pro`` (tested on the six snapshot projects); the one
implementation is ``toporlib.rules.via_setup_missing`` (the engine applies its via fallback under the same condition).

UNVERIFIED against a live KiCad 10 board file / project (documented, tests use synthetic inputs):
  * via protection = the ``(capping yes)`` and ``(filling yes)`` forms in the board file's ``(setup ...)``;
  * groups = top-level ``(group ...)`` forms in the board file.

Complexity: O(P * R + N + V + K) for P pads, R outline ring points, N nets, V vias, K kept tracks; the plane
check is O(L * pours); regex scans of `pcb_text` are O(len(pcb_text)).
"""

from __future__ import annotations

import fnmatch
import math
import re
from pathlib import Path

from toporlib import geom, pcbfile
from toporlib.kicad_cli import kicad_cli
from toporlib.rules import via_setup_missing as rules_via_setup_missing

from .findings import Finding
from .layers import resolve_layer_roles
from .messages import MESSAGES

_NM = 1_000_000.0
_TOL = 1000  # nm: outline endpoints closer than this are the same point
_GND_FLAGS = re.IGNORECASE


def _mk(code: str, message: str, items=(), severity: str | None = None, fix: str | None = None) -> Finding:
    """A finding with the catalogue's severity and fix unless overridden. O(1)."""
    m = MESSAGES[code]
    return Finding(code, severity or m.severity, message, fix if fix is not None else m.fix, tuple(items))


def _mm(nm) -> str:
    return f"{float(nm) / _NM:g}"


def _is_ground(name: str, pat) -> bool:
    return bool(name) and bool(pat.search(name))


def _is_power(name: str, gpat, ppat) -> bool:
    return bool(name) and bool(ppat.search(name)) and not gpat.search(name)


def _compile(pattern: str, flags=0):
    try:
        return re.compile(pattern, flags)
    except re.error:
        return re.compile(r"(?!)")  # never matches; the bad pattern is the config's CFG_VALUE business


# ------------------------------------------------------------------ stackup (M4-R7)
def _plane_findings(model: dict, cfg, layers: list) -> list:
    nets = {int(n["code"]): n["name"] for n in model.get("nets", [])}
    gpat = _compile(cfg.get("nets", "ground_pattern"), _GND_FLAGS)
    ppat = _compile(cfg.get("nets", "power_pattern"))
    grounds = [n for c, n in sorted(nets.items()) if _is_ground(n, gpat)]
    powers = [n for c, n in sorted(nets.items()) if _is_power(n, gpat, ppat)]
    filled: dict[int, list[str]] = {}  # layer id -> names of nets with a filled zone there
    for z in model.get("pours", []):
        if z.get("warning"):
            continue  # never filled: the model fell back to the outline
        name = nets.get(int(z.get("net", 0)), "")
        if not name:
            continue
        on = {int(f["layer"]) for f in z.get("fills", [])} or {int(i) for i in z.get("layers", [])}
        for lid in on:
            filled.setdefault(lid, []).append(name)
    out = []
    for g in layers:
        role = g.get("role")
        if role not in ("gnd", "pwr", "power"):
            continue
        have = filled.get(int(g["id"]), [])
        if role == "gnd":
            ok, kind, ex = any(_is_ground(n, gpat) for n in have), "ground", (grounds[:1] or ["a ground net"])[0]
        elif role == "pwr":
            ok, kind, ex = any(_is_power(n, gpat, ppat) for n in have), "power", (powers[:1] or ["a power net"])[0]
        else:
            ok = any(_is_ground(n, gpat) or _is_power(n, gpat, ppat) for n in have)
            kind = "ground or power"
            ex = (grounds[:1] or powers[:1] or ["a ground or power net"])[0]
        if ok:
            continue
        name = g["name"]
        out.append(_mk(
            "PLANE_NO_ZONE",
            f"{name} is a {'KiCad power' if role == 'power' else role} plane layer but has no filled zone on a {kind} net.",
            (name,),
            fix=f"add a zone on {name} with net {ex} covering the board outline (Place → Add Filled Zone), "
                f"then refill zones (B), or change the role in topor.toml [layers]"))
    return out


def _layer_type_findings(layers: list) -> list:
    out = []
    for g in layers:
        if g.get("role_source") != "config":
            continue
        kind, role = g.get("kind"), g.get("role")
        plane_role = role in ("gnd", "pwr")
        if (kind == "power" and role in ("signal", "mixed")) or (kind == "signal" and plane_role):
            out.append(_mk("LAYER_TYPES",
                           f"{g['name']}: topor.toml [layers] says {role!r} but KiCad's layer type is {kind!r}; the topor.toml role is used.",
                           (g["name"],)))
    return out


# ------------------------------------------------------------------ vias
def via_setup_missing(pro: dict | None, cfg=None) -> bool:
    """True when the ``.kicad_pro`` sets no via up (VIA_NOT_SET_UP, ruling C33): no pre-defined via and no net class
    via other than KiCad's factory 0.8 / 0.4 mm. One function for the harness, the plugin and the engine's rule:
    :func:`toporlib.rules.via_setup_missing`. O(#via sizes + #net classes)."""
    return rules_via_setup_missing(pro)


def smaller_profile_vias(rules: dict, d_nm: int, drill_nm: int, allowed_drills_nm=()) -> list:
    """The extra via sizes ``(d, drill)`` nm, largest first, that the engine may use for a net whose own via is
    ``(d_nm, drill_nm)``: mirrors ``engine/src/vias.rs`` ``sizes_for`` -- every ``rules.predefined_vias`` entry
    (KiCad's pre-defined vias; there is no fab-profile via list) with ``drill >= min_via_drill``,
    ``d >= min_via_diameter``, ``(d - drill) / 2 >= min_annular``, ``d < d_nm``, ``drill < drill_nm`` and, when
    ``allowed_drills_nm`` is not empty, its drill in it. Agreement with the engine is tested on the six snapshot
    projects (tests/plugin/test_via_rule_agreement.py). O(P log P)."""
    lo, ann = int(rules.get("min_via_drill") or 0), int(rules.get("min_annular") or 0)
    dmin = int(rules.get("min_via_diameter") or 0)
    cand = [(int(d), int(r)) for d, r in rules.get("predefined_vias") or []]
    allowed = {int(x) for x in allowed_drills_nm}
    sizes = {(d, r) for d, r in cand
             if r >= lo and d >= dmin and 2 * ann <= d - r and d < d_nm and r < drill_nm
             and (not allowed or r in allowed)}
    return sorted(sizes, reverse=True)


def _via_findings(model: dict, rules: dict, cfg, pcb_text: str | None, pro: dict | None) -> list:
    out = []
    if via_setup_missing(pro, cfg):
        d, r = cfg.get("vias", "fallback_diameter_mm"), cfg.get("vias", "fallback_drill_mm")
        # the fallback as run.apply_via_fallback writes it (the board's minimums still apply)
        d_nm = max(int(round(d * _NM)), int(rules.get("min_via_diameter") or 0))
        r_nm = max(int(round(r * _NM)), int(rules.get("min_via_drill") or 0))
        extra = smaller_profile_vias(rules, d_nm, r_nm,
                                     [int(round(float(x) * _NM)) for x in cfg.get("vias", "allowed_drills_mm")])
        more = ("; where space requires, also the smaller pre-defined via(s) "
                + ", ".join(f"{_mm(a)} / {_mm(b)} mm" for a, b in extra)) if extra else ""
        out.append(_mk("VIA_NOT_SET_UP",
                       f"KiCad has no via sizes set up (only the factory 0.8 / 0.4 mm net class via); the nets "
                       f"without a via size of their own get {_mm(d_nm)} / {_mm(r_nm)} mm vias (diameter / drill, "
                       f"vias.fallback_*){more}.",
                       ("vias.fallback_diameter_mm", "vias.fallback_drill_mm")))
    allowed = [float(x) for x in cfg.get("vias", "allowed_drills_mm")]
    if allowed:
        used = {int(v["drill"]) for v in model.get("vias", []) if v.get("drill")}
        for r in [rules.get("default") or {}, *(rules.get("per_net") or {}).values()]:
            if r.get("via_drill"):
                used.add(int(r["via_drill"]))
        bad = sorted(d for d in used if not any(abs(d / _NM - a) < 0.001 for a in allowed))
        if bad:
            out.append(_mk("DRILL_OUTSIDE_FILTER",
                           f"Via drill(s) {', '.join(_mm(d) for d in bad)} mm are not in vias.allowed_drills_mm "
                           f"({', '.join(f'{a:g}' for a in allowed)}).",
                           (_mm(d) for d in bad)))
    if cfg.get("vias", "via_in_pad") and pcb_text is not None:
        flags = dict(re.findall(r"\((capping|filling)\s+(yes|no)\)", pcb_text))
        if not (flags.get("capping") == "yes" and flags.get("filling") == "yes"):
            out.append(_mk("VIA_IN_PAD_UNPROTECTED",
                           "vias.via_in_pad is on but KiCad's via protection is not filled and capped "
                           f"(capping {flags.get('capping', 'not set')}, filling {flags.get('filling', 'not set')}).",
                           ("vias.via_in_pad",)))
    return out


# ------------------------------------------------------------------ outline / pads
def outline_rings(outline: list, max_err: int = 5000) -> list | None:
    """Closed rings (lists of [x, y] nm) chained from the model's Edge.Cuts primitives, arcs flattened to
    `max_err`; None when the outline is empty or any chain stays open. O(E^2) worst case, O(E) typical."""
    segs = []
    for p in outline:
        if p["kind"] == "seg":
            segs.append([list(p["a"]), list(p["b"])])
        elif p["kind"] == "arc":
            pts = geom.flatten_arc(p["start"], p["mid"], p["end"], max_err)
            segs.append([list(q) for q in pts])
    if not segs:
        return None

    def near(a, b):
        return abs(a[0] - b[0]) <= _TOL and abs(a[1] - b[1]) <= _TOL

    rings = []
    while segs:
        ring = segs.pop(0)
        progress = True
        while progress and not near(ring[0], ring[-1]):
            progress = False
            for i, s in enumerate(segs):
                if near(ring[-1], s[0]):
                    ring += s[1:]
                elif near(ring[-1], s[-1]):
                    ring += s[::-1][1:]
                else:
                    continue
                segs.pop(i)
                progress = True
                break
        if not near(ring[0], ring[-1]) or len(ring) < 4:
            return None
        rings.append(ring[:-1])
    return rings


def _pad_findings(model: dict, cfg) -> list:
    rings = outline_rings(model.get("outline", []))
    if not rings:
        return []
    ignored = set(cfg.get("nets", "ignore_pads"))
    bad = []
    for p in model.get("pads", []):
        ref = f"{p['ref']}.{p['number']}"
        if not p.get("net") or ref in ignored:
            continue
        if not geom.point_in_rings(tuple(p["center"]), rings):
            bad.append(ref)
    if not bad:
        return []
    return [_mk("PAD_OUTSIDE_BOARD",
                f"{len(bad)} pad(s) have their centre outside the board outline: {', '.join(bad[:8])}"
                + (" ..." if len(bad) > 8 else "") + ".", bad)]


# ------------------------------------------------------------------ small checks
def preexisting_drc(rep, cfg) -> Finding | None:
    """PREEXISTING_DRC for the DRC of the untouched copy: info, or error with ``run.fail_on_preexisting``. O(1)."""
    if rep is None:
        return None
    n, u = len(rep.violations), len(rep.unconnected)
    if not n and not u:
        return None
    types = ", ".join(f"{t} {c}" for t, c in sorted(rep.by_type.items(), key=lambda kv: (-kv[1], kv[0]))[:6])
    fail = bool(cfg.get("run", "fail_on_preexisting"))
    return _mk("PREEXISTING_DRC",
               f"KiCad DRC already reports {n} violation(s) and {u} unconnected item(s) on the board before "
               f"routing" + (f" ({types})" if types else "")
               + ("; run.fail_on_preexisting is on, so the run stops." if fail else "; they are listed as pre-existing."),
               severity="error" if fail else "info")


def _rule_findings(rules: dict) -> list:
    names = []
    for w in rules.get("warnings", []):
        m = re.match(r"REFUSE: rule (.+?) restricts (track angle|track segment length)", w)
        if m and m.group(1) not in names:
            names.append(m.group(1))
    if not names:
        return []
    return [_mk("TRACK_ANGLE_RULE",
                f"Custom rule(s) {', '.join(names)} restrict track angle or segment length, which curved traces cannot satisfy.",
                names)]


#: The run log / report line of a 0 copper-to-edge clearance (user 2026-11-13: "just leave it at 0 ... do warn").
EDGE_ZERO_TEXT = "copper-to-edge clearance is 0 (KiCad → File → Board Setup → Design Rules → Constraints → Copper to edge clearance)"


def _edge_zero_findings(rules: dict) -> list:
    """EDGE_CLEARANCE_ZERO: the resolved copper-to-edge clearance is 0 (Board Setup 0, no custom rule raising it). The
    value is used as it is (no fallback); this only warns. O(1)."""
    if rules.get("edge_clearance") is None or int(rules["edge_clearance"]) > 0:
        return []
    return [_mk("EDGE_CLEARANCE_ZERO", EDGE_ZERO_TEXT + ".")]


def _rule_fallback_findings(rules: dict) -> list:
    """RULE_FALLBACK: one finding listing every design-rule field the board does not provide and the value used
    instead (toporlib.rules RULE_FALLBACK; one item per field, naming the KiCad setting to fill in). O(fields)."""
    from toporlib.rules import fallback_text  # noqa: PLC0415

    rows = rules.get("fallbacks") or []
    if not rows:
        return []
    texts = [fallback_text(r) for r in rows]
    return [_mk("RULE_FALLBACK", f"{len(rows)} design rule(s) not set by the board (or outside KiCad's range), "
                "fallback used: "
                + "; ".join(f"{r['label']} {r['value'] / _NM:g} mm" for r in rows) + ".", texts)]


def _custom_rule_findings(rules: dict) -> list:
    """CUSTOM_RULES (how each .kicad_dru rule is used) and CUSTOM_RULE_LIMITED (the rules topor applies only
    conservatively or not at all, each named). O(rules)."""
    rows = rules.get("custom_rules") or []
    if not rows:
        return []
    used = [r for r in rows if r.get("use") in ("net", "net+pad", "pad", "region", "board", "net (precaution)")]
    out = []
    if used:
        txt = "; ".join(f"{r['rule']}: {r['constraint']} -> {r['use']}" for r in used[:12])
        out.append(_mk("CUSTOM_RULES", f"{len(used)} custom rule constraint(s) applied ({txt}"
                       + (" ..." if len(used) > 12 else "") + ").", sorted({r["rule"] for r in used})))
    notes = list(rules.get("custom_notes") or [])
    if notes:
        out.append(_mk("CUSTOM_RULE_LIMITED", f"{len(notes)} custom-rule note(s): " + " | ".join(notes[:6])
                       + (" ..." if len(notes) > 6 else ""), notes[:20]))
    return out


def _copper_findings(model: dict, cfg) -> list:
    out = []
    locked = [k["uuid"] for k in model.get("kept", []) if k.get("locked")] + \
             [v["uuid"] for v in model.get("vias", []) if v.get("locked")]
    if locked:
        respect = bool(cfg.get("run", "respect_locked"))
        out.append(_mk("LOCKED_COPPER",
                       f"{len(locked)} locked track/via item(s) " +
                       ("are kept as fixed obstacles." if respect else
                        "may be ripped or smoothed because run.respect_locked is off."),
                       locked[:20], severity="info" if respect else "warning"))
    dang = model.get("dangling_ends", [])
    if dang:
        act = cfg.get("search", "dangling_ends")
        out.append(_mk("DANGLING_ENDS",
                       f"{len(dang)} existing track end(s) connect to nothing; search.dangling_ends = {act!r}: "
                       + {"continue": "the new copper continues them tangentially.",
                          "fixed": "they stay fixed points; the new copper may leave at an angle.",
                          "drop": "they are removed and routed from the pad."}.get(str(act), "they are continued."),
                       [d.get("uuid", "") for d in dang[:20]]))
    return out


def _do_not_touch(model: dict, cfg) -> list:
    pats = list(cfg.get("nets", "do_not_touch"))
    if not pats:
        return []
    names = [n["name"] for n in model.get("nets", [])]
    hit, miss = [], []
    for p in pats:
        m = [n for n in names if fnmatch.fnmatchcase(n, p)]
        (hit if m else miss).append((p, m))
    out = []
    if hit:
        nets = sorted({n for _, m in hit for n in m})
        out.append(_mk("DO_NOT_TOUCH", f"{len(nets)} net(s) are fixed obstacles (nets.do_not_touch): {', '.join(nets[:8])}"
                       + (" ..." if len(nets) > 8 else "") + ".", nets, severity="info"))
    if miss:
        out.append(_mk("DO_NOT_TOUCH", "nets.do_not_touch pattern(s) match no net: " + ", ".join(p for p, _ in miss) + ".",
                       [p for p, _ in miss], severity="warning"))
    return out


def connection_count(model: dict) -> tuple:
    """(connections, nets with >= 2 pads): spanning-tree estimate, pads - 1 per net. O(pads)."""
    pads: dict = {}
    for p in model.get("pads", []):
        if p.get("net"):
            pads[p["net"]] = pads.get(p["net"], 0) + 1
    multi = [n for n, c in pads.items() if c >= 2]
    return sum(pads[n] - 1 for n in multi), len(multi)


def _info_findings(model: dict, rules: dict, cfg) -> list:
    out = []
    nd = cfg.non_defaults()
    if nd:
        out.append(_mk("NON_DEFAULTS", f"{len(nd)} setting(s) differ from the defaults: "
                       + ", ".join(f"{k} = {v!r}" for k, v in list(nd.items())[:8]) + (" ..." if len(nd) > 8 else "") + ".",
                       list(nd)))
    names = {str(n["code"]): n["name"] for n in model.get("nets", [])}
    dflt = sorted(names.get(code, code) for code, r in (rules.get("per_net") or {}).items()
                  if r.get("width_source") == "default")
    if dflt:
        out.append(_mk("DEFAULTS_USED", f"{len(dflt)} net(s) use topor's default track width (no netclass, existing "
                       f"copper or setting): {', '.join(dflt[:8])}" + (" ..." if len(dflt) > 8 else "") + ".", dflt))
    return out


def _diff_pairs(model: dict, cfg) -> list:
    """DIFF_PAIRS_SKIPPED (K1, D7): with ``[diffpair] enabled = false`` the topological core leaves the connections of
    the board's differential pairs (``model["rules"]["diff_pairs"]``, KiCad's name rule in ``toporlib.diffpair``;
    without that list the same rule on the net names) unrouted: the pairs whose halves have two or more pads.
    do_not_touch nets are left out (they are not routed anyway). With pairs enabled: nothing. O(N + pads)."""
    from toporlib.diffpair import detect_pairs  # noqa: PLC0415

    from .scope import do_not_touch_nets  # noqa: PLC0415

    if cfg.get("diffpair", "enabled"):
        return []
    pads: dict = {}
    for p in model.get("pads", []):
        if p.get("net"):
            pads[p["net"]] = pads.get(p["net"], 0) + 1
    skip = do_not_touch_nets(model, cfg)
    code = {n["name"]: n["code"] for n in model.get("nets", [])}
    listed = (model.get("rules") or {}).get("diff_pairs")
    pairs = [(d["p_name"], d["n_name"]) for d in listed] if listed is not None else detect_pairs(code)
    nets = sorted(x for pr in pairs if all(pads.get(code.get(h), 0) >= 2 and h not in skip for h in pr) for x in pr)
    if not nets:
        return []
    return [_mk("DIFF_PAIRS_SKIPPED",
                f"{len(nets) // 2} differential pair(s) will not be routed ([diffpair] enabled = false): "
                f"{', '.join(nets[:10])}" + (" ..." if len(nets) > 10 else "") + ". Route them by hand, or set "
                "[diffpair] enabled = true.", nets)]


def _keepout_pads(model: dict) -> list:
    """KEEPOUT_PAD: a pad of a net with two or more pads whose centre lies inside track keepouts on every copper
    layer of the pad. O(pads * keepouts * points)."""
    kos = [k for k in model.get("keepouts", []) if k.get("no_tracks") and len(k.get("poly") or []) >= 3]
    if not kos:
        return []
    count: dict = {}
    for p in model.get("pads", []):
        if p.get("net"):
            count[p["net"]] = count.get(p["net"], 0) + 1
    hit = []
    for p in model.get("pads", []):
        if count.get(p.get("net"), 0) < 2 or not p.get("layers"):
            continue
        c = p["center"]
        blocked = [l for l in p["layers"] if any(l in (k.get("layers") or []) and geom.point_in_ring(c, k["poly"])
                                                  for k in kos)]
        if len(blocked) == len(p["layers"]):
            hit.append(f"{p['ref']}.{p['number']}")
    if not hit:
        return []
    return [_mk("KEEPOUT_PAD", f"{len(hit)} pad(s) lie inside a track keepout on all their copper layers: "
                + ", ".join(hit[:10]) + (" ..." if len(hit) > 10 else "") + ".", hit)]


def _groups(pcb_text: str | None) -> list:
    if not pcb_text:
        return []
    n = len(re.findall(r"^\s*\(group\b", pcb_text, re.M))
    if not n:
        return []
    return [_mk("GROUPS_UNAVAILABLE", f"The board has {n} group(s); group membership is not visible to topor.", (str(n),))]


# ------------------------------------------------------------------ entry point
def preflight(model: dict, rules: dict, cfg, sel, orig_drc, cfg_findings, *, pcb_text: str | None = None,
              pro: dict | None = None) -> list:
    """All pre-flight findings, config findings first. See the module docstring. O(see module)."""
    out = list(cfg_findings)
    layers = list(model.get("layers", []))
    if pcb_text is not None:
        ver = pcbfile.file_version(pcb_text)
        if ver is None or ver < pcbfile.KICAD10_VERSION:
            out.append(_mk("PCB_FORMAT_OLD",
                           f"The board file format (version {ver}) is older than KiCad 10 ({pcbfile.KICAD10_VERSION}); "
                           "topor edits the file text and would silently skip items written in the old format.",
                           (str(ver),)))
        copper = [(g["name"], g["type"]) for g in pcbfile.parse_layers(pcb_text)]
        roles, rfind = resolve_layer_roles(copper, cfg.layer_roles)
        out += rfind
        # a model layer without a role takes the one resolved from the saved copy
        layers = [dict(g, role=roles.get(g["name"])) if g.get("role") is None else g for g in layers]
    if not Path(kicad_cli()).is_file():
        out.append(_mk("KICAD_CLI_MISSING", f"kicad-cli was not found at {kicad_cli()}.", (str(kicad_cli()),)))
    out += _plane_findings(model, cfg, layers)
    out += _layer_type_findings(layers)
    out += _via_findings(model, rules, cfg, pcb_text, pro)
    out += _pad_findings(model, cfg)
    out += _rule_findings(rules)
    out += _rule_fallback_findings(rules)
    out += _edge_zero_findings(rules)
    out += _custom_rule_findings(model.get("rules") or rules)
    out += _copper_findings(model, cfg)
    out += _do_not_touch(model, cfg)
    out += _diff_pairs(model, cfg)
    out += _keepout_pads(model)
    conns, _ = connection_count(model)
    if conns == 0:
        out.append(_mk("SCOPE_EMPTY", "There is no connection to route: no net has two or more pads in scope."))
    out += _info_findings(model, rules, cfg)
    out += _groups(pcb_text)
    pd = preexisting_drc(orig_drc, cfg)
    if pd is not None:
        out.append(pd)
    return out


__all__ = ["preflight", "preexisting_drc", "connection_count", "outline_rings", "via_setup_missing"]
