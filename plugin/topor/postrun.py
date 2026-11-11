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
    PLANE_PAD_SIGNAL    plane pads that got no stub into their pour and were joined by a trace instead (info)
    PLANE_VIA_OUTSIDE_POUR  a plane stub via that ended outside the filled copper of its plane
    VIA_SQUEEZE         vias placed in a narrow pass (the via_squeeze social factor's report)
    NECKED              stretches drawn narrower than the net width to get out of a pad (info)
    IGNORED_RULE        copper that breaks a KiCad check set to "ignore" (priced, not forbidden)
    VIOLATIONS          never drop: copper written although it breaks a rule (the drawing found no legal
                        geometry), one item per violation with place, type and cause (``result["violations"]``)
    TIME_BUDGET_HIT     run.timeout_s stopped the topological core before its work caps

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
POSTRUN_CODES = ("DIFF_PAIRS", "DIFF_PAIRS_SKIPPED", "POUR_UNJOINED", "UNROUTED", "CONNECTION_WIDTH", "POUR_ISLANDS", "POUR_STITCHED", "PLANE_PAD_SIGNAL",
                 "PLANE_VIA_OUTSIDE_POUR", "VIA_SQUEEZE", "NECKED", "IGNORED_RULE", "VIOLATIONS", "TIME_BUDGET_HIT")
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
    """One VIOLATIONS item: connection, type, place, depth and length, what it is too close to, cause."""
    at = v.get("at") or [0, 0]
    return (f"{v.get('name') or '?'} {v.get('from', '?')} -> {v.get('to', '?')}: {v.get('kind')} at "
            f"({at[0] / 1e6:.3f}, {at[1] / 1e6:.3f}) mm on {v.get('layer')}, {int(v.get('depth_nm') or 0) / 1e6:.3f} mm "
            f"below the rule over {int(v.get('length_nm') or 0) / 1e6:.3f} mm against {v.get('against')}; "
            f"cause: {v.get('cause')}")


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
    if sig:
        out.append(_mk("PLANE_PAD_SIGNAL",
                       f"{len(sig)} pad(s) of plane nets had no room for a stub and via into their pour and are "
                       "joined by a trace instead: " + ", ".join(x.split(":")[0].removeprefix("pad ") for x in sig[:10])
                       + (" ..." if len(sig) > 10 else "") + ".", [x.split(":")[0] for x in sig]))
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


__all__ = ["route_findings", "drc_pour_findings", "DIFF_PAIR_REASON", "POSTRUN_CODES"]
