//! draw-via-neck (docs/dev/design/draw-via-neck.md): the drawing step's via shift (A, `[drawing]
//! via_shift_mm`), neck only where the room ends (B, `neck_trim`) and pad entry along the pad's axis
//! (C, `pad_tip_anchor`), on the user-approved patterns (2026-11-24; harness/dvn_patterns.py) and
//! the public qmtech crops of the user's Q2/Q3 spots (harness.abstract_crop). Each mechanism's test
//! also routes with its option off: the pattern then shows the old failure (the mutant the test
//! kills). Fixtures: engine/tests/fixtures/dvn/model-*.json (harness.radii_patterns.model_fixture).
use topor_engine::control::RunControl;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options};

const MM: f64 = 1e6;

struct Seg {
    net: String,
    w: f64,
    a: (f64, f64),
    b: (f64, f64),
}

impl Seg {
    fn len(&self) -> f64 {
        ((self.b.0 - self.a.0).powi(2) + (self.b.1 - self.a.1).powi(2)).sqrt()
    }
}

struct Routed {
    segs: Vec<Seg>,
    vias: Vec<(String, f64, f64)>,
    violations: usize,
    unrouted: usize,
}

fn route(p: &str, extra: &[(&str, serde_json::Value)]) -> Routed {
    let path = format!("{}/tests/fixtures/dvn/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    let name = |n: i32| m.nets.iter().find(|x| x.code == n).map(|x| x.name.clone()).unwrap_or_default();
    let mut segs = Vec::new();
    let mut vias = Vec::new();
    for it in &r.items {
        match it {
            Item::Track(t) => {
                if let TrackPrim::Seg { a, b } = t.prim {
                    segs.push(Seg { net: name(t.net), w: t.width as f64 / MM, a: (a.x as f64 / MM, a.y as f64 / MM), b: (b.x as f64 / MM, b.y as f64 / MM) });
                }
            }
            Item::Via(v) => vias.push((name(v.net), v.at.x as f64 / MM, v.at.y as f64 / MM)),
        }
    }
    Routed { segs, vias, violations: r.connections.iter().map(|c| c.violations).sum(), unrouted: r.connections_total - r.routed }
}

/// Necked copper (narrower than `nominal`) of `net`: its farthest y and its length (mm).
fn necked(r: &Routed, net: &str, nominal: f64, keep: impl Fn(&Seg) -> bool) -> (f64, f64) {
    let thin: Vec<&Seg> = r.segs.iter().filter(|s| s.net == net && s.w < nominal - 1e-4 && keep(s)).collect();
    (thin.iter().map(|s| s.a.1.max(s.b.1)).fold(f64::NEG_INFINITY, f64::max), thin.iter().map(|s| s.len()).sum())
}

fn off(k: &str) -> [(&str, serde_json::Value); 1] {
    [(k, serde_json::json!(false))]
}

/// B: PWR (0.5 mm) leaves the 0.5 mm pitch row necked; the neck ends just past the row's tip
/// (y 4.4; expected 4.686, the pad gap's clearance), not along the whole 6 mm wire.
#[test]
fn neck_in_pad_row_ends_past_the_row() {
    let r = route("neck_in_pad_row", &[]);
    assert_eq!((r.unrouted, r.violations), (0, 0));
    let (y, _) = necked(&r, "PWR", 0.5, |_| true);
    assert!(y > 4.4 && y < 4.8, "neck ends at y {y}, expected about 4.686");
    let (y_off, _) = necked(&route("neck_in_pad_row", &off("neck_trim")), "PWR", 0.5, |_| true);
    assert!(y_off > 6.0, "neck_trim off: the plan's zone ({y_off})");
}

/// B: S runs beside PWR in a channel below the row and then leaves: the neck reaches down to where
/// S's copper leaves PWR's clearance and no further (the expected layout has S leave at y 6.4, neck
/// to 6.716; the drawn S turns at 6.795, so its 45 degree leg frees PWR at about y 7.0).
#[test]
fn neck_shared_channel_reaches_where_s_leaves() {
    let r = route("neck_shared_channel", &[]);
    assert_eq!((r.unrouted, r.violations), (0, 0));
    let (y, _) = necked(&r, "PWR", 0.5, |_| true);
    assert!(y > 6.4 && y < 7.15, "neck ends at y {y}, expected where S leaves (6.7-7.1)");
}

/// C (+B) on the crop of the user's Q3 spot (ADV7511 row): the ADV_AVDD bar under the row ends
/// on the x 134.72 pad's axis below its tip at the full 0.28 mm; no diagonal into the pad's side,
/// necked by the next pad. (The 0.09 mm stretch at the crop's boundary terminal, x < 133.7, is the
/// crop's board edge.)
#[test]
fn crop_q3_avdd_enters_along_the_pad_axis() {
    let near_row = |s: &Seg| s.a.0.min(s.b.0) > 133.8 && s.a.1.min(s.b.1) < 72.5;
    let r = route("dvn_crop_q3", &[]);
    assert_eq!(r.unrouted, 0);
    let (_, l) = necked(&r, "ADV_AVDD", 0.28, near_row);
    assert!(l < 1e-6, "ADV_AVDD necked {l} mm at the row");
    let (_, l_off) = necked(&route("dvn_crop_q3", &off("pad_tip_anchor")), "ADV_AVDD", 0.28, near_row);
    assert!(l_off > 0.1, "pad_tip_anchor off: the side hook ({l_off} mm necked)");
}

/// B on the crop of the user's Q2 spot (U9 fan-out), as a guard: CTS necks at most through the
/// choke beside the column (about 0.9 mm), never 4 mm to the terminal. The old failure (CTS
/// necked 3.98 mm) shows in the plugin's flow on the crop board (reports/draw-via-neck-r2.md,
/// Q2 PNGs), not in this one-call route of its model: here CTS keeps its width with every option
/// off, so this test has no option-off half (r1's half passed only because the unguarded tip
/// anchor of r1 took CTS's room; B's mutant is killed by the two pattern tests above).
#[test]
fn crop_q2_cts_necks_only_at_the_choke() {
    let r = route("dvn_crop_q2", &[]);
    assert_eq!(r.unrouted, 0);
    let (_, l) = necked(&r, "FPGA_UART1_CTS", 0.25, |_| true);
    assert!(l < 2.0, "CTS necked {l} mm");
}

/// A: the plan's via interval is a sliver pocket beside a pad column; the via goes into the free
/// room below the column next to the expected site (9.0, 7.2), not 1 mm further along the wire
/// (the first traversal with a legal site, today's pick without `via_shift_mm`), and never past
/// X (an offset site labelled with the interval's last traversal but lying beyond X's crossing:
/// X then detoured round V's F.Cu stub, 20.19 mm total; `Drawer::via_side_ok_` tests the
/// neighbouring triangles' chords for a site outside its own triangle).
#[test]
fn via_interval_sliver_shifts_into_the_pocket() {
    let d = |r: &Routed| r.vias.iter().map(|v| ((v.1 - 9.0).powi(2) + (v.2 - 7.2).powi(2)).sqrt()).fold(f64::INFINITY, f64::min);
    let r = route("via_interval_sliver", &[]);
    assert!(d(&r) < 0.5, "via {:?}", r.vias);
    let r0 = route("via_interval_sliver", &[("via_shift_mm", serde_json::json!(0.0))]);
    assert!(d(&r0) > 0.8, "via_shift_mm 0: the interval's first legal site ({:?})", r0.vias);
}

/// Guards (today's answers, kept): the via patterns place one via, nothing necked, no violation,
/// within 5 % of the expected length; the pad patterns leave along the axis, nothing necked.
#[test]
fn guards_keep_their_answers() {
    for (p, vias, len) in [("via_jam_shift", 1, 14.1368), ("via_interval_sliver", 1, 18.2368), ("pad_axis_entry", 0, 6.4842), ("pad_side_approach", 0, 5.6144)] {
        let r = route(p, &[]);
        assert_eq!((r.unrouted, r.violations, r.vias.len()), (0, 0, vias), "{p}");
        let thin = r.segs.iter().filter(|s| s.w < 0.25 - 1e-4).count();
        assert_eq!(thin, 0, "{p}: necked copper");
        let l: f64 = r.segs.iter().map(|s| s.len()).sum();
        assert!(l <= 1.05 * len, "{p}: {l} mm vs {len}");
    }
}
