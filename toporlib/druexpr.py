"""KiCad custom-rule condition expressions (``.kicad_dru`` ``(condition "...")``): parser and a three-valued
evaluator for the subset topor can decide. Pure; no pcbnew.

KiCad's semantics (source: ``pcbnew/drc/drc_rule_condition.cpp`` ``EvaluateFor``, ``common/libeval_compiler/
libeval_compiler.cpp`` ``VALUE::EqualTo``, ``pcbnew/pcbexpr_evaluator.cpp``; docs: KiCad 10 PCB Editor manual,
"Custom design rules"):

- a condition is evaluated for the item pair ``(A, B)`` and, when false, again for ``(B, A)`` (conditions are
  commutative);
- string ``==`` / ``!=`` is case-insensitive, and a literal containing ``*`` or ``?`` is a wildcard;
- ``A.NetClass == 'x'`` is true when ``x`` is one of the item's constituent net classes or its full
  (composite) class name;
- a property the item does not have is *undefined*: both ``==`` and ``!=`` are then false.

Values here are ``True`` / ``False`` / ``None`` (unknown: depends on the other item or on geometry topor does
not evaluate at this point). ``&&`` / ``||`` / ``!`` follow Kleene's logic. An expression with a function or
property outside the supported set raises :class:`Unsupported` at parse time, so a caller can name the rule in
a warning instead of guessing.

Complexity: parsing is O(n) in the expression length; one evaluation is O(nodes).
"""
from __future__ import annotations

import fnmatch
import re
from dataclasses import dataclass, field


class Unsupported(ValueError):
    """The condition uses syntax, a property or a function topor cannot evaluate."""


# ------------------------------------------------------------------ AST
@dataclass(frozen=True)
class Lit:
    value: object  # str | float


@dataclass(frozen=True)
class Prop:
    side: str  # "A" | "B"
    name: str  # canonical lower-case property name


@dataclass(frozen=True)
class Call:
    side: str
    fn: str  # canonical lower-case function name
    args: tuple


@dataclass(frozen=True)
class Cmp:
    op: str
    lhs: object
    rhs: object


@dataclass(frozen=True)
class Not:
    x: object


@dataclass(frozen=True)
class And:
    xs: tuple


@dataclass(frozen=True)
class Or:
    xs: tuple


#: Properties topor evaluates (lower case, KiCad accepts ``Pad_Type`` and ``Pad Type`` spellings).
PROPS = {"netclass", "netname", "type", "pad_type", "layer"}
#: Item-membership functions decided per item; geometric ones decide by position (regions).
ITEM_FNS = {"hasnetclass", "memberoffootprint", "indiffpair", "isplated"}
GEO_FNS = {"intersectscourtyard", "insidecourtyard", "intersectsfrontcourtyard", "intersectsbackcourtyard",
           "insidefrontcourtyard", "insidebackcourtyard", "insidearea", "intersectsarea", "enclosedbyarea"}
#: Functions that relate A and B (the pair), never decidable for one item alone.
PAIR_FNS = {"iscoupleddiffpair"}
FNS = ITEM_FNS | GEO_FNS | PAIR_FNS

_TOK = re.compile(r"""\s*(?:
    (?P<num>\d+(?:\.\d*)?(?:[eE][-+]?\d+)?|\.\d+)(?P<unit>mm|mil|um|in|deg)?
  | '(?P<sq>[^']*)'
  | "(?P<dq>[^"]*)"
  | (?P<op>\|\||&&|==|!=|<=|>=|[<>!(),.-])
  | (?P<id>[A-Za-z_][A-Za-z0-9_]*)
)""", re.VERBOSE)
_UNIT_NM = {"mm": 1e6, "um": 1e3, "mil": 25_400.0, "in": 25_400_000.0, "deg": 1.0, None: 1.0}


def _lex(text: str) -> list[tuple[str, object]]:
    out, i, n = [], 0, len(text)
    while i < n:
        if text[i].isspace():
            i += 1
            continue
        m = _TOK.match(text, i)
        if not m or m.end() == i:
            raise Unsupported(f"cannot read {text[i:i + 12]!r}")
        i = m.end()
        if m.group("num") is not None:
            out.append(("num", float(m.group("num")) * _UNIT_NM[m.group("unit")]))
        elif m.group("sq") is not None:
            out.append(("str", m.group("sq")))
        elif m.group("dq") is not None:
            out.append(("str", m.group("dq")))
        elif m.group("op") is not None:
            out.append(("op", m.group("op")))
        else:
            out.append(("id", m.group("id")))
    return out


class _Parser:
    def __init__(self, toks):
        self.t, self.i = toks, 0

    def peek(self, kind=None, val=None):
        if self.i >= len(self.t):
            return False
        k, v = self.t[self.i]
        return (kind is None or k == kind) and (val is None or v == val)

    def take(self, kind=None, val=None):
        if not self.peek(kind, val):
            got = self.t[self.i][1] if self.i < len(self.t) else "end"
            raise Unsupported(f"expected {val or kind}, got {got!r}")
        self.i += 1
        return self.t[self.i - 1][1]

    def parse(self):
        e = self.or_()
        if self.i != len(self.t):
            raise Unsupported(f"unexpected {self.t[self.i][1]!r}")
        return e

    def or_(self):
        xs = [self.and_()]
        while self.peek("op", "||"):
            self.i += 1
            xs.append(self.and_())
        return xs[0] if len(xs) == 1 else Or(tuple(xs))

    def and_(self):
        xs = [self.cmp()]
        while self.peek("op", "&&"):
            self.i += 1
            xs.append(self.cmp())
        return xs[0] if len(xs) == 1 else And(tuple(xs))

    def cmp(self):
        lhs = self.unary()
        for op in ("==", "!=", "<=", ">=", "<", ">"):
            if self.peek("op", op):
                self.i += 1
                return Cmp(op, lhs, self.unary())
        return lhs

    def unary(self):
        if self.peek("op", "!"):
            self.i += 1
            return Not(self.unary())
        if self.peek("op", "-"):
            self.i += 1
            x = self.unary()
            if isinstance(x, Lit) and isinstance(x.value, float):
                return Lit(-x.value)
            raise Unsupported("unary minus on a non-number")
        return self.atom()

    def atom(self):
        if self.peek("op", "("):
            self.i += 1
            e = self.or_()
            self.take("op", ")")
            return e
        if self.peek("str"):
            return Lit(self.take("str"))
        if self.peek("num"):
            return Lit(self.take("num"))
        if self.peek("id"):
            side = self.take("id")
            if side not in ("A", "B", "L"):
                raise Unsupported(f"unknown identifier {side!r}")
            self.take("op", ".")
            name = self.take("id")
            key = name.lower()
            if self.peek("op", "("):
                self.i += 1
                args = []
                while not self.peek("op", ")"):
                    args.append(self.unary())
                    if self.peek("op", ","):
                        self.i += 1
                self.take("op", ")")
                if side == "L" or key not in FNS:
                    raise Unsupported(f"function {side}.{name}() is not supported")
                return Call(side, key, tuple(a.value if isinstance(a, Lit) else a for a in args))
            if side == "L" or key not in PROPS:
                raise Unsupported(f"property {side}.{name} is not supported")
            return Prop(side, key)
        raise Unsupported("unexpected end")


def parse_condition(text: str | None):
    """AST of a condition; ``None`` for an empty / missing condition (always true). Raises
    :class:`Unsupported`. ``A.Pad Type`` style names with spaces are not accepted (KiCad writes them with
    underscores). O(len)."""
    if text is None or not text.strip():
        return None
    return _Parser(_lex(text)).parse()


def walk(node):
    """Every node of an AST, pre-order. O(nodes)."""
    if node is None:
        return
    yield node
    for ch in (getattr(node, "xs", None) or ()):
        yield from walk(ch)
    for attr in ("x", "lhs", "rhs"):
        ch = getattr(node, attr, None)
        if ch is not None and not isinstance(ch, (str, float)):
            yield from walk(ch)


def geo_terms(node) -> list[Call]:
    """The geometric calls (courtyard / area tests) of an AST, in order, duplicates dropped. O(nodes)."""
    out: list[Call] = []
    for n in walk(node):
        if isinstance(n, Call) and n.fn in GEO_FNS and n not in out:
            out.append(n)
    return out


def negated_geo(node, neg: bool = False) -> bool:
    """True when a geometric call sits under an odd number of ``!`` (or on a ``!=`` side): a rule that holds
    *outside* an area, which the region model does not express. O(nodes)."""
    if node is None:
        return False
    if isinstance(node, Call):
        return neg and node.fn in GEO_FNS
    if isinstance(node, Not):
        return negated_geo(node.x, not neg)
    if isinstance(node, (And, Or)):
        return any(negated_geo(x, neg) for x in node.xs)
    if isinstance(node, Cmp):
        n2 = neg != (node.op == "!=")
        return negated_geo(node.lhs, n2) or negated_geo(node.rhs, n2)
    return False


def sides(node) -> set[str]:
    """Item sides (``"A"`` / ``"B"``) the expression mentions. O(nodes)."""
    return {n.side for n in walk(node) if isinstance(n, (Prop, Call))}


# ------------------------------------------------------------------ items
@dataclass
class ItemCtx:
    """One item of the pair as topor knows it. ``kind``: ``track`` / ``via`` / ``pad`` (KiCad ``Type``:
    Track, Via, Pad). ``netclass``: the effective (possibly composite ``"a,b,Default"``) class name;
    ``pad_type``: KiCad's ``Pad_Type`` text (``SMD``, ``Through-hole``, ``NPTH, mechanical``, ``Edge
    connector``, ``Aperture``); ``layer``: the copper layer name the pair is tested on; ``ref``: parent
    footprint reference of a pad; ``geo``: truth of each geometric call by ``(fn, arg)`` (missing = False)."""
    kind: str
    net: str | None = ""
    netclass: str | None = "Default"
    layer: str | None = None
    ref: str | None = None
    pad_type: str | None = None
    plated: bool | None = None
    diff_base: str | None = None
    geo: dict = field(default_factory=dict)


_TYPE_NAME = {"track": "track", "via": "via", "pad": "pad", "zone": "zone"}


def _streq(value: str, lit: str) -> bool:
    if "*" in lit or "?" in lit:
        return fnmatch.fnmatchcase(value.lower(), lit.lower())
    return value.lower() == lit.lower()


def _prop(item: ItemCtx | None, name: str):
    """(value, defined). ``value`` None with defined True means "unknown to topor"."""
    if item is None:
        return None, True
    if name == "netname":
        return item.net, True  # None: unknown net
    if name == "netclass":
        return item.netclass, True
    if name == "type":
        return _TYPE_NAME.get(item.kind), True
    if name == "pad_type":
        return (item.pad_type, True) if item.kind == "pad" else (None, False)
    if name == "layer":
        if item.kind == "track":
            return item.layer, True
        return None, True  # a via / pad spans layers: KiCad compares against the tested layer
    return None, True


def _cmp(item: ItemCtx | None, prop: str, op: str, lit) -> bool | None:
    if op not in ("==", "!="):
        return None
    value, defined = _prop(item, prop)
    if not defined:
        return False  # KiCad: undefined == x and undefined != x are both false
    if value is None or not isinstance(lit, str):
        return None
    if prop == "netclass":
        parts = [p for p in value.split(",") if p]
        hit = any(_streq(p, lit) for p in parts) or _streq(value, lit)
    else:
        hit = _streq(str(value), lit)
    return hit if op == "==" else not hit


def _call(item: ItemCtx | None, c: Call) -> bool | None:
    if item is None:
        return None
    arg = c.args[0] if c.args else None
    if c.fn in GEO_FNS:
        if item.geo is None:
            return None
        return item.geo.get((c.fn, arg), False)
    if c.fn == "hasnetclass":
        if item.netclass is None:
            return None
        return isinstance(arg, str) and any(_streq(p, arg) for p in item.netclass.split(",") if p)
    if c.fn == "memberoffootprint":
        if item.kind != "pad":
            return False  # a routed track / via is no member of a footprint
        return item.ref is not None and isinstance(arg, str) and _streq(item.ref, arg)
    if c.fn == "indiffpair":
        if item.net is None:
            return None
        if item.diff_base is None:
            return False
        return isinstance(arg, str) and _streq(item.diff_base, arg)
    if c.fn == "isplated":
        if item.kind == "via":
            return True
        return item.plated if item.kind == "pad" else False
    return None  # pair functions


def _and(vals):
    vals = list(vals)
    if any(v is False for v in vals):
        return False
    return True if all(v is True for v in vals) else None


def _or(vals):
    vals = list(vals)
    if any(v is True for v in vals):
        return True
    return False if all(v is False for v in vals) else None


def _eval(node, a: ItemCtx | None, b: ItemCtx | None):
    side = lambda s: a if s == "A" else b  # noqa: E731
    if node is None:
        return True
    if isinstance(node, And):
        return _and(_eval(x, a, b) for x in node.xs)
    if isinstance(node, Or):
        return _or(_eval(x, a, b) for x in node.xs)
    if isinstance(node, Not):
        v = _eval(node.x, a, b)
        return None if v is None else not v
    if isinstance(node, Call):
        return _call(side(node.side), node)
    if isinstance(node, Cmp):
        l, r = node.lhs, node.rhs
        if isinstance(r, Prop) and isinstance(l, Lit):
            l, r = r, l
        if isinstance(l, Prop) and isinstance(r, Lit):
            return _cmp(side(l.side), l.name, node.op, r.value)
        if isinstance(l, Call) and isinstance(r, Lit) and isinstance(r.value, float):
            v = _call(side(l.side), l)
            if v is None:
                return None
            return (float(v) == r.value) if node.op == "==" else (float(v) != r.value) if node.op == "!=" else None
        return None
    if isinstance(node, Lit):
        return bool(node.value)
    if isinstance(node, Prop):
        return None
    return None


def evaluate(node, item: ItemCtx, other: ItemCtx | None = None) -> bool | None:
    """Does the condition hold for the pair (`item`, `other`) in either order (KiCad's commutative rule
    matching)? `other` None = any item (unknown). True / False / None (unknown). O(nodes)."""
    first = _eval(node, item, other)
    if first is True:
        return True
    second = _eval(node, other, item)
    if second is True:
        return True
    if first is False and second is False:
        return False
    return None
