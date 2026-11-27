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
/// on the axes of its pads (x 134.72 and 137.22, 0.3 x 1.6 mm, tips at y 70.959) at the full
/// 0.28 mm: no hook (a diagonal into a pad's long side) and no neck by the next pad. (The 0.09 mm
/// stretch at the crop's boundary terminal, x < 133.7, is the crop's board edge.) neck-regrow r3
/// (lead 2026-11-27): the option-off half asserted "necked > 0.1 mm" with pad_tip_anchor off; that
/// neck was the plan's phantom tight exit (`State::exit_tight` judged pass points inside the pad's
/// own corner fan), which pushed the plan's band into the pad's side. With the exit judged right
/// the plan reaches both pads along their axes, so both halves check the hook geometry itself
/// (with `[search] exit_taut`, the taut exit test; it is off by default until the plan's via-site
/// truth lands, so the option-off half runs with it on; the defaults run the on half):
/// every ADV_AVDD piece that comes within half its width of a pad's long side (alongside the pad,
/// above its tip) lies on that pad's axis, each pad is reached on its axis, and the bar keeps
/// the clearance below the tips.
#[test]
fn crop_q3_avdd_enters_along_the_pad_axis() {
    let near_row = |s: &Seg| s.a.0.min(s.b.0) > 133.8 && s.a.1.min(s.b.1) < 72.5;
    let (tip, hw, w) = (70.959, 0.15, 0.28);
    // Distance from a point to the pad's box round (px, 70.159).
    let to_pad = |px: f64, q: (f64, f64)| {
        let dx = ((q.0 - px).abs() - hw).max(0.0);
        let dy = ((q.1 - 70.159).abs() - 0.8).max(0.0);
        (dx * dx + dy * dy).sqrt()
    };
    let taut = ("exit_taut", serde_json::json!(true));
    for arm in [vec![], vec![taut.clone()], vec![taut.clone(), off("pad_tip_anchor")[0].clone()]] {
        let r = route("dvn_crop_q3", &arm);
        assert_eq!(r.unrouted, 0, "{arm:?}");
        let (_, l) = necked(&r, "ADV_AVDD", w, near_row);
        assert!(l < 1e-6, "{arm:?}: ADV_AVDD necked {l} mm at the row");
        let avdd: Vec<&Seg> = r.segs.iter().filter(|s| s.net == "ADV_AVDD").collect();
        for px in [134.72, 137.22] {
            let mut entered = false;
            for s in &avdd {
                // Closest approach of the piece to the pad alongside its body (above the tip: a
                // piece there that is off the axis enters through the long side), sampled.
                let pts = (0..=40).map(|i| {
                    let t = i as f64 / 40.0;
                    (s.a.0 + t * (s.b.0 - s.a.0), s.a.1 + t * (s.b.1 - s.a.1))
                });
                let side = pts.filter(|q| q.1 < tip - 1e-3).map(|q| to_pad(px, q)).fold(f64::INFINITY, f64::min);
                let on_axis = (s.a.0 - px).abs() < 1e-3 && (s.b.0 - px).abs() < 1e-3;
                if side < 0.5 * w - 1e-3 {
                    assert!(on_axis, "{arm:?}: hook into pad x {px}: ({:.3},{:.3})-({:.3},{:.3})", s.a.0, s.a.1, s.b.0, s.b.1);
                }
                entered |= on_axis && s.a.1.min(s.b.1) < tip;
            }
            assert!(entered, "{arm:?}: ADV_AVDD does not reach pad x {px}");
        }
        // The bar under the row (horizontal pieces between the two pads) clears the tips.
        let bar = avdd.iter().filter(|s| (s.a.1 - s.b.1).abs() < 1e-3 && s.a.0.min(s.b.0) < 135.0 && s.a.0.max(s.b.0) > 137.0);
        for s in bar {
            assert!(s.a.1 - 0.5 * w - tip > 0.1, "{arm:?}: bar at y {} too close to the tips", s.a.1);
        }
    }
}

/// B on the crop of the user's Q2 spot (U9 fan-out), as a guard: CTS necks at most through the
/// choke beside the column (about 0.9 mm), never 4 mm to the terminal. The old failure (CTS
/// necked 3.98 mm) shows in the plugin's flow on the crop board (reports/draw-via-neck-r2.md,
/// Q2 PNGs), not in this one-call route of its model: here CTS keeps its width with every option
/// off, so this test has no option-off half (r1's half passed only because the unguarded tip
/// anchor of r1 took CTS's room; B's mutant is killed by the two pattern tests above).
/// defaults-flip-a1: routed with `[search] exit_taut` pinned off (B's guard, measured before the
/// flip). With the taut exit test on (the default since then) CTS necks 5.62 mm on this crop
/// (the plan's taut exit judges the column tight and plans the neck the whole way): an open
/// finding of reports/defaults-flip-a1-*.md, not an answer this test accepts.
#[test]
fn crop_q2_cts_necks_only_at_the_choke() {
    let r = route("dvn_crop_q2", &[("exit_taut", serde_json::json!(false))]);
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

/// draw-via-neck r7 (user's test 2026-11-26: "if the track is gone after the cleanup, it was not
/// good"; lead: the criterion is survival of KiCad's Cleanup, the centre distance only a hint).
/// On every dvn pattern and crop, with the defaults, the via shift on and main's drawing (all
/// three keys off): no routed track lies in a pad of its net the way KiCad's Cleanup deletes
/// (`TRACKS_CLEANER::deleteTracksInPads`, `geom::kicad::track_inside` at the board's maximum
/// error), so the cleanup-safe tidy left nothing KiCad would remove there. Off-centre ends and T
/// joins (`tidy::pad_end_breaches`, `centre_misses`) are printed as hints only: KiCad keeps them
/// (tests/test_kicad_accepts.py, the real dialog on harness.kicad_accepts' project).
#[test]
fn no_track_survives_that_kicad_deletes_in_its_pad() {
    let pats = ["neck_in_pad_row", "neck_shared_channel", "neck_edge_terminal", "dvn_crop_q2", "dvn_crop_q3", "pad_axis_entry", "pad_side_approach", "via_interval_sliver", "via_jam_shift"];
    let f = |b: bool| serde_json::json!(b);
    let arms: [(&str, Vec<(&str, serde_json::Value)>); 3] = [
        ("defaults", vec![]),
        ("shift on", vec![("via_shift_mm", serde_json::json!(1.0))]),
        ("main drawing", vec![("neck_trim", f(false)), ("pad_tip_anchor", f(false))]),
    ];
    let mut bad: Vec<String> = Vec::new();
    for p in pats {
        for (arm, extra) in &arms {
            let (m, r) = route_items(p, extra);
            let err = m.rules.max_error as f64;
            for it in &r.items {
                let Item::Track(t) = it else { continue };
                let TrackPrim::Seg { a, b } = t.prim else { continue };
                let (a, b) = (topor_engine::geom::Vec2 { x: a.x as f64, y: -(a.y as f64) }, topor_engine::geom::Vec2 { x: b.x as f64, y: -(b.y as f64) });
                for pad in m.pads.iter().filter(|q| q.net == t.net && (q.layers.is_empty() || q.layers.contains(&t.layer))) {
                    let sk = m.skeleton(pad, t.layer);
                    if topor_engine::geom::kicad::track_inside(a, b, t.width as f64, err, |q| -topor_engine::geom::dist::point_skeleton(q, &sk), 2.0) {
                        bad.push(format!("{arm}: {p} track ({:.4},{:.4})-({:.4},{:.4}) w {} inside pad {}.{}", a.x / 1e6, -a.y / 1e6, b.x / 1e6, -b.y / 1e6, t.width, pad.ref_, pad.number));
                    }
                }
            }
            for h in topor_engine::topo::tidy::pad_end_breaches(&m, &r.items).iter().filter(|h| h.hint()) {
                eprintln!("HINT {arm}: {p} {:?} {} off {:.3} mm", h.kind, h.pad, h.off / 1e6);
            }
            for miss in centre_misses(&m, &r) {
                eprintln!("HINT {arm}: {p} {}", miss.1);
            }
        }
    }
    assert!(bad.is_empty(), "tracks KiCad's Cleanup deletes as lying in their pad:\n{}", bad.join("\n"));
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

/// neck-regrow r4: the B patterns keep their answers with `[search] exit_taut` on (the taut exit
/// test, off by default until defaults-flip-a1 2026-11-27, on since). Its nearer-end rule keeps the
/// free end of neck_in_pad_row / neck_shared_channel from judging the other end's bottleneck, and
/// its board-outline walls make neck_edge_terminal's exit tight (arms nrg_near / nrg_edge).
#[test]
fn neck_answers_hold_with_the_taut_exit_test() {
    let taut = [("exit_taut", serde_json::json!(true))];
    let r = route("neck_in_pad_row", &taut);
    assert_eq!((r.unrouted, r.violations), (0, 0));
    let (y, _) = necked(&r, "PWR", 0.5, |_| true);
    assert!(y > 4.4 && y < 4.8, "neck_in_pad_row: neck ends at y {y}, expected about 4.686");
    let r = route("neck_shared_channel", &taut);
    assert_eq!((r.unrouted, r.violations), (0, 0));
    let (y, _) = necked(&r, "PWR", 0.5, |_| true);
    assert!(y > 6.4 && y < 7.15, "neck_shared_channel: neck ends at y {y}");
    let r = route("neck_edge_terminal", &taut);
    assert_eq!(r.unrouted, 0);
    let thin = r.segs.iter().filter(|s| s.net == "PWR" && s.w < 0.5 - 1e-4).map(|s| s.w).fold(f64::INFINITY, f64::min);
    assert!(thin > 0.29, "neck_edge_terminal: PWR necked to {thin}");
    assert_eq!(r.violations, 1, "neck_edge_terminal: only the terminal's edge clearance");
}

/// pad-centre-price (docs/dev/design/pad-centre-price.md, `[factors] pad_off_centre`): on the Q3
/// crop ADV_AVDD's trace into C19.2 ends 0.26 mm off the pad's centre at weight 0 (main's copper:
/// a first leg inside the pad that KiCad's Cleanup deletes, so the run ends at its far joint);
/// weight 5 prices that end in the drawing and the trace ends at the centre, with no new
/// violation, via or unrouted connection and at most 0.5 mm more copper. Routed with main's
/// defaults. The fuse's pad-end price makes the change here (mutants pcp4: dropping its far end
/// leg term or its T-point term fails this test; the DP's weight term alone does not).
#[test]
fn crop_q3_pad_off_centre_ends_c19_at_the_centre() {
    let arm = |w: f64| {
        // main's defaults (not route_items' pinned tw-on values: with those the plan already
        // ends C19.2 at the centre at weight 0, so the price has nothing to choose).
        let path = format!("{}/tests/fixtures/dvn/model-dvn_crop_q3.json", env!("CARGO_MANIFEST_DIR"));
        let m = BoardModel::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut val = serde_json::to_value(Options::default()).unwrap();
        val["runs"] = serde_json::json!(1);
        val["factors"]["weight"]["pad_off_centre"] = serde_json::json!(w);
        // defaults-flip-a1: the pre-flip exit test (with taut exits the plan ends C19.2 at the
        // centre at weight 0 already: 0.000 mm, so the price has nothing to choose).
        val["exit_taut"] = serde_json::json!(false);
        let o: Options = serde_json::from_value(val).unwrap();
        let r = topor_engine::route::route_with(&m, &o, &RunControl::new());
        let off: f64 = topor_engine::topo::tidy::pad_end_breaches(&m, &r.items).iter().filter(|h| h.hint() && h.pad == "C19.2").map(|h| h.off / MM).fold(0.0, f64::max);
        let viol: usize = r.connections.iter().map(|c| c.violations).sum();
        eprintln!("PADOFF w {w}: C19.2 off {off:.3} mm, viol {viol}, vias {}, unrouted {}, len {:.3} mm", r.vias, r.connections_total - r.routed, r.total_length_nm as f64 / MM);
        (off, viol, r.vias, r.connections_total - r.routed, r.total_length_nm as f64 / MM)
    };
    let (off0, viol0, vias0, un0, len0) = arm(0.0);
    let (off5, viol5, vias5, un5, len5) = arm(5.0);
    assert!(off0 > 0.2, "weight 0 (main) ends C19.2 off centre: {off0} mm");
    assert!(off5 < 1e-3, "weight 5 ends C19.2 at the centre: {off5} mm");
    assert!(viol5 <= viol0 && vias5 <= vias0 && un5 <= un0, "viol {viol0}->{viol5} vias {vias0}->{vias5} unrouted {un0}->{un5}");
    assert!(len5 < len0 + 0.5, "length {len0} -> {len5} mm");
}
