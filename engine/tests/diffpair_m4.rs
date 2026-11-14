//! M4 (shopping list 5.7, planner part): a pair whose P and N arrive mirrored is a priced
//! decision in the plan, and every survivor of the polarity price is visible as a CRITICAL report
//! line (user rule "costly, not failed": never dropped, never silent).
//!
//! Boards: J1 on the left, J2 on the right, USB+ / USB- pads 0.65 mm apart (0.4 x 0.4 mm SMD on
//! F.Cu, two signal layers), the pair 0.2 / 0.2 mm with a 0.45 / 0.25 mm pair via. On J2 the pads
//! are mirrored, so P and N arrive on the other sides: the plan prices the swap (two via pairs)
//! against two single tracks. A third pad pair J3 next to J2 makes J2's pads serve two pair
//! connections, so a planned swap cannot fall back to two single tracks.

use topor_engine::model::{BoardModel, Options};
use topor_engine::topo::dpplan;

const MM: f64 = 1e6;

/// `pads`: (ref, number, net, x mm, y mm) on a 20 x 12 mm board.
fn board(pads: &[(&str, &str, i32, f64, f64)]) -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (20.0, 0.0), (20.0, 12.0), (0.0, 12.0)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pj: Vec<String> = pads
        .iter()
        .enumerate()
        .map(|(i, &(r, num, net, x, y))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"{r}","number":"{num}","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                p(x),
                p(y),
                p(0.4),
                p(0.4)
            )
        })
        .collect();
    let nr = r#"{"clearance":200000,"width":200000,"via_d":600000,"via_drill":300000,"netclass":"Default","width_source":"netclass"}"#;
    let s = format!(
        r#"{{"units":"nm","board":"m4pairs",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"USB+"}},{{"code":2,"name":"USB-"}}],
            "pads":[{}],"outline":[{}],
            "rules":{{"min_clearance":100000,"min_track_width":100000,"min_via_diameter":250000,"min_via_drill":150000,
                     "min_annular":50000,"hole_clearance":100000,"hole_to_hole":200000,"edge_clearance":200000,
                     "default":{{"clearance":200000,"width":200000,"via_d":600000,"via_drill":300000}},
                     "per_net":{{"1":{nr},"2":{nr}}},
                     "diff_pairs":[{{"p":1,"n":2,"p_name":"USB+","n_name":"USB-","netclass":"Default","width":200000,"gap":200000,
                                    "via_gap":200000,"via_d":450000,"via_drill":250000}}]}}}}"#,
        pj.join(","),
        segs.join(",")
    );
    serde_json::from_str(&s).expect("model json")
}

/// J1 (P above N) left, J2 (N above P: mirrored) right.
fn mirrored() -> BoardModel {
    board(&[("J1", "1", 1, 2.0, 5.0), ("J1", "2", 2, 2.0, 5.65), ("J2", "1", 1, 18.0, 5.65), ("J2", "2", 2, 18.0, 5.0)])
}

fn opts() -> Options {
    Options { topo_rounds: 1, ..Options::default() }
}

#[test]
fn a_mirrored_pair_plans_the_swap_and_reports_its_two_single_tracks_as_critical() {
    let r = topor_engine::route::route(&mirrored(), &opts());
    let w = r.warnings.join("\n");
    assert!(w.contains("swap planned at"), "the plan prices the swap and takes it (room on both sides):\n{w}");
    assert!(
        r.warnings.iter().any(|l| l.starts_with("CRITICAL: diff pair USB+ / USB-") && l.contains("planned swap is drawn as two single tracks")),
        "the survivor (two uncoupled single tracks instead of the planned swap) is a CRITICAL line:\n{w}"
    );
    // Two single tracks: no coupled pair report, and nothing crosses.
    assert!(r.diff_pairs.iter().all(|d| !d.polarity_cross), "{:?}", r.diff_pairs.iter().map(|d| (&d.from, &d.to, d.polarity_cross)).collect::<Vec<_>>());
}

#[test]
fn single_mode_routes_the_mirrored_pair_as_two_single_tracks_without_a_critical_line() {
    let o = Options { diffpair_swap: "single".into(), ..opts() };
    let r = topor_engine::route::route(&mirrored(), &o);
    let w = r.warnings.join("\n");
    assert!(w.contains("routed as two single tracks at the pair width ([diffpair] swap = \"single\")"), "{w}");
    assert!(!r.warnings.iter().any(|l| l.starts_with("CRITICAL: diff pair")), "a chosen option is no survivor:\n{w}");
}

#[test]
fn every_drawn_polarity_cross_has_its_critical_line() {
    // J2's pads serve two pair connections (J1 -> J2 mirrored, J2 -> J3): no fallback to two
    // single tracks, the pair is drawn coupled and its tracks cross.
    let m = board(&[
        ("J1", "1", 1, 2.0, 5.0),
        ("J1", "2", 2, 2.0, 5.65),
        ("J2", "1", 1, 14.0, 5.65),
        ("J2", "2", 2, 14.0, 5.0),
        ("J3", "1", 1, 18.0, 5.65),
        ("J3", "2", 2, 18.0, 5.0),
    ]);
    let r = topor_engine::route::route(&m, &opts());
    let w = r.warnings.join("\n");
    let crossed: Vec<_> = r.diff_pairs.iter().filter(|d| d.polarity_cross).collect();
    for d in &crossed {
        let want = format!("CRITICAL: diff pair USB+ / USB- {} -> {}: P and N cross between the ends", d.from, d.to);
        assert!(r.warnings.iter().any(|l| l.starts_with(&want)), "missing {want:?}:\n{w}");
    }
    assert!(!crossed.is_empty(), "the shared-pad board draws the mirrored connection coupled with its tracks crossed:\n{w}");
}

#[test]
fn the_critical_polarity_line_names_the_cause() {
    let l = dpplan::critical_polarity_line("A+", "A-", "U1.1", "J1.1", None);
    assert!(l.starts_with("CRITICAL: diff pair A+ / A- U1.1 -> J1.1: P and N cross between the ends"), "{l}");
    assert!(l.contains("no polarity plan"), "{l}");
}

#[test]
fn a_probe_short_of_the_clearance_by_nm_rounding_is_no_break_but_a_micron_is() {
    // Two pair tracks (P, N) on F.Cu, 0.2 mm wide, the clearance 0.2 mm: centre lines 0.4 mm apart
    // fit exactly; the quantised probes of the swap footprint land up to 1 um short.
    use topor_engine::check::{Checker, ItemIndex};
    use topor_engine::geom::Vec2;
    use topor_engine::topo::draw::track_item;
    let m = mirrored();
    let checker = Checker::new(&m, &opts());
    let base = ItemIndex::new(checker.cell());
    let at = |short_nm: f64| {
        let y = 0.4 * MM - short_nm;
        let p = [track_item(1, 0, 0.2 * MM, Vec2 { x: 6.0 * MM, y: 2.0 * MM }, Vec2 { x: 12.0 * MM, y: 2.0 * MM }, None)];
        let n = [track_item(2, 0, 0.2 * MM, Vec2 { x: 6.0 * MM, y: 2.0 * MM + y }, Vec2 { x: 12.0 * MM, y: 2.0 * MM + y }, None)];
        dpplan::static_viol(&checker, &base, &p, &n).0
    };
    assert_eq!(at(0.0), 0);
    assert_eq!(at(900.0), 0, "a shortfall of 1 um or less is nm rounding");
    assert_eq!(at(5_000.0), 1, "5 um short is a break");
}

#[test]
fn a_cheaper_reverse_exit_is_reported_but_the_plan_keeps_the_drawn_sides() {
    // Ends away from the board edge: leaving J1 on its far side and turning back would avoid the
    // swap for a few mm, but the drawing does not draw from the planned gateways yet, so the plan
    // keeps the drawn (preferred) sides and plans the swap.
    let m = board(&[("J1", "1", 1, 6.0, 5.0), ("J1", "2", 2, 6.0, 5.65), ("J2", "1", 1, 14.0, 5.65), ("J2", "2", 2, 14.0, 5.0)]);
    let r = topor_engine::route::route(&m, &opts());
    let w = r.warnings.join("\n");
    assert!(w.contains("would avoid the polarity flip for less"), "{w}");
    assert!(w.contains("swap planned at"), "{w}");
}
