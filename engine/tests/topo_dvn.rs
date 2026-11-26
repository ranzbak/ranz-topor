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
    let (m, r) = route_items(p, extra);
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

/// The pattern's model and its route (the options of [`route`]).
fn route_items(p: &str, extra: &[(&str, serde_json::Value)]) -> (BoardModel, topor_engine::route::RouteResult) {
    let path = format!("{}/tests/fixtures/dvn/model-{p}.json", env!("CARGO_MANIFEST_DIR"));
    let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut val = serde_json::to_value(Options::default()).unwrap();
    val["runs"] = serde_json::json!(1);
    // via-shift-free (rebased onto main ca2bb502 without the tw-on defaults c7f8bc8f): these
    // patterns and crops were measured with the TW5/TW7 defaults of tw-on (the plan they draw).
    // On main's defaults the plan differs: via_interval_sliver routes V round X without TW7's
    // pattern moves (22.64 mm, the via at (10.98, 10.36)), and the Q3 crop's plan no longer
    // reaches ADV_AVDD's pad through its side with pad_tip_anchor off (0 mm necked; 0.553 mm
    // with every tw-on value, none alone). Plan differences, not the drawing these tests check:
    // pinned as tw-on had them (as c7f8bc8f pinned the old values where tests measure them).
    for (k, v) in [
        ("pattern_moves", serde_json::json!(["clinch", "double", "unlayerable", "u_clinch", "evasion", "pour"])),
        ("oscillation", serde_json::json!("bounded")),
        ("escape", serde_json::json!("layering")),
        ("escape_take", serde_json::json!("always")),
        ("relay_count_fade", serde_json::json!(0.8)),
        ("loop_history", serde_json::json!(true)),
        ("float_wire", serde_json::json!(true)),
        ("preloop_p4", serde_json::json!(false)),
        ("via_pair_min", serde_json::json!(true)),
    ] {
        val[k] = v;
    }
    for (k, v) in extra {
        val[*k] = v.clone();
    }
    let o: Options = serde_json::from_value(val).unwrap();
    let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
    (m, r)
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
    // r6: the shift is opt-in (default 0, user 2026-11-26), so the on arm sets it.
    let r = route("via_interval_sliver", &[("via_shift_mm", serde_json::json!(1.0))]);
    assert!(d(&r) < 0.5, "via {:?}", r.vias);
    let r0 = route("via_interval_sliver", &[]);
    assert!(d(&r0) > 0.8, "via_shift_mm 0 (the default): the interval's first legal site ({:?})", r0.vias);
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

/// B (lead 2026-11-24): neck_in_pad_row with its terminal J1 (0.6 mm pad) 0.15 mm from the board's
/// bottom edge (edge clearance 0.5): PWR's last segment breaks the edge clearance at every width (as
/// the qmtech Q3 crop terminal XT4.1, 0.14 mm from its crop edge), so no width passes the checker.
/// PWR still takes the widest width that adds no violation to that one (the row's 0.3 neck, the pad
/// gap's room), not the floor (0.25: the board's minimum track width), and the edge violation stays
/// the only one.
#[test]
fn neck_edge_terminal_keeps_the_widest_width() {
    let r = route("neck_edge_terminal", &[]);
    assert_eq!(r.unrouted, 0);
    let thin = |r: &Routed| r.segs.iter().filter(|s| s.net == "PWR" && s.w < 0.5 - 1e-4).map(|s| s.w).fold(f64::INFINITY, f64::min);
    assert!(thin(&r) > 0.29, "PWR necked to {} (expected about 0.3)", thin(&r));
    assert_eq!(r.violations, 1, "only the terminal's edge clearance");
    let r0 = route("neck_edge_terminal", &off("neck_trim"));
    assert!(thin(&r0) < 0.26, "neck_trim off: the floor ({})", thin(&r0));
}

/// draw-via-neck r6 (user 2026-11-26: every trace still connects to the MIDDLE of its pad, else
/// KiCad's Cleanup deletes or moves it): on every dvn pattern and crop, with the defaults (the
/// via shift off since r6, neck_trim and pad_tip_anchor on) and with the via shift on, every
/// track end in a pad of its net lies at the pad's centre and no segment crosses such a pad
/// without ending in it (`tidy::pad_end_breaches`), and every routed pad terminal has a track
/// end at its centre. The tip anchor ends the run at the centre and makes the axis point past
/// the tip a bend (padend.rs `tip_anchor_path`: `[centre, tip point, ..]`); the neck trim's
/// width step never lies inside the pad. Listed apart (not breaches): the edge pattern's pads
/// within the edge clearance and pads whose centre breaks a clearance at the track's width
/// (the end moves inside the pad on purpose, edge-clear). The breaches of main's drawing
/// (all three keys off), pinned here so they stay visible: dvn_crop_q2 U9.4 (USB_C_D-), whose
/// drawn path leaves the centre with a 45 um jog inside the pad; the jog lies wholly in the
/// pad, so the cleanup-safe tidy drops it as KiCad's Cleanup would (`deleteTracksInPads`) and
/// the trace ends 45 um off the centre, still inside the pad (in every arm); dvn_crop_q3 C19.1,
/// a width step 0.192 mm off the centre inside the pad (main's neck over the whole zone; the
/// neck trim, on by default, removes it). No arm may add a breach to main's.
#[test]
fn every_track_at_a_pad_ends_at_its_centre() {
    let pats = ["neck_in_pad_row", "neck_shared_channel", "neck_edge_terminal", "dvn_crop_q2", "dvn_crop_q3", "pad_axis_entry", "pad_side_approach", "via_interval_sliver", "via_jam_shift"];
    let f = |b: bool| serde_json::json!(b);
    let arms: [(&str, Vec<(&str, serde_json::Value)>); 3] = [
        ("defaults", vec![]),
        ("shift on", vec![("via_shift_mm", serde_json::json!(1.0))]),
        ("main drawing", vec![("neck_trim", f(false)), ("pad_tip_anchor", f(false))]),
    ];
    let mut found: Vec<(String, String)> = Vec::new();
    for p in pats {
        for (arm, extra) in &arms {
            let (m, r) = route_items(p, extra);
            let bs = topor_engine::topo::tidy::pad_end_breaches(&m, &r.items);
            for b in bs.iter().filter(|b| b.counts()) {
                found.push((arm.to_string(), format!("{p} {:?} {} at ({:.3},{:.3}) off {:.3}", b.kind, b.pad, b.at.0 as f64 / 1e6, b.at.1 as f64 / 1e6, b.off / 1e6)));
            }
            // A pad whose centre cannot take the end (edge-clear, blocked centre) is judged by
            // the breach's flags above, not by the centre test.
            let exempt: Vec<&str> = bs.iter().filter(|b| b.edge || b.centre_blocked).map(|b| b.pad.as_str()).collect();
            for miss in centre_misses(&m, &r).into_iter().filter(|(pad, _)| !exempt.contains(&pad.as_str())) {
                found.push((arm.to_string(), format!("{p} {}", miss.1)));
            }
        }
    }
    let of = |arm: &str| found.iter().filter(|x| x.0 == arm).map(|x| x.1.clone()).collect::<Vec<_>>();
    let main = vec![
        "dvn_crop_q2 Terminal U9.4 at (99.632,58.000) off 0.045".to_string(),
        "dvn_crop_q2 USB_C_D- U9.4: no track end at the pad centre".to_string(),
        "dvn_crop_q3 Junction C19.1 at (134.515,74.629) off 0.192".to_string(),
    ];
    assert_eq!(of("main drawing"), main, "main's drawing: the pinned pre-existing breach only");
    for arm in ["defaults", "shift on"] {
        let new: Vec<String> = of(arm).into_iter().filter(|x| !main.contains(x)).collect();
        assert!(new.is_empty(), "{arm}: track ends off their pad centre that main's drawing does not have:\n{}", new.join("\n"));
    }
}

/// The pad terminals of the routed connections that no track end of their net reaches at the
/// pad's centre (within 1 um; a via there counts): the trace does not end at the middle of its
/// pad (an end moved out of it, a tip-anchor point taken as the end). Edge-clear pads (the centre
/// within the edge clearance) pass with a track end anywhere inside the pad.
fn centre_misses(m: &BoardModel, r: &topor_engine::route::RouteResult) -> Vec<(String, String)> {
    let mut ends: Vec<(i32, f64, f64)> = Vec::new();
    for it in &r.items {
        match it {
            Item::Track(t) => {
                let (a, b) = match t.prim {
                    TrackPrim::Seg { a, b } => (a, b),
                    TrackPrim::Arc { start, end, .. } => (start, end),
                };
                ends.push((t.net, a.x as f64, a.y as f64));
                ends.push((t.net, b.x as f64, b.y as f64));
            }
            Item::Via(v) => ends.push((v.net, v.at.x as f64, v.at.y as f64)),
        }
    }
    let mut out = Vec::new();
    for c in r.connections.iter().filter(|c| c.status == "routed") {
        for t in [&c.from, &c.to] {
            if t.kind != "pad" {
                continue;
            }
            let Some(p) = m.pads.iter().find(|p| format!("{}.{}", p.ref_, p.number) == t.label && p.net == c.net) else { continue };
            let pc = p.center.to_vec2();
            let (x, y) = (pc.x, -pc.y);
            let near = |d: f64| ends.iter().any(|e| e.0 == c.net && ((e.1 - x).powi(2) + (e.2 - y).powi(2)).sqrt() <= d);
            let in_pad = ends.iter().any(|e| {
                e.0 == c.net && p.layers.iter().chain(p.shapes.keys()).any(|&l| topor_engine::geom::dist::point_skeleton(topor_engine::geom::Vec2 { x: e.1, y: -e.2 }, &m.skeleton(p, l)) <= 1.0)
            });
            // Edge-clear (as `tidy::PadEndBreach::edge`): the centre within the edge clearance
            // plus half the width of the outline.
            let w = m.rules.net_rule(c.net).width as f64;
            let ec = m.rules.edge_clear().max(0) as f64;
            let near_edge = topor_engine::topo::board::outline_segments(m).iter().any(|&(a, b)| {
                let (d, q) = (b - a, pc - a);
                let t = (q.dot(d) / d.len2().max(1.0)).clamp(0.0, 1.0);
                (q - d * t).len() < ec + 0.5 * w + 2_000.0
            });
            let edge = near_edge && in_pad;
            if !near(1_000.0) && !edge {
                out.push((t.label.clone(), format!("{} {}: no track end at the pad centre", c.name, t.label)));
            }
        }
    }
    out
}
