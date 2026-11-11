"""KiCad's Edit -> Cleanup Tracks & Vias, every option on, replayed on a routed board with the `pcbnew` module.

    .venv/bin/python -m harness.cleanup_check <routed.kicad_pcb> [--refill] [--json OUT] [--only-new INPUT_PCB]

topor's copper must survive KiCad's cleanup unchanged (docs/user/operate.md, "topor's copper survives Cleanup Tracks
& Vias"). The cleaner itself (TRACKS_CLEANER) is not in KiCad's Python API, so this module replays it step by step
on an in-memory copy of the board, with KiCad's own connectivity and geometry calls doing every judgement. Each item
the cleaner would remove or change is one **cleanup-unstable item**; 0 is the target. The steps, in the order of
pcbnew/tracks_cleaner.cpp:TRACKS_CLEANER::CleanupBoard @ 86a7a3f (KiCad 10.0):

  1. ``cleanup()``: redundant vias (two vias at one position with one type and layer set, or a via connected to a
     pad on every copper layer), null segments (start == end), duplicate segments (same layer and width, both ends
     on the other's ends), then ``mergeCollinearSegments`` until nothing merges: two same-width straight segments
     that are directly connected, ``ApproxCollinear`` (SEG::ApproxCollinear, 1 nm) and whose four ends keep their
     connections (track / via ends compared exactly, ``IsPointOnEnds``; pads / zones by ``HitTest`` within
     width/2) are replaced by one; a segment that touches a track of another width is never merged
     (``TRACKS_CLEANER::cleanup`` merge lambda, the "necking-down" rule).
  2. ``removeShortingTrackSegments``: a track or via directly connected to a pad or track of another net.
  3. ``deleteTracksInPads``: a segment with both ends inside one connected pad and no copper outside it.
  4. ``deleteDanglingTracks`` (tracks and vias), repeated until nothing dangles:
     ``CONNECTIVITY_DATA::TestTrackEndpointDangling`` (pcbnew/connectivity/connectivity_data.cpp): a track end is
     connected when it lies within width/2 of a directly connected item's shape or of a zone's fill on its layer;
     an item that reaches both ends counts once (for the nearer end); a via dangles when everything connected to
     it is on one layer.
  5. a merge pass again when step 4 removed something.

Changes are applied to the in-memory board as KiCad applies them, so cascades (a removed via makes its tracks
dangle) are counted like KiCad counts them. The file is never written.

`--refill` fills the zones first (the dialog's "Refill zones before and after cleanup"). Without it the zones are
judged with the fill stored in the file, which is what the dialog does by default: topor's plugin must hand back a
board whose fills hold its own copper (``run.refill_after_accept``).

`--only-new INPUT`: only items whose copper is not in INPUT (by uuid) count as topor's; the rest are listed as
pre-existing.

Complexity: O(T log T) per cleanup round for T tracks; KiCad's BuildConnectivity once per dangling round and per
merge round.
"""
from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

#: rule ids of the cleanup-unstable items, with the KiCad function that removes or changes them (@ 86a7a3f)
RULES = {
    "redundant_via": "tracks_cleaner.cpp:TRACKS_CLEANER::cleanup (duplicate via / via on a through pad)",
    "null_segment": "tracks_cleaner.cpp:TRACKS_CLEANER::cleanup (IsNull)",
    "duplicate_segment": "tracks_cleaner.cpp:TRACKS_CLEANER::cleanup (duplicate segment)",
    "merge_collinear": "tracks_cleaner.cpp:TRACKS_CLEANER::mergeCollinearSegments",
    "shorting": "tracks_cleaner.cpp:TRACKS_CLEANER::removeShortingTrackSegments",
    "track_in_pad": "tracks_cleaner.cpp:TRACKS_CLEANER::deleteTracksInPads",
    "dangling_track": "connectivity_data.cpp:CONNECTIVITY_DATA::TestTrackEndpointDangling via deleteDanglingTracks",
    "dangling_via": "connectivity_data.cpp:CONNECTIVITY_DATA::TestTrackEndpointDangling via deleteDanglingTracks",
}


def _mm(v) -> float:
    return round(v / 1e6, 4)


class _Cleaner:
    """One replay of TRACKS_CLEANER::CleanupBoard on `board` (all options on). O(see module)."""

    def __init__(self, pcbnew, board):
        self.p = pcbnew
        self.b = board
        self.items: list = []
        self.deleted: set = set()  # m_Uuid strings of removed items
        self._c = None
        self._graveyard: list = []

    # ------------------------------------------------------------ helpers
    def tracks(self):
        return [t for t in self.b.GetTracks() if self.key(t) not in self.deleted]

    @staticmethod
    def key(it) -> str:
        return it.m_Uuid.AsString()

    def is_seg(self, t) -> bool:
        return t.Type() == self.p.PCB_TRACE_T

    def is_via(self, t) -> bool:
        return t.Type() == self.p.PCB_VIA_T

    def record(self, rule: str, t, detail: str = "", other=None) -> None:
        p = self.p
        d = {"rule": rule, "uuid": self.key(t), "net": t.GetNetname(),
             "layer": self.b.GetLayerName(t.GetLayer()) if not self.is_via(t) else "via",
             "kind": "via" if self.is_via(t) else ("arc" if t.Type() == p.PCB_ARC_T else "segment")}
        if self.is_via(t):
            pos = t.GetPosition()
            d["at"] = [_mm(pos.x), _mm(pos.y)]
        else:
            s, e = t.GetStart(), t.GetEnd()
            d["at"] = [_mm(s.x), _mm(s.y), _mm(e.x), _mm(e.y)]
            d["width"] = _mm(t.GetWidth())
            d["length"] = _mm(t.GetLength())
        if other is not None:
            d["other"] = self.key(other)
        if detail:
            d["detail"] = detail
        self.items.append(d)

    def remove(self, t) -> None:
        # RemoveNative, not Remove: pcbnew's Remove hands the C++ item to the Python proxy, which then deletes it
        # while other proxies and the connectivity still point at it (heap corruption seen as broken SWIG types).
        # The removed items are kept alive until the board goes.
        self.deleted.add(self.key(t))
        self.b.RemoveNative(t)
        self._graveyard.append(t)

    def conn(self):
        # BOARD::BuildConnectivity rebuilds the same CONNECTIVITY_DATA, so one proxy is kept for the whole replay:
        # dropping a CONNECTIVITY_DATA proxy breaks the SWIG type of every later GetConnectivity() (KiCad 10.0.8)
        self.b.BuildConnectivity()
        if self._c is None:
            self._c = self.b.GetConnectivity()
        return self._c

    # ------------------------------------------------------------ 1. cleanup()
    def redundant_null_duplicate(self, vias: bool, null: bool, dup: bool) -> None:
        p = self.p
        c = self.conn()
        all_cu = list(p.LSET.AllCuMask(self.b.GetCopperLayerCount()).Seq())
        rm, seen_vias, seen_segs = [], {}, {}
        for t in self.tracks():
            if t.IsLocked():
                continue
            if self.is_via(t):
                if not vias:
                    continue
                k = (t.GetPosition().x, t.GetPosition().y, int(t.GetViaType()), t.GetLayerSet().FmtHex())
                if k in seen_vias:
                    self.record("redundant_via", t, "a second via at the same position", seen_vias[k])
                    rm.append(t)
                    continue
                seen_vias[k] = t
                for pad in c.GetConnectedPads(t):
                    if all(pad.GetLayerSet().Contains(l) for l in all_cu):
                        self.record("redundant_via", t, f"via on through pad {pad.GetParentFootprint().GetReference()}"
                                    f"-{pad.GetNumber()}", pad)
                        rm.append(t)
                        break
                continue
            if null and t.IsNull():
                self.record("null_segment", t, "start == end")
                rm.append(t)
                continue
            if dup and self.is_seg(t) and not t.IsNull():
                s, e = t.GetStart(), t.GetEnd()
                a, b2 = sorted([(s.x, s.y), (e.x, e.y)])
                k = (a, b2, t.GetWidth(), int(t.GetLayer()))
                if k in seen_segs:
                    self.record("duplicate_segment", t, "same ends, width and layer as another segment", seen_segs[k])
                    rm.append(t)
                    continue
                seen_segs[k] = t
        for t in rm:
            self.remove(t)

    def _hit_flags(self, seg, cluster, other) -> set:
        """Ends of `seg` touched by something else of its cluster (testMergeCollinearSegments' collectPts)."""
        p = self.p
        s, e = seg.GetStart(), seg.GetEnd()
        acc = (seg.GetWidth() + 1) // 2
        hit = set()
        for it in cluster:
            if it is None or self.key(it) in self.deleted or self.key(it) in (self.key(seg), self.key(other)):
                continue
            if it.Type() in (p.PCB_TRACE_T, p.PCB_ARC_T, p.PCB_VIA_T):
                if it.IsPointOnEnds(s):
                    hit.add("s")
                if it.IsPointOnEnds(e):
                    hit.add("e")
            else:
                if "s" not in hit and it.HitTest(s, acc):
                    hit.add("s")
                if "e" not in hit and it.HitTest(e, acc):
                    hit.add("e")
        return hit

    def _try_merge(self, c, seg1, seg2) -> tuple | None:
        """TRACKS_CLEANER::testMergeCollinearSegments: the merged (start, end) or None. O(cluster)."""
        if seg1.IsLocked() or seg2.IsLocked():
            return None
        V = self.p.VECTOR2I
        cl1 = [it for it in c.GetConnectedItems(seg1)]
        cl2 = [it for it in c.GetConnectedItems(seg2)]
        f1 = self._hit_flags(seg1, cl1, seg2)
        f2 = self._hit_flags(seg2, cl2, seg1)
        pts = {"1s": seg1.GetStart(), "1e": seg1.GetEnd(), "2s": seg2.GetStart(), "2e": seg2.GetEnd()}
        flags = {f"1{x}" for x in f1} | {f"2{x}" for x in f2}
        if len(flags) > 2:
            return None
        xs = [q.x for q in pts.values()]
        ys = [q.y for q in pts.values()]
        s1, e1 = seg1.GetStart(), seg1.GetEnd()
        if (s1.x > e1.x) == (s1.y > e1.y):
            ns, ne = V(min(xs), min(ys)), V(max(xs), max(ys))
        else:
            ns, ne = V(min(xs), max(ys)), V(max(xs), min(ys))
        for f in flags:
            q = pts[f]
            if not ((q.x == ns.x and q.y == ns.y) or (q.x == ne.x and q.y == ne.y)):
                return None
        return ns, ne

    def merge_pass(self) -> int:
        """One round of the merge lambda; returns the number of merges applied. O(T * cluster)."""
        c = self.conn()
        c.RecalculateRatsnest()
        pairs, used = [], set()
        for seg in self.tracks():
            if not self.is_seg(seg):
                continue
            same, diff = [], False
            for o in c.GetConnectedTracks(seg):
                if o.Type() != self.p.PCB_TRACE_T or self.key(o) == self.key(seg) or self.key(o) in self.deleted:
                    continue
                if o.GetWidth() == seg.GetWidth():
                    same.append(o)
                else:
                    diff = True
                    break
            if diff:
                continue
            for o in same:
                pk = tuple(sorted((self.key(seg), self.key(o))))
                if pk in used:
                    continue
                if seg.ApproxCollinear(o) and self._try_merge(c, seg, o) is not None:
                    used.add(pk)
                    pairs.append((seg, o))
                    break
        n = 0
        for seg1, seg2 in pairs:
            if self.key(seg1) in self.deleted or self.key(seg2) in self.deleted:
                continue
            c = self.conn()
            m = self._try_merge(c, seg1, seg2)
            if m is None:
                continue
            os_, oe = seg1.GetStart(), seg1.GetEnd()
            gap = min(math.dist((a.x, a.y), (b.x, b.y)) for a in (os_, oe) for b in (seg2.GetStart(), seg2.GetEnd()))
            self.record("merge_collinear", seg2, f"merged into {self.key(seg1)}; nearest ends {gap:.0f} nm apart"
                        + ("" if gap == 0 else " (not bit-identical)"), seg1)
            seg1.SetStart(m[0])
            seg1.SetEnd(m[1])
            self.remove(seg2)
            n += 1
        return n

    def merge_all(self) -> None:
        while self.merge_pass():
            pass

    # ------------------------------------------------------------ 2. shorting
    def shorting(self) -> None:
        c = self.conn()
        rm = {}
        for t in self.tracks():
            if t.IsLocked():
                continue
            for o in list(c.GetConnectedPads(t)) + list(c.GetConnectedTracks(t)):
                if o.GetNetCode() != t.GetNetCode():
                    what = (f"pad {o.GetParentFootprint().GetReference()}-{o.GetNumber()}" if o.Type() == self.p.PCB_PAD_T
                            else "track")
                    if self.key(t) not in rm:
                        self.record("shorting", t, f"touches {what} of net {o.GetNetname()}", o)
                    rm[self.key(t)] = t
        for t in rm.values():
            self.remove(t)

    # ------------------------------------------------------------ 3. tracks in pads
    def tracks_in_pads(self) -> None:
        p = self.p
        c = self.conn()
        rm = {}
        for t in self.tracks():
            if t.IsLocked() or self.is_via(t):
                continue
            for pad in c.GetConnectedPads(t):
                if pad.HitTest(t.GetStart()) and pad.HitTest(t.GetEnd()):
                    poly = p.SHAPE_POLY_SET()
                    t.TransformShapeToPolygon(poly, t.GetLayer(), 0, t.GetMaxError(), p.ERROR_INSIDE)
                    poly.BooleanSubtract(pad.GetEffectivePolygon(t.GetLayer(), p.ERROR_INSIDE))
                    if poly.IsEmpty():
                        if self.key(t) not in rm:
                            self.record("track_in_pad", t, f"inside pad {pad.GetParentFootprint().GetReference()}"
                                        f"-{pad.GetNumber()}", pad)
                        rm[self.key(t)] = t
        for t in rm.values():
            self.remove(t)

    # ------------------------------------------------------------ 4. dangling
    def dangling(self, tracks: bool, vias: bool) -> bool:
        removed = False
        rnd = 0
        while True:
            c = self.conn()
            rnd += 1
            d = [t for t in self.tracks() if not t.IsLocked() and (vias if self.is_via(t) else tracks)
                 and c.TestTrackEndpointDangling(t, False)]
            if not d:
                return removed
            for t in d:
                rule = "dangling_via" if self.is_via(t) else "dangling_track"
                self.record(rule, t, f"cascade round {rnd}" + ("" if rnd == 1 else " (after an earlier removal)"))
                self.remove(t)
            removed = True

    def run(self, opts: dict) -> None:
        """TRACKS_CLEANER::CleanupBoard with the dialog's options `opts` (keys of :data:`OPTIONS`)."""
        merge = opts["merge"]
        self.redundant_null_duplicate(vias=opts["vias"], null=merge or opts["short"], dup=merge)
        if merge:
            self.merge_all()
        else:  # "If we didn't remove duplicates above, do it now"
            self.redundant_null_duplicate(vias=False, null=False, dup=True)
        if opts["short"]:
            self.shorting()
        if opts["in_pads"]:
            self.tracks_in_pads()
        if (opts["unconnected"] or opts["dangling_vias"]) and \
                self.dangling(tracks=opts["unconnected"], vias=opts["dangling_vias"]) and merge:
            self.merge_all()


#: the dialog's action check boxes (pcbnew/dialogs/dialog_cleanup_tracks_and_vias_base.cpp) -> CleanupBoard flags
OPTIONS = {
    "short": "Delete tracks connecting different nets",
    "vias": "Delete redundant vias",
    "dangling_vias": "Delete vias connected on only one layer",
    "merge": "Merge co-linear tracks",
    "unconnected": "Delete tracks unconnected at one end",
    "in_pads": "Delete tracks fully inside pads",
}
ALL_ON = {k: True for k in OPTIONS}


def cleanup_items(pcb: Path, *, refill: bool = False, opts: dict | None = None) -> list:
    """The cleanup-unstable items of `pcb` (list of dicts: rule, uuid, net, layer, kind, at, ...), in KiCad's
    order, with the dialog options `opts` (default: all on). `refill`: fill the zones first. O(see module)."""
    import pcbnew
    b = pcbnew.LoadBoard(str(pcb))
    if refill:
        pcbnew.ZONE_FILLER(b).Fill(b.Zones())
    cl = _Cleaner(pcbnew, b)
    cl.run(opts or ALL_ON)
    return cl.items


def _ours(items: list, pre: set) -> list:
    return [i for i in items if i["uuid"] not in pre]


def _by_rule(items: list) -> dict:
    out: dict = {}
    for i in items:
        out[i["rule"]] = out.get(i["rule"], 0) + 1
    return out


def check(pcb: Path, *, refill: bool = False, input_pcb: Path | None = None, per_option: bool = True) -> dict:
    """Cleanup-unstable items of `pcb`: ``count`` / ``by_rule`` / ``items`` of the replay with every option on
    (topor's copper only: items whose uuid is in `input_pcb` go to ``preexisting``), and with `per_option` one
    replay per dialog option alone (``per_option``: option -> {count, by_rule}), which separates the root causes
    from the cascades of the full run. O(7 replays)."""
    pre: set = set()
    if input_pcb is not None:
        from toporlib import pcbfile
        pre = {s.uuid for s in pcbfile.find_items(Path(input_pcb).read_text(), ("segment", "arc", "via"))}
    items = cleanup_items(Path(pcb), refill=refill)
    ours = _ours(items, pre)
    r = {"count": len(ours), "by_rule": _by_rule(ours), "refill": refill, "items": ours,
         "preexisting": [i for i in items if i["uuid"] in pre]}
    if per_option:
        r["per_option"] = {}
        alone_dangling: set = set()
        for k in OPTIONS:
            one = _ours(cleanup_items(Path(pcb), refill=refill, opts={o: o == k for o in OPTIONS}), pre)
            r["per_option"][k] = {"count": len(one), "by_rule": _by_rule(one)}
            if k in ("unconnected", "dangling_vias"):
                alone_dangling |= {i["uuid"] for i in one if i["rule"] in DANGLING}
        r["dangling_split"] = dangling_split(ours, alone_dangling)
    return r


#: the rules of deleteDanglingTracks
DANGLING = ("dangling_track", "dangling_via")


def dangling_split(items: list, alone: set) -> dict:
    """Root cause of every dangling removal of the full replay `items`: **independent** when the track or via
    also dangles with "Delete tracks unconnected at one end" / "Delete vias connected on only one layer" alone
    (uuids `alone`; KiCad's own end rule, TestTrackEndpointDangling, fails for it on the board as written), else
    a **cascade**: it dangles only after an earlier step removed copper it was joined to (``cascade_after``: those
    earlier removals by rule, e.g. ``shorting`` = "Delete tracks connecting different nets"). O(items)."""
    d = [i for i in items if i["rule"] in DANGLING]
    ind = [i for i in d if i["uuid"] in alone]
    return {"independent": len(ind), "cascade": len(d) - len(ind),
            "independent_items": ind,
            "cascade_after": _by_rule([i for i in items if i["rule"] not in DANGLING])}


def summary(r: dict) -> str:
    rules = ", ".join(f"{k} {v}" for k, v in sorted(r["by_rule"].items())) or "none"
    out = (f"cleanup-unstable items {r['count']} (all options, zones {'refilled' if r['refill'] else 'as stored'}): "
           f"{rules}")
    ds = r.get("dangling_split")
    if ds and (ds["independent"] or ds["cascade"]):
        after = ", ".join(f"{k} {v}" for k, v in sorted(ds["cascade_after"].items())) or "none"
        out += (f"\n  dangling removals: {ds['independent']} independent (KiCad's end rule fails on the board as "
                f"written), {ds['cascade']} cascade (dangle only after the earlier removals: {after})")
    for k, v in (r.get("per_option") or {}).items():
        out += f"\n  alone: {OPTIONS[k]}: {v['count']}" + (f" ({', '.join(f'{a} {b}' for a, b in sorted(v['by_rule'].items()))})"
                                                          if v["by_rule"] else "")
    return out


def main(argv: list | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("pcb", type=Path)
    ap.add_argument("--refill", action="store_true", help="fill the zones first (the dialog's refill option)")
    ap.add_argument("--only-new", type=Path, default=None, metavar="INPUT_PCB",
                    help="count only copper that is not in this input board (by uuid)")
    ap.add_argument("--no-per-option", action="store_true", help="only the replay with every option on")
    ap.add_argument("--json", type=Path, default=None)
    ap.add_argument("--list", type=int, default=30, help="items to print (default 30)")
    a = ap.parse_args(argv)
    r = check(a.pcb, refill=a.refill, input_pcb=a.only_new, per_option=not a.no_per_option)
    print(summary(r))
    for i in r["items"][: a.list]:
        print(f"  {i['rule']:16s} {i['net']:22s} {i['layer']:6s} {i['at']} {i.get('detail', '')}")
    if a.json:
        a.json.write_text(json.dumps(r, indent=1))
    return 0 if r["count"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
