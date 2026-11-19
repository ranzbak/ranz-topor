"""KiCad DRC for the MCP tools: kicad-cli runs with the board's project provably used, the delta vs the unrouted
input, classes, our copper -> engine connection ids, and per-class comparisons. No GUI, no pcbnew.

Project proof (why: a DRC copy without its .kicad_pro silently uses KiCad's default rules; plugin
DRC_WITHOUT_PROJECT, branch drc-project-fix): the .kicad_pro / .kicad_dru bytes are read ONCE from the board copy,
written from memory beside every DRC copy, and must be present with the same sha256 before AND after kicad-cli;
every clearance KiCad quotes must be one the project defines (a net class clearance, the Board Setup minimum or a
.kicad_dru clearance). Either check failing = ToolError ``DRC_WITHOUT_PROJECT`` / ``RULES_MISMATCH`` and no numbers.

Delta (as the drc-feedback skill): a routed-board violation counts as OURS iff (type + sorted item uuids) is not in
the input board's DRC and at least one item is new copper (a track / arc / via uuid absent from the input board).

Complexity: one kicad-cli run per board state (cached by board + project sha and the kicad-cli version); the index
is O(V + T log T) for V violations and T copper items.
"""
from __future__ import annotations

import collections
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

from toporlib import pcbfile

PROJECT_EXTS = (".kicad_pro", ".kicad_dru")
CLEARANCE_LIKE = {"clearance", "hole_clearance", "shorting_items", "tracks_crossing", "hole_to_hole",
                  "solder_mask_bridge", "copper_edge_clearance", "items_not_allowed"}
KIND_ORDER = {"track": 0, "via": 1, "pad": 2, "zone": 3, "hole": 4}
#: an engine connection end counts as touched by a piece of our copper within this distance (mm): a track ends
#: inside its pad, up to about half a pad from the pad centre the engine reports as the terminal
END_TOL_MM = 1.5
#: the nearest engine violation of the same net(s) within this distance (mm) is attached to a row
ENGINE_NEAR_MM = 1.5
KICAD_CLI_TIMEOUT_S = 600.0


class DrcError(ValueError):
    """A DRC that cannot be trusted or did not run; `code` is the plugin-style error code."""

    def __init__(self, code: str, message: str):
        super().__init__(f"{code}: {message}")
        self.code = code


# ------------------------------------------------------------------ kicad-cli with the project proof

def sha256(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def project_snapshot(pcb: Path) -> dict:
    """{ext: bytes} of the .kicad_pro / .kicad_dru beside `pcb` (only those that exist). Read once. O(file)."""
    return {ext: pcb.with_suffix(ext).read_bytes() for ext in PROJECT_EXTS if pcb.with_suffix(ext).is_file()}


#: bump when the DRC command or its environment changes (part of the cache key)
CACHE_VERSION = "2"
TEMPLATE = Path("/usr/share/kicad/template")


def _cli_env(home: Path) -> dict:
    """kicad-cli with its own HOME/XDG dirs (the user's KiCad config is never read or written) and KiCad's default
    global library tables (as harness.gui_e2e.E2E.make_home: else every footprint is a lib_footprint_issues)."""
    cfg = home / ".config" / "kicad" / "10.0"
    cfg.mkdir(parents=True, exist_ok=True)
    for n in ("fp-lib-table", "sym-lib-table"):
        if not (cfg / n).exists() and (TEMPLATE / n).is_file():
            shutil.copy(TEMPLATE / n, cfg / n)
    h = str(home)
    e = {k: v for k, v in os.environ.items() if not k.startswith(("KICAD_API", "XDG_"))}
    e.update(HOME=h, XDG_CONFIG_HOME=f"{h}/.config", XDG_DATA_HOME=f"{h}/.local/share", XDG_CACHE_HOME=f"{h}/.cache",
             XDG_STATE_HOME=f"{h}/.local/state", KICAD_CONFIG_HOME=f"{h}/.config/kicad")
    e.pop("DISPLAY", None)
    return e


def kicad_cli_path() -> str:
    return os.environ.get("KICAD_CLI", "/usr/bin/kicad-cli")


def kicad_cli_version() -> str:
    try:
        return subprocess.run([kicad_cli_path(), "version"], capture_output=True, text=True, timeout=60).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return "?"


def run_kicad_cli(pcb: Path, out_json: Path, home: Path, timeout_s: float = KICAD_CLI_TIMEOUT_S) -> None:
    """The fixed DRC command (as toporlib.drc.run_drc): JSON, zones refilled, every track error; exit 0 / 5 = ran."""
    cmd = [kicad_cli_path(), "pcb", "drc", "--format", "json", "--refill-zones", "--all-track-errors",
           "--exit-code-violations", "-o", str(out_json), str(pcb)]
    try:
        cp = subprocess.run(cmd, capture_output=True, text=True, env=_cli_env(home), timeout=timeout_s,
                            stdin=subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        raise DrcError("KICAD_CLI_TIMEOUT", f"kicad-cli drc did not finish within {timeout_s:.0f} s on {pcb}") from None
    if cp.returncode not in (0, 5) or not out_json.is_file():
        raise DrcError("KICAD_CLI_FAILED", f"kicad-cli drc exit {cp.returncode} on {pcb}: {(cp.stderr or '')[-800:]}")


def run_checked(pcb: Path, work: Path, tag: str, *, project: dict | None = None, runner=run_kicad_cli,
                timeout_s: float = KICAD_CLI_TIMEOUT_S) -> dict:
    """kicad-cli DRC of `pcb` on a copy in ``work/<tag>/`` with the project snapshot written beside it; the
    project proof (module docstring) must pass. `project` = the snapshot ({ext: bytes}; default: read from beside
    `pcb` now). Cached: the same board + project + kicad-cli version returns the stored result. Returns
    {"json": path, "doc": parsed report, "proof": {...}, "cached": bool}. O(kicad-cli)."""
    pcb = Path(pcb)
    project = project_snapshot(pcb) if project is None else project
    board = pcb.read_bytes()
    ver = (kicad_cli_version() if runner is run_kicad_cli else "runner") + "/" + CACHE_VERSION
    key = sha256(board + b"".join(project.get(e, b"") for e in PROJECT_EXTS) + ver.encode())[:16]
    d = Path(work) / f"{tag}-{key}"
    meta = d / "drc-proof.json"
    if meta.is_file() and (d / "drc.json").is_file():
        proof = json.loads(meta.read_text())
        return {"json": str(d / "drc.json"), "doc": json.loads((d / "drc.json").read_text()), "proof": proof,
                "cached": True}
    if d.exists():
        shutil.rmtree(d)
    d.mkdir(parents=True)
    copy = d / pcb.name
    copy.write_bytes(board)
    for ext, b in project.items():
        copy.with_suffix(ext).write_bytes(b)
    for n in ("fp-lib-table",):
        if (pcb.parent / n).is_file():
            shutil.copy(pcb.parent / n, d / n)
    want = {ext: sha256(b) for ext, b in project.items()}
    t0 = time.monotonic()
    runner(copy, d / "drc.json", Path(work) / "cli-home", timeout_s)
    seconds = round(time.monotonic() - t0, 2)
    after = {ext: (sha256(copy.with_suffix(ext).read_bytes()) if copy.with_suffix(ext).is_file() else None)
             for ext in PROJECT_EXTS}
    bad = {ext: after.get(ext) for ext, h in want.items() if after.get(ext) != h}
    if bad:
        raise DrcError("DRC_WITHOUT_PROJECT",
                       f"the DRC copy {copy} lost or changed {sorted(bad)} during kicad-cli: its numbers would use "
                       f"other rules than the board's; nothing reported")
    doc = json.loads((d / "drc.json").read_text())
    rules = rules_check(doc.get("violations") or [], project)
    if rules.get("verdict", "").startswith("MISMATCH"):
        raise DrcError("RULES_MISMATCH", f"{rules['verdict']} ({copy})")
    proof = {"project_files": sorted(project), "sha256": want, "sha256_after": {k: after[k] for k in want},
             "rules": rules, "kicad_cli": ver, "seconds": seconds, "board_sha256": sha256(board),
             "project_used": bool(project) and not bad,
             "note": None if project else "the board has no .kicad_pro: KiCad's default rules apply (legitimately)"}
    meta.write_text(json.dumps(proof, indent=1))
    return {"json": str(d / "drc.json"), "doc": doc, "proof": proof, "cached": False}


_QUOTED = re.compile(r"\((?:([^)]*?)\s)?(clearance|hole clearance) ([\d.,]+) mm;")
_DRU_CLEAR = re.compile(r"\(constraint\s+(?:clearance|hole_clearance)\s+\(min\s+([\d.]+)\s*mm\)")


def rules_check(violations: list, project: dict) -> dict:
    """The clearances KiCad quotes ("clearance 0,1500 mm;") against the ones the project defines (net classes, the
    Board Setup minimum, .kicad_dru clearance constraints). verdict "rules = .kicad_pro" or "MISMATCH: ...";
    without a .kicad_pro only the quoted values. O(V)."""
    quoted = collections.Counter()
    for v in violations:
        for m in _QUOTED.finditer(v.get("description", "")):
            quoted[(v.get("type"), m.group(2), m.group(3).replace(",", "."))] += 1
    out: dict = {"quoted": [{"type": t, "what": w, "mm": mm, "n": n} for (t, w, mm), n in sorted(quoted.items())]}
    pro = project.get(".kicad_pro")
    if pro is None:
        out["verdict"] = "no .kicad_pro"
        return out
    p = json.loads(pro)
    classes = {c.get("name"): c.get("clearance") for c in (p.get("net_settings") or {}).get("classes") or []}
    rules = ((p.get("board") or {}).get("design_settings") or {}).get("rules") or {}
    allowed = {round(float(v), 4) for v in classes.values() if v is not None}
    for k in ("min_clearance", "min_hole_clearance"):
        if rules.get(k) is not None:
            allowed.add(round(float(rules[k]), 4))
    dru = project.get(".kicad_dru")
    if dru:
        allowed |= {round(float(x), 4) for x in _DRU_CLEAR.findall(dru.decode("utf-8", "replace"))}
    out["allowed_mm"] = sorted(allowed)
    bad = sorted({q["mm"] for q in out["quoted"] if q["what"] == "clearance" and round(float(q["mm"]), 4) not in allowed})
    out["verdict"] = (f"MISMATCH: quoted clearance {bad} mm not defined by the project {sorted(allowed)} "
                      f"(DRC without the project file?)") if bad else "rules = .kicad_pro"
    return out


# ------------------------------------------------------------------ board items and DRC records

_NET = re.compile(r'\(net\s+(?:\d+\s+)?"((?:[^"\\]|\\.)*)"\)|\(net\s+(\d+)\)')
_LAYERS = re.compile(r'\(layers?\s+((?:"[^"]+"\s*)+)\)')
_PTS = re.compile(r"\((start|mid|end|at)\s+(-?[\d.]+)\s+(-?[\d.]+)")
_W = re.compile(r"\((?:width|size)\s+([\d.]+)\)")


def board_items(text: str) -> dict:
    """{uuid: {kind: track|via, net, layers, pts {start,mid,end,at: (x, y) mm}, width}} of the top-level segments,
    arcs and vias (toporlib.pcbfile.find_items). O(len(text))."""
    out = {}
    for sp in pcbfile.find_items(text):
        body = text[sp.start:sp.end]
        n = _NET.search(body)
        lay = _LAYERS.search(body)
        pts = {k: (float(x), float(y)) for k, x, y in _PTS.findall(body)}
        w = _W.search(body)
        out[sp.uuid] = {"kind": "via" if sp.kind == "via" else "track", "net": (n.group(1) or n.group(2)) if n else "",
                        "layers": re.findall(r'"([^"]+)"', lay.group(1)) if lay else [], "pts": pts,
                        "width": float(w.group(1)) if w else None}
    return out


def kind_of(desc: str) -> str:
    d = (desc or "").lower()
    for k, pat in (("via", r"^(blind/buried |micro)?via\b"), ("track", r"^(track|arc)\b"),
                   ("pad", r"^((n?pth|smd) )?pad\b"), ("zone", r"^zone\b"), ("hole", r"hole")):
        if re.search(pat, d):
            return k
    return d.split(" ")[0] if d else "?"


def nets_of(desc: str) -> list:
    return [n for n in re.findall(r"\[([^\]]+)\]", desc or "") if n != "<no net>"]


def layers_of(desc: str) -> list:
    return re.findall(r"\b(?:on|of) ([FB]\.Cu|In\d+\.Cu)", desc or "")


def _items(v: dict) -> list:
    return [{"uuid": i.get("uuid", ""), "desc": i.get("description", ""),
             "x": (i.get("pos") or {}).get("x"), "y": (i.get("pos") or {}).get("y")} for i in v.get("items") or []]


def vkey(v: dict) -> tuple:
    """Identity of a violation: type + sorted item uuids (never the text, which carries measured values)."""
    return (v.get("type"), tuple(sorted(i.get("uuid", "") for i in v.get("items") or [])))


def vclass(v: dict) -> str:
    """`<type>` with the item-kind pair for the clearance-like types ("clearance track-via"), else the type."""
    items = v.get("items") or []
    if v.get("type") in CLEARANCE_LIKE and items:
        ks = sorted((kind_of(i.get("description", "")) for i in items), key=lambda k: (KIND_ORDER.get(k, 9), k))
        return f"{v['type']} {'-'.join(ks)}"
    return v.get("type", "?")


def centre(items: list):
    pts = [(i["x"], i["y"]) for i in items if i.get("x") is not None]
    if not pts:
        return None, None
    return round(sum(p[0] for p in pts) / len(pts), 4), round(sum(p[1] for p in pts) / len(pts), 4)


# ------------------------------------------------------------------ our copper -> engine connections

def _seg_dist(p, a, b) -> float:
    ax, ay = a
    bx, by = b
    dx, dy = bx - ax, by - ay
    L = dx * dx + dy * dy
    t = 0.0 if L == 0 else max(0.0, min(1.0, ((p[0] - ax) * dx + (p[1] - ay) * dy) / L))
    return math.hypot(p[0] - ax - t * dx, p[1] - ay - t * dy)


def _pieces(it: dict) -> list:
    """Straight pieces (a, b) of an item in mm (an arc as start-mid-end, a via as a point)."""
    p = it["pts"]
    if it["kind"] == "via":
        return [(p["at"], p["at"])] if "at" in p else []
    if "mid" in p:
        return [(p["start"], p["mid"]), (p["mid"], p["end"])]
    return [(p["start"], p["end"])] if "start" in p and "end" in p else []


def components(items: dict, uuids) -> dict:
    """uuid -> component id: our items of one net joined where an end of one lies on another (same layer, or a
    via). Union-find over the items of each net, O(T^2) per net in the worst case (nets carry few items)."""
    parent = {u: u for u in uuids}

    def find(u):
        while parent[u] != u:
            parent[u] = parent[parent[u]]
            u = parent[u]
        return u

    by_net = collections.defaultdict(list)
    for u in uuids:
        by_net[items[u]["net"]].append(u)
    for us in by_net.values():
        for i, a in enumerate(us):
            ia = items[a]
            ends_a = [ia["pts"][k] for k in ("start", "end", "at") if k in ia["pts"]]
            for b in us[i + 1:]:
                ib = items[b]
                if ia["kind"] == "track" and ib["kind"] == "track" and not set(ia["layers"]) & set(ib["layers"]):
                    continue
                ends_b = [ib["pts"][k] for k in ("start", "end", "at") if k in ib["pts"]]
                touch = any(_seg_dist(e, x, y) < 1e-3 for e in ends_a for x, y in _pieces(ib)) or \
                    any(_seg_dist(e, x, y) < 1e-3 for e in ends_b for x, y in _pieces(ia))
                if touch:
                    parent[find(a)] = find(b)
    return {u: find(u) for u in uuids}


def _conn_end(c: dict, which: str):
    at = (c.get(which) or {}).get("at")
    return (at[0] / 1e6, at[1] / 1e6) if at else None


def conn_map(items: dict, ours: set, connections: list) -> dict:
    """uuid of our copper -> [{conn, d_mm}]: the engine connections of the item's net whose BOTH ends lie within
    END_TOL_MM of the item's connected component (a Steiner tree serves several); none -> the nearest one with
    "approx": true. The engine writes no per-item wire id, so this is the link. O(T^2 per net + C·T)."""
    comp = components(items, ours)
    members = collections.defaultdict(list)
    for u, r in comp.items():
        members[r].append(u)
    by_name = collections.defaultdict(list)
    for c in connections or []:
        by_name[c.get("name")].append(c)
    out = {}
    for root, us in members.items():
        net = items[us[0]]["net"]
        pieces = [pc for u in us for pc in _pieces(items[u])]
        cand = []
        for c in by_name.get(net, []):
            ends = [_conn_end(c, "from"), _conn_end(c, "to")]
            if None in ends or not pieces:
                continue
            ds = [min(_seg_dist(e, a, b) for a, b in pieces) for e in ends]
            cand.append((max(ds), c.get("id")))
        hits = [{"conn": cid, "d_mm": round(d, 3)} for d, cid in sorted(cand) if d <= END_TOL_MM]
        if not hits and cand:
            d, cid = min(cand)
            hits = [{"conn": cid, "d_mm": round(d, 3), "approx": True}]
        for u in us:
            out[u] = hits
    return out


# ------------------------------------------------------------------ index (delta vs input)

def engine_violations(result: dict | None) -> list:
    route = (result or {}).get("route") or {}
    ev = []
    for v in route.get("violations") or []:
        at = v.get("at") or [None, None]
        if at[0] is not None:
            ev.append({**v, "x": at[0] / 1e6, "y": at[1] / 1e6})
    return ev


def unconnected_by_net(doc: dict) -> dict:
    """net -> [{items: [{uuid, desc, x, y}]}] of the DRC's unconnected_items (first net named in the pair)."""
    out = collections.defaultdict(list)
    for u in doc.get("unconnected_items") or []:
        its = _items(u)
        ns = sorted({n for i in its for n in nets_of(i["desc"])})
        out[ns[0] if ns else "-"].append({"items": its, "key": tuple(sorted(i["uuid"] for i in its))})
    return dict(out)


def index(input_text: str, input_doc: dict, routed_text: str, routed_doc: dict, result: dict | None = None) -> dict:
    """The routed board's DRC as a delta vs the unrouted input: one row per new violation touching our copper
    (id V001.., type, class, sev, nets, layers, x/y mm, text, items with kind/ours, `ours` = our track/via uuids
    with the engine connection ids they serve, the nearest engine violation), counts per class
    (input / routed / ours), other new violations (no item of ours) per class, unconnected per net. O(V + T^2/net)."""
    inp = board_items(input_text)
    rout = board_items(routed_text)
    ours = {u for u in rout if u not in inp}
    route = (result or {}).get("route") or {}
    cmap = conn_map(rout, ours, route.get("connections") or [])
    ikeys = {vkey(v) for v in input_doc.get("violations") or []}
    ev = engine_violations(result)
    rows, other = [], collections.Counter()
    for v in routed_doc.get("violations") or []:
        if vkey(v) in ikeys:
            continue
        its = [{**i, "kind": kind_of(i["desc"]), "ours": i["uuid"] in ours} for i in _items(v)]
        if not any(i["ours"] for i in its):
            other[vclass(v)] += 1
            continue
        x, y = centre(its)
        nets = sorted({n for i in its for n in nets_of(i["desc"])})
        layers = sorted({lay for i in its for lay in layers_of(i["desc"])}
                        | {lay for i in its if i["ours"] for lay in rout[i["uuid"]]["layers"]})
        near = None
        if x is not None:
            cand = [e for e in ev if (e.get("name") in nets or not nets) and math.hypot(e["x"] - x, e["y"] - y) < ENGINE_NEAR_MM]
            if cand:
                near = min(cand, key=lambda e: math.hypot(e["x"] - x, e["y"] - y))
        rows.append({"type": v.get("type"), "class": vclass(v), "sev": v.get("severity", ""), "nets": nets,
                     "layers": layers, "x": x, "y": y, "text": v.get("description", ""),
                     "items": [{k: i[k] for k in ("uuid", "desc", "kind", "ours", "x", "y")} for i in its],
                     "ours": [{"uuid": i["uuid"], "kind": rout[i["uuid"]]["kind"], "net": rout[i["uuid"]]["net"],
                               "conns": cmap.get(i["uuid"], [])} for i in its if i["ours"]],
                     "engine": ({k: near.get(k) for k in ("conn", "name", "kind", "against", "cause", "class")}
                                | {"d_mm": round(math.hypot(near["x"] - x, near["y"] - y), 3)}) if near else None})
    rows.sort(key=lambda r: (r["class"], r["x"] if r["x"] is not None else 0, r["y"] if r["y"] is not None else 0))
    for n, r in enumerate(rows, 1):
        r["id"] = f"V{n:03d}"
    cnt = {"input": collections.Counter(vclass(v) for v in input_doc.get("violations") or []),
           "routed": collections.Counter(vclass(v) for v in routed_doc.get("violations") or []),
           "ours": collections.Counter(r["class"] for r in rows)}
    classes = sorted(set().union(*cnt.values()))
    ui, ur = unconnected_by_net(input_doc), unconnected_by_net(routed_doc)
    nets = sorted(set(ui) | set(ur))
    return {"rows": rows, "by_class": {c: {k: cnt[k].get(c, 0) for k in cnt} for c in classes},
            "other_new": dict(other), "ours_items": {"tracks": sum(rout[u]["kind"] == "track" for u in ours),
                                                      "vias": sum(rout[u]["kind"] == "via" for u in ours)},
            "unconnected": {n: {"input": len(ui.get(n, [])), "routed": len(ur.get(n, []))} for n in nets},
            "unconnected_totals": {"input": sum(map(len, ui.values())), "routed": sum(map(len, ur.values()))}}


def newly_unconnected(before_doc: dict, after_doc: dict, connections: list, before_items: dict,
                      removed: set, ours_map: dict) -> dict:
    """net -> {before, after, new: [{items, conns}]}: unconnected pairs after (e.g. a cleanup) that were not
    unconnected before, each with the engine connection ids it belongs to: the connections of that net whose two
    ends are nearest the pair's two items (both orders, d_mm = the larger end distance), plus `lost` = the removed
    uuids of that net with the connections they served (`ours_map`, conn_map of the board before). O(U·C + R)."""
    ub, ua = unconnected_by_net(before_doc), unconnected_by_net(after_doc)
    by_name = collections.defaultdict(list)
    for c in connections or []:
        by_name[c.get("name")].append(c)
    out = {}
    for net in sorted(set(ub) | set(ua)):
        seen = {p["key"] for p in ub.get(net, [])}
        new = []
        for p in ua.get(net, []):
            if p["key"] in seen:
                continue
            pts = [(i["x"], i["y"]) for i in p["items"] if i["x"] is not None]
            cand = []
            if len(pts) == 2:
                for c in by_name.get(net, []):
                    e0, e1 = _conn_end(c, "from"), _conn_end(c, "to")
                    if e0 is None or e1 is None:
                        continue
                    d = min(max(math.dist(pts[0], e0), math.dist(pts[1], e1)),
                            max(math.dist(pts[0], e1), math.dist(pts[1], e0)))
                    cand.append((d, c.get("id")))
            cand.sort()
            new.append({"items": [{k: i[k] for k in ("uuid", "desc", "x", "y")} for i in p["items"]],
                        "conns": [{"conn": cid, "d_mm": round(d, 3)} for d, cid in cand[:3]]})
        lost = sorted(u for u in removed if before_items.get(u, {}).get("net") == net)
        if new or lost:
            out[net] = {"before": len(ub.get(net, [])), "after": len(ua.get(net, [])), "new": new,
                        "lost_copper": [{"uuid": u, "kind": before_items[u]["kind"],
                                         "conns": [c["conn"] for c in ours_map.get(u, [])]} for u in lost]}
    return out


def class_delta(a: dict, b: dict, key: str = "ours") -> dict:
    """class -> {a, b, delta} from two index() `by_class` tables (missing class = 0). O(classes)."""
    cs = sorted(set(a) | set(b))
    rows = {c: {"a": (a.get(c) or {}).get(key, 0), "b": (b.get(c) or {}).get(key, 0)} for c in cs}
    for r in rows.values():
        r["delta"] = r["b"] - r["a"]
    return {c: r for c, r in rows.items() if r["a"] or r["b"]}
