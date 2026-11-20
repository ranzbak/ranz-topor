"""mcp-drc: drc / compare / cleanup-label / spots-planning units on recorded fixtures (no KiCad GUI, no kicad-cli).

Fixtures (fixtures/drc/, public board wiccon-2023): kicad-cli 10.0.8 JSON of the unrouted input and of a topor
route (base run f1cf2ba, `--refill-zones --all-track-errors`, the board's .kicad_pro used), trimmed to a few
pre-existing rows + every row of ours; the routed board's copper of the two nets involved; the route's connections
of those nets; the board's .kicad_pro. Synthetic boards below cover the clearance classes and Steiner matching.
"""
from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from mcp_server import drc as D
from mcp_server import kicad_gui as G
from mcp_server import tools

FIX = Path(__file__).parent / "fixtures" / "drc"


def _j(name):
    return json.loads((FIX / name).read_text())


def _idx():
    return D.index((FIX / "wiccon-input-mini.kicad_pcb").read_text(), _j("wiccon-input-drc.json"),
                   (FIX / "wiccon-routed-mini.kicad_pcb").read_text(), _j("wiccon-routed-drc.json"),
                   _j("wiccon-result-mini.json"))


def test_recorded_delta_counts_only_new_rows_with_our_copper():
    idx = _idx()
    # pre-existing (type + uuids in the input DRC) never ours; starved_thermal names no item of ours -> other_new
    assert [r["class"] for r in idx["rows"]] == ["solder_mask_bridge track-polygon"] * 2
    assert idx["by_class"]["solder_mask_bridge track-polygon"] == {"input": 0, "routed": 2, "ours": 2}
    assert idx["by_class"]["padstack"]["ours"] == 0 and idx["by_class"]["padstack"]["input"] >= 1
    assert idx["other_new"] == {"starved_thermal": 2}
    assert [r["id"] for r in idx["rows"]] == ["V001", "V002"]


def test_recorded_rows_carry_nets_layers_items_and_engine_connection_ids():
    r = _idx()["rows"][0]
    assert r["nets"] == ["Net-(D3-Pad2)"] and r["layers"] == ["F.Cu"] and r["x"] is not None
    ours = [i for i in r["items"] if i["ours"]]
    assert len(ours) == 1 and ours[0]["kind"] == "track"
    # the track is on the connection D3.2 -> D4.4 (id 13 of the recorded route): both ends on its component
    assert r["ours"][0]["conns"] == [{"conn": 13, "d_mm": 0.0}]


def _seg(u, net, a, b, layer="F.Cu"):
    return (f'\t(segment (start {a[0]} {a[1]}) (end {b[0]} {b[1]}) (width 0.2) (layer "{layer}") (net "{net}") '
            f'(uuid "{u}"))\n')


def _via(u, net, at):
    return f'\t(via (at {at[0]} {at[1]}) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net "{net}") (uuid "{u}"))\n'


def _conn(cid, name, a, b):
    return {"id": cid, "name": name, "from": {"at": [a[0] * 1e6, a[1] * 1e6]}, "to": {"at": [b[0] * 1e6, b[1] * 1e6]}}


def test_conn_map_steiner_tree_serves_every_connection_it_joins_and_vias_join_layers():
    board = ("(kicad_pcb\n" + _seg("s1", "A", (0, 0), (5, 0)) + _seg("s2", "A", (5, 0), (5, 5), "B.Cu")
             + _via("v1", "A", (5, 0)) + _seg("s3", "A", (5, 0), (10, 0)) + _seg("t1", "B", (0, 9), (9, 9)) + ")")
    items = D.board_items(board)
    conns = [_conn(1, "A", (0, 0), (5, 5)), _conn(2, "A", (0, 0), (10, 0)), _conn(3, "A", (0, 0), (30, 30)),
             _conn(4, "B", (0, 9), (9, 9))]
    m = D.conn_map(items, set(items), conns)
    assert sorted(c["conn"] for c in m["s2"]) == [1, 2]  # B.Cu piece joined through the via; conn 3 is far
    assert m["v1"] == m["s1"] == m["s3"] == m["s2"]
    assert m["t1"] == [{"conn": 4, "d_mm": 0.0}]
    # no connection within END_TOL_MM: the nearest one, flagged approx
    m2 = D.conn_map(items, {"t1"}, [_conn(9, "B", (0, 9), (40, 40))])
    assert m2["t1"][0]["conn"] == 9 and m2["t1"][0]["approx"] is True


def test_components_keep_layers_apart_without_a_via():
    board = "(kicad_pcb\n" + _seg("a", "N", (0, 0), (5, 0)) + _seg("b", "N", (5, 0), (9, 0), "B.Cu") + ")"
    items = D.board_items(board)
    comp = D.components(items, set(items))
    assert comp["a"] != comp["b"]


def _v(t, items, desc="d"):
    return {"type": t, "severity": "error", "description": desc,
            "items": [{"uuid": u, "description": d, "pos": {"x": x, "y": y}} for u, d, x, y in items]}


def test_classes_name_the_item_kinds_in_fixed_order():
    v = _v("clearance", [("p", "Pad 1 [A] of U1 on F.Cu", 0, 0), ("t", "Track [B] on F.Cu, length 1 mm", 1, 1)])
    assert D.vclass(v) == "clearance track-pad"
    assert D.vclass(_v("hole_clearance", [("v", "Via [A] on F.Cu - B.Cu", 0, 0), ("t", "Track [B] on F.Cu", 0, 0)])) \
        == "hole_clearance track-via"
    assert D.vclass(_v("track_dangling", [("t", "Track [B] on F.Cu", 0, 0)])) == "track_dangling"


PRO = {"net_settings": {"classes": [{"name": "Default", "clearance": 0.15}, {"name": "PWR", "clearance": 0.3}]},
       "board": {"design_settings": {"rules": {"min_clearance": 0.1}}}}


def test_rules_check_accepts_project_values_and_flags_kicad_defaults():
    proj = {".kicad_pro": json.dumps(PRO).encode()}
    ok = [_v("clearance", [], "Clearance violation (netclass 'PWR' clearance 0,3000 mm; actual 0,2000 mm)")]
    assert D.rules_check(ok, proj)["verdict"] == "rules = .kicad_pro"
    bad = [_v("clearance", [], "Clearance violation (netclass 'Default' clearance 0,2000 mm; actual 0,1000 mm)")]
    assert D.rules_check(bad, proj)["verdict"].startswith("MISMATCH")
    dru = {**proj, ".kicad_dru": b'(rule x (constraint clearance (min 0.2mm)))'}
    assert D.rules_check(bad, dru)["verdict"] == "rules = .kicad_pro"
    assert D.rules_check(bad, {})["verdict"] == "no .kicad_pro"


def _board_dir(tmp_path, pro=PRO):
    d = tmp_path / "b"
    d.mkdir()
    shutil.copy(FIX / "wiccon-routed-mini.kicad_pcb", d / "x.kicad_pcb")
    (d / "x.kicad_pro").write_text(json.dumps(pro))
    return d / "x.kicad_pcb"


def _runner(doc, mutate=None):
    calls = []

    def run(copy, out_json, home, timeout_s):
        calls.append(copy)
        assert copy.with_suffix(".kicad_pro").is_file()  # the snapshot is beside the copy when kicad-cli starts
        if mutate:
            mutate(copy)
        out_json.write_text(json.dumps(doc))
    run.calls = calls
    return run


def test_run_checked_writes_the_snapshot_and_caches(tmp_path):
    pcb = _board_dir(tmp_path)
    snap = D.project_snapshot(pcb)
    (pcb.with_suffix(".kicad_pro")).unlink()  # gone from the source dir after the snapshot: the copy still has it
    run = _runner({"violations": []})
    r = D.run_checked(pcb, tmp_path / "w", "routed", project=snap, runner=run)
    assert r["proof"]["project_used"] and r["proof"]["sha256"] == r["proof"]["sha256_after"]
    assert run.calls[0].with_suffix(".kicad_pro").read_bytes() == snap[".kicad_pro"]
    r2 = D.run_checked(pcb, tmp_path / "w", "routed", project=snap, runner=run)
    assert r2["cached"] and len(run.calls) == 1


@pytest.mark.parametrize("mutate", [lambda c: c.with_suffix(".kicad_pro").unlink(),
                                    lambda c: c.with_suffix(".kicad_pro").write_text("{}")])
def test_run_checked_refuses_when_kicad_cli_lost_or_changed_the_project(tmp_path, mutate):
    pcb = _board_dir(tmp_path)
    with pytest.raises(D.DrcError) as ei:
        D.run_checked(pcb, tmp_path / "w", "routed", runner=_runner({"violations": []}, mutate))
    assert ei.value.code == "DRC_WITHOUT_PROJECT"
    assert not list((tmp_path / "w").glob("*/drc-proof.json"))  # nothing cached as valid


def test_run_checked_refuses_numbers_under_other_rules(tmp_path):
    pcb = _board_dir(tmp_path)
    doc = {"violations": [_v("clearance", [], "x (netclass 'Default' clearance 0,2000 mm; actual 0,1 mm)")]}
    with pytest.raises(D.DrcError) as ei:
        D.run_checked(pcb, tmp_path / "w", "routed", runner=_runner(doc))
    assert ei.value.code == "RULES_MISMATCH"


def test_cli_env_never_points_at_the_users_kicad(tmp_path):
    e = D._cli_env(tmp_path / "home")
    for k in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "KICAD_CONFIG_HOME"):
        assert Path(e[k]).resolve().is_relative_to((tmp_path / "home").resolve()), k
    assert (tmp_path / "home/.config/kicad/10.0").is_dir()


def test_newly_unconnected_names_the_connection_and_the_lost_copper():
    board = "(kicad_pcb\n" + _seg("s1", "A", (0, 0), (5, 0)) + ")"
    items = D.board_items(board)
    conns = [_conn(7, "A", (0, 0), (5, 0)), _conn(8, "A", (20, 20), (30, 30))]
    after = {"unconnected_items": [{"items": [
        {"uuid": "p1", "description": "Pad 1 [A] of U1 on F.Cu", "pos": {"x": 0.1, "y": 0}},
        {"uuid": "p2", "description": "Pad 1 [A] of U2 on F.Cu", "pos": {"x": 5, "y": 0.1}}]}]}
    out = D.newly_unconnected({"unconnected_items": []}, after, conns, items, {"s1"},
                              D.conn_map(items, {"s1"}, conns))
    a = out["A"]
    assert (a["before"], a["after"]) == (0, 1)
    assert a["new"][0]["conns"][0]["conn"] == 7
    assert a["lost_copper"] == [{"uuid": "s1", "kind": "track", "conns": [7]}]
    # an unconnected pair that was there before is not new
    assert D.newly_unconnected(after, after, conns, items, set(), {}) == {}


def test_class_delta_and_compare_tool(tmp_path, monkeypatch):
    a = {"x": {"ours": 2}, "y": {"ours": 1}}
    b = {"x": {"ours": 1}, "z": {"ours": 3}}
    assert D.class_delta(a, b) == {"x": {"a": 2, "b": 1, "delta": -1}, "y": {"a": 1, "b": 0, "delta": -1},
                                   "z": {"a": 0, "b": 3, "delta": 3}}
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    ids = []
    for n, doc in (("a", "wiccon-routed-drc.json"), ("b", "wiccon-input-drc.json")):
        d = tmp_path / n
        (d / "w-input").mkdir(parents=True)
        (d / "w-topor").mkdir()
        shutil.copy(FIX / "wiccon-input-mini.kicad_pcb", d / "w-input/2023.kicad_pcb")
        shutil.copy(FIX / "wiccon-routed-mini.kicad_pcb", d / "w-topor/2023.kicad_pcb")
        for sub in ("w-input", "w-topor"):
            shutil.copy(FIX / "wiccon.kicad_pro", d / sub / "2023.kicad_pro")
        tools._remember(n, {"run_id": n, "board_copy": str(d / "w-topor/2023.kicad_pcb"), "out_dir": str(d),
                            "report_dir": None, "status": "ok", "name": "w"})
        ids.append((n, doc))
    docs = dict(ids)

    def fake(copy, out_json, home, timeout_s):  # input boards -> the input DRC, routed: per run
        run = copy.parents[3].name
        out_json.write_text((FIX / ("wiccon-input-drc.json" if "input-" in copy.parent.name else docs[run])).read_text())
    monkeypatch.setattr(D, "run_kicad_cli", fake)
    c = tools.compare("a", "b")
    assert c["by_class"]["solder_mask_bridge track-polygon"] == {"a": 2, "b": 0, "delta": -2}
    assert c["ours_total"] == {"a": 2, "b": 0, "delta": -2} and c["same_input_board"]
    r = tools.drc("a", limit=1)
    assert r["rules"] == "rules = .kicad_pro" and r["project_used"] and len(r["rows"]) == 1
    assert r["rows_truncated"] == 1 and Path(r["index_json"]).is_file()


def test_cleanup_options_are_kicads_labels():
    on = tools.cleanup_labels(None)
    assert sorted(k for k, v in on.items() if v) == sorted(tools.DEFAULT_CLEANUP)
    assert set(on) == set(G.CLEANUP_LABELS.values())
    assert tools.cleanup_labels(["merge", "Delete redundant vias"])["Merge co-linear tracks"] is True
    assert tools.cleanup_labels({"Delete tracks fully inside pads": True})["Delete redundant vias"] is False
    with pytest.raises(tools.ToolError, match="KiCad's labels"):
        tools.cleanup_labels(["Delete everything"])


def test_gui_rows_match_cli_rows_by_text_and_items():
    gui = [{"k": 0, "text": "Error: Front solder mask aperture bridges items with different nets",
            "items": ["Polygon of Ref** on F.Mask", "Track [N] on F.Cu, length 58,2048 mm",
                      "Zone [GND] on F.Cu and B.Cu, priority 0"]},
           {"k": 1, "text": "Warning: Text height out of range (min height 0,8000 mm; actual 0,3000 mm)",
            "items": ["PCB text 'x' on B.Silkscreen"]}]
    free = {}
    for g in gui:
        free.setdefault(G.gui_key(g), []).append(g)
    cli = {"text": "Front solder mask aperture bridges items with different nets",
           "items": [{"desc": "Polygon of Ref** on F.Mask"}, {"desc": "Track [N] on F.Cu, length 58.2048 mm"}]}
    assert G.find_row(free, G.cli_key(cli))["k"] == 0  # the dialog lists one item more (subset match)
    assert G.find_row(free, G.cli_key(cli)) is None  # consumed: one GUI row per record
    other = {"text": "Text height out of range (min height 0.8000 mm; actual 0.3000 mm)",
             "items": [{"desc": "PCB text 'y' on B.Silkscreen"}]}
    assert G.find_row(free, G.cli_key(other)) is None  # same headline, other item


def test_wanted_rows_all_by_default_cap_per_class_keeps_shorts():
    rows = [{"class": "clearance track-track", "type": "clearance"}] * 4 + \
           [{"class": "shorting_items track-pad", "type": "shorting_items"}] * 3
    assert len(tools._wanted_rows(rows, None, 0)) == 7
    assert len(tools._wanted_rows(rows, None, 1)) == 1 + 3
    assert len(tools._wanted_rows(rows, "clearance", 2)) == 2
    with pytest.raises(tools.ToolError):
        tools._wanted_rows(rows, "hole_clearance", 0)


def test_gui_session_paths_stay_in_the_session_dir(tmp_path):
    e = G._E2E(tmp_path / "s", ":29", None)
    env = e.env()
    for k in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "KICAD_CONFIG_HOME",
              "TOPOR_GUI_HELPER_TOKEN_FILE"):
        assert Path(env[k]).resolve().is_relative_to((tmp_path / "s").resolve()), k
    assert Path(env["TOPOR_GUI_HELPER_SOCK"]).parent.resolve().is_relative_to((tmp_path / "s").resolve())


def test_session_start_writes_nothing_outside_its_dir(tmp_path, monkeypatch):
    """A refused start (display busy) leaves the user's KiCad config untouched and the helper only in the session."""
    user_cfg = Path.home() / ".config" / "kicad"
    before = {p: p.stat().st_mtime_ns for p in user_cfg.rglob("*")} if user_cfg.is_dir() else {}
    monkeypatch.setattr(G._E2E, "start_x", lambda self: False)
    s = G.GuiSession(tmp_path / "b.kicad_pcb", tmp_path / "s", display=":29")
    with pytest.raises(G.GuiError, match="already running"):
        s.start()
    helper = list((tmp_path / "s").rglob("topor_gui_helper.py"))
    assert helper and all("scripting/plugins" in str(h) for h in helper)
    after = {p: p.stat().st_mtime_ns for p in user_cfg.rglob("*")} if user_cfg.is_dir() else {}
    assert after == before
    assert G.GuiSession.starts_total == 0 or s.kicad_starts == 0


class _FakeSession:
    """GuiSession stand-in: records starts; the helper-facing steps return canned rows."""
    mm_px = (1.0, 1.0)  # the fake view's scale (tests: a tiny scale puts every marker far off the 1000 px view)

    def __init__(self, board, rows):
        self.board, self.rows, self.board_state, self.drc_state_of = board, rows, "as opened", None
        self.drc_rows, self.main, self.steps, self.display = [], "0x1", [], ":29"
        self.drc_severities, self.filters = None, []
        self.e = type("E", (), {"shot": staticmethod(lambda tag, win=None: FIX / "shot.png")})()

    def alive(self):
        return True

    def pid(self):
        return 1

    def step(self, *a):
        self.steps.append(a)

    def drc_open(self):
        pass

    def must(self, cmd, step, **a):
        return {}

    def drc_run(self, t):
        self.drc_rows, self.drc_state_of, self.drc_severities = self.rows, self.board_state, None
        return {"tabs": ["Violations (2)", "Unconnected Items (0)"]}

    def drc_filter(self, sev):  # KiCad lists only the ticked severities (row k stays the list position here)
        self.filters.append(tuple(sev))
        self.drc_rows = [r for r in self.rows if r["text"].split(":")[0].lower() in sev]
        self.drc_severities = tuple(sev)
        return self.drc_rows

    def xid(self, t):
        return "0x2"

    def set_zoom(self, v):
        return v

    def drc_walk(self, ks):
        self.walked = list(ks)
        for n, k in enumerate(ks):
            self.cur = n
            yield k, self.rows[k]["text"], None

    def focus(self):  # KiCad centred the n-th marker (the view: 1 mm per px, 1000 px wide, focus at px 500)
        return {"focus_mm": list(self.marks[self.cur]), "canvas": [0, 0, 1000, 1000], "spot_px": [500, 500],
                "mm_px": list(self.mm_px), "view_px": [0, 0, 1000, 1000]}

    origin = (0, 37)  # the main window's xwd image starts below the title bar: screen px - origin = image px
    marker_reads = None  # tests: list of drc_marker answers to return instead of the row's own marker

    marker_off = (3.0, -2.0)  # KiCad centred the items (marks[n]); its marker for the row is this far from there

    def marker(self):  # the marker KiCad highlights for the n-th row: not where it centred, not the items' mean
        if self.marker_reads is not None:
            return self.marker_reads.pop(0) if self.marker_reads else None
        return (self.marks[self.cur][0] + self.marker_off[0], self.marks[self.cur][1] + self.marker_off[1])

    def window_origin(self, win=None):
        return self.origin

    def stop(self):
        pass


def _spots(tmp_path, monkeypatch, off_mm=0.0, warn_second=False, **kw):
    """tools.drc_spots over the 2 recorded rows with a fake session; KiCad 'centres' off_mm away from the track.
    warn_second: the second row is a warning (index row + its dialog line)."""
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    from PIL import Image
    ox, oy = _FakeSession.origin  # screenshot pixel (x, y) shows screen pixel (x + ox, y + oy): colours encode the SCREEN px
    im = Image.new("RGB", (1000 - ox, 1000 - oy))
    im.putdata([((x + ox) // 4, (y + oy) // 4, 7) for y in range(1000 - oy) for x in range(1000 - ox)])
    im.save(tmp_path / "shot.png")
    routed = _j("wiccon-routed-drc.json")
    rows = [{"k": k, "text": "Error: " + v["description"], "items": [i["description"] for i in v["items"]]}
            for k, v in enumerate(x for x in routed["violations"] if x["type"] == "solder_mask_bridge")]
    starts = []
    idx = _idx()
    if warn_second:
        idx["rows"][1]["sev"] = "warning"
        rows[1]["text"] = rows[1]["text"].replace("Error: ", "Warning: ", 1)

    def fake_session_for(board, out, **kw):
        starts.append(board)
        s = _FakeSession(board, rows)
        geom = D.board_items((FIX / "wiccon-routed-mini.kicad_pcb").read_text())  # a point on each row's track
        s.marks = [(lambda p: (p[0], p[1] + off_mm))(geom[r["ours"][0]["uuid"]]["pts"]["start"]) for r in idx["rows"]]
        s.e.shot = lambda tag, win=None: tmp_path / "shot.png"
        G._SESSION = _FakeSession.last = s
        return s
    monkeypatch.setattr(G, "session_for", fake_session_for)
    monkeypatch.setattr(G, "stop_session", lambda: {"stopped": True})
    fake_full = {"info": {"out_dir": str(tmp_path / "run"), "run_id": "r"}, "index": idx,
                 "routed": {"doc": routed}}
    monkeypatch.setattr(tools, "_drc_full", lambda run_id, timeout_s=600.0: fake_full)
    monkeypatch.setattr(tools, "_boards", lambda info: (FIX / "wiccon-input-mini.kicad_pcb",
                                                         FIX / "wiccon-routed-mini.kicad_pcb"))
    monkeypatch.setattr(G, "copy_board", lambda src, dst, stem=None: dst / src.name)
    G._SESSION = None
    try:
        return tools.drc_spots("r", **kw), starts, idx
    finally:
        G._SESSION = None


def test_one_drc_spots_call_over_n_rows_starts_one_kicad(tmp_path, monkeypatch):
    r, starts, idx = _spots(tmp_path, monkeypatch)
    assert len(starts) == 1 and r["wanted"] == len(idx["rows"]) == 2
    assert [x["match"] for x in r["records"]] == [True, True] and r["complete"]
    assert all(Path(x["board_png"]).is_file() for x in r["records"])


def test_drc_spots_walks_errors_only_by_default_with_warnings_unticked(tmp_path, monkeypatch):
    r, _, idx = _spots(tmp_path, monkeypatch, warn_second=True)
    s = _FakeSession.last
    assert s.filters == [("error",)] and s.walked == [0]  # Warnings unticked before the walk; row 1 never visited
    assert r["severities"] == ["error"] and r["wanted"] == 1 and [x["id"] for x in r["records"]] == ["V001"]
    assert r["records"][0]["match"] and r["ours_by_severity"] == {"error": 1, "warning": 1}
    assert r["summary"].startswith("ours: 1 errors, 1 warnings; listed: errors")
    assert r["gui_violations"] == 1  # the dialog list was re-read under the filter


def test_drc_spots_severities_parameter_brings_warnings_back(tmp_path, monkeypatch):
    r, _, _ = _spots(tmp_path, monkeypatch, warn_second=True, severities=["error", "warning"])
    assert _FakeSession.last.filters == [("error", "warning")] and _FakeSession.last.walked == [0, 1]
    assert r["severities"] == ["error", "warning"] and r["wanted"] == 2
    assert [x["match"] for x in r["records"]] == [True, True]
    assert "(severities=" not in r["summary"]
    with pytest.raises(tools.ToolError, match="severities"):
        tools._severities(["fatal"])
    with pytest.raises(tools.ToolError, match="severities"):
        tools._severities([])
    assert tools._severities("warnings") == ("warning",) and tools._severities(None) == ("error",)


def test_drc_rows_are_errors_only_by_default_counts_keep_both(tmp_path, monkeypatch):
    idx = _idx()
    idx["rows"][1]["sev"] = "warning"
    proof = {"rules": {"verdict": "ok"}, "project_used": True}
    full = {"info": {"out_dir": str(tmp_path / "run"), "run_id": "r"}, "index": idx, "result": None,
            "input": {"json": "i.json"}, "routed": {"json": "r.json"},
            "project": {"proof_routed": proof, "proof_input": proof, "files": [], "routed_copy_differs": []}}
    monkeypatch.setattr(tools, "_drc_full", lambda run_id, timeout_s=600.0: full)
    r = tools.drc("r")
    assert [x["id"] for x in r["rows"]] == ["V001"] and r["ours_total"] == 2 and r["rows_listed"] == 1
    assert r["ours_by_severity"] == {"error": 1, "warning": 1} and "1 warnings" in r["summary"]
    r = tools.drc("r", severities=["error", "warning"])
    assert len(r["rows"]) == 2 and r["severities"] == ["error", "warning"]


def test_gui_drc_filter_unticks_warnings_and_rereads_the_list(tmp_path):
    s = G.GuiSession(tmp_path / "b.kicad_pcb", tmp_path / "s", display=":29")
    calls = []

    def must(cmd, step, **a):
        calls.append((cmd, a))
        if cmd == "set_checkboxes":
            return [{"label": k, "value": v, "enabled": True} for k, v in a["values"].items()]
        return [{"k": 0, "text": "Error: x", "items": []}]
    s.must = must
    assert s.drc_filter(("error",)) == [{"k": 0, "text": "Error: x", "items": []}]
    assert calls[0] == ("set_checkboxes", {"title": G.DRC_TITLE, "values": {"Errors": True, "Warnings": False}})
    assert calls[1][0] == "drc_rows" and s.drc_severities == ("error",)
    s.must = lambda cmd, step, **a: ([{"label": "Errors", "value": True}, {"label": "Warnings", "value": True}]
                                     if cmd == "set_checkboxes" else [])
    with pytest.raises(G.GuiError, match="severity filter"):
        s.drc_filter(("error",))


def test_spot_match_needs_kicads_focus_on_the_marker(tmp_path, monkeypatch):
    r, _, _ = _spots(tmp_path, monkeypatch, off_mm=50.0)
    assert [x["match"] for x in r["records"]] == [False, False]
    assert all(x["selected_ok"] and "focus_score" in x["why"] and x["focus_score"] < r["match_min"]
               for x in r["records"])


def test_spot_focus_score_is_graded_not_a_cutoff(tmp_path, monkeypatch):
    """verify r4 F3: the focus check is a smooth score of the distance, scaled by the row's geometry extent."""
    scores = []
    for off in (0.0, 0.5, 1.5, 3.0, 6.0):
        (tmp_path / f"o{off}").mkdir()
        r, _, _ = _spots(tmp_path / f"o{off}", monkeypatch, off_mm=off)
        scores.append(r["records"][0]["focus_score"])
        assert r["records"][0]["focus_scale_mm"] >= tools.FOCUS_TOL_MM
    # these rows hold a 58 / 69 mm track + a mask polygon point: a large scale, the score falls slowly but surely
    assert scores[0] == 1.0 and all(a >= b for a, b in zip(scores, scores[1:])) and scores[-1] < scores[1], scores
    assert tools.focus_score(1.0, 1.0) == 0.5 and tools.focus_score(None, 1.0) == 1.0
    # a long track: the same 4 mm offset scores far better than on a short one
    long_row = {"items": [{"uuid": "t", "x": 0, "y": 0}]}
    short_row = {"items": [{"uuid": "s", "x": 0, "y": 0}]}
    geom = {"t": {"kind": "track", "pts": {"start": (0, 0), "end": (20, 0)}},
            "s": {"kind": "track", "pts": {"start": (0, 0), "end": (0.5, 0)}}}
    sc = lambda row: tools.FOCUS_TOL_MM + tools.FOCUS_EXTENT_SHARE * D.row_extent(row, geom)  # noqa: E731
    assert tools.focus_score(4.0, sc(long_row)) > 0.8 > 0.2 > tools.focus_score(4.0, sc(short_row))


def test_marker_dist_counts_pads_and_polygons_at_their_points():
    row = {"items": [{"uuid": "t", "x": 0, "y": 0}, {"uuid": "zone-1", "x": 10, "y": 0}], "x": 5, "y": 0}
    geom = {"t": {"kind": "track", "pts": {"start": (0, 0), "end": (1, 0)}}}
    assert D.marker_dist((10, 1), row, geom) == 1.0  # the polygon's point, not the far track
    assert D.row_extent(row, geom) == 10.0


ROOT = Path(__file__).resolve().parents[2]


def test_gui_helper_ships_only_in_harness_never_in_the_plugin_or_wheel():
    """The wx helper is test-only: build.sh's install_into copies no harness/ path, none of what it copies holds
    the helper, and the engine wheel is Rust-only (maturin without python-source)."""
    helpers = sorted(p.relative_to(ROOT).as_posix() for d in ("harness", "plugin", "toporlib", "mcp_server", "tools",
                                                              "docs", "engine/src") for p in (ROOT / d).rglob("*gui_helper*.py"))
    assert helpers == ["harness/gui_helper/topor_gui_helper.py"]
    build = (ROOT / "tools/build.sh").read_text()
    body = build[build.index("install_into() {"):build.index("\n}\n", build.index("install_into() {"))]
    srcs = [w for line in body.splitlines() if line.strip().startswith("cp ")
            for w in line.split()[1:] if not w.startswith(("-", '"$', "$")) and not w.startswith("'")]
    assert srcs and not [s for s in srcs if s.startswith("harness")]
    for s in srcs:
        p = ROOT / s.rstrip(".").rstrip("/")
        files = [p] if p.is_file() else list(p.rglob("*"))
        assert not [f for f in files if "gui_helper" in f.name], s
    assert "python-source" not in (ROOT / "engine/pyproject.toml").read_text()


def test_helper_dispatch_checks_token_command_and_arguments():
    import importlib.util
    spec = importlib.util.spec_from_file_location("h", ROOT / "harness/gui_helper/topor_gui_helper.py")
    h = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(h)  # no wx needed at import (wx is imported inside the commands)
    run = lambda f: f()  # noqa: E731
    cmds = {"ping": lambda: "pong", "boom": lambda: 1 / 0, "two": lambda a, b: a + b}
    assert h.dispatch({"token": "t", "cmd": "ping"}, "t", run, cmds) == {"ok": True, "result": "pong"}
    assert h.dispatch({"token": "x", "cmd": "ping"}, "t", run, cmds)["error"] == "bad token"
    assert h.dispatch(["ping"], "t", run, cmds)["error"] == "bad token"
    assert "unknown command" in h.dispatch({"token": "t", "cmd": "exec"}, "t", run, cmds)["error"]
    assert "bad arguments" in h.dispatch({"token": "t", "cmd": "two", "a": 1}, "t", run, cmds)["error"]
    assert "ZeroDivisionError" in h.dispatch({"token": "t", "cmd": "boom"}, "t", run, cmds)["error"]
    assert h.dispatch({"token": "t", "cmd": "two", "a": 1, "b": 2}, "t", run, cmds)["result"] == 3
    assert not {"exec", "eval", "python", "shell"} & set(h.COMMANDS)


def test_board_items_read_pre_kicad8_tstamp_ids_and_net_numbers():
    board = ('(kicad_pcb\n  (net 0 "")\n  (net 2 "GND")\n'
             '  (segment (start 1 2) (end 3 2) (width 0.25) (layer "F.Cu") (net 2) (tstamp aa-1))\n'
             '  (via (at 3 2) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 2) (tstamp "bb-2"))\n)')
    it = D.board_items(board)
    assert set(it) == {"aa-1", "bb-2"} and it["aa-1"]["net"] == "GND" and it["bb-2"]["kind"] == "via"


def test_marker_dist_is_to_the_nearest_item_not_the_mean():
    items = D.board_items("(kicad_pcb\n" + _seg("a", "A", (0, 0), (10, 0)) + _seg("b", "B", (0, 1), (10, 1)) + ")")
    row = {"items": [{"uuid": "a"}, {"uuid": "b"}, {"uuid": "pad-x"}], "x": 50.0, "y": 50.0}
    assert D.marker_dist((5, 0.4), row, items) == 0.4
    assert D.marker_dist((50, 53), {"items": [{"uuid": "pad"}], "x": 50.0, "y": 50.0}, items) == 3.0
    assert D.marker_dist((0, 0), {"items": [], "x": None}, items) is None


# ------------------------------------------------------------------ fix round 2 (verify-mcp-drc-20261119-1900)

ISOLATED = ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME", "KICAD_CONFIG_HOME")


def _assert_isolated(env, root):
    assert env is not None, "a KiCad process started with the user's environment"
    for k in ISOLATED:
        assert Path(env[k]).resolve().is_relative_to(Path(root).resolve()), (k, env[k])


def test_every_kicad_process_of_the_tools_gets_an_isolated_home(tmp_path, monkeypatch):
    """`kicad-cli version` once rewrote the user's ~/.config/kicad/10.0/kicad_common.json (no env=)."""
    import subprocess
    calls = []

    def rec(cmd, *a, **kw):
        calls.append((cmd, kw.get("env")))
        if "-o" in cmd:
            Path(cmd[cmd.index("-o") + 1]).write_text('{"violations": []}')
        return subprocess.CompletedProcess(cmd, 1 if "route_board.sh" in str(cmd) else 0, "10.0.8", "")
    monkeypatch.setattr(subprocess, "run", rec)
    monkeypatch.setattr(D, "_VERSION", {})
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    pcb = _board_dir(tmp_path)
    D.run_checked(pcb, tmp_path / "w", "routed")  # kicad-cli version + kicad-cli pcb drc
    with pytest.raises(tools.ToolError):
        tools.route(str(pcb), {"png": False})  # route_board.sh (pcbnew + kicad-cli inside)
    assert [c[0][1] for c in calls[:2]] == ["version", "pcb"] and "route_board.sh" in str(calls[2][0])
    _assert_isolated(calls[0][1], tmp_path / "w")
    _assert_isolated(calls[1][1], tmp_path / "w")
    _assert_isolated(calls[2][1], pcb.parent.parent)
    # and no subprocess call in the MCP server code without an env=
    import re
    for f in (ROOT / "mcp_server").glob("*.py"):
        for m in re.finditer(r"subprocess\.(?:run|Popen|call|check_output|check_call)\(", f.read_text()):
            body = f.read_text()[m.end():m.end() + 400]
            assert "env=" in body[:body.find("\n\n") if "\n\n" in body else 400], f"{f.name}: {m.group()} without env="


def _tree_state(d: Path) -> dict:
    import hashlib
    return {p: (p.stat().st_mtime_ns, hashlib.sha256(p.read_bytes()).hexdigest())
            for p in sorted(d.rglob("*")) if p.is_file()} if d.is_dir() else {}


@pytest.mark.skipif(not Path(D.kicad_cli_path()).is_file(), reason="needs kicad-cli")
def test_real_kicad_cli_leaves_the_users_kicad_config_untouched(tmp_path, monkeypatch):
    from _helpers import eprom_dir
    user = Path.home() / ".config" / "kicad"
    before = _tree_state(user)
    monkeypatch.setattr(D, "_VERSION", {})
    src = eprom_dir()
    shutil.copytree(src, tmp_path / "b")
    pcb = next((tmp_path / "b").glob("*.kicad_pcb"))
    r = D.run_checked(pcb, tmp_path / "w", "input")
    assert r["proof"]["project_used"] and not r["cached"]
    assert D._VERSION  # the real `kicad-cli version` ran too
    assert _tree_state(user) == before


def test_no_project_beside_the_input_is_drc_without_project(tmp_path):
    pcb = _board_dir(tmp_path)
    with pytest.raises(D.DrcError) as ei:
        D.run_checked(pcb, tmp_path / "w", "input", project={}, runner=_runner({"violations": []}))
    assert ei.value.code == "DRC_WITHOUT_PROJECT"


def test_drc_tool_refuses_an_input_copy_without_its_project(tmp_path, monkeypatch):
    monkeypatch.setenv("TOPOR_MCP_STATE", str(tmp_path / "state"))
    d = tmp_path / "r"
    (d / "w-input").mkdir(parents=True)
    (d / "w-topor").mkdir()
    shutil.copy(FIX / "wiccon-input-mini.kicad_pcb", d / "w-input/2023.kicad_pcb")
    shutil.copy(FIX / "wiccon-routed-mini.kicad_pcb", d / "w-topor/2023.kicad_pcb")
    shutil.copy(FIX / "wiccon.kicad_pro", d / "w-topor/2023.kicad_pro")  # only the routed copy has one
    tools._remember("r", {"run_id": "r", "board_copy": str(d / "w-topor/2023.kicad_pcb"), "out_dir": str(d),
                          "report_dir": None, "status": "ok", "name": "w"})
    monkeypatch.setattr(D, "run_kicad_cli", _runner({"violations": []}))
    with pytest.raises(tools.ToolError, match="DRC_WITHOUT_PROJECT"):
        tools.drc("r")


def test_violation_identity_is_type_and_uuids_never_the_text():
    a = _v("clearance", [("u1", "Track [A] on F.Cu, length 1,0000 mm", 0, 0), ("u2", "Pad 1 [B] of U1", 0, 0)],
           "Clearance violation (clearance 0,2000 mm; actual 0,1000 mm)")
    b = _v("clearance", [("u2", "Pad 1 [B] of U1", 0, 0), ("u1", "Track [A] on F.Cu, length 2,5000 mm", 0, 0)],
           "Clearance violation (clearance 0,2000 mm; actual 0,0500 mm)")
    c = _v("clearance", [("u3", "Track [A] on F.Cu, length 1,0000 mm", 0, 0), ("u2", "Pad 1 [B] of U1", 0, 0)],
           a["description"])
    assert D.vkey(a) == D.vkey(b) and D.vkey(a) != D.vkey(c)
    # end to end: the same pair with other measured values is pre-existing, the same text on new copper is new
    board = "(kicad_pcb\n" + _seg("u1", "A", (0, 0), (1, 0)) + _seg("u3", "A", (0, 1), (1, 1)) + ")"
    idx = D.index("(kicad_pcb\n" + _seg("u1", "A", (0, 0), (1, 0)) + ")", {"violations": [a]}, board,
                  {"violations": [b, c]}, None)
    assert [r["items"][0]["uuid"] for r in idx["rows"]] in (["u2"], ["u3"]) and len(idx["rows"]) == 1
    assert {i["uuid"] for i in idx["rows"][0]["items"]} == {"u2", "u3"}


def test_cache_key_carries_the_project_bytes(tmp_path):
    pcb = _board_dir(tmp_path)
    run = _runner({"violations": []})
    snap = D.project_snapshot(pcb)
    D.run_checked(pcb, tmp_path / "w", "routed", project=snap, runner=run)
    other = dict(PRO, board={"design_settings": {"rules": {"min_clearance": 0.12}}})
    r = D.run_checked(pcb, tmp_path / "w", "routed", project={".kicad_pro": json.dumps(other).encode()}, runner=run)
    assert not r["cached"] and len(run.calls) == 2  # other rules = another DRC, never the cached numbers


def test_sigterm_stops_the_session_by_pid_then_chains(monkeypatch):
    stopped, chained = [], []
    fake = type("S", (), {"stop": lambda self: stopped.append(1)})()
    monkeypatch.setattr(G, "_SESSION", fake)
    monkeypatch.setattr(G, "_PREV_HANDLERS", {15: lambda s, f: chained.append(s)})
    G._on_signal(15, None)
    assert stopped == [1] and chained == [15]
    monkeypatch.setattr(G, "_PREV_HANDLERS", {15: None})
    with pytest.raises(SystemExit) as ei:
        G._on_signal(15, None)
    assert ei.value.code == 143 and stopped == [1, 1]


def test_install_signal_stop_hooks_sigterm_and_sighup(monkeypatch):
    import signal
    seen = {}
    monkeypatch.setattr(signal, "signal", lambda s, h: seen.__setitem__(s, h))
    monkeypatch.setattr(G, "_PREV_HANDLERS", {})
    assert G.install_signal_stop()
    assert seen == {signal.SIGTERM: G._on_signal, signal.SIGHUP: G._on_signal}


def test_a_timed_out_x_tool_is_a_step_error_with_the_command():
    import subprocess
    ex = subprocess.TimeoutExpired(["xdotool", "windowactivate", "--sync", "0x1"], 30)
    assert isinstance(ex, G.STEP_ERRORS) and "xdotool windowactivate --sync" in G.describe(ex)
    from harness import gui_e2e
    assert gui_e2e.X_TIMEOUT_S <= 60 and gui_e2e.E2E.x.__kwdefaults__["timeout"] == gui_e2e.X_TIMEOUT_S


def test_escaped_net_names_match_the_drc_text():
    """Board file / result.json say "D out{slash}in", the DRC text "D out/in" (wiccon e2e: conns were [])."""
    assert D.net_name("D out{slash}in") == "D out/in" and D.net_name("A{unknown}") == "A{unknown}"
    board = "(kicad_pcb\n" + _seg("s1", "D out{slash}in", (0, 0), (5, 0)) + ")"
    items = D.board_items(board)
    assert items["s1"]["net"] == "D out/in"
    conns = [_conn(4, "D out{slash}in", (0, 0), (5, 0))]
    assert D.conn_map(items, {"s1"}, conns)["s1"] == [{"conn": 4, "d_mm": 0.0}]
    after = {"unconnected_items": [{"items": [
        {"uuid": "p1", "description": "Pad 1 [D out/in] of U1 on F.Cu", "pos": {"x": 0, "y": 0}},
        {"uuid": "p2", "description": "Pad 2 [D out/in] of U2 on F.Cu", "pos": {"x": 5, "y": 0}}]}]}
    out = D.newly_unconnected({"unconnected_items": []}, after, conns, items, {"s1"}, D.conn_map(items, {"s1"}, conns))
    assert out["D out/in"]["new"][0]["conns"][0]["conn"] == 4 and out["D out/in"]["lost_copper"][0]["conns"] == [4]


def _walk_session(tmp_path, model, shown, click_line=0, lag=0, fail_once=None):
    """GuiSession whose dialog shows `shown` (a flat list of line texts) while drc_rows (the model read) is `model`;
    keys move a cursor over `shown`, a click on the first line selects shown[click_line].
    lag: the selection read stays on the old line for this many reads after a key (KiCad busy).
    fail_once: the n-th selection read (1-based) raises a helper RuntimeError."""
    s = G.GuiSession(tmp_path / "b.kicad_pcb", tmp_path / "s", display=":29")
    s.drc_rows = model
    cur = {"i": None, "pending": None, "wait": 0, "reads": 0}
    s.log = []

    def settle():
        pass

    def call(cmd, **a):
        if cmd == "drc_selected":
            cur["reads"] += 1
            if fail_once == cur["reads"]:
                raise RuntimeError("helper: boom")
            if cur["pending"] is not None:
                if cur["wait"] > 0:
                    cur["wait"] -= 1
                else:
                    cur["i"], cur["pending"] = cur["pending"], None
                    settle()
            return {"selected": None if cur["i"] is None else shown[cur["i"]]}
        if cmd == "drc_list_rect":
            return {"rect": [0, 0, 100, 100]}
        raise AssertionError(cmd)

    def x(*args, **kw):
        s.log.append(args[1:])
        if args[:2] == ("xdotool", "key"):
            nxt = max(0, min(len(shown) - 1, cur["i"] + (1 if args[2] == "Down" else -1)))
            if lag:
                cur["pending"], cur["wait"] = nxt, lag
            else:
                cur["i"] = nxt
                settle()

    def click(*a):
        cur["i"] = click_line
        settle()
    s.call, s.activate, s.alive = call, (lambda t: None), (lambda: True)
    s.e = type("E", (), {"x": staticmethod(x), "click_abs": staticmethod(click), "log": staticmethod(lambda m: None),
                            "shot": staticmethod(lambda *a, **k: None)})()
    return s


def test_drc_walk_relocates_when_the_dialog_shows_other_item_lines(tmp_path, monkeypatch):
    """verify r4 F2: the dialog lists row 0 with ONE item line where the model read two (Down lands on row 1's
    headline): the walk relocates by text and still reaches every row; nothing ends the call."""
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    model = [{"k": 0, "text": "A", "items": ["a1", "a2"]}, {"k": 1, "text": "B", "items": ["b1"]},
             {"k": 2, "text": "C", "items": ["c1"]}]
    shown = ["A", "a1", "B", "b1", "C", "c1"]
    s = _walk_session(tmp_path, model, shown)
    assert list(s.drc_walk([0, 1, 2])) == [(0, "A", None), (1, "B", None), (2, "C", None)]
    assert any(st["step"] == "drc walk resync" for st in s.steps)


def test_drc_walk_unreachable_row_is_reported_and_the_walk_goes_on(tmp_path, monkeypatch):
    """A first-line click that selects nothing known fails that row only; the next row starts from a new click."""
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    model = [{"k": 0, "text": "A", "items": ["a1"]}, {"k": 1, "text": "B", "items": ["b1"]}]
    s = _walk_session(tmp_path, model, ["?", "A", "a1", "B", "b1"], click_line=0)  # the click lands on '?'
    out = list(s.drc_walk([0, 1]))
    assert [o[0] for o in out] == [0, 1] and all(o[1] is None and o[2] for o in out)


_MODEL3 = [{"k": 0, "text": "A", "items": ["a1"]}, {"k": 1, "text": "B", "items": ["b1"]},
           {"k": 2, "text": "C", "items": ["c1"]}]


def test_drc_walk_an_unread_dialog_line_does_not_make_later_rows_unreachable(tmp_path, monkeypatch):
    """verify r6 N2: the dialog shows a line the model never read ('zz'): the key goes on the same way until a known
    line shows, so row 1 and every later row are reached (before: each restarted and died on 'zz' again)."""
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    s = _walk_session(tmp_path, _MODEL3, ["A", "a1", "zz", "B", "b1", "C", "c1"])
    assert list(s.drc_walk([0, 1, 2])) == [(0, "A", None), (1, "B", None), (2, "C", None)]
    assert any(st["step"] == "drc walk unknown line" for st in s.steps)


def test_drc_walk_unknown_line_that_never_ends_fails_the_row_not_the_walk(tmp_path, monkeypatch):
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    model = [{"k": 0, "text": "A", "items": []}, {"k": 1, "text": "B", "items": []}]
    s = _walk_session(tmp_path, model, ["A", "zz", "yy", "xx"])  # B is never shown
    out = list(s.drc_walk([0, 1]))
    assert out[0] == (0, "A", None) and out[1][0] == 1 and out[1][1] is None and "never read" in out[1][2]


def test_drc_walk_a_busy_kicad_does_not_use_up_the_relocation_allowance(tmp_path, monkeypatch):
    """verify r6 N3: the selection reads back the OLD line for a while (KiCad busy): wait, not a relocation."""
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    shown = ["A", "a1", "B", "b1", "C", "c1"]
    s = _walk_session(tmp_path, _MODEL3, shown, lag=2)
    assert list(s.drc_walk([0, 1, 2], max_resyncs=0, busy_polls=4)) == [(0, "A", None), (1, "B", None), (2, "C", None)]
    assert not any(st["step"] == "drc walk resync" for st in s.steps)


def test_drc_walk_a_failed_helper_step_fails_that_row_and_the_walk_goes_on(tmp_path, monkeypatch):
    """verify r6 M6: a helper error on one row is yielded with its error; the next row is reached."""
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    shown = ["A", "a1", "B", "b1", "C", "c1"]
    s = _walk_session(tmp_path, _MODEL3, shown, fail_once=2)  # read 1 = the first click, read 2 = row 0's own selection read
    out = list(s.drc_walk([0, 1, 2]))
    assert out[0][0] == 0 and out[0][1] is None and "boom" in out[0][2]
    assert out[1:] == [(1, "B", None), (2, "C", None)]


def test_spot_records_scale_from_the_row_extent_and_match_min_is_honoured(tmp_path, monkeypatch):
    """verify r6 M13/M14: focus_scale_mm = 1 mm + half the row's geometry extent (read from the routed copper);
    a stricter / looser match_min flips match for the same focus."""
    r, _, idx = _spots(tmp_path, monkeypatch, off_mm=1.0)
    geom = D.board_items((FIX / "wiccon-routed-mini.kicad_pcb").read_text())
    for rec, row in zip(r["records"], idx["rows"]):
        want = round(tools.FOCUS_TOL_MM + tools.FOCUS_EXTENT_SHARE * D.row_extent(row, geom), 3)
        assert rec["focus_scale_mm"] == want and want > 10  # long tracks: far above the 1 mm floor
    assert all(x["match"] for x in r["records"]) and r["match_min"] == tools.MATCH_MIN
    (tmp_path / "strict").mkdir()
    r2, _, _ = _spots(tmp_path / "strict", monkeypatch, off_mm=1.0, match_min=0.9999)
    assert [x["match"] for x in r2["records"]] == [False, False] and r2["match_min"] == 0.9999
    assert all("match_min 0.9999" in x["why"] for x in r2["records"])
    (tmp_path / "loose").mkdir()
    r3, _, _ = _spots(tmp_path / "loose", monkeypatch, off_mm=50.0, match_min=0.0)
    assert [x["match"] for x in r3["records"]] == [True, True]


def test_spot_flags_a_focus_that_repeats_although_the_marker_moved(tmp_path, monkeypatch):
    monkeypatch.setattr(_FakeSession, "focus", lambda self: {"focus_mm": [1.0, 2.0], "canvas": [0, 0, 1000, 1000],
                                                              "spot_px": [500, 500], "mm_px": [1.0, 1.0],
                                                              "view_px": [0, 0, 1000, 1000]})
    r, _, idx = _spots(tmp_path, monkeypatch)
    far = ((idx["rows"][0]["x"] - idx["rows"][1]["x"]) ** 2 + (idx["rows"][0]["y"] - idx["rows"][1]["y"]) ** 2) ** .5
    assert far > 1.0
    assert [x.get("focus_repeat", False) for x in r["records"]] == [False, True]


def test_mcp_drc_collects_without_plugin_on_the_path():
    """verify r6 N4: `import mcp_server.drc` first (as pytest tests/mcp_server/test_mcp_drc.py alone) finds topor."""
    import subprocess
    import sys
    code = "import mcp_server.drc, sys; print('topor' in sys.modules)"
    env = {k: v for k, v in __import__('os').environ.items() if k != "PYTHONPATH"}
    r = subprocess.run([sys.executable, "-I", "-c", "import sys; sys.path.insert(0, %r); " % str(ROOT) + code],
                       capture_output=True, text=True, env=env, cwd="/")
    assert r.returncode == 0, r.stderr[-400:]


def test_the_ring_goes_on_the_rows_own_marker_not_on_where_kicad_centred(tmp_path, monkeypatch):
    """verify r6 N1: KiCad centres the row's items (for tracks the middle of their bounding box), possibly mm away from
    the marker; the ring / crop is put on the row's marker by the measured view transform, and a marker that is not
    on screen is reported (no ring on the wrong spot)."""
    r, _, idx = _spots(tmp_path, monkeypatch, off_mm=0.0)
    for n, (rec, row) in enumerate(zip(r["records"], idx["rows"])):
        mk = (_FakeSession.last.marks[n][0] + _FakeSession.marker_off[0],
              _FakeSession.last.marks[n][1] + _FakeSession.marker_off[1])  # KiCad's marker, not the items' mean
        G_marker = want = tools.marker_px({"mm_px": [1.0, 1.0], "spot_px": [500, 500], "focus_mm": rec["focus_mm"]},
                                          mk[0], mk[1])
        assert rec["marker_mm"] == [round(mk[0], 4), round(mk[1], 4)] and rec["marker_source"] == "kicad"
        assert (row["x"], row["y"]) != pytest.approx(mk, abs=0.5)  # the two really differ in this fixture
        assert rec["marker_px"] == [round(want[0]), round(want[1])] and rec["marker_visible"] and G_marker
        assert rec["marker_px"] != [500, 500]  # not the spot where KiCad centred the items
        rx, ry = _ring_centre(rec["board_png"])
        assert abs(rx - rec["marker_px"][0]) <= 4 and abs(ry - rec["marker_px"][1]) <= 4, (rx, ry, rec["marker_px"])
    (tmp_path / "far").mkdir()
    monkeypatch.setattr(_FakeSession, "mm_px", (0.001, 0.001))  # the marker 10-60 mm from KiCad's focus: 10000+ px off
    r2, _, _ = _spots(tmp_path / "far", monkeypatch, off_mm=0.0)  # KiCad focused ON the row's track: a good score
    assert [x["focus_score"] for x in r2["records"]] == [1.0, 1.0]
    assert [x["marker_visible"] for x in r2["records"]] == [False, False] and not any(x["match"] for x in r2["records"])
    assert all("not on screen" in x["why"] for x in r2["records"])


def test_ring_is_in_screenshot_pixels_not_screen_pixels(tmp_path, monkeypatch):
    """verify n1 M2: the main window's xwd image starts at screen (0, 37): the ring sits on the screenshot pixel that
    shows the marker's SCREEN pixel (the test screenshot's colours encode the screen position)."""
    for origin in ((0, 37), (0, 0), (5, 61)):
        d = tmp_path / f"o{origin[0]}_{origin[1]}"
        d.mkdir()
        monkeypatch.setattr(_FakeSession, "origin", origin)
        r, _, _ = _spots(d, monkeypatch)
        for rec in r["records"]:
            rx, ry = _ring_centre(rec["board_png"])
            assert abs(rx - rec["marker_px"][0]) <= 4 and abs(ry - rec["marker_px"][1]) <= 4, (origin, rx, ry, rec["marker_px"])


def test_a_row_without_a_single_kicad_marker_cannot_match(tmp_path, monkeypatch):
    """verify n1 H1: no brightened marker (or several) -> marker_source items_mean, match False with the reason."""
    monkeypatch.setattr(_FakeSession, "marker_reads", [None, None])
    r, _, _ = _spots(tmp_path, monkeypatch)
    assert [x["marker_source"] for x in r["records"]] == ["items_mean"] * 2 and not any(x["match"] for x in r["records"])
    assert all("no single highlighted marker" in x["why"] for x in r["records"])


def test_gui_marker_reads_the_one_brightened_marker_and_retries(tmp_path, monkeypatch):
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    s = G.GuiSession(tmp_path / "b.kicad_pcb", tmp_path / "s", display=":29")
    answers = [{"markers": []}, {"markers": [{"x": 1, "y": 2}, {"x": 3, "y": 4}]}, {"markers": [{"x": 5.5, "y": 6.5}]}]
    s.call = lambda cmd, **a: answers.pop(0) if cmd == "drc_marker" else pytest.fail(cmd)
    assert s.marker() == (5.5, 6.5)
    answers[:] = [{"markers": []}] * 5
    assert s.marker() is None
    s.e = type("E", (), {"geom": staticmethod(lambda w: (3, 37, 1920, 1043))})()
    s.main = "0x1"
    assert s.window_origin() == (3, 37)


def test_helper_drc_marker_returns_only_brightened_markers(monkeypatch):
    import sys
    import types
    import importlib.util
    spec = importlib.util.spec_from_file_location("h", ROOT / "harness/gui_helper/topor_gui_helper.py")
    h = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(h)
    mk = lambda x, y, b: types.SimpleNamespace(IsBrightened=lambda: b, GetPosition=lambda: types.SimpleNamespace(x=x, y=y))  # noqa: E731
    fake = types.SimpleNamespace(GetBoard=lambda: types.SimpleNamespace(Markers=lambda: [mk(1e6, 2e6, False), mk(3.5e6, 4.25e6, True)]))
    monkeypatch.setitem(sys.modules, "pcbnew", fake)
    assert h.cmd_drc_marker() == {"markers": [{"x": 3.5, "y": 4.25}]} and "drc_marker" in h.COMMANDS


def _ring_centre(png, marker_px=None, half=350):
    """Position in the SCREENSHOT of the yellow ring's centre in a drc_spots crop (the test screenshot encodes each
    pixel's own position in its colour, 4 px steps; the crop is cut at the image's left / top edge)."""
    from PIL import Image
    im = Image.open(png).convert("RGB")
    pts = [(x, y) for x in range(im.width) for y in range(im.height) if im.getpixel((x, y)) == (255, 230, 0)]
    assert pts, "no ring in the crop"
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    c = ((min(xs) + max(xs)) // 2, (min(ys) + max(ys)) // 2)
    r, g, _ = im.getpixel(c)  # the pixel inside the ring: where on the screenshot it was cut from
    return (r * 4, g * 4)


def test_marker_px_follows_the_view_transform_and_needs_a_scale():
    f = {"mm_px": [0.014, 0.014], "spot_px": [700, 500], "focus_mm": [100.0, 50.0]}
    assert tools.marker_px(f, 101.4, 49.3) == pytest.approx((800.0, 450.0))
    assert tools.marker_px({**f, "mm_px": [None, 0.014]}, 101.4, 49.3) is None


def test_gui_focus_reads_x_y_at_the_spot_and_measures_the_scale_once_per_zoom(tmp_path, monkeypatch):
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    s = G.GuiSession(tmp_path / "b.kicad_pcb", tmp_path / "s", display=":29")
    ptr = {"p": (0, 0)}
    moves = []

    def x(*args, **kw):
        if args[:2] == ("xdotool", "mousemove"):
            ptr["p"] = (int(args[2]), int(args[3]))
            moves.append(ptr["p"])

    def call(cmd, **a):
        if cmd == "canvas_rect":
            return [340, 178, 1312, 840]
        if cmd == "status":  # X = 100 + 0.014 * px, Y = 50 + 0.014 * py (0.01 grid)
            return [f"X {100 + 0.014 * ptr['p'][0]:.2f}  Y {50 + 0.014 * ptr['p'][1]:.2f}".replace("  ", " ")]
        raise AssertionError(cmd)
    s.call, s.e = call, type("E", (), {"x": staticmethod(x)})()
    f = s.focus()
    sx, sy = 340 + (G.DRC_RECT[0] - 340) // 2, 178 + 420
    assert f["spot_px"] == [sx, sy] and f["focus_mm"] == [round(100 + 0.014 * sx, 2), round(50 + 0.014 * sy, 2)]
    assert f["mm_px"][0] == pytest.approx(0.014, rel=0.02) and f["mm_px"][1] == pytest.approx(0.014, rel=0.02)
    assert f["view_px"] == [340, 178, G.DRC_RECT[0], 1018]
    n = len(moves)
    s.focus()
    assert len(moves) - n < n  # the scale is kept: no far read the second time
    s.must = lambda cmd, step, **a: {"zoom": "Zoom 20,00"}
    s.set_zoom(20)
    assert s._scale is None  # a new zoom asks for a new scale


def test_drc_walk_relocates_to_the_nearest_duplicate_line_not_the_first(tmp_path, monkeypatch):
    """verify r6 M4: row B shows no 'q' line, so Down from B lands on 'x' (a text three lines carry): the relocation
    takes the NEAREST 'x' (line 4), after which C is one key away; the first 'x' (line 1) needs a 2nd relocation."""
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    model = [{"k": 0, "text": "A", "items": ["x"]}, {"k": 1, "text": "B", "items": ["q", "x"]},
             {"k": 2, "text": "C", "items": ["x"]}]
    s = _walk_session(tmp_path, model, ["A", "x", "B", "x", "C", "x"])
    assert list(s.drc_walk([2], max_resyncs=1)) == [(2, "C", None)]
    assert sum(st["step"] == "drc walk resync" for st in s.steps) == 1


def test_drc_walk_relocation_allowance_is_enforced(tmp_path, monkeypatch):
    """verify r6 M7: more relocations than max_resyncs fail the row (error names the relocations), not loop."""
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    model = [{"k": 0, "text": "A", "items": ["a1", "a2"]}, {"k": 1, "text": "B", "items": ["b1"]},
             {"k": 2, "text": "C", "items": ["c1"]}]
    s = _walk_session(tmp_path, model, ["A", "a1", "C", "c1", "B", "b1"])
    out = list(s.drc_walk([1], max_resyncs=0))
    assert len(out) == 1 and out[0][0] == 1 and out[0][1] is None and "relocations" in out[0][2]


def test_drc_walk_after_an_unreachable_row_the_next_row_starts_from_a_click(tmp_path, monkeypatch):
    """verify r6 M2: after a failed row the position is unknown: the next row clicks the first line again (a stale
    position would make the keys start from the wrong line)."""
    monkeypatch.setattr(G.time, "sleep", lambda t: None)
    s = _walk_session(tmp_path, _MODEL3, ["A", "a1", "B", "b1", "C", "c1"], fail_once=2)
    clicks = []
    orig = s.e.click_abs
    s.e.click_abs = lambda *a: (clicks.append(a), orig(*a))[1]
    out = list(s.drc_walk([0, 1, 2]))
    assert out[0][2] and out[1:] == [(1, "B", None), (2, "C", None)]
    assert len(clicks) == 2  # the first click + the restart after row 0 failed
