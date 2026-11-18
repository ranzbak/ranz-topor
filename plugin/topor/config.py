"""``topor.toml`` loader, writer and in-place updater (46 §1).

Reading uses stdlib ``tomllib``; writing is done by hand so every key carries its documentation
comment. ``update_config`` edits single lines and keeps every comment and unrelated user edit.

Complexity: all operations are O(file size + #settings); no loops over board data.
"""

from __future__ import annotations

import json
import math
import os
import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from .findings import Finding
from .layers import ROLES
from . import schema as _schema
from .schema import (BACKLOG_VALUES, CONFIG_VERSION, CURRENT_VERSION, DEPRECATED, KICAD_ONLY, REMOVED, REMOVED_IN,
                     REMOVED_WHEN, SECTIONS, SETTINGS, SETTINGS_BY_KEY, Setting, available)

CONFIG_NAME = "topor.toml"
DOCS_HINT = "see docs/user/configuration.md"

# engine_field -> multiplier from the config unit to the engine unit (mm -> nm, µm -> nm)
_ENGINE_SCALE = {"margin_nm": 1000, "allowed_drills_nm": 1_000_000, "neck_max_nm": 1_000_000,
                 "diffpair_zip_max_nm": 1_000_000, "via_drill_grid_nm": 1_000_000}


def _taut(values: dict) -> bool:
    """[vias] relocate / between_crossings = "auto": on with [search] length = "taut" in any loop (user 2026-11-11:
    "cycles" too; the relocation needs the plan's taut path, Polubasov 2001 p. 4 puts a via between two crossings)."""
    return values[("search", "length")] == "taut"


def _cycles(values: dict) -> bool:
    """[search] drawing_feedback = "auto": on with [optimize] loop = "cycles" (its whole-route cycles feed the drawing
    back into the next plan, the behaviour before M0), off with the TopoR loop, which draws the picked topology once
    (Polubasov 2010, introduction: the wire shapes are computed on the metric stage, from the topology)."""
    return values[("optimize", "loop")] == "cycles"


# engine_field of an auto / on / off setting -> what "auto" means for the current values (the engine gets a bool)
_ENGINE_AUTO = {"via_relocate": _taut, "via_between": _taut, "drawing_feedback": _cycles}

# ---------------------------------------------------------------- value handling

def _is_num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def coerce(s: Setting, raw):
    """Return (value, error message or None). On error the value is the default."""
    err = None
    v = raw
    if s.key == "timeout_s" and isinstance(raw, str) and raw.strip().lower() == "none":
        return 0.0, None
    if s.key == "timeout_overrun_s" and isinstance(raw, str):  # F3: "auto" (the default), "none" = 0 = no cap
        word = raw.strip().lower()
        if word in ("auto", "none"):
            return ("auto" if word == "auto" else 0.0), None
        return s.default, 'must be a number of seconds, "auto" or "none"'
    if s.key == "refine_lanes" and isinstance(raw, str):  # par-refine: "auto" (the default) or a lane count
        if raw.strip().lower() == "auto":
            return "auto", None
        return s.default, 'must be a lane count (1 .. 256) or "auto"'
    if s.type == "bool":
        if not isinstance(raw, bool):
            err = "must be true or false"
    elif s.type in ("int", "float"):
        if not _is_num(raw):
            err = "must be a number"
        elif isinstance(raw, float) and not math.isfinite(raw):
            err = "must be a finite number (not inf or nan; for the grace settings any negative number means no bound)"
        elif s.type == "int" and isinstance(raw, float) and raw != int(raw):
            err = "must be a whole number"
        else:
            v = int(raw) if s.type == "int" else float(raw)
            lo, hi = s.range if s.range else (None, None)
            if (lo is not None and v < lo) or (hi is not None and v > hi):
                err = f"must be in {_range_text(s)}"
    elif s.type == "str":
        if isinstance(raw, bool) and s.choices and "on" in s.choices and "off" in s.choices:
            v = raw = "on" if raw else "off"  # an auto / on / off setting also takes true / false
        if not isinstance(raw, str):
            err = "must be a string"
        elif s.choices and raw not in s.choices:
            err = "must be one of " + ", ".join(s.choices)
    elif s.type == "list":
        if not isinstance(raw, list):
            err = "must be a list"
        elif s.elem == "str" and not all(isinstance(x, str) for x in raw):
            err = "must be a list of strings"
        elif s.elem == "float" and not all(_is_num(x) and x > 0 for x in raw):
            err = "must be a list of positive numbers"
    elif s.type == "table":
        if not isinstance(raw, dict):
            err = "must be a table, e.g. { NET = 1 }"
    if not err and s.type in ("list", "table") and _has_nonfinite(raw):
        err = "must be finite (no inf or nan inside)"
    if err:
        return s.default, err
    return v, None


def _has_nonfinite(v) -> bool:
    """A float inf / nan anywhere inside a list or table value (they would crash or poison the engine JSON). O(size)."""
    if isinstance(v, float):
        return not math.isfinite(v)
    if isinstance(v, dict):
        return any(_has_nonfinite(x) for x in v.values())
    if isinstance(v, (list, tuple)):
        return any(_has_nonfinite(x) for x in v)
    return False


def _range_text(s: Setting) -> str:
    if s.choices:
        return "|".join(s.choices)
    if s.type == "bool":
        return "true|false"
    if s.range:
        lo, hi = s.range
        if lo is not None and hi is not None:
            return f"{_fmt_num(lo)} .. {_fmt_num(hi)}"
        return f">= {_fmt_num(lo)}" if lo is not None else f"<= {_fmt_num(hi)}"
    return "any"


def _fmt_num(x):
    return str(int(x)) if isinstance(x, float) and x == int(x) else str(x)


def toml_value(v) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        return repr(float(v))
    if isinstance(v, str):
        return json.dumps(v, ensure_ascii=False)
    if isinstance(v, (list, tuple)):
        return "[" + ", ".join(toml_value(x) for x in v) + "]"
    if isinstance(v, dict):
        if not v:
            return "{}"
        return "{ " + ", ".join(f"{json.dumps(str(k), ensure_ascii=False)} = {toml_value(x)}" for k, x in v.items()) + " }"
    raise TypeError(f"cannot write {type(v).__name__} to TOML")


# ---------------------------------------------------------------- Config

def cache_root() -> Path:
    """The topor cache folder: ``${TOPOR_REPORT_ROOT:-${XDG_CACHE_HOME:-~/.cache}/kicad/10.0/topor}`` (reports
    and the run lock)."""
    root = os.environ.get("TOPOR_REPORT_ROOT")
    if root:
        return Path(root)
    cache = os.environ.get("XDG_CACHE_HOME") or str(Path(os.path.expanduser("~")) / ".cache")
    return Path(cache) / "kicad" / "10.0" / "topor"


@dataclass(frozen=True)
class Config:
    values: dict = field(default_factory=dict)  # (section, key) -> value, always complete
    layer_roles: dict = field(default_factory=dict)  # copper layer name -> role ([layers] F.Cu = "signal")

    @staticmethod
    def defaults() -> "Config":
        return Config({(s.section, s.key): s.default for s in SETTINGS})

    def get(self, section: str, key: str):
        return self.values[(section, key)]

    def with_values(self, changes: dict) -> "Config":
        for k in changes:
            if k not in SETTINGS_BY_KEY:
                raise KeyError(k)
        return Config({**self.values, **changes}, dict(self.layer_roles))

    def non_defaults(self) -> dict:
        """{"section.key": value} for every value that differs from its default (run report)."""
        return {f"{s.section}.{s.key}": self.values[(s.section, s.key)] for s in SETTINGS
                if self.values[(s.section, s.key)] != s.default}

    def engine_options(self) -> dict:
        """Options dict for topor_engine: mm/µm -> nm, ``timeout_s`` -> ``wall_clock_s`` (0.0 = disabled),
        nested ``expert`` table. Settings whose engine feature is not available yet are omitted."""
        out: dict = {}
        for s in SETTINGS:
            if not s.engine_field or not available(s):
                continue
            v = self.values[(s.section, s.key)]
            f = s.engine_field
            if f == "refine_lanes" and v == "auto":
                v = 0  # the engine's "auto"
            if f in _ENGINE_AUTO:
                v = v if isinstance(v, bool) else _ENGINE_AUTO[f](self.values) if v == "auto" else v == "on"
            if f in _ENGINE_SCALE and isinstance(v, (list, tuple)):
                v = [int(round(x * _ENGINE_SCALE[f])) for x in v]
            elif f in _ENGINE_SCALE:
                v = int(round(v * _ENGINE_SCALE[f]))
            elif s.type == "float":
                v = float(v)
            if "." in f:
                head, tail = f.split(".", 1)
                out.setdefault(head, {})[tail] = v
            else:
                out[f] = v
        return out

    def report_dir_for(self, project_dir: Path, project_name: str, timestamp: str) -> Path:
        """Report folder of one run (ruling C1)."""
        rd = self.values[("output", "report_dir")]
        if not rd:
            return cache_root() / project_name / timestamp
        p = Path(rd)
        if p.is_absolute():
            return p / timestamp
        return Path(project_dir) / p / timestamp


# ---------------------------------------------------------------- rendering

def _doc_line(s: Setting) -> str:
    parts = [s.doc]
    if s.unit:
        parts.append(f"[{s.unit}]")
    parts.append(f"default {toml_value(s.default)}, range {_range_text(s)}")
    return "# " + " ".join(parts)


def _since_note(s: Setting) -> str:
    return f"  (available from {s.since})"


def render_toml(cfg: Config) -> str:
    out = [
        "# Topor router configuration (docs/user/configuration.md is the generated reference).",
        "# Every key below is documented: meaning, [unit], default and allowed range.",
        "# A line '# key = value' is a DEFAULT: Topor follows its current default, so a new version can change it.",
        "# To set a value, remove the '# ' and edit it. To follow the defaults again, comment the line out or delete it.",
        "# Unknown keys produce a warning. Things set in KiCad are listed at the end (not set here).",
        f"version = {CONFIG_VERSION}",
        "",
    ]
    for sec, sec_doc in SECTIONS.items():
        out += [f"[{sec}]", f"# {sec_doc}", ""]
        for s in (x for x in SETTINGS if x.section == sec):
            v = cfg.values[(s.section, s.key)]
            if s.kicad_place:
                out.append(f"# KiCad: {s.kicad_place}")
            out.append(_doc_line(s))
            if not available(s) and v == s.default:
                out.append(f"# {s.key} = {toml_value(v)}{_since_note(s)}")
            elif v == s.default:
                out.append(f"# {s.key} = {toml_value(v)}")
            else:
                out.append(f"{s.key} = {toml_value(v)}")
            out.append("")
        if sec == "layers":
            out += ["# Stackup roles, top to bottom: signal | mixed | gnd | pwr | jumper (routed = signal or mixed).",
                    "# Precedence: roles here > KiCad layer types (if any is power/mixed/jumper) > default by copper count",
                    "# (2: S,S  4: S,GND,PWR,S  6: S,GND,PWR,S,GND,S  8: S,GND,S,PWR,GND,S,GND,S). Other counts need roles here.",
                    "# KiCad: File → Board Setup → Board Editor Layers → layer type"]
            if cfg.layer_roles:
                out += [f"{_layer_key(n)} = {toml_value(r)}" for n, r in cfg.layer_roles.items()]
            else:
                out += ['# F.Cu = "signal"', '# In1.Cu = "gnd"', '# In2.Cu = "pwr"', '# B.Cu = "signal"']
            out.append("")
    out.append("# ---- Set in KiCad, not here ----")
    for name, place in KICAD_ONLY:
        out.append(f"# {name}: {place} (not set here)")
    return "\n".join(out) + "\n"


def _layer_key(name: str) -> str:
    return name if re.fullmatch(r"[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*", name) else json.dumps(name)


def write_default(path: Path) -> None:
    _atomic_write(Path(path), render_toml(Config.defaults()))


def _atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


# ---------------------------------------------------------------- loading

def load_config(project_dir: Path) -> tuple[Config, list[Finding]]:
    """Load ``<project_dir>/topor.toml``; create it with all defaults when missing (CFG_CREATED).

    Findings: CFG_CREATED (info), CFG_PARSE, CFG_VERSION, CFG_UNKNOWN_KEY, CFG_REMOVED_KEY (a key removed in
    the CL cleanup or M6: ignored), CFG_DEPRECATED_KEY (a deprecated key: ignored, its line named), CFG_PINNED_OLD_DEFAULT,
    CFG_VALUE, CFG_VALUE_BACKLOG (warnings; the default is used for the offending value) and CFG_WRITE (could not
    create the file). The two pinned-line codes name the line and carry ``(section.key,)`` as items: the pre-flight dialog's
    "use the current default" comments that line out (:func:`comment_out_key`).
    """
    path = Path(project_dir) / CONFIG_NAME
    findings: list[Finding] = []
    if not path.exists():
        try:
            write_default(path)
            findings.append(Finding("CFG_CREATED", "info", f"Created {path} with all defaults.",
                                    f"Edit {CONFIG_NAME} to change settings; {DOCS_HINT}.", (str(path),)))
        except OSError as e:
            findings.append(Finding("CFG_WRITE", "warning", f"Could not create {path}: {e}.",
                                    "Make the project directory writable or run with the built-in defaults.", (str(path),)))
        return Config.defaults(), findings
    try:
        text = path.read_text(encoding="utf-8")
        data = tomllib.loads(text)
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as e:
        findings.append(Finding("CFG_PARSE", "warning", f"{path} is not valid TOML ({e}); built-in defaults are used.",
                                f"Fix the syntax in {CONFIG_NAME} or delete the file to get a fresh commented one.", (str(path),)))
        return Config.defaults(), findings

    values = dict(Config.defaults().values)
    layer_roles: dict = {}
    ver = data.pop("version", CONFIG_VERSION)
    if ver != CONFIG_VERSION:
        findings.append(Finding("CFG_VERSION", "warning", f"{CONFIG_NAME} has version = {ver!r}; this plugin reads version {CONFIG_VERSION}.",
                                f"Set version = {CONFIG_VERSION} after checking the keys against {DOCS_HINT}.", ("version",)))
    for sec, table in data.items():
        if sec not in SECTIONS and isinstance(table, dict) and table and all((sec, k) in REMOVED for k in table):
            for key in table:
                findings.append(_removed(sec, key, key_line(text, sec, key)))
            continue
        if sec not in SECTIONS or not isinstance(table, dict):
            findings.append(Finding("CFG_UNKNOWN_KEY", "warning", f"Unknown section or key '{sec}' in {CONFIG_NAME} is ignored.",
                                    f"Check the spelling; valid sections are {', '.join(SECTIONS)} ({DOCS_HINT}).", (sec,)))
            continue
        items = list(table.items())
        if sec == "layers":
            items = []
            for key, raw in table.items():
                if (sec, key) in SETTINGS_BY_KEY:
                    items.append((key, raw))
                    continue
                for lname, lraw in _flatten_layer(key, raw):
                    if lname.endswith(".Cu"):
                        full = f"layers.{lname}"
                        if lraw in ROLES:
                            layer_roles[lname] = lraw
                        else:
                            findings.append(Finding("CFG_VALUE", "warning", f"{full} = {lraw!r} is not a layer role; the layer's role is decided by KiCad layer types or the stackup default.",
                                                    f"Use one of: {'|'.join(ROLES)} in {CONFIG_NAME} [layers].", (full,)))
                    else:
                        items.append((lname, lraw))
        for key, raw in items:
            s = SETTINGS_BY_KEY.get((sec, key))
            full = f"{sec}.{key}"
            if s is None and (sec, key) in REMOVED:
                findings.append(_removed(sec, key, key_line(text, sec, key)))
                continue
            if s is None and (sec, key) in DEPRECATED:
                findings.append(_deprecated(sec, key, raw, key_line(text, sec, key)))
                continue
            if s is None:
                findings.append(Finding("CFG_UNKNOWN_KEY", "warning", f"Unknown key '{full}' in {CONFIG_NAME} is ignored.",
                                        f"Check the spelling against {DOCS_HINT}.", (full,)))
                continue
            if isinstance(raw, str) and raw in BACKLOG_VALUES.get((sec, key), ()):
                findings.append(Finding("CFG_VALUE_BACKLOG", "warning", f"{full} = {raw!r} is planned but not implemented; the default {s.default!r} is used.",
                                        f"Use one of: {_range_text(s)}.", (full,)))
                continue
            v, err = coerce(s, raw)
            if err:
                findings.append(Finding("CFG_VALUE", "warning", f"{full} = {raw!r} {err}; the default {s.default!r} is used.",
                                        f"Correct {full} in {CONFIG_NAME} (allowed: {_range_text(s)}).", (full,)))
            values[(sec, key)] = v
            if not err:
                pin = _pinned_old_default(s, v, key_line(text, sec, key))
                if pin:
                    findings.append(pin)
    return Config(values, layer_roles), findings


#: The findings whose pinned line the pre-flight dialog offers to comment out ("use the current default").
PINNED_LINE_CODES = ("CFG_PINNED_OLD_DEFAULT", "CFG_DEPRECATED_KEY")


def _line_text(line: int | None) -> str:
    return f" (line {line})" if line else ""


def _pinned_old_default(s: Setting, v, line: int | None = None) -> Finding | None:
    """CFG_PINNED_OLD_DEFAULT when active value ``v`` equals a past default of ``s`` but not the current one;
    ``line``: its line in topor.toml (1-based). O(history)."""
    if v == s.default:
        return None
    for old, when in _schema.PAST_DEFAULTS.get((s.section, s.key), ()):
        if isinstance(old, bool) == isinstance(v, bool) and old == v:
            full = f"{s.section}.{s.key}"
            how = ("set it explicitly to the value you want" if s.type != "bool"
                   else "the warning stays while the value is pinned")
            return Finding("CFG_PINNED_OLD_DEFAULT", "warning",
                           f"{CONFIG_NAME}{_line_text(line)} pins {full} = {toml_value(v)}, the old default; the current default is {toml_value(s.default)} "
                           f"(changed {when}). Delete or comment the line to follow defaults (the pre-flight dialog does it for you: "
                           f"Use the current default); to keep it on purpose {how}.",
                           f"{DOCS_HINT}#pinned-defaults.", (full,))
    return None


def _deprecated(sec: str, key: str, raw, line: int | None) -> Finding:
    """CFG_DEPRECATED_KEY for a key of ``schema.DEPRECATED``: ignored, its line and why named. O(1)."""
    full = f"{sec}.{key}"
    when, why = DEPRECATED[(sec, key)]
    val = toml_value(raw) if isinstance(raw, (bool, int, float, str)) else repr(raw)
    return Finding("CFG_DEPRECATED_KEY", "warning",
                   f"{CONFIG_NAME}{_line_text(line)} sets {full} = {val}, which is deprecated since {when} and ignored: {why}.",
                   f"Delete or comment the line (the pre-flight dialog does it for you: Use the current default); "
                   f"{DOCS_HINT}#deprecated-keys.", (full,))


def key_line(text: str, sec: str, key: str) -> int | None:
    """The 1-based line of the active ``key = ...`` line in section ``[sec]`` of a topor.toml text, or None.
    O(lines)."""
    lines = text.split("\n")
    hdr = next((i for i, ln in enumerate(lines) if (m := _HEADER.match(ln)) and m.group(1).strip() == sec), None)
    if hdr is None:
        return None
    active = re.compile(rf"^\s*{re.escape(key)}\s*=")
    for i in range(hdr + 1, len(lines)):
        if _HEADER.match(lines[i]):
            return None
        if active.match(lines[i]):
            return i + 1
    return None


def comment_out_key(path: Path, full: str) -> int | None:
    """Comment out the active line of ``section.key`` in topor.toml (``# `` in front of each of its lines, a
    multi-line value included), keeping everything else; the option then follows the current default (the
    pre-flight dialog's "Use the current default", user 2026-11-17). Returns the line (1-based), None when there was
    no active line (nothing written). O(lines)."""
    path = Path(path)
    sec, _, key = full.partition(".")
    text = path.read_text(encoding="utf-8")
    line = key_line(text, sec, key)
    if line is None:
        return None
    lines = text.split("\n")
    i = last = line - 1
    depth = _balance(lines[i].split("=", 1)[1])
    while depth > 0 and last + 1 < len(lines):
        last += 1
        depth += _balance(lines[last])
    for j in range(i, last + 1):
        lines[j] = "# " + lines[j]
    _atomic_write(path, "\n".join(lines))
    return line


def _removed(sec: str, key: str, line: int | None = None) -> Finding:
    """CFG_REMOVED_KEY for a key of ``schema.REMOVED``: ignored, with why and its line (like CFG_DEPRECATED_KEY),
    never an error. O(1)."""
    full = f"{sec}.{key}"
    return Finding("CFG_REMOVED_KEY", "warning",
                   f"'{full}' in {CONFIG_NAME}{_line_text(line)} is ignored: removed in {REMOVED_WHEN.get((sec, key), REMOVED_IN)} ({REMOVED[(sec, key)]}).",
                   f"Delete the line from {CONFIG_NAME}; {DOCS_HINT}.", (full,))


def _flatten_layer(key, raw):
    """TOML parses bare F.Cu as {F: {Cu: v}}: yield ("F.Cu", v); quoted "F.Cu" arrives flat."""
    if isinstance(raw, dict):
        for k, v in raw.items():
            yield from _flatten_layer(f"{key}.{k}", v)
    else:
        yield key, raw


# ---------------------------------------------------------------- in-place update

_HEADER = re.compile(r"^\s*\[([^\[\]]+)\]\s*(#.*)?$")


def _balance(text: str) -> int:
    """Open brackets/braces in a value fragment, ignoring those inside strings. O(len)."""
    depth, in_str, esc = 0, False, False
    for ch in text:
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
        elif ch == '"':
            in_str = True
        elif ch == "#":
            break
        elif ch in "[{":
            depth += 1
        elif ch in "]}":
            depth -= 1
    return depth


def update_config(path: Path, changes: dict) -> None:
    """Set ``{"section.key": value}`` in the file, keeping all comments and unrelated lines.

    A commented-out ``# key = ...`` line is replaced by an active one; a missing key is appended to
    its section after the last key line (with its documentation comment). Raises ValueError for an
    unknown key or an invalid value, before anything is written.
    """
    path = Path(path)
    prepared = []
    for dotted, raw in changes.items():
        sec, _, key = dotted.partition(".")
        s = SETTINGS_BY_KEY.get((sec, key))
        if s is None:
            raise ValueError(f"unknown setting {dotted!r}")
        v, err = coerce(s, raw)
        if err:
            raise ValueError(f"{dotted} = {raw!r} {err}")
        prepared.append((s, v))
    lines = path.read_text(encoding="utf-8").split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    for s, v in prepared:
        new = f"{s.key} = {toml_value(v)}"
        hdr = next((i for i, ln in enumerate(lines) if (m := _HEADER.match(ln)) and m.group(1).strip() == s.section), None)
        if hdr is None:
            lines += ["", f"[{s.section}]", _doc_line(s), new]
            continue
        end = next((i for i in range(hdr + 1, len(lines)) if _HEADER.match(lines[i])), len(lines))
        active = re.compile(rf"^\s*{re.escape(s.key)}\s*=")
        commented = re.compile(rf"^\s*#\s*{re.escape(s.key)}\s*=")
        idx = next((i for i in range(hdr + 1, end) if active.match(lines[i])), None)
        if idx is None:
            idx = next((i for i in range(hdr + 1, end) if commented.match(lines[i])), None)
        if idx is not None:
            last = idx
            if active.match(lines[idx]):
                depth = _balance(lines[idx].split("=", 1)[1])
                while depth > 0 and last + 1 < end:
                    last += 1
                    depth += _balance(lines[last])
            lines[idx:last + 1] = [new]
            continue
        keyline = re.compile(r"^\s*#?\s*[A-Za-z_][A-Za-z0-9_]*\s=\s")
        last_key = max((i for i in range(hdr + 1, end) if keyline.match(lines[i])), default=hdr)
        lines[last_key + 1:last_key + 1] = [_doc_line(s), new]
    _atomic_write(path, "\n".join(lines) + "\n")
