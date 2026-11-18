//! exist-term (design note docs/dev/design/exist-term.md): the topological core routes
//! connections whose terminal is existing copper (an existing via, a track end inside a pad or
//! via, a kept track joined anywhere along it, a dangling end continued along its direction),
//! and the kept tracks of other nets are obstacles of the projection (TopoR 6.3 manual
//! pp. 110-111: fixed wires and vias count in every mode; a new wire grows from a fixed wire).

use topor_engine::control::RunControl;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::{route_with, RouteResult};

const MM: f64 = 1e6;

/// Pad `(ref, net, x, y, size mm, layer id)`, KiCad frame (y down).
type PadSpec<'a> = (&'a str, i32, f64, f64, f64, u32);
/// A new track: layer, ends (mm, KiCad frame), width.
type TrackOut = (u32, (f64, f64), (f64, f64), f64);
/// The piece leaving a kept end: its start, far end (mm) and width.
type Leaving = ((f64, f64), (f64, f64), f64);
/// Kept track `(uuid, net, layer id, x0, y0, x1, y1, width mm)`.
type TrackSpec<'a> = (&'a str, i32, u32, f64, f64, f64, f64, f64);
/// Via `(uuid, net, x, y)` (0.6 / 0.3 mm, through).
type ViaSpec<'a> = (&'a str, i32, f64, f64);
/// Dangling end `(track uuid, net, layer, width mm, x, y, dx, dy)` (direction KiCad frame).
type DangSpec<'a> = (&'a str, i32, u32, f64, f64, f64, f64, f64);

/// Two-layer board `w` x `h` mm, nets 1 = SIG, 2 = OTHER, clearance 0.2 mm, width 0.25 mm.
fn board(w: f64, h: f64, pads: &[PadSpec], tracks: &[TrackSpec], vias: &[ViaSpec], dang: &[DangSpec]) -> BoardModel {
    board_with_pours(w, h, pads, tracks, vias, dang, "")
}

/// [`board`] with `pours`: the model's "pours" array entries as JSON (KiCad frame).
fn board_with_pours(w: f64, h: f64, pads: &[PadSpec], tracks: &[TrackSpec], vias: &[ViaSpec], dang: &[DangSpec], pours: &str) -> BoardModel {
    board_full(w, h, pads, tracks, vias, dang, pours, "")
}

/// [`board_with_pours`] with `keepouts`: the model's "keepouts" array entries as JSON (KiCad frame).
#[allow(clippy::too_many_arguments)]
fn board_full(w: f64, h: f64, pads: &[PadSpec], tracks: &[TrackSpec], vias: &[ViaSpec], dang: &[DangSpec], pours: &str, keepouts: &str) -> BoardModel {
    let p = |v: f64| (v * MM).round() as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pj: Vec<String> = pads
        .iter()
        .map(|&(r, net, x, y, s, l)| {
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[{l}],"shapes":{{"{l}":{{"kind":"rect","w":{},"h":{}}}}}}}"#,
                p(x),
                p(y),
                p(s),
                p(s)
            )
        })
        .collect();
    let tj: Vec<String> = tracks
        .iter()
        .map(|&(u, net, l, x0, y0, x1, y1, wd)| format!(r#"{{"uuid":"{u}","net":{net},"layer":{l},"width":{},"prim":{{"kind":"seg","a":[{},{}],"b":[{},{}]}}}}"#, p(wd), p(x0), p(y0), p(x1), p(y1)))
        .collect();
    let vj: Vec<String> = vias.iter().map(|&(u, net, x, y)| format!(r#"{{"uuid":"{u}","net":{net},"at":[{},{}],"d":600000,"drill":300000,"layers":[0,2]}}"#, p(x), p(y))).collect();
    let dj: Vec<String> = dang
        .iter()
        .map(|&(u, net, l, wd, x, y, dx, dy)| format!(r#"{{"item":"{u}","net":{net},"layer":{l},"width":{},"end":[{},{}],"direction":[{dx},{dy}]}}"#, p(wd), p(x), p(y)))
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"SIG"}},{{"code":2,"name":"OTHER"}}],
            "pads":[{}],"kept":[{}],"vias":[{}],"dangling_ends":[{}],"pours":[{}],"keepouts":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        pj.join(","),
        tj.join(","),
        vj.join(","),
        dj.join(","),
        pours,
        keepouts,
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// The new tracks `(layer, a, b, width)` in mm, KiCad frame.
fn tracks(items: &[Item]) -> Vec<TrackOut> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) => match t.prim {
                TrackPrim::Seg { a, b } => Some((t.layer, (a.x as f64 / MM, a.y as f64 / MM), (b.x as f64 / MM, b.y as f64 / MM), t.width as f64 / MM)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn n_vias(items: &[Item]) -> usize {
    items.iter().filter(|i| matches!(i, Item::Via(_))).count()
}

fn d(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Some new track ends at `p` (within 1 um) on `layer`.
fn ends_at(r: &RouteResult, layer: u32, p: (f64, f64)) -> bool {
    tracks(&r.items).iter().any(|t| t.0 == layer && (d(t.1, p) < 1e-3 || d(t.2, p) < 1e-3))
}

/// The new track that leaves the dangling end `e` of the kept track `uuid` (from `k0` to `e`) on
/// `layer`, as `(e, far end, width)`: a track with an end at `e`, or, where the tidy merged the
/// lead into the hand track (KiCad's "merge co-linear tracks"; `uuid` in `remove_uuids`), the
/// track from `k0` through `e` (the part past `e`).
fn leaving(r: &RouteResult, layer: u32, k0: (f64, f64), e: (f64, f64), uuid: &str) -> Option<Leaving> {
    let merged = r.remove_uuids.iter().any(|u| u == uuid);
    tracks(&r.items).into_iter().filter(|t| t.0 == layer).find_map(|t| {
        if d(t.1, e) < 1e-3 {
            Some((e, t.2, t.3))
        } else if d(t.2, e) < 1e-3 {
            Some((e, t.1, t.3))
        } else if merged && pt_seg(e, t.1, t.2) < 1e-3 && d(t.1, k0) < 1e-3 {
            Some((e, t.2, t.3))
        } else if merged && pt_seg(e, t.1, t.2) < 1e-3 && d(t.2, k0) < 1e-3 {
            Some((e, t.1, t.3))
        } else {
            None
        }
    })
}

/// New tracks on `layer` with an end at `p`, except one that took over the kept track `uuid`
/// (merged: it runs through the kept track's other end `e`).
fn others_at(r: &RouteResult, layer: u32, p: (f64, f64), e: (f64, f64), uuid: &str) -> usize {
    let merged = r.remove_uuids.iter().any(|u| u == uuid);
    tracks(&r.items).iter().filter(|t| t.0 == layer && (d(t.1, p) < 1e-3 || d(t.2, p) < 1e-3) && !(merged && pt_seg(e, t.1, t.2) < 1e-3)).count()
}

fn summary(r: &RouteResult) -> String {
    format!("routed {}/{} reasons {:?} violations {:?} warnings {:?}", r.routed, r.connections_total, r.connections.iter().map(|c| c.reason.clone()).collect::<Vec<_>>(), r.violations, r.warnings)
}

/// A lone existing via (no track on it) is a terminal like a pad: the pad's wire ends on its
/// centre, with no DRC violation.
#[test]
fn lone_via_is_a_terminal() {
    let m = board(24.0, 12.0, &[("R1", 1, 3.0, 6.0, 1.0, 0)], &[], &[("v1", 1, 20.0, 6.0)], &[]);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(r.violations.is_empty(), "{}", summary(&r));
    assert!(ends_at(&r, 0, (20.0, 6.0)) || ends_at(&r, 2, (20.0, 6.0)), "the wire ends on the via centre: {:?}", tracks(&r.items));
    assert_eq!(n_vias(&r.items), 0, "no new via: {}", summary(&r));
}

/// An own-net via whose hand copper is on the far layer (a B.Cu track from it to a B.Cu pad): the
/// F.Cu pad's wire ends on the via on F.Cu, no new via.
#[test]
fn far_layer_via_is_a_terminal() {
    let m = board(24.0, 12.0, &[("R1", 1, 3.0, 6.0, 1.0, 0), ("R2", 1, 21.0, 10.0, 1.0, 2)], &[("t1", 1, 2, 21.0, 10.0, 18.0, 6.0, 0.25)], &[("v1", 1, 18.0, 6.0)], &[]);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(r.violations.is_empty(), "{}", summary(&r));
    assert!(ends_at(&r, 0, (18.0, 6.0)), "the wire ends on the via on F.Cu: {:?}", tracks(&r.items));
    assert_eq!(n_vias(&r.items), 0, "no new via: {}", summary(&r));
}

/// A dangling end (a hand-started F.Cu track from R1 that ends at (8, 6) heading +x, R2 at (14,
/// 10) so the direct line turns off +x), with
/// `dangling_ends = "continue"` (default): the new copper starts at the end and its first track
/// runs along the hand direction (within 1 degree), at least one pitch long (the lead, w + s =
/// 0.45 mm; the drawing may merge it with a collinear run after it), at the track's width; the
/// wire reaches R2.
#[test]
fn dangling_end_is_continued_along_its_direction() {
    let pads = [("R1", 1, 3.0, 6.0, 1.0, 0), ("R2", 1, 14.0, 10.0, 1.0, 0)];
    let kept = [("t1", 1, 0, 3.0, 6.0, 8.0, 6.0, 0.25)];
    let dang = [("t1", 1, 0, 0.25, 8.0, 6.0, 1.0, 0.0)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(r.violations.is_empty(), "{}", summary(&r));
    let ts = tracks(&r.items);
    let (q0, q1, wd) = leaving(&r, 0, (3.0, 6.0), (8.0, 6.0), "t1").unwrap_or_else(|| panic!("no new track at the dangling end: {ts:?}"));
    let ang = (q1.1 - q0.1).atan2(q1.0 - q0.0).to_degrees();
    assert!(ang.abs() < 1.0, "the first track leaves along +x: {ang:.2} deg, {ts:?}");
    assert!(d(q0, q1) > 0.45 - 0.02, "at least one pitch of lead: {:.3} mm", d(q0, q1));
    assert!((wd - 0.25).abs() < 1e-6, "at the hand track's width");
    // r2 (verifier F5): the lead goes on straight along the hand track at its width, so it is one
    // segment with it (KiCad's Cleanup "merge co-linear tracks" finds nothing): the hand track is
    // replaced
    assert_eq!(r.remove_uuids, vec!["t1".to_string()], "the hand track is merged into the lead");
    assert!(ts.iter().any(|t| (d(t.1, (3.0, 6.0)) < 1e-3 || d(t.2, (3.0, 6.0)) < 1e-3) && pt_seg((8.0, 6.0), t.1, t.2) < 1e-3), "one segment from R1 through the end: {ts:?}");
    assert!(ends_at(&r, 0, (14.0, 10.0)), "it reaches R2: {ts:?}");
}

/// The same dangling end with `dangling_ends = "fixed"`: the new copper starts at the end at the
/// angle the plan takes (no lead); still routed without violations.
#[test]
fn dangling_end_fixed_has_no_lead() {
    let pads = [("R1", 1, 3.0, 6.0, 1.0, 0), ("R2", 1, 20.0, 1.5, 1.0, 0)];
    let kept = [("t1", 1, 0, 3.0, 6.0, 8.0, 6.0, 0.25)];
    let dang = [("t1", 1, 0, 0.25, 8.0, 6.0, 1.0, 0.0)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
    let o = Options { dangling_ends: "fixed".into(), ..Options::default() };
    let r = route_with(&m, &o, &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(r.violations.is_empty(), "{}", summary(&r));
    let (_, q1, _) = leaving(&r, 0, (3.0, 6.0), (8.0, 6.0), "t1").unwrap_or_else(|| panic!("the wire starts at the end: {:?}", tracks(&r.items)));
    assert!(!((q1.1 - 6.0).abs() < 1e-6 && d(q1, (8.45, 6.0)) < 0.02), "no planned lead: {:?}", tracks(&r.items));
}

/// A kept track with a T: R1 (10, 2) - joint (10, 6) - R3 (10, 10), and a stub joint - R2 (12, 6).
/// The connection from R4 (2, 5) is planned from the joint (a track end that is no pad), but the
/// whole kept track is the terminal (lead 2026-11-17): the wire joins it where that is cheapest
/// (the nearest track point (10, 5) is 8 mm away; the joint is 8.06 mm straight, 8.41 mm
/// octilinear), on F.Cu, without violations.
#[test]
fn kept_track_is_joined_anywhere_along_it() {
    let pads = [("R1", 1, 10.0, 2.0, 0.8, 0), ("R3", 1, 10.0, 10.0, 0.8, 0), ("R2", 1, 12.0, 6.0, 0.8, 0), ("R4", 1, 2.0, 5.0, 0.8, 0)];
    let kept = [("a", 1, 0, 10.0, 2.0, 10.0, 6.0, 0.25), ("b", 1, 0, 10.0, 6.0, 10.0, 10.0, 0.25), ("c", 1, 0, 10.0, 6.0, 12.0, 6.0, 0.25)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &[]);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(r.violations.is_empty(), "{}", summary(&r));
    let ts = tracks(&r.items);
    let len: f64 = ts.iter().map(|t| d(t.1, t.2)).sum();
    assert!(len <= 8.0 + 0.2, "joined near (10, 5), not at the joint: {len:.3} mm {ts:?}");
    let on_track = |p: (f64, f64)| (p.0 - 10.0).abs() < 1e-3 && (2.0..=10.0).contains(&p.1);
    assert!(ts.iter().any(|t| on_track(t.1) || on_track(t.2)), "the wire ends on the kept track: {ts:?}");
}

/// A kept track of another net across the direct line (OTHER, F.Cu from R5 (12, 1) to R6 (12,
/// 11)) is an obstacle of the projection: the SIG wire R1 - R2 does not cross it on F.Cu (it
/// changes layer), and the result has no violation.
#[test]
fn foreign_kept_track_is_an_obstacle() {
    let pads = [("R1", 1, 3.0, 6.0, 1.0, 0), ("R2", 1, 21.0, 6.0, 1.0, 0), ("R5", 2, 12.0, 1.0, 0.8, 0), ("R6", 2, 12.0, 11.0, 0.8, 0)];
    let kept = [("o", 2, 0, 12.0, 1.0, 12.0, 11.0, 0.25)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &[]);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(r.violations.is_empty(), "{}", summary(&r));
    let crosses = tracks(&r.items).iter().any(|t| t.0 == 0 && (t.1 .0 - 12.0) * (t.2 .0 - 12.0) <= 0.0);
    assert!(!crosses, "no F.Cu track crosses x = 12: {:?}", tracks(&r.items));
}

/// A continued dangling end that the decomposition picked as its cluster's port stays the root
/// (A14) even where its direction points away: the hand track R1 (3, 6) - (8, 6) ends at (8, 6)
/// heading +x, R2 (6.5, 8) lies behind the end (2.5 mm; R1 4.3 mm, so the nearest port pair is
/// end - R2). The wire starts at the end (its lead, then the turn back), not at R1. This holds by
/// the decomposition's pick here: R1 is the costlier start anyway, so it does not test the root
/// retain (see a_dangling_end_the_decomposition_picked_stays_the_root_behind_a_wall) and says
/// nothing about a nearer cluster pad (known_gap_a_nearer_cluster_pad_takes_the_root).
#[test]
fn dangling_end_is_the_root_even_when_it_points_away() {
    let pads = [("R1", 1, 3.0, 6.0, 1.0, 0), ("R2", 1, 6.5, 8.0, 0.8, 0)];
    let kept = [("t1", 1, 0, 3.0, 6.0, 8.0, 6.0, 0.25)];
    let dang = [("t1", 1, 0, 0.25, 8.0, 6.0, 1.0, 0.0)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(r.violations.is_empty(), "{}", summary(&r));
    assert!(leaving(&r, 0, (3.0, 6.0), (8.0, 6.0), "t1").is_some(), "the wire starts at the dangling end: {:?}", tracks(&r.items));
    assert_eq!(others_at(&r, 0, (3.0, 6.0), (8.0, 6.0), "t1"), 0, "not at R1: {:?}", tracks(&r.items));
}

/// The root retain (core.rs route_round, `src.retain` / `dst.retain`): the decomposition picks the
/// dangling end (8, 6) as the port (3.6 mm to R2 (9, 9.5); R1 (3, 6) is 7.0 mm away), but a kept
/// OTHER track on F.Cu along y = 7.5 from R5 (4.5, 7.5) to R6 (20, 7.5) walls the end off from R2:
/// from the end the F.Cu path runs round the wall's west end (about 10.9 mm with the lead), from
/// R1 it is about 7.3 mm (a B.Cu hop costs two vias, 50 mm at the default via weight). Without the
/// retain the search would start at R1 (same component through the hand copper) and leave the
/// hand stub dangling; with it the wire starts at the end, along its direction.
#[test]
fn a_dangling_end_the_decomposition_picked_stays_the_root_behind_a_wall() {
    let pads = [("R1", 1, 3.0, 6.0, 1.0, 0), ("R2", 1, 9.0, 9.5, 0.8, 0), ("R5", 2, 4.5, 7.5, 0.8, 0), ("R6", 2, 20.0, 7.5, 0.8, 0)];
    let kept = [("t1", 1, 0, 3.0, 6.0, 8.0, 6.0, 0.25), ("o", 2, 0, 4.5, 7.5, 20.0, 7.5, 0.25)];
    let dang = [("t1", 1, 0, 0.25, 8.0, 6.0, 1.0, 0.0)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(r.violations.is_empty(), "{}", summary(&r));
    assert!(leaving(&r, 0, (3.0, 6.0), (8.0, 6.0), "t1").is_some(), "the wire starts at the dangling end: {:?}", tracks(&r.items));
    assert_eq!(others_at(&r, 0, (3.0, 6.0), (8.0, 6.0), "t1"), 0, "not at R1: {:?}", tracks(&r.items));
}

/// Known gap (r1 verifier probe vf_root_beats_a_nearer_cluster_pad; decision D2, out of scope):
/// the decomposition takes the nearest port pair of the MST edge, so a pad of the hand track's own
/// cluster that lies nearer the goal than the dangling end takes the connection and the hand stub
/// stays dangling. R2 (3.5, 9) is 3.04 mm from R1 and 5.41 mm from the end (8, 6): the wire starts
/// at R1 today. The test states the wanted behaviour and fails until D2 is decided (a graded port
/// cost that prefers continued dangling ends); run it with --ignored.
#[test]
#[ignore = "known gap D2: the decomposition's nearest port pair lets a nearer cluster pad take the root; the hand stub stays dangling"]
fn known_gap_a_nearer_cluster_pad_takes_the_root() {
    let pads = [("R1", 1, 3.0, 6.0, 1.0, 0), ("R2", 1, 3.5, 9.0, 0.8, 0)];
    let kept = [("t1", 1, 0, 3.0, 6.0, 8.0, 6.0, 0.25)];
    let dang = [("t1", 1, 0, 0.25, 8.0, 6.0, 1.0, 0.0)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    assert_eq!(r.routed, 1, "{}", summary(&r));
    assert!(leaving(&r, 0, (3.0, 6.0), (8.0, 6.0), "t1").is_some(), "the wire starts at the dangling end: {:?}", tracks(&r.items));
}

/// The pour case of route_round's failure arm (exist-term r2, verifier F7): a hand track R1 (3, 6)
/// to via v1 (10, 6) of SIG and a SIG pour on B.Cu over (16..22, 2..10) that nothing joins yet. The
/// pour completion joins the cluster's port nearest the pour (the via, or the track end on it) to
/// the pour, which the topological core does not route yet: the reason names that end, not "an
/// existing via or track end" for every pour pair (the text of each pair is in core.rs
/// unsupported_ends_names_both_ends_for_every_pour_and_branch_pair).
#[test]
fn existing_copper_joined_to_a_pour_fails_with_a_reason_that_names_the_end() {
    let pads = [("R1", 1, 3.0, 6.0, 1.0, 0)];
    let kept = [("t1", 1, 0, 3.0, 6.0, 10.0, 6.0, 0.25)];
    let p = |x: f64| (x * MM).round() as i64;
    let ring = format!("[[{},{}],[{},{}],[{},{}],[{},{}]]", p(16.0), p(2.0), p(22.0), p(2.0), p(22.0), p(10.0), p(16.0), p(10.0));
    let pour = format!(
        r#"{{"uuid":"z0","net":1,"layers":[2],"pads":[],"vias":[],"clearance":200000,"min_thickness":250000,"thermal_gap":500000,"spoke_width":500000,"pad_connection":2,"fills":[{{"layer":2,"outer":{ring}}}]}}"#
    );
    let m = board_with_pours(24.0, 12.0, &pads, &kept, &[("v1", 1, 10.0, 6.0)], &[], &pour);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    let tail = " joined to a pour: not supported by the topological core yet (existing vias and track ends are terminals towards pads, vias and tracks; a pad joins its pour through a plane stub)";
    let hit: Vec<&String> = r.connections.iter().map(|c| &c.reason).filter(|t| t.ends_with(tail)).collect();
    assert_eq!(hit.len(), 1, "one connection fails with the pour text: {}", summary(&r));
    assert!(hit[0].starts_with("an existing via") || hit[0].starts_with("an existing track end"), "names the end: {}", hit[0]);
}

/// Point to segment distance (mm).
fn pt_seg(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
    d(p, (a.0 + dx * t, a.1 + dy * t))
}

/// A lead cut short by another net's pad ahead (r1 verifier F4, the shape of badge conn 116): the
/// hand track R1 (12, 2) - (12, 5) ends heading +y (KiCad frame, down) 0.7 mm above the top edge
/// of OTHER's pad P (12, 6.4), 1.4 mm square, so the lead stops at the clearance plus the ERBS
/// margin (0.375 mm, short of one pitch); R2 (6, 6) lies to the west, so the trace leaves the lead
/// end along P's top edge. Every new F.Cu track keeps the 0.2 mm clearance to P (tracks are
/// 0.25 mm), there is no violation, and the first new track still leaves the end along +y.
#[test]
fn a_lead_cut_by_a_foreign_pad_keeps_its_clearance_after_the_turn() {
    let pads = [("R1", 1, 12.0, 2.0, 1.0, 0), ("R2", 1, 6.0, 6.0, 0.8, 0), ("P", 2, 12.0, 6.4, 1.4, 0)];
    let kept = [("t1", 1, 0, 12.0, 2.0, 12.0, 5.0, 0.25)];
    let dang = [("t1", 1, 0, 0.25, 12.0, 5.0, 0.0, 1.0)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
    // r4: the wire needs no via, so the lead keeps its track-only length with the default
    // dangling_lead_via as with it off: P cuts it at y = 5.7 - 0.2 - 0.125 = 5.375 mm (0.375 mm)
    let mut ts = Vec::new();
    for via in [true, false] {
        let o = Options { dangling_lead_mm: 1.0, dangling_lead_via: via, ..Options::default() };
        let r = route_with(&m, &o, &RunControl::new());
        assert_eq!(r.routed, 1, "{}", summary(&r));
        assert!(r.violations.is_empty(), "{}", summary(&r));
        assert_eq!(n_vias(&r.items), 0, "{}", summary(&r));
        ts = tracks(&r.items);
        let (_, far, _) = leaving(&r, 0, (12.0, 2.0), (12.0, 5.0), "t1").unwrap_or_else(|| panic!("no new track at the end: {ts:?}"));
        assert!((far.0 - 12.0).abs() < 1e-6 && far.1 > 5.0, "lead via {via}: the lead goes on along +y: {far:?} {ts:?}");
        // cut by P with the ERBS margin (~10 nm) on top of the clearance: no zero slack, and no more than the margin
        let slack = 5.7 - far.1 - 0.125 - 0.2;
        assert!((5e-6..=1e-3).contains(&slack), "lead via {via}: lead end {far:?}: {:.1} nm of slack to P's clearance {ts:?}", slack * 1e6);
    }
    // P's copper: x 11.3..12.7, y 5.7..7.1
    let (x0, y0, x1, y1) = (11.3, 5.7, 12.7, 7.1);
    let edges = [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))];
    for t in ts.iter().filter(|t| t.0 == 0) {
        // the closest approach of a segment to a rectangle it does not enter: at an end of one of them
        let mut gap = f64::INFINITY;
        for &(a, b) in &edges {
            gap = gap.min(pt_seg(a, t.1, t.2)).min(pt_seg(t.1, a, b)).min(pt_seg(t.2, a, b));
        }
        // every piece keeps the margin too (a zero-slack lead drawn on past the turn's start leaves a 10 nm spur at 0.2)
        assert!(gap - 0.5 * t.3 >= 0.2 + 5e-6, "track {t:?} keeps {:.6} mm to P: {ts:?}", gap - 0.5 * t.3);
    }
}

/// Round 3 (badge items_not_allowed: two B.Cu wires drawn through a module's antenna keepout, beside
/// an F.Cu kept track 0.25 / 0.5 mm off its sides): a guard, not a repro. The keepout is not lost
/// next to kept copper: a wire round it (one corner, or the whole left side), with kept tracks of
/// another net beside the side, keeps out of it. The badge's two wires were forced (never drop: no
/// legal drawing among other new wires, shorts with them as well), and the forced copper reports
/// the keepout as a violation, as the last case checks.
#[test]
fn a_wire_keeps_out_of_a_keepout_beside_a_long_kept_track() {
    // Two shapes: a wire that rounds the keepout's corner (A1-B1), and one that runs along its whole
    // left side (A2-B2, the badge's case: a B.Cu wire beside an F.Cu kept track 0.15 mm off the side).
    let sets: [[PadSpec; 2]; 2] = [[("A", 1, 20.0, 12.5, 1.0, 2), ("B", 1, 4.0, 12.5, 1.0, 2)], [("A", 1, 12.0, 1.0, 1.0, 2), ("B", 1, 12.0, 15.0, 1.0, 2)]];
    let ko = r#"{"name":"antenna","layers":[0,2],"poly":[[10000000,2000000],[16000000,2000000],[16000000,14000000],[10000000,14000000]],"no_tracks":true,"no_vias":true,"no_pour":true}"#;
    for (si, pads) in sets.iter().enumerate() {
    for kept in [&[][..], &[("t2", 2, 0, 9.75, 1.0, 9.75, 14.5, 0.2)][..], &[("t2", 2, 0, 9.75, 2.5, 9.75, 13.5, 0.2)][..]] {
        let m = board_full(30.0, 20.0, pads, kept, &[], &[], "", ko);
        let r = route_with(&m, &Options::default(), &RunControl::new());
        let tag = format!("pads {si}, kept {kept:?}");
        eprintln!("R3KO {tag}: routed {} tracks {:?} violations {:?}", r.routed, tracks(&r.items), r.violations);
        assert_eq!(r.routed, 1, "{tag}: {}", summary(&r));
        let ts = tracks(&r.items);
        // signed distance of a point to the keepout rectangle x 10..16, y 2..14 (negative inside)
        let sd = |q: (f64, f64)| {
            let (dx, dy) = ((10.0 - q.0).max(q.0 - 16.0), (2.0 - q.1).max(q.1 - 14.0));
            if dx <= 0.0 && dy <= 0.0 { dx.max(dy) } else { dx.max(0.0).hypot(dy.max(0.0)) }
        };
        for t in &ts {
            let n = 2000;
            let gap = (0..=n).map(|i| { let f = i as f64 / n as f64; sd((t.1 .0 + (t.2 .0 - t.1 .0) * f, t.1 .1 + (t.2 .1 - t.1 .1) * f)) }).fold(f64::INFINITY, f64::min);
            assert!(gap - 0.5 * t.3 >= -1e-6, "{tag}: track {t:?} enters the keepout by {:.4} mm: {ts:?} {}", 0.5 * t.3 - gap, summary(&r));
        }
        assert!(r.violations.is_empty(), "{tag}: {}", summary(&r));
    }
    }
    // No legal way: the keepout spans the board. The wire is still placed (never drop) and its
    // copper in the keepout is a reported violation against it (costly, not failed).
    let wall = r#"{"name":"antenna","layers":[0,2],"poly":[[10000000,0],[16000000,0],[16000000,20000000],[10000000,20000000]],"no_tracks":true,"no_vias":true,"no_pour":true}"#;
    let m = board_full(30.0, 20.0, &sets[0], &[], &[], &[], "", wall);
    let r = route_with(&m, &Options::default(), &RunControl::new());
    eprintln!("R3KO wall: routed {} tracks {:?} violations {:?}", r.routed, tracks(&r.items), r.violations);
    assert!(r.violations.iter().any(|v| v.against.contains("keepout")), "the keepout is reported: {}", summary(&r));
}

/// Round 3 (badge conn 116: the via at a cut lead's end sat 23 um inside a foreign pad's
/// clearance): the dangling end of R1's hand track (12, 2) - (12, 5), 0.2 mm, heads +y at the
/// pad P of another net, so its lead is cut short for the track. The wire to R2 on B.Cu straight
/// on (under P) changes layer at once: F.Cu walls of P's net (x = 11.2, 12.8) leave no F.Cu way
/// out, and B.Cu walls of it beside the hand track (x = 12 -/+ 0.55, to `wall_end`) take the via
/// sites along it. The lead must end where that via keeps every clearance: all copper legal.
/// Variants: P's position, the B.Cu walls' end (none: sites along the whole track).
#[test]
fn a_via_at_a_cut_lead_keeps_its_clearance() {
    let mut bad: Vec<String> = Vec::new();
    for py in [6.15, 6.1] {
        for wall_end in [0.0, 4.6, 4.7] {
            let pads = [("R1", 1, 12.0, 2.0, 1.0, 0), ("R2", 1, 12.0, 8.0, 0.8, 2), ("P", 2, 12.0, py, 1.0, 0)];
            let mut kept = vec![("t1", 1, 0, 12.0, 2.0, 12.0, 5.0, 0.2), ("w1", 2, 0, 11.2, 2.5, 11.2, 7.1, 0.2), ("w2", 2, 0, 12.8, 2.5, 12.8, 7.1, 0.2)];
            if wall_end > 0.0 {
                kept.push(("b1", 2, 2, 11.45, 2.5, 11.45, wall_end, 0.2));
                kept.push(("b2", 2, 2, 12.55, 2.5, 12.55, wall_end, 0.2));
            }
            let dang = [("t1", 1, 0, 0.2, 12.0, 5.0, 0.0, 1.0)];
            let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
            let r = route_with(&m, &Options::default(), &RunControl::new());
            let vias: Vec<(f64, f64)> = r.items.iter().filter_map(|i| if let Item::Via(v) = i { Some((v.at.x as f64 / MM, v.at.y as f64 / MM)) } else { None }).collect();
            let tag = format!("P y {py}, B.Cu walls to {wall_end}: routed {}, vias {vias:?}, tracks {:?}", r.routed, tracks(&r.items));
            eprintln!("R3VIA {tag} violations {:?}", r.violations);
            if r.routed != 1 || !r.violations.is_empty() {
                bad.push(format!("{tag}: {}", summary(&r)));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Round 4 (the I2C_SDA fold-back of the verifier's r3 run, merge_collinear 1): a 0.2 mm SIG hand
/// track from R1 (8, 14) heads NE to its dangling end (12, 10); an OTHER track 0.6-1 mm east of
/// the lead end runs north-south on F.Cu, and the target R2 lies ESE on B.Cu (the wire changes
/// layer near the lead end) or SSW of the lead end on F.Cu (the run turns back past the lead).
/// No new F.Cu copper doubles back along the hand track or its lead: no two F.Cu tracks on their
/// line share an end with both far ends on one side of it (KiCad's cleanup merges such a pair).
/// The run from the lead end arrives there along the lead, so its first leg back along it is a
/// full reversal ([`octi`] `lead_in`).
#[test]
fn a_wire_leaving_a_lead_does_not_fold_back_along_it() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let mut bad: Vec<String> = Vec::new();
    let b = 2;
    #[rustfmt::skip]
    let cases = [
        (12.8, (16.0, 11.5), b, false), (12.75, (16.0, 11.0), b, false), (12.8, (15.0, 12.5), b, false), (12.8, (16.0, 11.5), b, true), (12.7, (16.0, 10.5), b, true), (13.0, (16.0, 11.0), b, true),
        (13.0, (11.7, 11.7), 0, false), (13.0, (12.0, 12.2), 0, false), (13.0, (12.0, 11.0), 0, true), (13.0, (11.9, 11.2), 0, false),
    ];
    for (ox, r2, l2, scl) in cases {
        for via in [true, false] {
            let pads = [("R1", 1, 8.0, 14.0, 0.8, 0), ("R2", 1, r2.0, r2.1, 0.8, l2)];
            let mut kept = vec![("t1", 1, 0, 8.0, 14.0, 12.0, 10.0, 0.2), ("o1", 2, 0, ox, 6.0, ox, 14.0, 0.2)];
            let mut vs = vec![];
            if scl {
                // the badge's neighbours: a parallel OTHER track 0.81 mm NW turning north, an OTHER via SW
                kept.push(("o2", 2, 0, 9.026, 11.826, 11.25, 9.602, 0.2));
                kept.push(("o3", 2, 0, 11.25, 9.602, 11.25, 2.0, 0.2));
                vs.push(("v1", 2, 11.0, 11.6));
            }
            let dang = [("t1", 1, 0, 0.2, 12.0, 10.0, s, -s)];
            let m = board(24.0, 18.0, &pads, &kept, &vs, &dang);
            let r = route_with(&m, &Options { dangling_lead_via: via, ..Options::default() }, &RunControl::new());
            let vias: Vec<(f64, f64)> = r.items.iter().filter_map(|i| if let Item::Via(v) = i { Some((v.at.x as f64 / MM, v.at.y as f64 / MM)) } else { None }).collect();
            let ts = tracks(&r.items);
            let tag = format!("OTHER x {ox}, R2 {r2:?} layer {l2}, neighbours {scl}, lead via {via}: routed {}, vias {vias:?}, tracks {ts:?}", r.routed);
            eprintln!("R4FOLD {tag} violations {:?}", r.violations);
            // on the hand track's line x + y = 22
            let on = |p: (f64, f64)| ((p.0 + p.1 - 22.0) * s).abs() < 1e-3;
            let line: Vec<_> = ts.iter().filter(|t| t.0 == 0 && on(t.1) && on(t.2) && d(t.1, t.2) > 1e-3).collect();
            let mut back = false;
            for (i, t) in line.iter().enumerate() {
                for u in &line[i + 1..] {
                    for (p, a) in [(t.1, t.2), (t.2, t.1)] {
                        for (q, c) in [(u.1, u.2), (u.2, u.1)] {
                            // a shared end, both far ends on one side of it
                            back |= d(p, q) < 1e-3 && (a.0 - p.0) * (c.0 - p.0) + (a.1 - p.1) * (c.1 - p.1) > 0.0;
                        }
                    }
                }
            }
            if r.routed != 1 || back {
                bad.push(format!("{tag}: fold-back {back}: {}", summary(&r)));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Round 5 (lead decision from the power via rules, verifier r4 F1): the cut-lead shape of
/// [`a_via_at_a_cut_lead_keeps_its_clearance`] (no B.Cu walls) with two via sizes, the net
/// class's 0.6 / 0.3 mm and a pre-defined 0.45 / 0.25 mm. The plan changes layer at the lead end,
/// so the lead ends at the via, the net class via first: with P's top edge at 5.65 mm it fits
/// at y = 5.65 - 0.2 - 0.3 = 5.15 mm (the 0.45 mm via would fit at 5.225). With P's edge at
/// 5.46 mm the 0.6 mm via fits nowhere on the lead (it needs 5.5), so the 0.45 mm one is the
/// fallback, at 5.46 - 0.2 - 0.225 = 5.035 mm. With the pre-defined drill 0.06 mm
/// (pi x 0.06 < the 0.25 mm trace) it is no fallback: no via at that lead end. With
/// `dangling_lead_via` off the lead keeps its track-only length (5.65 - 0.2 - 0.1 = 5.35).
#[test]
fn a_lead_ends_at_the_net_class_via_first_and_the_smaller_one_as_fallback() {
    let run = |py: f64, small: [i64; 2], via: bool| {
        let pads = [("R1", 1, 12.0, 2.0, 1.0, 0), ("R2", 1, 12.0, 8.0, 0.8, 2), ("P", 2, 12.0, py, 1.0, 0)];
        let kept = [("t1", 1, 0, 12.0, 2.0, 12.0, 5.0, 0.2), ("w1", 2, 0, 11.2, 2.5, 11.2, 7.1, 0.2), ("w2", 2, 0, 12.8, 2.5, 12.8, 7.1, 0.2)];
        let dang = [("t1", 1, 0, 0.2, 12.0, 5.0, 0.0, 1.0)];
        let mut m = board(24.0, 12.0, &pads, &kept, &[], &dang);
        m.rules.predefined_vias = vec![small];
        m.rules.min_via_drill = small[1];
        let r = route_with(&m, &Options { dangling_lead_via: via, ..Options::default() }, &RunControl::new());
        let vias: Vec<((f64, f64), i64)> = r.items.iter().filter_map(|i| if let Item::Via(v) = i { Some(((v.at.x as f64 / MM, v.at.y as f64 / MM), v.d)) } else { None }).collect();
        let tag = format!("P y {py}, small {small:?}, lead via {via}: vias {vias:?}, tracks {:?}: {}", tracks(&r.items), summary(&r));
        eprintln!("R5VSZ {tag}");
        (r, vias, tag)
    };
    // the via at the lead's via end, the lead up to it, no F.Cu copper past it
    let at_lead = |r: &RouteResult, vias: &[((f64, f64), i64)], y: f64, want: i64, tag: &str| {
        assert_eq!(r.routed, 1, "{tag}");
        assert!(r.violations.is_empty(), "{tag}");
        assert_eq!(vias.len(), 1, "{tag}");
        let ((vx, vy), vd) = vias[0];
        assert!((vx - 12.0).abs() < 1e-6 && (y - 2e-3..=y).contains(&vy) && vd == want, "the {want} nm via at y {y}: {tag}");
        assert!(ends_at(r, 0, (vx, vy)) || leaving(r, 0, (12.0, 2.0), (12.0, 5.0), "t1").is_some_and(|l| d(l.1, (vx, vy)) < 1e-3), "the lead runs to the via: {tag}");
        assert!(tracks(&r.items).iter().filter(|t| t.0 == 0).all(|t| t.1 .1.max(t.2 .1) <= vy + 1e-6), "F.Cu ends at the via: {tag}");
    };
    let (r, vias, tag) = run(6.15, [450_000, 250_000], true);
    at_lead(&r, &vias, 5.15, 600_000, &tag);
    let (r, vias, tag) = run(5.96, [450_000, 250_000], true);
    at_lead(&r, &vias, 5.035, 450_000, &tag);
    let (r, vias, tag) = run(5.96, [450_000, 60_000], true);
    assert_eq!(r.routed, 1, "{tag}");
    assert!(!vias.iter().any(|&((x, y), _)| (x - 12.0).abs() < 1e-6 && (5.0..=5.04).contains(&y)), "no lead via with a drill that cannot carry the trace: {tag}");
    // off: the track-only lead
    let (r, _, tag) = run(6.15, [450_000, 250_000], false);
    assert_eq!(r.routed, 1, "{tag}");
    let (_, far, _) = leaving(&r, 0, (12.0, 2.0), (12.0, 5.0), "t1").unwrap_or_else(|| panic!("no lead: {tag}"));
    assert!((far.0 - 12.0).abs() < 1e-6 && (5.35 - 2e-3..=5.35).contains(&far.1), "track-only lead: {far:?} {tag}");
}

/// Round 4 (r3 verifier F5, the lead decision): the hand track R1 (12, 2) - (12, 5) heads +y at
/// OTHER's pad P on B.Cu (top edge 5.505 mm): the F.Cu lead has its full room, the net's via fits
/// only 5 um into the lead (below the 10 um of a lead), and the wire to R2 (6, 6) stays on F.Cu.
/// The lead keeps its track-only length (r3 cut it to the via end and, that being below 10 um,
/// made the end a plain disc with the note "no room to continue it ..."): no such note, and the
/// lead goes on along +y.
#[test]
fn a_lead_that_needs_no_via_keeps_its_length_and_no_note() {
    let pads = [("R1", 1, 12.0, 2.0, 1.0, 0), ("R2", 1, 6.0, 6.0, 0.8, 0), ("P", 2, 12.0, 6.005, 1.0, 2)];
    let kept = [("t1", 1, 0, 12.0, 2.0, 12.0, 5.0, 0.25)];
    let dang = [("t1", 1, 0, 0.25, 12.0, 5.0, 0.0, 1.0)];
    let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
    let r = route_with(&m, &Options { dangling_lead_mm: 0.4, ..Options::default() }, &RunControl::new());
    let tag = format!("tracks {:?}: {}", tracks(&r.items), summary(&r));
    assert_eq!(r.routed, 1, "{tag}");
    assert!(r.violations.is_empty(), "{tag}");
    assert!(!r.warnings.iter().any(|w| w.contains("no room to continue")), "{tag}");
    let (_, far, _) = leaving(&r, 0, (12.0, 2.0), (12.0, 5.0), "t1").unwrap_or_else(|| panic!("no lead: {tag}"));
    assert!((far.0 - 12.0).abs() < 1e-6 && far.1 > 5.0 + 0.3, "the lead goes on along +y: {far:?} {tag}");
}

/// Round 4 (the lead decision, the plan's pass interval): the lead of the hand track (8, 14) -> (12, 10)
/// (NE, F.Cu) has a via end, but an OTHER track on B.Cu runs east-west 0.9 mm south of the lead
/// end and R2 lies beyond it on B.Cu: the plan crosses that track on F.Cu and changes layer after
/// it. The via stays in the plan's interval (not at the lead's via end, before the crossing,
/// which would short the B.Cu run with the OTHER track): no violation.
#[test]
fn a_lead_via_stays_in_the_plans_pass_interval() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let mut bad = Vec::new();
    for (yb, r2) in [(10.6, (15.0, 12.5)), (10.8, (14.0, 13.0)), (10.6, (12.3, 13.0))] {
        let pads = [("R1", 1, 8.0, 14.0, 0.8, 0), ("R2", 1, r2.0, r2.1, 0.8, 2)];
        let kept = [("t1", 1, 0, 8.0, 14.0, 12.0, 10.0, 0.2), ("o1", 2, 0, 12.8, 6.0, 12.8, 10.0, 0.2), ("b1", 2, 2, 4.0, yb, 22.0, yb, 0.2)];
        let dang = [("t1", 1, 0, 0.2, 12.0, 10.0, s, -s)];
        let m = board(24.0, 18.0, &pads, &kept, &[], &dang);
        let r = route_with(&m, &Options::default(), &RunControl::new());
        let vias: Vec<(f64, f64)> = r.items.iter().filter_map(|i| if let Item::Via(v) = i { Some((v.at.x as f64 / MM, v.at.y as f64 / MM)) } else { None }).collect();
        let tag = format!("B.Cu wall y {yb}, R2 {r2:?}: vias {vias:?}, tracks {:?}: {}", tracks(&r.items), summary(&r));
        eprintln!("R4IV {tag}");
        if r.routed != 1 || !r.violations.is_empty() || vias.iter().any(|v| v.1 < yb) {
            bad.push(tag);
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Round 5 (badge I2C_SDA, KiCad Cleanup merge_collinear 1 at r5's first cut): two wires start
/// at one dangling end, R1 (12, 2) - (12, 5) heading +y at P. The one to R2 on B.Cu changes layer
/// at the lead end (its via at the lead's via end), the one to R3 on F.Cu east goes on from the
/// lead's end. The lead is one copper: no new F.Cu track along the hand track's line lies inside
/// another one (a shorter lead to the via beside the full one is what KiCad's Cleanup merges).
#[test]
fn two_wires_from_one_dangling_end_share_one_lead() {
    let mut bad = Vec::new();
    for (py, r3) in [(6.15, (15.0, 5.2)), (6.1, (15.0, 5.6)), (6.15, (16.0, 4.6))] {
        let pads = [("R1", 1, 12.0, 2.0, 1.0, 0), ("R2", 1, 12.0, 8.0, 0.8, 2), ("R3", 1, r3.0, r3.1, 0.8, 0), ("P", 2, 12.0, py, 1.0, 0)];
        let kept = [("t1", 1, 0, 12.0, 2.0, 12.0, 5.0, 0.2), ("w1", 2, 0, 11.2, 2.5, 11.2, 7.1, 0.2)];
        let dang = [("t1", 1, 0, 0.2, 12.0, 5.0, 0.0, 1.0)];
        let m = board(24.0, 12.0, &pads, &kept, &[], &dang);
        let r = route_with(&m, &Options::default(), &RunControl::new());
        let ts = tracks(&r.items);
        let vias: Vec<(f64, f64)> = r.items.iter().filter_map(|i| if let Item::Via(v) = i { Some((v.at.x as f64 / MM, v.at.y as f64 / MM)) } else { None }).collect();
        let tag = format!("P y {py}, R3 {r3:?}: vias {vias:?}, tracks {ts:?}: {}", summary(&r));
        eprintln!("R5SHARE {tag}");
        // F.Cu pieces on x = 12 from the end on (y >= 5): (lo, hi)
        let on: Vec<(f64, f64)> = ts.iter().filter(|t| t.0 == 0 && (t.1 .0 - 12.0).abs() < 1e-6 && (t.2 .0 - 12.0).abs() < 1e-6).map(|t| (t.1 .1.min(t.2 .1).max(5.0), t.1 .1.max(t.2 .1))).filter(|s| s.1 > 5.0 + 1e-3).collect();
        let inside = on.iter().enumerate().any(|(i, a)| on.iter().enumerate().any(|(j, b)| i != j && b.0 >= a.0 - 1e-6 && b.1 <= a.1 + 1e-6 && (b.1 - b.0) < (a.1 - a.0) - 1e-3));
        if r.routed != 2 || !r.violations.is_empty() || inside {
            bad.push(format!("{tag}: overlapping leads {inside} {on:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
