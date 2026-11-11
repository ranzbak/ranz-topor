"""KiCad custom design rules (``.kicad_dru``) as topor's router honours them. Pure; no pcbnew.

KiCad resolves a constraint for an item pair by walking the rules in file order and letting every rule whose
``(layer ...)`` and ``(condition ...)`` match overwrite the min / opt / max it sets: the last matching rule
wins, also when it is smaller than the net class value; only when no custom rule matched do the net classes
and the board minimum apply (``pcbnew/drc/drc_engine.cpp`` ``compileRules``, ``EvalRules``; conditions are
tried for ``(A, B)`` and ``(B, A)``, ``drc_rule_condition.cpp`` ``EvaluateFor``; KiCad 10 PCB Editor manual,
"Custom design rules"). A rule with ``(severity ignore)`` still wins, but KiCad reports nothing for it.

The router's model is coarser: a clearance per net (a pair keeps the larger of its two nets'), a clearance
override per pad (it can only add clearance), clearance regions (inside a courtyard / rule area a fixed value
replaces the nets'), one track width and via size per net, board-wide edge and hole clearances. Each rule is
mapped onto it by evaluating its condition with :mod:`toporlib.druexpr`:

- for the router's own copper (a track and a via of each net on each copper layer; the other item a track or
  via of an unknown net -- pads are covered by the pad pass, positions by the regions);
- for every pad (as either item of the pair);
- inside each courtyard / rule area a rule names.

The result of a condition decides how its value is used:

- true: the value replaces (exact);
- unknown (depends on the other item, or on something topor does not evaluate): a *stricter* value is applied
  (safe: KiCad never sees less clearance than it requires), a looser one is not used; both with a warning
  naming the rule;
- false: no effect.

Where the model cannot carry a value exactly (a clearance that differs per layer or between tracks and vias,
a width that differs per layer, a looser value for one footprint's pads), the DRC-safe value is used and a
warning names the rule. Rules with ``(severity ignore)`` / ``exclude`` are skipped: KiCad does not check them,
topor keeps the net class value.

Complexity: compiling is O(R x L) for R rules of length L; :meth:`CustomRules.net` is O(R x layers) per net;
:meth:`CustomRules.plan_regions` O(G x N x R x layers) for G courtyard / area terms and N nets;
:meth:`CustomRules.pad_overrides` O(P x R x layers).
"""
from __future__ import annotations

from dataclasses import dataclass, field

from . import druexpr as dx
from .dru import DruRule, parse_dru

#: Constraints the router honours.
HONOURED = ("clearance", "edge_clearance", "hole_clearance", "track_width", "via_diameter", "hole_size")
#: Constraints that never restrict routed copper (silkscreen, courtyards, mask, paste, zone fill, text).
NOT_ROUTING = {
    "silk_clearance", "courtyard_clearance", "text_height", "text_thickness", "solder_mask_expansion",
    "solder_paste_abs_margin", "solder_paste_rel_margin", "zone_connection", "thermal_relief_gap",
    "thermal_spoke_width", "min_resolved_spokes", "bridged_mask", "solder_mask_sliver", "creepage",
}
#: Constraints other topor modules resolve (keepouts: ``dru.dru_keepouts``; pairs: ``diffpair``).
ELSEWHERE = {"disallow": "rule-area keepouts", "diff_pair_gap": "diff pairs", "diff_pair_uncoupled": "diff pairs",
             "skew": "diff pairs"}

#: The other item of a pair in the router-copper pass: a track or a via of an unknown net, outside every region
#: (a region holds for either item of a pair, so the other item's position is the region model's business).
_OTHERS = (dx.ItemCtx("track", net=None, netclass=None, layer=None, geo={}),
           dx.ItemCtx("via", net=None, netclass=None, layer=None, geo={}))


def _mm(v: int) -> str:
    return f"{v / 1e6:g} mm"


def _or(vals):
    vals = list(vals)
    if any(v is True for v in vals):
        return True
    return False if all(v is False for v in vals) else None


class Notes:
    """Named warnings, each once, in first-seen order; a warning about nets lists them once (``net``)."""

    def __init__(self):
        self._order: list[str] = []
        self._nets: dict[str, list[str]] = {}

    def add(self, text: str, net: str | None = None):
        if text not in self._nets:
            self._order.append(text)
            self._nets[text] = []
        if net is not None and net not in self._nets[text]:
            self._nets[text].append(net)

    @property
    def items(self) -> list[str]:
        out = []
        for t in self._order:
            nets = [n or "(no net)" for n in self._nets[t]]
            if not nets:
                out.append(t)
                continue
            more = f" and {len(nets) - 4} more" if len(nets) > 4 else ""
            out.append(f"{t} [{len(nets)} net(s): {', '.join(nets[:4])}{more}]")
        return out


@dataclass
class CRule:
    """A parsed rule: index in the file, AST (None = no condition), parse error, geometric calls; `role` ``B``
    when the condition only speaks of ``B`` (it is then read with the sides swapped, as KiCad's commutative
    match allows); `region` the geometric calls the region model carries exactly."""
    idx: int
    rule: DruRule
    ast: object = None
    error: str | None = None
    geo: list = field(default_factory=list)
    negated_geo: bool = False
    role: str = "A"
    region: set = field(default_factory=set)

    @property
    def name(self) -> str:
        return self.rule.name

    @property
    def active(self) -> bool:
        return self.rule.severity not in ("ignore", "exclude")

    def layer_ok(self, layer: str | None, outer: bool | None) -> bool | None:
        """Does the rule's ``(layer ...)`` admit copper layer `layer`? None when `layer` is unknown."""
        sel = self.rule.layer
        if sel is None:
            return True
        if layer is None:
            return None
        if sel == "outer":
            return bool(outer)
        if sel == "inner":
            return not outer
        return sel.lower() == layer.lower()

    def why(self) -> str:
        if self.error is not None:
            return f"condition \"{self.rule.condition}\" not understood: {self.error}"
        if self.negated_geo:
            return f"condition \"{self.rule.condition}\" holds outside a courtyard / area"
        return f"condition \"{self.rule.condition}\" depends on the other item"


def compile_rules(text_or_rules) -> list[CRule]:
    """Parse a ``.kicad_dru`` text (or parsed rules) into :class:`CRule` s. O(R x L)."""
    rules = parse_dru(text_or_rules) if isinstance(text_or_rules, str) else list(text_or_rules or [])
    out = []
    for i, r in enumerate(rules):
        c = CRule(i, r)
        try:
            c.ast = dx.parse_condition(r.condition)
            c.geo = dx.geo_terms(c.ast)
            c.negated_geo = dx.negated_geo(c.ast)
            for g in c.geo:
                arg = g.args[0] if g.args else None
                if not isinstance(arg, str) or arg in ("A", "B") or ":" in arg or "${" in arg:
                    raise dx.Unsupported(f"{g.fn}({arg!r}): only a footprint reference or rule-area name is supported")
            if dx.sides(c.ast) == {"B"}:
                c.role = "B"
        except dx.Unsupported as e:
            c.error = str(e)
        out.append(c)
    return out


def _geo_key(call) -> tuple:
    return (call.fn, call.args[0] if call.args else None)


def _ev(c: CRule, mine, other):
    return dx._eval(c.ast, other, mine) if c.role == "B" else dx._eval(c.ast, mine, other)


def _match_copper(c: CRule, item: dx.ItemCtx):
    """Does rule `c` hold for the router's track / via `item` against some other track / via?"""
    if c.error is not None or c.negated_geo:
        return None
    if c.geo:
        geo = dict(item.geo or {})
        for g in c.geo:
            k = _geo_key(g)
            if k not in geo:
                geo[k] = False if k in c.region else None
        item = dx.ItemCtx(**{**item.__dict__, "geo": geo})
    return _or(_ev(c, item, o) for o in _OTHERS)


def _match_pad(c: CRule, pad: dx.ItemCtx):
    """Does rule `c` hold for a pair of `pad` and the router's copper (a track / via of an unknown net), in
    either order? Pad-to-pad and pad-to-zone pairs are not the router's business."""
    if c.error is not None:
        return None
    return _or([_ev(c, pad, o) for o in _OTHERS] + [_ev(c, o, pad) for o in _OTHERS])


def _walk_rules(crules, constraint: str, field_: str, base, ctxs, match, notes: Notes | None, what: str,
                hits: set | None = None):
    """KiCad's last-matching-rule walk for each ``(item, layer, outer)`` of `ctxs`; the largest result
    (smallest for ``max``). `base`: the value without custom rules (None = unset); `hits` collects the names
    of the rules that set a value. O(R x ctxs)."""
    best = None
    for item, layer, outer in ctxs:
        val = base
        for c in crules:
            vals = c.rule.constraints.get(constraint)
            if not vals or field_ not in vals or not c.active:
                continue
            v = int(vals[field_])
            lay = c.layer_ok(layer, outer)
            if lay is False:
                continue
            m = match(c, item) if lay else None
            if m is True:
                val = v
                if hits is not None:
                    hits.add(c.name)
            elif m is None:
                stricter = val is None or (v < val if field_ == "max" else v > val)
                if stricter:
                    val = v
                    if hits is not None:
                        hits.add(c.name)
                    if notes is not None:
                        notes.add(f"custom rule '{c.name}': {constraint} {_mm(v)} applied as a precaution "
                                  f"({c.why()})", what)
                elif notes is not None:
                    notes.add(f"custom rule '{c.name}': looser {constraint} {_mm(v)} not used "
                              f"({c.why()}; topor keeps the stricter value)", what)
        if val is not None:
            best = val if best is None else (min(best, val) if field_ == "max" else max(best, val))
    return best


@dataclass
class NetCustom:
    """Custom-rule values for one net (nm; None = none). ``clearance``: the largest over its tracks and vias
    on all layers; ``width``: the target (``opt``) most layers share, clamped into every layer's
    ``[min, max]``; ``min_width``: the largest ``min``."""
    clearance: int | None = None
    width: int | None = None
    min_width: int = 0
    max_width: int | None = None
    via_d: int | None = None
    via_drill: int | None = None
    width_rule: str | None = None


def canonical_name(layer_id: int) -> str:
    """KiCad's canonical copper layer name of an engine / pcbnew id (0 F.Cu, 2 B.Cu, 2k+2 In<k>.Cu)."""
    i = int(layer_id)
    return "F.Cu" if i == 0 else "B.Cu" if i == 2 else f"In{(i - 2) // 2}.Cu"


def canonical_layers(n_copper: int) -> list[tuple[str, bool, int]]:
    """``[(canonical name, outer, engine id)]`` of a board's copper layers, top to bottom (``F.Cu``,
    ``In1.Cu`` ..., ``B.Cu``; ids 0, 4, 6, ..., 2). A rule naming a layer by its user name is matched through
    :class:`CustomRules` ``aliases``. O(n)."""
    n = max(2, int(n_copper))
    ids = [0] + [4 + 2 * k for k in range(n - 2)] + [2]
    return [(canonical_name(i), i in (0, 2), i) for i in ids]


class CustomRules:
    """The compiled ``.kicad_dru`` of one board with the board's copper `layers` (``[(name, outer, id)]``;
    a rule's ``(layer "F.Cu")`` also matches the canonical name when `aliases` maps it)."""

    def __init__(self, text_or_rules, layers, aliases: dict | None = None):
        self.rules = compile_rules(text_or_rules)
        self.layers = list(layers)
        self.aliases = {k.lower(): v for k, v in (aliases or {}).items()}
        for c in self.rules:
            if c.rule.layer and c.rule.layer.lower() in self.aliases:
                c.rule.layer = self.aliases[c.rule.layer.lower()]
        self.notes = Notes()
        self.regions: list[dict] = []  # plan_regions: {rule, fn, arg, clearance, enclosed, layers}

    def __bool__(self):
        return bool(self.rules)

    # -------------------------------------------------------------- warnings for rules topor cannot use
    def rule_warnings(self, skip=()) -> list[str]:
        """One warning per active rule constraint that restricts routing but is not honoured (constraints in
        `skip` are reported elsewhere), and per rule whose condition cannot be read. Each names the rule."""
        out = []
        for row in self.classify():
            if row["use"] == "warned" and row["constraint"] not in skip:
                out.append(f"custom rule '{row['rule']}': {row['constraint']} is not honoured by topor"
                           + (f" ({row['note']})" if row.get("note") else ""))
        for c in self.rules:
            if c.active and c.error is not None and any(k in HONOURED for k in c.rule.constraints):
                out.append(f"custom rule '{c.name}': condition \"{c.rule.condition}\" not understood ({c.error}); "
                           "its values are applied only where stricter")
        return out

    # -------------------------------------------------------------- contexts
    def _ctxs(self, kind, net, netclass, diff_base, geo=None):
        return [(dx.ItemCtx(kind, net=net, netclass=netclass, layer=n, diff_base=diff_base, geo=dict(geo or {})), n, o)
                for n, o, _i in self.layers]

    # -------------------------------------------------------------- regions (before the per-net pass)
    def plan_regions(self, nets, base_clearance: dict, floor: int) -> list[dict]:
        """Decide which courtyard / area terms the region model carries: a term whose rule gives *every* net
        the same clearance inside (by KiCad's walk over all rules) becomes a region with that value (floored at
        `floor`, with a ``floor:`` warning); the others stay unknown (precaution). `nets`: ``[(name, netclass,
        diff_base)]``; `base_clearance`: net name -> clearance without custom rules. Sets
        :attr:`regions`. O(G x N x R x layers)."""
        self.regions = []
        cand = [c for c in self.rules if c.active and c.error is None and c.geo and not c.negated_geo
                and "min" in c.rule.constraints.get("clearance", {})]
        for c in cand:
            for g in c.geo:
                key = _geo_key(g)
                vals = set()
                for net, nc, db in nets:
                    ctxs = self._ctxs("track", net, nc, db, geo={key: True})
                    vals.add(_walk_rules(self.rules, "clearance", "min", base_clearance.get(net), ctxs,
                                         _match_copper, None, ""))
                label = f"{g.args[0]!r}"
                if len(vals) != 1 or None in vals:
                    self.notes.add(f"custom rule '{c.name}': the clearance inside {g.fn}({label}) depends on the net; "
                                   "topor applies it as a precaution where stricter and does not use it where looser")
                    continue
                v = vals.pop()
                if v < floor:
                    self.notes.add(f"floor: custom rule '{c.name}' clearance {_mm(v)} -> {_mm(floor)} inside "
                                   f"{g.fn}({label})")
                    v = floor
                c.region.add(key)
                layers = [] if c.rule.layer is None else [i for n, o, i in self.layers if c.layer_ok(n, o)]
                self.regions.append({"rule": c.name, "fn": g.fn, "arg": g.args[0], "clearance": int(v),
                                     "enclosed": g.fn == "enclosedbyarea", "layers": layers})
        return self.regions

    # -------------------------------------------------------------- per net
    def net(self, *, net: str, netclass: str, base_clearance: int, base_width: int, base_via: tuple[int, int],
            diff_base: str | None) -> NetCustom:
        """The per-net constraints of `net` (class `netclass`): KiCad's rule walk for its tracks and vias on
        every copper layer, outside every region. O(R x layers)."""
        out = NetCustom()
        if not self.rules:
            return out
        what = net
        notes = self.notes
        tr = self._ctxs("track", net, netclass, diff_base)
        vi = self._ctxs("via", net, netclass, diff_base)
        chits: set = set()
        per_layer = [_walk_rules(self.rules, "clearance", "min", base_clearance, [c], _match_copper, notes, what, chits)
                     for c in tr]
        per_via = _walk_rules(self.rules, "clearance", "min", base_clearance, vi, _match_copper, notes, what, chits)
        cl = max([v for v in per_layer + [per_via] if v is not None] or [base_clearance])
        if cl != base_clearance:
            out.clearance = cl
        if len(set(per_layer + [per_via])) > 1:
            names = ", ".join(sorted(chits))
            notes.add(f"custom clearance differs between layers / tracks and vias "
                      f"({', '.join(f'{n} {_mm(v)}' for (n, _o, _i), v in zip(self.layers, per_layer))}, vias "
                      f"{_mm(per_via)}; rule {names}); topor uses {_mm(cl)} for all of a net's copper", what)
        # width
        lo, hi, targets, hits = 0, None, [], set()
        for c1 in tr:
            mn = _walk_rules(self.rules, "track_width", "min", None, [c1], _match_copper, notes, what, hits)
            op = _walk_rules(self.rules, "track_width", "opt", None, [c1], _match_copper, None, what, hits)
            mx = _walk_rules(self.rules, "track_width", "max", None, [c1], _match_copper, notes, what, hits)
            lo = max(lo, mn or 0)
            if mx is not None:
                hi = mx if hi is None else min(hi, mx)
            targets.append(op)
        if any(t is not None for t in targets) or lo or hi is not None:
            names = ", ".join(sorted(hits))
            tvals = [t if t is not None else base_width for t in targets]
            counts: dict[int, int] = {}
            for t in tvals:
                counts[t] = counts.get(t, 0) + 1
            w = max(counts, key=lambda t: (counts[t], t))
            if lo and w < lo:
                w = lo
            if hi is not None and w > hi:
                w = max(hi, lo)
            per = ", ".join(f"{n} {_mm(t)}" for (n, _o, _i), t in zip(self.layers, tvals))
            if lo and hi is not None and lo > hi:
                notes.add(f"custom track width: the layers' [min, max] do not overlap ({per}; rule {names}); topor "
                          f"routes one width per net, {_mm(w)}: KiCad will flag it on the layers whose max is "
                          "smaller", what)
            elif len(counts) > 1:
                notes.add(f"track width differs per layer ({per}; rule {names}); topor routes one width per net: "
                          f"{_mm(w)}", what)
            if any(t is not None for t in targets) or w != base_width:
                out.width, out.width_rule = w, names
            out.min_width, out.max_width = lo, hi
        # via size
        d0, h0 = base_via
        vd = _walk_rules(self.rules, "via_diameter", "opt", None, vi, _match_copper, None, what)
        vmn = _walk_rules(self.rules, "via_diameter", "min", None, vi, _match_copper, notes, what)
        hs = _walk_rules(self.rules, "hole_size", "opt", None, vi, _match_copper, None, what)
        hmn = _walk_rules(self.rules, "hole_size", "min", None, vi, _match_copper, notes, what)
        if vd is not None or (vmn is not None and vmn > d0):
            out.via_d = max(vd if vd is not None else d0, vmn or 0)
        if hs is not None or (hmn is not None and hmn > h0):
            out.via_drill = max(hs if hs is not None else h0, hmn or 0)
        return out

    # -------------------------------------------------------------- board-wide
    def board(self, constraint: str, base: int, nets) -> int:
        """A board-wide constraint (``edge_clearance``, ``hole_clearance``) for the router's copper: the largest
        value over every net's tracks and vias on every layer (the router has one), `base` (Board Setup) where
        no custom rule matches. A looser value for some nets only is not used (warning). `nets`: ``[(name,
        netclass, diff_base)]``. O(R x N x layers)."""
        if not any(constraint in c.rule.constraints for c in self.rules):
            return base
        vals = []
        for net, nc, db in nets:
            for kind in ("track", "via"):
                v = _walk_rules(self.rules, constraint, "min", base, self._ctxs(kind, net, nc, db), _match_copper,
                                self.notes, net)
                vals.append(base if v is None else v)
        hi, lo = max(vals), min(vals)
        if lo < hi:
            names = sorted({c.name for c in self.rules if constraint in c.rule.constraints and c.active})
            self.notes.add(f"custom {constraint} differs between nets / layers ({_mm(lo)} .. {_mm(hi)}; rule "
                           f"{', '.join(names)}); topor keeps {_mm(hi)} for all routed copper")
        return hi

    # -------------------------------------------------------------- pads (model export)
    def pad_overrides(self, pads, net_clearance: dict, default_clearance: int, layer_ids_of_pad) -> dict[int, int]:
        """Pad clearance overrides: for every pad a rule that tells a pad from a track (pad type, item type,
        footprint membership, plating) gives a value for, KiCad's walk over those rules. A stricter value than
        the pad has becomes its ``local_clearance`` (the router keeps the larger of a pad's and a net's
        clearance, which is KiCad's result for a stricter rule); a looser one is not used (warning).

        `pads`: model pad dicts with ``net_name``, ``netclass``, ``pad_type`` added; `net_clearance`: net name
        -> resolved clearance; `layer_ids_of_pad(pad)`: its copper engine ids. Returns ``{pad index:
        clearance}``. O(P x R x layers)."""
        prs = [c for c in self.rules if c.active and "min" in c.rule.constraints.get("clearance", {})
               and (c.error is not None or _pad_specific(c.ast))]
        out: dict[int, int] = {}
        if not prs:
            return out
        looser: dict[str, list[int]] = {}
        raised: dict[str, int] = {}
        for i, p in enumerate(pads):
            base = max(int(net_clearance.get(p.get("net_name", ""), default_clearance)),
                       int(p.get("local_clearance") or 0))
            item = dx.ItemCtx("pad", net=p.get("net_name", ""), netclass=p.get("netclass", "Default"),
                              ref=p.get("ref"), pad_type=p.get("pad_type"), plated=p.get("attr") != "npth", geo={})
            ids = set(layer_ids_of_pad(p))
            ctxs = [(item, n, o) for n, o, i_ in self.layers if i_ in ids] or [(item, None, None)]
            val = _walk_rules(prs, "clearance", "min", base, ctxs, _match_pad, None, "pads")
            if val is None or val == base:
                continue
            hit = next((c.name for c in reversed(prs) if c.rule.constraints["clearance"]["min"] == val), prs[-1].name)
            if val > base:
                out[i] = int(val)
                raised[hit] = raised.get(hit, 0) + 1
            else:
                looser.setdefault(hit, []).append(val)
        for name, k in raised.items():
            self.notes.add(f"custom rule '{name}': clearance raised on {k} pad(s)")
        for name, vals in looser.items():
            self.notes.add(f"custom rule '{name}': looser clearance ({_mm(min(vals))}) on {len(vals)} pad(s) not used "
                           "(topor's pad override can only add clearance; the net class value stays)")
        for c in prs:
            if c.error is not None:
                self.notes.add(f"custom rule '{c.name}': condition \"{c.rule.condition}\" not understood ({c.error}); "
                               "its clearance is applied to pads only where stricter")
        return out

    # -------------------------------------------------------------- report
    def classify(self) -> list[dict]:
        """One row per (rule, constraint): how topor uses it. ``use``: ``net`` (per-net values), ``region``
        (courtyard / rule area), ``pad`` (pad overrides), ``net+pad``, ``board`` (edge / hole clearance),
        ``elsewhere`` (keepouts, diff pairs), ``not routing``, ``ignored`` (severity), ``no routed copper``
        (no routed track or via can match), ``no effect`` (a clearance <= 0 topor does not model), ``warned``
        (not honoured). O(R x layers)."""
        out = []
        for c in self.rules:
            for con, vals in c.rule.constraints.items():
                row = {"rule": c.name, "constraint": con, "values": dict(vals), "condition": c.rule.condition,
                       "layer": c.rule.layer, "severity": c.rule.severity}
                if not c.active:
                    row["use"] = "ignored"
                elif con in NOT_ROUTING:
                    row["use"] = "not routing"
                elif con in ELSEWHERE:
                    row["use"], row["note"] = "elsewhere", ELSEWHERE[con]
                elif con not in HONOURED:
                    row["use"] = "no effect" if con.endswith("clearance") and all(v <= 0 for v in vals.values()) \
                        else "warned"
                elif c.error is not None:
                    row["use"], row["note"] = "warned", c.error
                elif _no_routed_copper(c):
                    row["use"] = "zone fill" if _zone_only(c) else "no routed copper"
                    if row["use"] == "zone fill":
                        row["note"] = "KiCad's zone refill applies it"
                elif c.geo and con == "clearance":
                    row["use"] = "region" if c.region else "net (precaution)"
                elif con == "clearance" and _pad_specific(c.ast):
                    row["use"] = "pad" if _no_routed_copper(c, pads_too=False) else "net+pad"
                else:
                    row["use"] = "board" if con in ("edge_clearance", "hole_clearance") else "net"
                out.append(row)
        return out


def _no_routed_copper(c: CRule, pads_too: bool = True) -> bool:
    """No routed track or via of any net, anywhere, matches `c` -- and (`pads_too`) neither does a pair of a
    pad and routed copper. O(nodes)."""
    unk = [dx.ItemCtx(k, net=None, netclass=None, layer=None, geo=None) for k in ("track", "via")]
    if any(_ev(c, a, b) is not False for a in unk for b in unk):
        return False
    if not pads_too:
        return True
    pad = dx.ItemCtx("pad", net=None, netclass=None, layer=None, ref=None, pad_type=None, plated=None, geo=None)
    return all(_ev(c, a, pad) is False and _ev(c, pad, a) is False for a in unk)



def _zone_only(c: CRule) -> bool:
    """`c` can match a pair with a copper zone (``B.Type == 'Zone'``) but no pair of routed copper and a
    track / via / pad: it shapes zone fills only."""
    unk = [dx.ItemCtx(k, net=None, netclass=None, layer=None, ref=None, pad_type=None, plated=None, geo=None)
           for k in ("track", "via", "pad")]
    zone = dx.ItemCtx("zone", net=None, netclass=None, layer=None, geo=None)
    return any(_ev(c, a, zone) is not False or _ev(c, zone, a) is not False for a in unk)


def _pad_specific(ast) -> bool:
    """Does the condition test something a pad has and a routed track of the same net has not?"""
    for n in dx.walk(ast):
        if isinstance(n, dx.Prop) and n.name in ("pad_type", "type"):
            return True
        if isinstance(n, dx.Call) and n.fn in ("memberoffootprint", "isplated"):
            return True
    return False


def format_report(rows: list[dict]) -> list[str]:
    """Human lines for :meth:`CustomRules.classify` rows."""
    out = []
    for r in rows:
        vals = " ".join(f"{k} {v / 1e6:g} mm" for k, v in r["values"].items())
        cond = f" if {r['condition']}" if r["condition"] else ""
        lay = f" on {r['layer']}" if r["layer"] else ""
        sev = f" [severity {r['severity']}]" if r["severity"] else ""
        note = f" ({r['note']})" if r.get("note") else ""
        out.append(f"{r['rule']}: {r['constraint']} {vals}{lay}{cond}{sev} -> {r['use']}{note}")
    return out


def region_polys(fn: str, arg: str, courtyards: dict, areas: dict) -> list[list]:
    """Polygons a geometric call refers to: the courtyards of the footprints whose reference matches `arg`
    (wildcards; ``courtyards``: reference -> ``{"F"|"B": [poly]}``; the front / back variants take one side)
    or the outlines of the rule areas named `arg` (``areas``: name -> ``[poly]``). O(F + A)."""
    if "courtyard" in fn:
        side = "F" if "front" in fn else "B" if "back" in fn else None
        out = []
        for ref, by_side in sorted(courtyards.items()):
            if dx._streq(ref, arg):
                for s in ("F", "B"):
                    if side in (None, s):
                        out += by_side.get(s, [])
        return out
    return [p for name, polys in sorted(areas.items()) if dx._streq(name, arg) for p in polys]


#: Model pad ``attr`` -> KiCad's ``Pad_Type`` text (``pcbnew/pad.cpp`` ENUM_MAP<PAD_ATTRIB>).
PAD_TYPE = {"smd": "SMD", "pth": "Through-hole", "npth": "NPTH, mechanical", "conn": "Edge connector"}


def apply_to_model(model: dict, dru_text: str | None, *, courtyards: dict, areas: dict | None = None) -> dict:
    """The model with the custom rules' courtyard / rule-area regions (``rules.clearance_regions``: name,
    ref, poly, clearance, enclosed, layers) and pad clearance overrides (``local_clearance`` raised) applied,
    and their warnings appended to ``rules.warnings``. `courtyards`: footprint reference -> ``{"F"|"B": [poly]}``
    (nm, KiCad frame); `areas`: rule-area name -> ``[poly]`` (default: the model's own rule-area keepouts).
    ``rules.custom_regions`` (from :func:`toporlib.rules.resolve_rules_from_project`) says which terms are
    regions. Without a rule the model is returned unchanged. O(R x P + G x F)."""
    if not dru_text:
        return model
    rules = dict(model.get("rules") or {})
    layers = [(canonical_name(l["id"]), int(l["id"]) in (0, 2), int(l["id"])) for l in model.get("layers", [])]
    aliases = {str(l["name"]): canonical_name(l["id"]) for l in model.get("layers", []) if l.get("name")}
    cr = CustomRules(dru_text, layers, aliases)
    if not cr:
        return model
    if areas is None:
        areas = {}
        for k in model.get("keepouts", []):
            if k.get("name") and not str(k["name"]).startswith("dru:"):
                areas.setdefault(k["name"], []).append(k["poly"])
    regions = []
    for rp in rules.get("custom_regions") or []:
        polys = region_polys(rp["fn"], rp["arg"], courtyards, areas)
        if not polys:
            cr.notes.add(f"custom rule '{rp['rule']}': {rp['fn']}('{rp['arg']}') names no footprint / rule area on "
                         "this board; rule not used")
        for p in polys:
            if len(p) >= 3:
                regions.append({"name": rp["rule"], "ref": rp["arg"], "poly": [list(map(int, q)) for q in p],
                                "clearance": int(rp["clearance"]), "enclosed": bool(rp.get("enclosed")),
                                "layers": list(rp.get("layers") or [])})
    names = {int(n["code"]): n["name"] for n in model.get("nets", [])}
    per_net = rules.get("per_net") or {}
    default = rules.get("default") or {}
    net_clr = {names.get(int(c), ""): int(v["clearance"]) for c, v in per_net.items()}
    pads = [dict(p) for p in model.get("pads", [])]
    view = [{**p, "net_name": names.get(int(p.get("net") or 0), ""),
             "netclass": (per_net.get(str(int(p.get("net") or 0))) or default).get("netclass", "Default"),
             "pad_type": PAD_TYPE.get(str(p.get("attr", "smd")), "SMD")} for p in pads]
    over = cr.pad_overrides(view, net_clr, int(default.get("clearance") or 0), lambda p: p.get("layers") or [])
    for i, v in over.items():
        pads[i]["local_clearance"] = int(v)
    have = list(rules.get("warnings") or [])
    rules["warnings"] = have + [w for w in cr.notes.items if w not in have]
    rules["clearance_regions"] = regions
    return {**model, "pads": pads, "rules": rules}


def courtyard_rings(prims: list[dict], tol: int = 1_000) -> list[list[list[int]]]:
    """Closed rings from courtyard outline primitives (``seg`` dicts ``a``/``b``, ``arc`` dicts
    ``start``/``mid``/``end`` taken as two chords), chained end to end within `tol` nm (KiCad's
    ``ConvertOutlineToPolygon`` chains the same way). Open chains are dropped. O(n^2) for n primitives."""
    segs = []
    for p in prims:
        if p.get("kind") == "seg":
            segs.append((tuple(p["a"]), tuple(p["b"])))
        elif p.get("kind") == "arc":
            segs.append((tuple(p["start"]), tuple(p["mid"])))
            segs.append((tuple(p["mid"]), tuple(p["end"])))
    near = lambda u, v: abs(u[0] - v[0]) <= tol and abs(u[1] - v[1]) <= tol  # noqa: E731
    rings, used = [], [False] * len(segs)
    for i in range(len(segs)):
        if used[i]:
            continue
        used[i] = True
        ring = [segs[i][0], segs[i][1]]
        while len(ring) < 4 or not near(ring[-1], ring[0]):
            for j in range(len(segs)):
                if used[j]:
                    continue
                a, b = segs[j]
                if near(a, ring[-1]):
                    ring.append(b)
                elif near(b, ring[-1]):
                    ring.append(a)
                else:
                    continue
                used[j] = True
                break
            else:
                ring = []
                break
        if len(ring) >= 4:
            rings.append([list(map(int, q)) for q in ring[:-1]])
    return rings
