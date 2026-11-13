"""Post-run findings (K1): the topological core's report (``RouteResult`` JSON: ``connections`` with their status /
reason and the ``warnings`` lines) turned into findings with a code, the concrete numbers and an actionable fix,
so they reach the pre-flight-style findings list of report.md and the preview summary instead of staying in
result.json.

Codes (catalogued in messages.py, one section each in docs/user/troubleshoot.md):
    DIFF_PAIRS          differential-pair connections routed (info): per pair connection lengths, skew, coupled %
    DIFF_PAIRS_SKIPPED  connections of differential pairs left unrouted because [diffpair] enabled = false
    UNROUTED            connections the router could not finish, grouped by reason
    CONNECTION_WIDTH    connections dropped because a track would graze its own pad / via (KiCad's minimum
                        connection width check)
    POUR_ISLANDS        pour pieces that stay cut off from their pour after the stitching
    POUR_STITCHED       stitching vias / pad stubs topor added to join cut-off pour pieces (info)
    PLANE_PAD_SIGNAL    plane pads that got no stub into their pour and were joined by a trace instead (info):
                        to the pour's filled copper (planes.terminals) or to another pad of the net
    PLANE_VIA_OUTSIDE_POUR  a plane stub via that ended outside the filled copper of its plane
    VIA_SQUEEZE         vias placed in a narrow pass (the via_squeeze social factor's report)
    NECKED              stretches drawn narrower than the net width to get out of a pad (info)
    IGNORED_RULE        copper that breaks a KiCad check set to "ignore" (priced, not forbidden)
    VIOLATIONS          never drop: copper written although it breaks a rule (the drawing found no legal
                        geometry), one item per violation with place, type and cause (``result["violations"]``)
    TIME_BUDGET_HIT     run.timeout_s stopped the topological core before its work caps
    DRC_NEW_VIOLATIONS  KiCad's DRC of the routed copy finds new violations (counts by type, the first places);
                        a warning: the result can still be accepted ("costly, not failed")
    RULES_MISMATCH      self-check (error): KiCad's DRC required a larger clearance between two nets than the
                        clearance the engine was given for them, i.e. topor read the design rules wrong

The engine's warning lines are parsed by their stable prefixes (the formats are pinned by tests/plugin/
test_postrun.py with lines copied from real runs); an unknown line is simply not turned into a finding (it stays
in result.json and the report's engine notes).

Complexity: O(connections + warning lines).
"""

from __future__ import annotations

import re
from collections import Counter

from .findings import Finding
from .messages import MESSAGES

#: every code route_findings can emit (the preview summary names those present)
POSTRUN_CODES = ("DIFF_PAIRS", "DIFF_PAIR_VIA_FORCED", "DIFF_PAIRS_SKIPPED", "POUR_UNJOINED", "UNROUTED", "CONNECTION_WIDTH", "POUR_ISLANDS", "POUR_STITCHED", "PLANE_PAD_SIGNAL",
                 "PLANE_VIA_OUTSIDE_POUR", "VIA_SQUEEZE", "NECKED", "IGNORED_RULE", "VIOLATIONS", "TIME_BUDGET_HIT", "DRC_NEW_VIOLATIONS", "RULES_MISMATCH")
DIFF_PAIR_REASON = "diff pair: not in the rough version"  # engine/src/topo/core.rs DIFF_PAIR
_ITEMS = 40  # items kept per finding (the report lists them; result.json keeps all engine lines)


def _mk(code: str, message: str, items=(), severity: str | None = None) -> Finding:
    m = MESSAGES[code]
    return Finding(code, severity or m.severity, message, m.fix, tuple(list(items)[:_ITEMS]))


def _reason_class(reason: str) -> str:
    r = reason or ""
    if r.startswith("via ") and "no site" in r:
        return "no room for a via"
    if "minimum connection width" in r:
        return "minimum connection width"
    if r.startswith("drawing: clearance gap"):
        return "no room at the clearance when drawn"
    if "branch" in r:
        return "its branch point could not be drawn"
    if r.startswith("time limit"):
        return "time limit"
    if r.startswith("drawing:"):
        return "drawing"
    return r.split(":")[0] if r else "unknown"


def _label(c: dict) -> str:
    f, t = (c.get("from") or {}).get("label", "?"), (c.get("to") or {}).get("label", "?")
    return f"{c.get('name') or '?'} {f} -> {t}"


def _violation_text(v: dict) -> str:
    """One VIOLATIONS item: connection, type, place, depth and length, what it is too close to, and the
    connection's first failed check (per connection: why its drawing found no legal geometry, not this item)."""
    at = v.get("at") or [0, 0]
    return (f"{v.get('name') or '?'} {v.get('from', '?')} -> {v.get('to', '?')}: {v.get('kind')} at "
            f"({at[0] / 1e6:.3f}, {at[1] / 1e6:.3f}) mm on {v.get('layer')}, {int(v.get('depth_nm') or 0) / 1e6:.3f} mm "
            f"below the rule over {int(v.get('length_nm') or 0) / 1e6:.3f} mm against {v.get('against')}; "
            f"first failed check of the connection: {v.get('cause')}")


def route_findings(result: dict | None) -> list:
    """Findings from one engine result dict (see the module doc). O(connections + warnings)."""
    if not result:
        return []
    out: list = []
    conns = result.get("connections") or []
    failed = [c for c in conns if c.get("status") != "routed" and c.get("status") not in ("ignored", "excluded")]
    diff = [c for c in failed if c.get("reason") == DIFF_PAIR_REASON]
    if diff:
        nets = sorted({c.get("name") or "?" for c in diff})
        out.append(_mk("DIFF_PAIRS_SKIPPED",
                       f"{len(diff)} connection(s) of {len(nets)} differential-pair net(s) were not routed: "
                       f"{', '.join(nets[:10])}{' ...' if len(nets) > 10 else ''} ([diffpair] enabled = false). "
                       "Route these by hand, or set [diffpair] enabled = true.", nets))
    dps = result.get("diff_pairs") or []
    if dps:
        lines = [str(x) for x in result.get("warnings") or [] if str(x).startswith("diff pair ")]
        worst = max(abs(int(d.get("skew_nm") or 0)) for d in dps) / 1e6
        cpl = sorted(float(d.get("coupled_pct") or 0) for d in dps)
        out.append(_mk("DIFF_PAIRS",
                       f"{len(dps)} differential-pair connection(s) routed as coupled pairs: coupled at the gap "
                       f"{cpl[0]:.0f}-{cpl[-1]:.0f} % (median {cpl[len(cpl) // 2]:.0f} %), largest skew {worst:.3f} mm.",
                       lines))
    pvv = [str(x)[len("pair via violations: "):] for x in result.get("warnings") or [] if str(x).startswith("pair via violations: ")]
    if pvv:
        out.append(_mk("DIFF_PAIR_VIA_FORCED",
                       f"{len(pvv)} differential-pair via pair(s) break a clearance rule in the routed copper "
                       "(how each was placed and its violations are listed).", pvv))
    width = [c for c in failed if c not in diff and "minimum connection width" in (c.get("reason") or "")]
    if width:
        out.append(_mk("CONNECTION_WIDTH",
                       f"{len(width)} connection(s) were not drawn because the track would graze a pad or via of "
                       "its own net and leave a neck thinner than KiCad's minimum connection width.",
                       [_label(c) for c in width]))
    rest = [c for c in failed if c not in diff]
    if rest:
        by = Counter(_reason_class(c.get("reason") or "") for c in rest)
        out.append(_mk("UNROUTED",
                       f"{len(rest)} of {len(conns)} connection(s) are not routed: "
                       + ", ".join(f"{n} {k}" for k, n in by.most_common()) + ".",
                       [f"{_label(c)}: {c.get('reason')}" for c in rest]))
    w = [str(x) for x in result.get("warnings") or []]

    def lines(prefix=None, contains=None):
        return [x for x in w if (prefix is None or x.startswith(prefix)) and (contains is None or contains in x)]

    st = next((x for x in w if x.startswith("pour stitching:")), None)
    m = re.match(r"pour stitching: (\d+) pour pieces cut off by the routed copper before, (\d+) after; "
                 r"(\d+) island vias, (\d+) pad stubs", st or "")
    left = lines("no stitch site for ")
    if m and int(m.group(2)) > 0 or left:
        n_after = int(m.group(2)) if m else len(left)
        out.append(_mk("POUR_ISLANDS",
                       f"{n_after} piece(s) of copper pours are cut off from their pour by the new copper and could "
                       "not be stitched back; KiCad's DRC lists them as unconnected items after a zone refill.",
                       [x.removeprefix("no stitch site for ") for x in left]))
    stitched = lines("stitch via ") + lines("pad stub ")
    if m and (int(m.group(3)) or int(m.group(4))):
        out.append(_mk("POUR_STITCHED",
                       f"{m.group(3)} stitching via(s) and {m.group(4)} pad stub(s) join pour pieces the new copper "
                       f"cut off ({m.group(1)} pieces were cut off).", stitched))
    sig = lines(contains="joined to its net's copper by a signal connection instead")
    # 4.4 (planes.terminals): such a pad is wired to its pour's filled copper instead.
    far = lines(contains="wired to its pour's filled copper at")
    if sig or far:
        both = sig + far
        out.append(_mk("PLANE_PAD_SIGNAL",
                       f"{len(both)} pad(s) of plane nets had no room for a stub and via into their pour and are "
                       f"joined by a trace instead ({len(far)} to their pour's filled copper, {len(sig)} to another "
                       "pad of their net): " + ", ".join(x.split(":")[0].removeprefix("pad ") for x in both[:10])
                       + (" ..." if len(both) > 10 else "") + ".", [x.split(":")[0] for x in both]))
    isl = lines("plane island: ")
    if isl:
        out.append(_mk("PLANE_VIA_OUTSIDE_POUR",
                       f"{len(isl)} plane via(s) ended outside the filled copper of their plane.",
                       [x.removeprefix("plane island: ") for x in isl]))
    vs = next((x for x in w if x.startswith("via_squeeze: ")), None)
    m2 = re.match(r"via_squeeze: (\d+) of (\d+) wire vias", vs or "")
    if m2 and int(m2.group(1)):
        out.append(_mk("VIA_SQUEEZE",
                       f"{m2.group(1)} of {m2.group(2)} vias stand in a narrow pass (less slack than half the via "
                       "diameter next to them); KiCad's DRC passed them, but there is little room left for edits.",
                       lines(contains="via in a narrow pass")))
    nk = lines(contains=": necked from ")
    if nk:
        out.append(_mk("NECKED", f"{len(nk)} stretch(es) are narrower than their net's track width (to get out of "
                       "a pad).", nk))
    viol = result.get("violations") or []
    if viol:
        by = Counter(str(v.get("kind")) for v in viol)
        nconn = len({v.get("conn") for v in viol})
        cls = Counter(str(v.get("class")) for v in viol if v.get("class"))
        out.append(_mk("VIOLATIONS",
                       f"{len(viol)} rule violation(s) in the copper of {nconn} connection(s), written on purpose "
                       "(never drop: the drawing found no legal geometry, so they are drawn on their best one): "
                       + ", ".join(f"{n} {k}" for k, n in by.most_common()) + "."
                       + (" By origin: " + ", ".join(f"{n} {_VIOL_CLASS.get(k, k)}" for k, n in cls.most_common()) + "."
                          if cls else "")
                       + _short_cleanup_note(viol),
                       [_violation_text(v) for v in viol]))
    ig = lines("ignored_rule: ")
    if ig:
        out.append(_mk("IGNORED_RULE", ig[0].removeprefix("ignored_rule: ") + ".", ig))
    tb = lines("time budget hit")
    if tb:
        out.append(_mk("TIME_BUDGET_HIT", tb[0] + ".", tb))
    return out


def _short_cleanup_note(viol: list) -> str:
    """What KiCad's Edit -> Cleanup Tracks & Vias does to the connections with a short (qmtech 2026-11-11: the user's
    cleanup removed 107 shorting tracks, then 290 tracks and vias of the same 28 nets as unconnected at one end;
    "Delete tracks unconnected at one end" alone removes nothing). Empty without shorts. O(violations)."""
    nets = sorted({str(v.get("name") or v.get("net")) for v in viol if v.get("kind") == "short"})
    if not nets:
        return ""
    return (f" The {len(nets)} net(s) with a short ({', '.join(nets[:6])}{' ...' if len(nets) > 6 else ''}) lose "
            "their routed copper in KiCad → Edit → Cleanup Tracks & Vias: \"Delete tracks connecting different nets\" "
            "removes the shorting tracks, then \"Delete tracks unconnected at one end\" the rest of those traces up to "
            "their pads; fix the shorts first (Inspect → Design Rules Checker), then run the cleanup.")


#: Origin classes of a violation (engine ``violation_class``, M-B2).
_VIOL_CLASS = {
    "pad_exit": "at a pad exit (a neighbour pin of its own part; necking and pad exits are planned later)",
    "via_site": "from a via without a legal site",
    "crossing": "at a crossing with another net",
    "foreign_copper": "against other routed copper in a narrow passage",
    "static": "against other pads, holes or the board edge",
}


def drc_pour_findings(unconnected) -> list:
    """POUR_UNJOINED from KiCad's DRC of the routed copy after the zone refill (``toporlib.drc.ratsnest_marks``):
    zone <-> zone unconnected pairs = pour pieces cut off (per net), item <-> zone pairs = pads / vias / tracks not
    joined to their pour. The same counts the preview legend and the compare PNG captions show. O(U)."""
    from toporlib.drc import ratsnest_marks  # noqa: PLC0415

    mk = ratsnest_marks(unconnected)
    n_isl = sum(mk["islands"].values())
    if not n_isl and not mk["unjoined"]:
        return []
    parts = [f"{n} {net} pour piece{'s' if n != 1 else ''} cut off from the rest of the pour"
             for net, n in sorted(mk["islands"].items())]
    if mk["unjoined"]:
        parts.append(f"{len(mk['unjoined'])} item(s) not joined to their pour: " + ", ".join(mk["unjoined"][:8])
                     + (" ..." if len(mk["unjoined"]) > 8 else ""))
    return [_mk("POUR_UNJOINED", "KiCad's DRC after the zone refill: " + "; ".join(parts) + ".",
                [f"{net}: {n} piece(s) cut off" for net, n in sorted(mk["islands"].items())] + mk["unjoined"])]


# ------------------------------------------------------------------------------------- KiCad DRC of the result
#: places named in the DRC_NEW_VIOLATIONS message (all of them, up to _ITEMS, are in its items)
DRC_PLACES_SHOWN = 3
#: KiCad's clearance text in a DRC description: "( clearance 0,2500 mm; actual 0,2000 mm)" (the decimal mark
#: follows the locale kicad-cli runs in)
_CLEARANCE = re.compile(r"clearance\s+([0-9]+(?:[.,][0-9]+)?)\s*(mm|mils?|in)\b.*?actual\s+([0-9]+(?:[.,][0-9]+)?)\s*"
                        r"(mm|mils?|in)\b", re.I)
_UNIT_NM = {"mm": 1_000_000, "mil": 25_400, "mils": 25_400, "in": 25_400_000}
_NET_IN_ITEM = re.compile(r"\[([^\]]*)\]")
#: KiCad prints a value with 4 decimals (0.1 um); a required clearance up to this much above the engine's is equal
MISMATCH_TOLERANCE_NM = 500


def _vd(v) -> dict:
    """A DRC violation as a dict (``toporlib.drc.Violation`` or the JSON entry). O(items)."""
    if isinstance(v, dict):
        return v
    return {"type": v.type, "severity": v.severity, "description": v.description, "items": list(v.items)}


def required_clearance_nm(description: str) -> tuple[int, int] | None:
    """(required, actual) nm from a KiCad clearance description ("clearance 0,2500 mm; actual 0,2000 mm"; a decimal
    comma or point; mm, mil or in), None when the text carries no such pair. O(len)."""
    m = _CLEARANCE.search(description or "")
    if not m:
        return None

    def nm(num, unit):
        return int(round(float(num.replace(",", ".")) * _UNIT_NM[unit.lower()]))

    return nm(m.group(1), m.group(2)), nm(m.group(3), m.group(4))


def item_net(item: dict) -> str | None:
    """Net name of a DRC item from its description ("Track [A13] on B.Cu ..." -> "A13"); None without a net. O(1)."""
    m = _NET_IN_ITEM.search(str(item.get("description", "")))
    if not m or m.group(1).startswith("<"):  # "<no net>"
        return None
    return m.group(1)


def _place_text(v: dict, at) -> str:
    """One DRC_NEW_VIOLATIONS item: type, nets, place (mm), layer, KiCad's own numbers. O(items)."""
    nets = [n for n in (item_net(i) for i in v.get("items", [])) if n]
    layer = next((m.group(1) for i in v.get("items", [])
                  if (m := re.search(r" on ([A-Za-z0-9_.]+)", str(i.get("description", ""))))), "")
    desc = re.sub(r"^[^(]*\(\s*|\)\s*$", "", str(v.get("description", ""))).strip()
    desc = re.sub(r"(\d),(\d)", r"\1.\2", desc)  # KiCad's locale decimal comma -> the point the rest uses
    where = f" at ({at[0]:.3f}, {at[1]:.3f}) mm" if at is not None else ""
    return (f"{v.get('type')} {' – '.join(dict.fromkeys(nets)) or 'no net'}{where}"
            f"{f' on {layer}' if layer else ''}{f': {desc}' if desc else ''}")


def drc_new_findings(new_violations, tracks: dict | None = None) -> list:
    """DRC_NEW_VIOLATIONS (warning) from the routing violations of KiCad's DRC of the routed copy that are new
    (``toporlib.drc.new_routing``): counts by type, the first places with nets and position (mm, between the two
    items when `tracks` = ``pcbfile.track_lines`` of the checked board gives their geometry). It never blocks
    Accept: costly, not failed. [] without new violations. O(V x pieces per item)."""
    from toporlib.drc import violation_marks  # noqa: PLC0415

    vs = [_vd(v) for v in new_violations or ()]
    if not vs:
        return []
    by = Counter(v.get("type", "?") for v in vs)
    types = ", ".join(f"{t} {n}" for t, n in sorted(by.items(), key=lambda kv: (-kv[1], kv[0])))
    marks = []
    for v in vs:  # one marker per violation (violation_marks skips one that names only zones)
        mk = violation_marks([v], tracks)
        marks.append(mk[0][:2] if mk else None)
    places = [_place_text(v, at) for v, at in zip(vs, marks)]
    shown = "; ".join(places[:DRC_PLACES_SHOWN]) + (f"; … and {len(places) - DRC_PLACES_SHOWN} more"
                                                     if len(places) > DRC_PLACES_SHOWN else "")
    return [_mk("DRC_NEW_VIOLATIONS",
                f"KiCad's DRC finds {len(vs)} new violation{'s' if len(vs) != 1 else ''} in the result ({types}): "
                f"{shown}.", places)]


def _engine_clearances(model: dict) -> tuple[dict, dict, int]:
    """(net name -> clearance nm the engine was given, item uuid -> its own clearance nm (pad override, pour),
    default clearance nm) from the engine's model (``model["rules"]["per_net"]`` keyed by net code). O(N + pads)."""
    rules = model.get("rules") or {}
    per_net = rules.get("per_net") or {}
    default = int((rules.get("default") or {}).get("clearance") or 0)
    by_name = {}
    for n in model.get("nets") or ():
        r = per_net.get(str(int(n.get("code", 0))))
        if r is not None and n.get("name"):
            by_name[n["name"]] = int(r.get("clearance") or 0)
    local = {p["uuid"]: int(p["local_clearance"]) for p in model.get("pads") or ()
             if p.get("uuid") and p.get("local_clearance")}
    local.update({z["uuid"]: int(z["clearance"]) for z in model.get("pours") or ()
                  if z.get("uuid") and z.get("clearance")})
    return by_name, local, default


def rules_mismatch(new_violations, model: dict) -> "Finding | None":
    """Self-check RULES_MISMATCH (error): for every new ``clearance`` violation of KiCad's DRC, the clearance KiCad
    required (parsed from its text, decimal comma or point) against the largest clearance the engine was given for
    the two items (their nets' ``per_net`` clearance, a pad's own override, a pour's clearance). KiCad requiring
    more than that (beyond MISMATCH_TOLERANCE_NM) means topor read the design rules wrong: the whole result was
    routed to the wrong rule. Pairs of the two nets of a differential pair are skipped (their gap is the
    clearance between them, by design). None when every violation agrees. O(V)."""
    by_name, local, default = _engine_clearances(model or {})
    pairs = {frozenset((d.get("p_name"), d.get("n_name")))
             for d in ((model or {}).get("rules") or {}).get("diff_pairs") or ()}
    worst: dict = {}  # (nets) -> (required, given, actual)
    for v in (_vd(x) for x in new_violations or ()):
        if v.get("type") != "clearance":
            continue
        req = required_clearance_nm(v.get("description", ""))
        items = v.get("items") or []
        nets = tuple(sorted({n for n in (item_net(i) for i in items) if n}))
        if req is None or not nets or (len(nets) == 2 and frozenset(nets) in pairs):
            continue
        given = max([by_name.get(n, default) for n in nets]
                    + [local[i["uuid"]] for i in items if i.get("uuid") in local])
        if req[0] > given + MISMATCH_TOLERANCE_NM:
            prev = worst.get(nets)
            if prev is None or req[0] - given > prev[0] - prev[1]:
                worst[nets] = (req[0], given, req[1])
    if not worst:
        return None
    rows = [f"{' – '.join(n)}: KiCad requires {r / 1e6:.4f} mm, topor routed with {g / 1e6:.4f} mm "
            f"(actual {a / 1e6:.4f} mm)" for n, (r, g, a) in sorted(worst.items())]
    return _mk("RULES_MISMATCH",
               f"topor read the design rules wrong — please report this. KiCad's DRC requires a larger clearance than "
               f"the router was given for {len(rows)} net pair{'s' if len(rows) != 1 else ''}: "
               + "; ".join(rows[:4]) + (f"; … and {len(rows) - 4} more" if len(rows) > 4 else "")
               + ". Nothing was applied to the board.", rows)


__all__ = ["route_findings", "drc_pour_findings", "drc_new_findings", "rules_mismatch", "required_clearance_nm",
           "DIFF_PAIR_REASON", "POSTRUN_CODES"]
