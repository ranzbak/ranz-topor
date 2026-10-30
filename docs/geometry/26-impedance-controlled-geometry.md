# 26 — Impedance-controlled geometry (width/gap from target impedance)

**Purpose.** Feasibility and knowledge for deriving trace width (and diff-pair gap) per layer
from a target impedance and the board stackup, so the router emits impedance-correct
geometry. Records what Eremex SimPCB Lite offers as a reference, the closed-form models we
would implement ourselves, and how the result plugs into width resolution and diff pairs.

Conventions: see [20](20-clearance-model.md#conventions-used-in-all-geometry-docs).

---

## 1. What SimPCB Lite calculates (reference tool)

Documentation read: Eremex SimPCB Lite "Calculations" pages (see References). The pages
describe workflow and parameters only; **no solver, formula family or accuracy is named
anywhere** (field solver vs. closed form: UNVERIFIED).

**Mode.** "Lossless" is the first-pass mode ("typically performed first"); the lossless
assumption is not defined further on the pages. Other modes exist (frequency domain: general
parameters, crosstalk, S-parameters; time domain; via parameters) but are out of scope here.

**Line types (targets).** Single-ended; coplanar single-ended; differential pair; coplanar
differential pair. Microstrip/stripline/embedded are expressed as *models* (stackup
position, optional coating, optional "trace inverted" for inner-layer models) rather than
separate pages.

| Inputs | Meaning |
|---|---|
| H1–H4 (H for diff) | Dielectric thickness per layer |
| Er1–Er4 (Er for diff) | Dielectric constant per layer |
| W1 / W2 | Trace width / width after undercut (trapezoid etching) |
| T1 | Trace (copper) thickness |
| C1, CEr | Coating (solder mask) thickness and permittivity |
| S1 | Diff-pair spacing |
| D1 | Distance trace to reference/ground (coplanar types only) |

| Outputs | Single-ended | Differential |
|---|---|---|
| Impedance | Zo | Zdiff, Zcomm, Zodd, Zeven |
| Timing | Tpd (ps/m), Vp (m/s) | same |
| Per length | C (pF/m), L (nH/m) | same |
| Effective permittivity | EEr | EEr |

**Calculation kinds.** Single calculation (geometry → Z); **reverse calculation** (target
impedance → one selected parameter, e.g. width; works for any model and all four target
types; algorithm not described, limits not stated); **parametric sweep** (min/max/step of any
parameter that has a range button; table + graph; a swept parameter cannot be the reverse
target); and a reverse parametric sweep (target Z across a swept parameter).

Takeaway: the *interface* (stackup + width/gap/coplanar distance in, Z and Er_eff out, plus
reverse solve) is exactly what we need; the *numerics* we must provide ourselves.

## 2. Closed-form models to implement

| Structure | Model | Notes / accuracy |
|---|---|---|
| Microstrip, width/height | Hammerstad–Jensen (1980): Er_eff(u, Er), Z0(u) with finite-thickness width correction | ~1% for 0.01 ≤ u=W/h ≤ 100, Er ≤ 128 (as published; UNVERIFIED for our use). Wheeler gives a comparable alternative. |
| Microstrip, thickness | Effective width W_eff = W + ΔW(t, W, h) (Wheeler / Hammerstad) | Matters for 1–2 oz copper on thin dielectrics |
| Solder-mask coating | Empirical Er_eff correction (IPC-2141A / Wadell) | Lowers Z by a few Ω; accuracy ~ few % UNVERIFIED |
| Symmetric stripline | Cohn / Wadell closed forms (b = plane spacing) | ~1–2% typical for W/b moderate |
| Asymmetric / embedded | Wadell composite forms; IPC-2141A embedded formula (rough) | Lower accuracy; flag UNVERIFIED |
| Edge-coupled microstrip pair | Kirschning–Jansen (even/odd Er_eff and Z) or Hammerstad–Jensen coupled forms | Zdiff = 2·Zodd; ~2–3% typical |
| Edge-coupled stripline pair | Cohn / Wadell even/odd forms | ~2–3% |
| Coplanar waveguide (± ground) | Conformal-mapping with elliptic-integral ratios K(k)/K(k') (Wadell ch. on CPW) | Needs ground gap D1; accuracy ~ few % |

Practical stance: closed forms are design-stage estimates (typically within a few percent),
below what a fab's 2D field solver or controlled-impedance coupon achieves (±10% is the
usual fabrication tolerance). Therefore **display the estimate and its tolerance, and
cross-check against KiCad's calculator** (§3.6).

**Reverse calculation.** For fixed stackup, Z0 is monotonically decreasing in W (and Zdiff
monotonically increasing in gap, at fixed W). So: bracket [W_min, W_max] (board/fab limits),
bisect or use Brent/secant on f(W) = Z(W) − Z_target. Cost O(log(1/ε)) evaluations of a
closed form (≈ 30–40 for sub-nm width), microseconds each; no tabulation needed. For a diff
pair with two unknowns (W, S) given Zdiff: fix W (from the single-ended or fab-preferred
value) and solve S, or fix S (routing density) and solve W; both are 1-D. Joint solve is
a 2-D problem with a one-parameter family of answers; pick by preference.
Result is quantised to the manufacturing grid (e.g. 0.01 mm) and re-evaluated to report
the achieved Z.

## 3. Integration with the router

1. **Target.** Per netclass or net: impedance (Z0, or Zdiff for pairs) plus optional
   tolerance, a user setting in the plugin. KiCad 10 introduces tuning profiles with
   per-layer width/gap values (UNVERIFIED: exact schema, API exposure, whether they hold
   targets or only results); if readable, import them as the source of truth.
2. **Stackup.** Read from the board stackup (UNVERIFIED which API surface exposes it):
   dielectric thickness, Er, loss tangent (unused here), copper thickness per layer, mask.
   Reference planes are derived by the plugin from which neighbouring layers carry GND/pours.
3. **Per-layer resolution.** Outer layers → microstrip (with mask coating); inner → stripline
   or embedded, using the nearest reference plane(s). Width/gap are therefore a function
   `(net, layer)`. A net changing layer through a via changes width, which the router must
   treat as a width step at the via (the via pad/annular ring already hides the step).
   Neighbour-layer plane gaps (anti-pads, split planes) are not modelled: note as a
   warning.
4. **Width resolution order.** Impedance is inserted in the order of
   [30 §5.1](../kicad/30-kicad-constraints.md#51-missing-or-default-track-widths) between
   the custom rule and the netclass width: an explicit custom rule still wins, an impedance
   target beats a plain netclass width (which is typically a default), and the existing
   routed width is only a fallback.
5. **Diff pairs.** Supplies (w, gap) per layer to [24 §2](24-length-diffpairs-tuning.md#2-differential-pairs-as-offset-curves):
   pair-as-one-wire width becomes `2w + gap` per layer; the stack changes at layer
   transitions. Uncoupled sections (fan-out, around obstacles) have a different Zdiff;
   report their length, and keep them short.
6. **Cross-check.** Compare the computed width to KiCad's PCB calculator (transmission-line
   tab) in testing; the calculator is a GUI tool and not exposed via API (UNVERIFIED), so
   it serves as a manual oracle, not a runtime dependency.

### Benefits of arcs

- No 90°/45° corner discontinuity (corner capacitance step; mitre geometry), so local Z is
  closer to nominal.
- Concentric arcs keep **constant diff gap through bends**, so Zdiff stays constant, unlike
  45° routing where gap varies locally ([24 §2](24-length-diffpairs-tuning.md#2-differential-pairs-as-offset-curves)).
- Inner radius floor `R − s/2 ≥ ~2w` keeps the curved section electrically smooth.

### Coplanar ground and pours

Coplanar models require a defined ground gap D1 on the same layer. This interacts with
pours: the pour clearance (netclass/zone clearance) *becomes part of the impedance*.
Options: (a) ignore coplanar coupling and compute pure microstrip/stripline, warning that
close pour fill lowers Z (conservative: width computed without it will be too narrow/high-Z
when the pour is near, UNVERIFIED magnitude); (b) when the user marks a net coplanar, force
the local clearance to the modelled D1 (custom-rule style) and compute width with the
coplanar model. Pour fill is generated after routing, so (b) must set the keepout/clearance
before fill.

### Priority

**Should** for M5 (diff pairs / RF). Not needed for the first routing milestones; the width
resolution hook (row in 30 §5.1) is cheap and lets power/default behaviour remain unchanged
when no target is set.

## References

- Eremex SimPCB Lite, Calculations overview: https://docs.eremex.com/SimPCBLite/calculations/index.html
- Lossless calculations: https://docs.eremex.com/SimPCBLite/calculations/calculate-transmission-line-parameters/lossless-calculations/index.html
- Single-ended and coplanar lines: https://docs.eremex.com/SimPCBLite/calculations/calculate-transmission-line-parameters/lossless-calculations/calculate-single-ended-and-coplanar-transmission-lines.html
- Differential and coplanar differential lines: https://docs.eremex.com/SimPCBLite/calculations/calculate-transmission-line-parameters/lossless-calculations/calculate-differential-and-coplanar-differential-transmission-lines.html
- Reverse calculation: https://docs.eremex.com/SimPCBLite/calculations/calculate-transmission-line-parameters/lossless-calculations/reverse-calculation.html
- Parametric sweep: https://docs.eremex.com/SimPCBLite/calculations/calculate-transmission-line-parameters/lossless-calculations/parametric-sweep.html
- Hammerstad and Jensen, "Accurate Models for Microstrip Computer-Aided Design", IEEE MTT-S 1980 (from memory; UNVERIFIED citation details).
- Wheeler, "Transmission-line properties of a strip on a dielectric sheet on a plane", IEEE MTT 1977 (UNVERIFIED).
- Kirschning and Jansen, "Accurate wide-range design equations for the frequency-dependent characteristic of parallel coupled microstrip lines", IEEE MTT 1984 (UNVERIFIED).
- IPC-2141A, Design Guide for High-Speed Controlled Impedance Circuit Boards.
- Cohn, stripline impedance formulas (IRE Trans. MTT, 1955); Wadell, "Transmission Line Design Handbook", Artech House, 1991.
