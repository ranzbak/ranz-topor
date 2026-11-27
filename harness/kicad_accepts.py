"""Small KiCad projects that answer "does KiCad's Cleanup accept this copper?" by the real dialog, not by asking
(user 2026-11-26: "create a test KiCad project, and run Cleanup Tracks & Vias with the option to remove dangling
tracks. If the track is gone after the cleanup, it was not good.").

  python -m harness.kicad_accepts <out_dir>        writes <out_dir>/pad_centre.kicad_pcb (+ .kicad_pro)
  python -m harness.kicad_accepts <out_dir> shorting   writes <out_dir>/shorting.kicad_pcb (see SHORTING)

Run the real dialog on it (Xephyr, its own KiCad home, every box on):
  tools/gui_e2e.sh <out_dir>/pad_centre.kicad_pcb <run_dir> --existing --cleanup --display :6
and compare <run_dir>/cleanup/after/pad_centre.kicad_pcb with the input per net (:func:`gone_by_net`). The board is
kept as a regression fixture (tests/fixtures/kicad_accepts/pad_centre.kicad_pcb, its dialog result in
tests/test_kicad_accepts.py, replayed there with harness.cleanup_check, which matched the dialog).

pad_centre (draw-via-neck r7, verifier vfy-draw-via-neck-r6 F1/F2): one net per case, each a pad P of the case (the
pad the question is about) and far pads Q1/Q2 of the same net that the tracks run to (their ends at the Q centres):
  CTRL        a track from P's centre out of the pad (the control).
  TEE017/040/075  a host track from P's centre out through P's tip, and a T track that starts ON the host inside P,
                  0.17 / 0.40 / 0.75 mm off P's centre, and leaves P through its long side (wiccon U1.19 0.75 mm).
  TEE45       the host leaves P's centre at 45 degrees, the T starts on it 0.17 mm off the centre (esp D5.1).
  TERM017/040 the T track alone, no host: an end inside P off its centre (esp D5.1 after Cleanup removed the host).
  JOG         qmtech ADV7511KSTZ1.29: a 0.1407 mm 45-degree jog from the centre of a 0.3 x 1.6 mm pad to 65.011 um
              from its long side (true round end 5.35 um out of the pad), then along the axis out of the tip.
  JOGOUT      the same jog 1 um further out (KiCad's polygon of it sticks out of the pad).
"""
import sys
from pathlib import Path

MM = 1_000_000

#: case -> (P centre, P size (x, y) mm, track width mm, tracks [(a, b)] mm, far pads [centre])
CASES = {
    "CTRL": ((3.0, 5.0), (0.6, 2.0), 0.2, [((3.0, 5.0), (7.0, 5.0))], [(7.0, 5.0)]),
    "TEE017": ((10.0, 5.0), (0.6, 2.0), 0.2, [((10.0, 5.0), (10.0, 1.5)), ((10.0, 4.83), (14.0, 4.83))], [(10.0, 1.5), (14.0, 4.83)]),
    "TEE040": ((17.0, 5.0), (0.6, 2.0), 0.2, [((17.0, 5.0), (17.0, 1.5)), ((17.0, 4.6), (21.0, 4.6))], [(17.0, 1.5), (21.0, 4.6)]),
    "TEE075": ((24.0, 5.0), (0.6, 2.0), 0.2, [((24.0, 5.0), (24.0, 1.5)), ((24.0, 4.25), (28.0, 4.25))], [(24.0, 1.5), (28.0, 4.25)]),
    "TEE45": ((3.0, 12.0), (0.7, 0.6), 0.2, [((3.0, 12.0), (4.5, 10.5)), ((3.120208, 11.879792), (7.0, 11.879792))], [(4.5, 10.5), (7.0, 11.879792)]),
    "TERM017": ((10.0, 12.0), (0.6, 2.0), 0.2, [((10.0, 11.83), (14.0, 11.83))], [(14.0, 11.83)]),
    "TERM040": ((17.0, 12.0), (0.6, 2.0), 0.2, [((17.0, 11.6), (21.0, 11.6))], [(21.0, 11.6)]),
    "JOG": ((24.0, 12.0), (0.3, 1.6), 0.1407, [((24.0, 12.0), (23.915011, 12.084989)), ((23.915011, 12.084989), (23.915011, 15.0))], [(23.915011, 15.0)]),
    "JOGOUT": ((28.0, 12.0), (0.3, 1.6), 0.1407, [((28.0, 12.0), (27.914, 12.086)), ((27.914, 12.086), (27.914, 15.0))], [(27.914, 15.0)]),
}


def make_pad_centre(out_dir: Path) -> Path:
    """Write the pad_centre project (see the module doc) into `out_dir`; returns the .kicad_pcb path."""
    import pcbnew

    from harness.mechfix import Fx, _v
    f = Fx(32, 18, nets=tuple(CASES))
    for net, (pc, size, w, tracks, far) in CASES.items():
        f.pad(f"P_{net}", net, pc, size)
        for k, q in enumerate(far):
            f.pad(f"Q{k + 1}_{net}", net, q, (1.0, 1.0))
        for a, b in tracks:
            t = pcbnew.PCB_TRACK(f.b)
            t.SetStart(_v(*a))
            t.SetEnd(_v(*b))
            t.SetWidth(int(round(w * MM)))
            t.SetLayer(pcbnew.F_Cu)
            t.SetNet(f.nets[net])
            f.b.Add(t)
    return f.save(Path(out_dir) / "pad_centre.kicad_pcb")


#: shorting (pad-centre-main, 2026-11-27): what KiCad's Cleanup does with a short topor's never-drop pass writes.
#: Each case is two nets: a forced wire X (from a pad PX centre to a pad QX centre, two segments) and a legal victim
#: wire Y (one segment PY -> QY); case -> (X points, Y points, width mm). TOUCH: X's bend lies on Y (an end inside
#: foreign copper: KiCad's connectivity joins them); CROSS: X crosses Y mid-segment (no end inside foreign copper:
#: a DRC short that KiCad's connectivity does not join); NEAR: X passes Y 0.05 mm apart (a clearance violation only).
SHORTING = {
    "TOUCH": ([(2.0, 3.0), (6.0, 6.0), (10.0, 3.0)], [(2.0, 6.0), (10.0, 6.0)], 0.25),
    "CROSS": ([(14.0, 3.0), (18.0, 9.0), (22.0, 3.0)], [(14.0, 6.0), (22.0, 6.0)], 0.25),
    "NEAR": ([(26.0, 3.0), (30.0, 5.7), (34.0, 3.0)], [(26.0, 6.0), (34.0, 6.0)], 0.25),
}


def make_shorting(out_dir: Path) -> Path:
    """Write the shorting project (see :data:`SHORTING`) into `out_dir`; returns the .kicad_pcb path. Nets X_<case>
    and Y_<case>; every wire end sits on its pad's centre."""
    import pcbnew

    from harness.mechfix import Fx, _v
    nets = tuple(f"{k}_{c}" for c in SHORTING for k in ("X", "Y"))
    f = Fx(36, 12, nets=nets)
    for c, (xs, ys, w) in SHORTING.items():
        for k, pts in (("X", xs), ("Y", ys)):
            net = f"{k}_{c}"
            f.pad(f"P{k}_{c}", net, pts[0], (1.0, 1.0))
            f.pad(f"Q{k}_{c}", net, pts[-1], (1.0, 1.0))
            for a, b in zip(pts, pts[1:]):
                t = pcbnew.PCB_TRACK(f.b)
                t.SetStart(_v(*a))
                t.SetEnd(_v(*b))
                t.SetWidth(int(round(w * MM)))
                t.SetLayer(pcbnew.F_Cu)
                t.SetNet(f.nets[net])
                f.b.Add(t)
    return f.save(Path(out_dir) / "shorting.kicad_pcb")


def tracks_by_net(pcb: Path) -> dict:
    """net -> sorted list of (start, end, width) in nm of the tracks of `pcb` (pcbnew; run in an isolated env)."""
    import pcbnew
    b = pcbnew.LoadBoard(str(pcb))
    out: dict = {}
    for t in b.GetTracks():
        if t.GetClass() != "PCB_TRACK":
            continue
        s, e = t.GetStart(), t.GetEnd()
        out.setdefault(t.GetNetname(), []).append(((s.x, s.y), (e.x, e.y), t.GetWidth()))
    return {k: sorted(v) for k, v in out.items()}


def gone_by_net(before: Path, after: Path) -> dict:
    """net -> the tracks of `before` that `after` does not have (KiCad's Cleanup removed or changed them)."""
    a, b = tracks_by_net(before), tracks_by_net(after)
    return {n: [t for t in ts if t not in b.get(n, [])] for n, ts in a.items()}


if __name__ == "__main__":
    print(make_shorting(Path(sys.argv[1])) if sys.argv[2:] == ["shorting"] else make_pad_centre(Path(sys.argv[1])))
