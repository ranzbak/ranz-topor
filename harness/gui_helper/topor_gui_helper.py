"""TEST-ONLY helper inside a KiCad started by mcp_server.kicad_gui (isolated Xephyr home): drives KiCad's own DRC and
Cleanup Tracks & Vias dialogs by widget label / id with wx events. Never shipped (tools/build.sh copies no
harness/), never installed into a user's KiCad: kicad_gui copies it into <session home>/.local/share/kicad/10.0/
scripting/plugins/ only, and it does nothing unless TOPOR_GUI_HELPER_SOCK is set (only that session's env has it).

Why: KiCad 10's API (kipy 0.8) can open the dialogs (run_action) but has no message to set their options, run
them or read their rows. A legacy action-plugin module is imported by KiCad's own Python at start, where wx sees
the dialogs as objects: no pixel coordinates.

Protocol: unix socket TOPOR_GUI_HELPER_SOCK (in the session's 0700 home), one JSON line per request
{"token": ..., "cmd": ..., **args} -> one line {"ok": true, "result": ...} | {"ok": false, "error": "..."}.
The token is read from TOPOR_GUI_HELPER_TOKEN_FILE (mode 0600). FIXED command set (COMMANDS), no code execution.
Every command runs on the wx UI thread (wx.CallAfter) and is bounded by UI_CALL_TIMEOUT; button clicks and menu
items are POSTED (wx.PostEvent), so a modal dialog they open never blocks the helper.

Portions (the listener-thread + wx.CallAfter executor, label normalisation, button lookup / posted click event)
are adapted from mixelpixx/KiCAD-MCP-Server, gui_driver_plugin/plugins/{listener,driver}.py at commit
1e654552f409f9f484034d31ec5ffb0ed2af6a2d, under this licence:

    MIT License

    Copyright (c) 2024 mixelpixx

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

The rest: Copyright (C) the topor authors, GPL-3.0-only (as the repository).
"""
from __future__ import annotations

import hmac
import json
import os
import socketserver
import threading
import traceback

UI_CALL_TIMEOUT = 20.0
MAX_REQUEST_BYTES = 64 * 1024
DRC_TITLE = "Design Rules Checker"
DRC_TABS = {"violations": 0, "unconnected": 1}


def normalize_label(label: str) -> str:
    """Casefolded label with mnemonic '&', trailing ellipsis and accelerator stripped (mixelpixx driver.py)."""
    text = (label or "").split("\t", 1)[0].replace("&", "").strip()
    while text.endswith(("...", "…")):
        text = text[:-3] if text.endswith("...") else text[:-1]
        text = text.strip()
    return " ".join(text.split()).casefold()


def plain_label(label: str) -> str:
    """The label as KiCad shows it: mnemonic '&' removed, case kept."""
    return " ".join((label or "").split("\t", 1)[0].replace("&", "").split())


# ------------------------------------------------------------------ wx side (only inside KiCad)

def _wx():
    import wx  # noqa: PLC0415 - only reachable inside a live KiCad
    return wx


def _all(w, acc=None):
    acc = [] if acc is None else acc
    acc.append(w)
    for c in w.GetChildren():
        _all(c, acc)
    return acc


def _top(title: str):
    wx = _wx()
    want = normalize_label(title)
    for w in wx.GetTopLevelWindows():
        t = w.GetTitle() if hasattr(w, "GetTitle") else ""
        if w.IsShown() and want in normalize_label(t):
            return w
    raise LookupError(f"no shown window titled ~{title!r}")


def _frame():
    wx = _wx()
    for w in wx.GetTopLevelWindows():
        if w.GetName() == "PcbFrame":
            return w
    raise LookupError("no PcbFrame")


def _dvcs(win):
    import wx.dataview as dv  # noqa: PLC0415
    return [w for w in _all(win) if isinstance(w, dv.DataViewCtrl)]


def _rows(dvc, limit: int = 100000) -> list:
    import wx.dataview as dv  # noqa: PLC0415
    m = dvc.GetModel()
    kids = dv.DataViewItemArray()
    m.GetChildren(dv.NullDataViewItem, kids)
    out = []
    for k, it in enumerate(list(kids)[:limit]):
        sub = dv.DataViewItemArray()
        m.GetChildren(it, sub)
        out.append({"k": k, "text": str(m.GetValue(it, 0)), "items": [str(m.GetValue(s, 0)) for s in sub]})
    return out


def cmd_windows() -> list:
    wx = _wx()
    return [{"title": w.GetTitle() if hasattr(w, "GetTitle") else "", "class": type(w).__name__,
             "name": w.GetName(), "shown": bool(w.IsShown()), "rect": list(w.GetScreenRect())}
            for w in wx.GetTopLevelWindows()]


def _leaves(menu, top: str, acc: list) -> list:
    for it in menu.GetMenuItems():
        sub = it.GetSubMenu()
        if sub is not None:
            _leaves(sub, top, acc)
        elif not it.IsSeparator():
            acc.append((top, it))
    return acc


def cmd_menu(path: list) -> dict:
    """Post the menu item named by the last element of `path` (labels, e.g. ["Tools", "Cleanup Tracks & Vias"]) of
    the PCB editor. KiCad's GTK menu bar reports empty top-level labels, so the leaf is found in every menu and the
    top label (GetMenuLabelText / the menu title) only disambiguates. Exactly one match, enabled, or an error."""
    wx = _wx()
    f = _frame()
    mb = f.GetMenuBar()
    leaves = []
    for i in range(mb.GetMenuCount()):
        m = mb.GetMenu(i)
        top = mb.GetMenuLabelText(i) or m.GetTitle() or ""
        _leaves(m, top, leaves)
    want = normalize_label(path[-1])
    hits = [(t, it) for t, it in leaves if normalize_label(it.GetItemLabelText()) == want]
    if len(hits) > 1 and len(path) > 1:
        hits = [(t, it) for t, it in hits if normalize_label(t) == normalize_label(path[0])] or hits
    if len(hits) != 1:
        raise LookupError(f"menu item {path!r}: {len(hits)} matches among {len(leaves)} items")
    item = hits[0][1]
    if not item.IsEnabled():
        raise LookupError(f"menu item {path} is disabled")
    # KiCad's ACTION_MENU handles the event on the menu itself (OnMenuEvent), not on the frame
    ev = wx.CommandEvent(wx.wxEVT_MENU, item.GetId())
    ev.SetEventObject(item.GetMenu())
    wx.PostEvent(item.GetMenu(), ev)
    return {"posted": plain_label(item.GetItemLabelText()), "menu": hits[0][0]}


def cmd_place(title: str, x: int, y: int, w: int, h: int) -> dict:
    win = _top(title)
    win.SetSize(int(x), int(y), int(w), int(h))
    win.Raise()
    return {"rect": list(win.GetScreenRect())}


def cmd_buttons(title: str) -> list:
    wx = _wx()
    return [{"label": plain_label(b.GetLabel()), "id": b.GetId(), "enabled": bool(b.IsEnabled())}
            for b in _all(_top(title)) if isinstance(b, wx.Button)]


def cmd_button(title: str, label: str) -> dict:
    wx = _wx()
    want = normalize_label(label)
    hits = [b for b in _all(_top(title)) if isinstance(b, wx.Button) and normalize_label(b.GetLabel()) == want]
    if not hits:
        raise LookupError(f"no button {label!r} in {title!r}")
    b = hits[0]
    if not b.IsEnabled():
        raise LookupError(f"button {label!r} in {title!r} is disabled")
    ev = wx.CommandEvent(wx.wxEVT_COMMAND_BUTTON_CLICKED, b.GetId())
    ev.SetEventObject(b)
    wx.PostEvent(b.GetEventHandler(), ev)
    return {"clicked": plain_label(b.GetLabel())}


def cmd_checkboxes(title: str) -> list:
    wx = _wx()
    return [{"label": plain_label(c.GetLabel()), "value": bool(c.GetValue()), "enabled": bool(c.IsEnabled())}
            for c in _all(_top(title)) if isinstance(c, wx.CheckBox)]


def cmd_set_checkboxes(title: str, values: dict) -> list:
    """Set the boxes named in `values` ({label as shown: bool}); an unknown label is an error (nothing set)."""
    wx = _wx()
    boxes = {normalize_label(c.GetLabel()): c for c in _all(_top(title)) if isinstance(c, wx.CheckBox)}
    want = {normalize_label(k): bool(v) for k, v in values.items()}
    unknown = sorted(k for k, n in zip(values, want) if n not in boxes)
    if unknown:
        raise LookupError(f"no check box {unknown} in {title!r}; there are {[plain_label(c.GetLabel()) for c in boxes.values()]}")
    for n, v in want.items():
        c = boxes[n]
        if c.GetValue() != v:
            c.SetValue(v)
            ev = wx.CommandEvent(wx.wxEVT_CHECKBOX, c.GetId())
            ev.SetEventObject(c)
            ev.SetInt(int(v))
            c.GetEventHandler().ProcessEvent(ev)
    return cmd_checkboxes(title)


def cmd_list(title: str, index: int = 0, limit: int = 100000) -> list:
    """Top-level rows (text + child rows) of the `index`-th data view of the window."""
    d = _dvcs(_top(title))
    if index >= len(d):
        raise LookupError(f"{title!r} has {len(d)} data views, not {index + 1}")
    return _rows(d[index], limit)


def cmd_drc_state() -> dict:
    """The DRC dialog: result tab labels, whether the result page is shown (not the progress page), Run enabled."""
    wx = _wx()
    d = _top(DRC_TITLE)
    ws = _all(d)
    nbs = [w for w in ws if isinstance(w, wx.Notebook)]
    res = [n for n in nbs if n.GetPageCount() >= 2]
    tabs = [res[0].GetPageText(i) for i in range(res[0].GetPageCount())] if res else []
    run = [b for b in ws if isinstance(b, wx.Button) and normalize_label(b.GetLabel()) == "run drc"]
    book = [w for w in ws if type(w).__name__ == "BookCtrlBase" or (isinstance(w, wx.BookCtrlBase) and w not in nbs)]
    page = book[0].GetSelection() if book else -1
    boxes = {plain_label(c.GetLabel()): bool(c.GetValue()) for c in ws if isinstance(c, wx.CheckBox) and c.IsShown()}
    status = [sb.GetStatusText(i) for sb in ws if isinstance(sb, wx.StatusBar) for i in range(sb.GetFieldsCount())]
    return {"tabs": tabs, "results_page": page, "run_enabled": bool(run and run[0].IsEnabled()), "boxes": boxes,
            "status": " ".join(t for t in status if t)}


def _drc_dvc(tab: str):
    d = _top(DRC_TITLE)
    v = _dvcs(d)
    if tab not in DRC_TABS or DRC_TABS[tab] >= len(v):
        raise LookupError(f"DRC tab {tab!r}: {len(v)} data views")
    return d, v[DRC_TABS[tab]]


def cmd_drc_rows(tab: str = "violations") -> list:
    return _rows(_drc_dvc(tab)[1])


def cmd_drc_list_rect(tab: str = "violations") -> dict:
    """Screen rect of the tab's list (the control, never a row: on KiCad 10.0.8 / wxGTK, Select / EnsureVisible /
    GetItemRect on some rows crashed pcbnew, reproduced 3x on a solder-mask-bridge row, while a real click there
    did not). Rows are reached by a click on the first line and Down keys, each step read back (drc_selected)."""
    wx = _wx()
    d, v = _drc_dvc(tab)
    nb = [w for w in _all(d) if isinstance(w, wx.Notebook) and w.GetPageCount() >= 2][0]
    if nb.GetSelection() != DRC_TABS[tab]:
        nb.SetSelection(DRC_TABS[tab])
    return {"rect": list(v.GetScreenRect())}


def cmd_drc_selected(tab: str = "violations") -> dict:
    _, v = _drc_dvc(tab)
    sel = v.GetSelection()
    return {"selected": str(v.GetModel().GetValue(sel, 0)) if sel.IsOk() else None}


def cmd_dialog_status(title: str) -> list:
    wx = _wx()
    return [sb.GetStatusText(i) for sb in _all(_top(title)) if isinstance(sb, wx.StatusBar)
            for i in range(sb.GetFieldsCount())]


def cmd_status() -> list:
    sb = _frame().GetStatusBar()
    return [sb.GetStatusText(i) for i in range(sb.GetFieldsCount())] if sb else []


def cmd_canvas_rect() -> list:
    """Screen rect of the board canvas (the GL canvas of the PCB editor)."""
    ws = [w for w in _all(_frame()) if w.GetName() == "GLCanvas" and w.IsShown()]
    if not ws:
        raise LookupError("no GLCanvas")
    return list(max(ws, key=lambda w: w.GetSize()[0] * w.GetSize()[1]).GetScreenRect())


def _choice(pred):
    wx = _wx()
    for c in _all(_frame()):
        if isinstance(c, wx.Choice) and c.GetCount() and pred([c.GetString(i) for i in range(c.GetCount())]):
            return c
    raise LookupError("no such tool bar choice")


def _pick(c, i):
    wx = _wx()
    c.SetSelection(i)
    ev = wx.CommandEvent(wx.wxEVT_CHOICE, c.GetId())
    ev.SetEventObject(c)
    ev.SetInt(i)
    c.GetEventHandler().ProcessEvent(ev)
    return c.GetString(i)


def cmd_zoom(value: float) -> dict:
    """Pick the zoom preset nearest `value` in the tool bar's zoom choice ("Zoom 13,00")."""
    c = _choice(lambda xs: any(x.startswith("Zoom ") for x in xs))
    best = None
    for i in range(c.GetCount()):
        s = c.GetString(i)[5:].replace(",", ".")
        try:
            z = float(s)
        except ValueError:
            continue
        if best is None or abs(z - value) < abs(best[1] - value):
            best = (i, z)
    if best is None:
        raise LookupError("no numeric zoom preset")
    return {"zoom": _pick(c, best[0])}


def cmd_finest_grid() -> dict:
    """Pick the smallest grid of the tool bar's grid choice (the status bar's cursor position snaps to it)."""
    import re  # noqa: PLC0415
    c = _choice(lambda xs: any(re.match(r"^[\d.,]+ mm", x) for x in xs))
    best = None
    for i in range(c.GetCount()):
        m = re.match(r"^([\d.,]+) mm", c.GetString(i))
        if m:
            g = float(m.group(1).replace(",", "."))
            if best is None or g < best[1]:
                best = (i, g)
    return {"grid": _pick(c, best[0]), "grid_mm": best[1]}


COMMANDS = {"windows": cmd_windows, "menu": cmd_menu, "place": cmd_place, "buttons": cmd_buttons,
            "button": cmd_button, "checkboxes": cmd_checkboxes, "set_checkboxes": cmd_set_checkboxes,
            "list": cmd_list, "drc_state": cmd_drc_state, "drc_rows": cmd_drc_rows, "drc_list_rect": cmd_drc_list_rect,
            "drc_selected": cmd_drc_selected, "dialog_status": cmd_dialog_status, "status": cmd_status,
            "canvas_rect": cmd_canvas_rect, "zoom": cmd_zoom, "finest_grid": cmd_finest_grid,
            "ping": lambda: {"pid": os.getpid()}}


# ------------------------------------------------------------------ dispatch (pure; unit-tested with a stub executor)

def wx_executor(fn, timeout: float = UI_CALL_TIMEOUT):
    """Run `fn` on the wx UI thread, block this (listener) thread for the result (mixelpixx listener.py)."""
    wx = _wx()
    done = threading.Event()
    box: dict = {}

    def on_ui():
        try:
            box["result"] = fn()
        except Exception as exc:  # noqa: BLE001 - marshalled back as the error
            box["error"] = exc
        finally:
            done.set()

    wx.CallAfter(on_ui)
    if not done.wait(timeout):
        raise TimeoutError(f"UI thread did not answer within {timeout} s (KiCad busy)")
    if "error" in box:
        raise box["error"]
    return box.get("result")


def dispatch(request: dict, token: str, executor=wx_executor, commands=None) -> dict:
    """One request -> one response dict. Token compared in constant time; unknown command / bad args = error."""
    commands = COMMANDS if commands is None else commands
    if not isinstance(request, dict) or not hmac.compare_digest(str(request.get("token", "")), token):
        return {"ok": False, "error": "bad token"}
    cmd = request.get("cmd")
    fn = commands.get(cmd)
    if fn is None:
        return {"ok": False, "error": f"unknown command {cmd!r}; known: {sorted(commands)}"}
    args = {k: v for k, v in request.items() if k not in ("token", "cmd")}
    try:
        return {"ok": True, "result": executor(lambda: fn(**args))}
    except TypeError as ex:
        return {"ok": False, "error": f"{cmd}: bad arguments {sorted(args)}: {ex}"}
    except Exception as ex:  # noqa: BLE001
        return {"ok": False, "error": f"{cmd}: {type(ex).__name__}: {ex}"}


class _Handler(socketserver.StreamRequestHandler):
    def handle(self):
        while True:
            line = self.rfile.readline(MAX_REQUEST_BYTES)
            if not line:
                return
            try:
                req = json.loads(line)
            except ValueError:
                resp = {"ok": False, "error": "not JSON"}
            else:
                resp = dispatch(req, self.server.token)
            self.wfile.write((json.dumps(resp, default=str) + "\n").encode())
            self.wfile.flush()


class _Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True


def start() -> object | None:
    """Start the listener when this KiCad was started by kicad_gui (env set), else do nothing."""
    sock = os.environ.get("TOPOR_GUI_HELPER_SOCK")
    tok = os.environ.get("TOPOR_GUI_HELPER_TOKEN_FILE")
    if not sock or not tok:
        return None
    try:
        if os.path.exists(sock):
            os.unlink(sock)
        srv = _Server(sock, _Handler)
        os.chmod(sock, 0o600)
        with open(tok, encoding="utf-8") as f:
            srv.token = f.read().strip()
        threading.Thread(target=srv.serve_forever, name="topor-gui-helper", daemon=True).start()
        return srv
    except Exception:  # noqa: BLE001 - never break KiCad's start; kicad_gui reports "helper not answering"
        with open(sock + ".err", "w", encoding="utf-8") as f:
            f.write(traceback.format_exc())
        return None


_SERVER = start() if __name__ != "__main__" else None
