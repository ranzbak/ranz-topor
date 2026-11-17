"""fx_sweep_via.py OUT: variants of the via_fold pattern (R wires, keep-out width, pad rows) as problem boards, for a probe engine
(`FXP erbs_x_any_layer differs` on stderr) to find a layout where the other layer's ERBS of the lane neighbour is nearer the taut
crossing (erbs_x_any_layer reach). Throw-away search, not a pattern."""
import itertools
import sys
from pathlib import Path

from harness.fit_patterns import DEFAULT_CLASS, Fp, Keepout, Pad, Pattern, _strip, write_board

OUT = Path(sys.argv[1])
n = 0
for rys, kx, qy1, qy2 in itertools.product([(6.0,), (5.0,), (7.0,), (4.5, 6.5), (5.5, 6.5, 7.5), (3.0, 5.0, 7.0)], [4.0, 6.0], [3.0, 4.0], [8.0, 9.0]):
    W, H = 14.0, 12.0
    fps = [Fp("J1", [Pad("1", "Q", (2.0, qy1), (0.5, 0.5))], "terminal"),
           Fp("J2", [Pad("1", "Q", (2.0, qy2), (0.5, 0.5), layer="B.Cu")], "terminal")]
    nets = ["Q"]
    conns = [("Q", "J1.1", "J2.1")]
    for i, ry in enumerate(rys):
        nm = f"R{i}"
        nets.append(nm)
        fps.append(Fp(f"A{i}", [Pad("1", nm, (1.0, ry), (0.5, 0.5))], "terminal"))
        fps.append(Fp(f"B{i}", [Pad("1", nm, (13.0, ry), (0.5, 0.5))], "terminal"))
        conns.append((nm, f"A{i}.1", f"B{i}.1"))
    pat = Pattern(name="v", title="v", kind="solvable", gap_classes=[], purpose="", answer="", why="", size=(W, H), layers=2,
                  nets=nets, fps=fps, connections=conns, cuts=[], classes=[dict(DEFAULT_CLASS)],
                  keepouts=[Keepout(["F.Cu", "B.Cu"], [(0.5, 1.5), (kx, 1.5), (kx, 10.5), (0.5, 10.5)], tracks=False, vias=True)])
    name = f"v{n:02d}_r{'-'.join(str(r) for r in rys)}_k{kx}_q{qy1}-{qy2}"
    write_board(pat, OUT / name / "problem" / f"{name}.kicad_pcb", copper=False)
    n += 1
print(n)
