//! Same-net Steiner T-junctions (M4a task A5, M4-D3; design note
//! `docs/progress/notes/m4-capacity-steiner-design.md` §2): a connection may end on the copper
//! of a routed sibling of its net (or on kept hand copper of its target cluster) instead of at
//! its pad. Fixtures: the dcsgonebadge USB corner crop (`fixtures/dcsgone-usb-crop.json`, see
//! `search_entry.rs`) and a synthetic one-layer board with a hand track.
use topor_engine::check::Checker;
use topor_engine::decompose::{decompose, order};
use topor_engine::geom::quant::TrackPrim;
use topor_engine::geom::{dequantise, Prim};
use topor_engine::model::{BoardModel, ExistingCopper, Item, Options};
use topor_engine::route::{route, ConnReport, RouteResult};

const MM: f64 = 1_000_000.0;
const CROP: &str = include_str!("fixtures/dcsgone-usb-crop.json");
const EPROM: &str = include_str!("fixtures/eprom-to-kernal-ripped.json");

/// Hand geometry of #207 / #208 on snapshot `3e7e66f862a7` (B.Cu): the USB+ pads U7.6, J1.A6
/// and J1.B6 meet in one T at (90.075, 123.375) on the trace that clears J1.A7's tip (y 123.68)
/// by c + w/2 = 0.3 mm. Branch lengths to the T: U7.6 1.389 mm, J1.A6 1.102 mm, J1.B6 1.824 mm
/// (4.315 mm in all). No T on that trace lies closer than 1.375 mm to U7.6's centre, so the
/// plan's "<= 1.2 mm" (from the M3 estimate of a 0.9 mm hand T) is not reachable from U7.6; the
/// bounds here are 1.2 x the hand branch of the joining pad and 1.05 x the hand tree.
const HAND_U7_6_NM: f64 = 1_389_000.0;
const HAND_B6_NM: f64 = 1_824_000.0;
const HAND_TREE_NM: f64 = 4_315_000.0;

fn opts() -> Options {
    Options {
        wall_clock_s: 600.0,
        threads: 1,
        ..Options::default()
    }
}

/// The crop routed for its USB nets only (every other pad stays an obstacle): the #207 / #208
/// situation without the crop's unrelated failing connections.
fn usb() -> Options {
    let mut o = opts();
    o.scope.nets = vec!["USB+".into(), "USB-".into()];
    o
}

fn crop() -> BoardModel {
    BoardModel::from_json(CROP).expect("crop fixture")
}

fn clean(m: &BoardModel, r: &RouteResult) {
    let o = Options {
        margin_nm: 0,
        ..Options::default()
    };
    let rep = Checker::new(m, &o).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

fn conn<'a>(r: &'a RouteResult, name: &str, a: &str, b: &str) -> &'a ConnReport {
    r.connections
        .iter()
        .find(|c| {
            c.name == name
                && ((c.from.label == a && c.to.label == b)
                    || (c.from.label == b && c.to.label == a))
        })
        .unwrap_or_else(|| panic!("{name} {a}-{b}"))
}

/// Tracks of connection `id` (KiCad frame).
fn tracks(r: &RouteResult, id: usize) -> Vec<(u32, TrackPrim)> {
    r.items
        .iter()
        .zip(&r.item_conn)
        .filter(|(_, &c)| c == id)
        .filter_map(|(it, _)| match it {
            Item::Track(t) => Some((t.layer, t.prim)),
            Item::Via(_) => None,
        })
        .collect()
}

/// FNV-1a 64 of a string (stable digest of a result).
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Geometry + per-connection outcome of a result (independent of the A5 report fields).
fn digest(r: &RouteResult) -> u64 {
    let items = serde_json::to_string(&r.items).unwrap();
    let rows: Vec<String> = r
        .connections
        .iter()
        .map(|c| {
            format!(
                "{}:{}:{}:{}:{}",
                c.id, c.status, c.length_nm, c.vias, c.reason
            )
        })
        .collect();
    fnv(&format!("{items}|{}|{}", rows.join(";"), r.iterations))
}

/// #207 / #208 of the M3 detour diagnosis: USB+ U7.6 -> J1.A6 and J1.A6 -> J1.B6 share J1.A6's
/// single exit past J1.A7 (#207 took 2 escape vias, 5.55 mm, in M3). With the junction rule one
/// of them ends on the other's trace in a T (the hand's geometry): no via, the T within 1.2 x the
/// hand branch of its pad, the pair within 1.05 x the hand tree and shorter than pad to pad, the
/// sibling's track split at the T, the whole result self-check clean.
#[test]
fn t_junction_onto_routed_sibling() {
    let m = crop();
    let r = route(&m, &usb());
    let u = conn(&r, "USB+", "U7.6", "J1.A6");
    let a = conn(&r, "USB+", "J1.A6", "J1.B6");
    assert_eq!((u.status.as_str(), a.status.as_str()), ("routed", "routed"));
    assert_eq!((u.vias, a.vias), (0, 0));
    assert!(u.junction != a.junction, "exactly one T: {u:?} {a:?}");
    let (t, sib, hand) = if u.junction {
        (u, a, HAND_U7_6_NM)
    } else {
        (a, u, HAND_B6_NM)
    };
    assert!(
        (t.length_nm as f64) <= 1.2 * hand,
        "T {} nm vs hand branch {hand}",
        t.length_nm
    );
    let pair = (u.length_nm + a.length_nm) as f64;
    assert!(
        pair <= 1.05 * HAND_TREE_NM,
        "pair {pair} vs hand tree {HAND_TREE_NM}"
    );
    let ang = t.junction_angle_deg.expect("angle");
    assert!(ang >= 60.0, "{ang}");
    // The T's end lies on the sibling's copper, and the sibling has a track vertex there.
    let end = tracks(&r, t.id)
        .last()
        .map(|x| x.1.end())
        .expect("T tracks");
    let st = tracks(&r, sib.id);
    assert!(
        st.iter().any(|(_, p)| p.start() == end || p.end() == end),
        "sibling not split at {end:?}"
    );
    assert!(st.iter().any(|(_, p)| {
        let q = end.to_f64();
        match dequantise(p) {
            Prim::Seg(s) => topor_engine::geom::point_seg(q, &s) <= 2.0,
            Prim::Arc(c) => topor_engine::geom::point_arc(q, &c) <= 2.0,
        }
    }));
    clean(&m, &r);
    // Pad to pad (steiner off) routes no more. Since A6 (exit stubs, 3cddcc9) USB- U7.4 -> J1.B7
    // routes pad to pad and takes the room U7.6 -> J1.A6 needs (self-check, the same on the
    // A6-only engine), so with both USB nets live the pad-to-pad pair is incomplete (7 of 8);
    // the T routes all 8.
    let off = route(
        &m,
        &Options {
            steiner: false,
            ..usb()
        },
    );
    assert!(off.connections.iter().all(|c| !c.junction));
    assert!(r.routed >= off.routed, "{} < {}", r.routed, off.routed);
    // The pad-to-pad pair (both routed with USB- only an obstacle) is longer than the T pair.
    let mut plus = usb();
    plus.steiner = false;
    plus.scope.nets = vec!["USB+".into()];
    let offp = route(&m, &plus);
    let (u0, a0) = (
        conn(&offp, "USB+", "U7.6", "J1.A6"),
        conn(&offp, "USB+", "J1.A6", "J1.B6"),
    );
    assert_eq!(
        (u0.status.as_str(), a0.status.as_str()),
        ("routed", "routed")
    );
    assert!(!u0.junction && !a0.junction);
    assert!(pair < (u0.length_nm + a0.length_nm) as f64);
}

/// A one-layer board with a hand track A1 -> A2 (kept, `existing_copper = fixed`) and a third
/// pad A3 of the same net 5 mm off the track's middle. The decomposition joins A3 to the nearest
/// port (a pad, 11.2 mm away); the junction rule lets it end on the hand track instead (~5 mm).
fn hand_board() -> BoardModel {
    let pad = |n: &str, x: f64, y: f64| {
        format!(
            r#"{{ "uuid": "A{n}", "ref": "A", "number": "{n}", "net": 1, "center": [{}, {}],
              "layers": [0], "shapes": {{ "0": {{ "kind": "circle", "d": 1000000 }} }} }}"#,
            (x * MM) as i64,
            (-y * MM) as i64
        )
    };
    let seg = |a: (i64, i64), b: (i64, i64)| {
        format!(
            r#"{{ "kind": "seg", "a": [{}, {}], "b": [{}, {}] }}"#,
            a.0 * 1_000_000,
            a.1 * 1_000_000,
            b.0 * 1_000_000,
            b.1 * 1_000_000
        )
    };
    let outline = [
        seg((0, 0), (40, 0)),
        seg((40, 0), (40, -40)),
        seg((40, -40), (0, -40)),
        seg((0, -40), (0, 0)),
    ]
    .join(",");
    let s = format!(
        r#"{{ "layers": [ {{ "id": 0, "name": "F.Cu" }} ],
          "nets": [ {{ "code": 1, "name": "N1" }} ],
          "pads": [ {}, {}, {} ],
          "kept": [ {{ "uuid": "k1", "net": 1, "layer": 0, "width": 250000,
            "prim": {{ "kind": "seg", "a": [10000000, -20000000], "b": [30000000, -20000000] }} }} ],
          "outline": [ {outline} ],
          "rules": {{ "default": {{ "clearance": 200000, "width": 250000 }}, "min_clearance": 0,
            "hole_clearance": 250000, "edge_clearance": 500000 }} }}"#,
        pad("1", 10.0, 20.0),
        pad("2", 30.0, 20.0),
        pad("3", 20.0, 25.0),
    );
    BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{e}\n{s}"))
}

#[test]
fn t_junction_onto_hand_copper() {
    let m = hand_board();
    let o = Options {
        existing_copper: ExistingCopper::Fixed,
        ..opts()
    };
    let r = route(&m, &o);
    assert_eq!(r.connections_total, 1, "{:?}", r.connections);
    let c = &r.connections[0];
    assert_eq!(c.status, "routed", "{c:?}");
    assert!(c.junction, "{c:?}");
    assert!((c.length_nm as f64) < 5.5 * MM, "{}", c.length_nm);
    assert!(c.junction_angle_deg.unwrap_or(0.0) >= 60.0);
    // the T ends on the hand track's centreline (y = -20 mm, KiCad frame)
    let end = tracks(&r, c.id).last().map(|t| t.1.end()).unwrap();
    assert!(
        (end.y + 20_000_000).abs() <= 2 && end.x > 10_500_000 && end.x < 29_500_000,
        "{end:?}"
    );
    clean(&m, &r);
    // Off: the pad-to-pad connection of M3.
    let off = route(
        &m,
        &Options {
            steiner: false,
            ..o.clone()
        },
    );
    assert_eq!(off.connections[0].status, "routed");
    assert!(!off.connections[0].junction);
    assert!(off.connections[0].length_nm as f64 > 11.0 * MM);
}

/// `steiner = false` runs no junction code path: the result digests (items, per-connection
/// status / length / vias / reason, iterations) equal those of the engines without A5, and no
/// connection reports a junction. Evidence for the pins (TE, the `probe` digest of this file on
/// each engine, threads 1): main 946530c (before A5 / A6) crop `0xc58dd349af135e63`, eprom
/// `0x4bbf8e934c970d2f`; A6 alone (3cddcc9: exit stubs, U7 bridges, touching same-net pads one
/// cluster, crop 41 -> 37 connections) crop `0xf913eefd138a3898`, which the A5 merge (a233d62)
/// reproduces with steiner off; A16 (escalating rip cost) + A13 (via relaxation, 6ff3d00) crop
/// `0xeb872b371095cb1d`, and 6ff3d00 with `via_relaxation = false, ripup_growth = 1,
/// ripup_max_per_pair = 1000` (the pre-A16 constant rip cost, no backstop) gives `0xf913...`
/// again, so that change is theirs; A14 (ea1f064) and TE leave both unchanged. The eprom digest
/// is the same on every engine (no stubs, no rips, no via to relax).
#[test]
fn steiner_off_is_m3_behaviour() {
    // The pins predate the blocker rip pass and the continuation rounds: compare without them.
    let off = Options {
        steiner: false,
        rip_pass: false,
        continue_rounds: 0,
        ..opts()
    };
    // The ea1f064 pin was taken at that engine's default `ripup_growth = 2.0` (A-C24 moved the
    // default to 1.25 afterwards): compare at the growth the pin was taken with.
    let rc = route(
        &crop(),
        &Options {
            ripup_growth: 2.0,
            ..off.clone()
        },
    );
    assert!(rc.connections.iter().all(|c| !c.junction));
    let re = route(&BoardModel::from_json(EPROM).unwrap(), &off);
    assert!(re.connections.iter().all(|c| !c.junction));
    assert_eq!(
        (digest(&rc), digest(&re)),
        (CROP_DIGEST_EA1F064, EPROM_DIGEST_946530C),
        "steiner off differs from the engine without A5"
    );
    // With A16 / A13 neutralised: the A6-only engine, bit for bit.
    let a6 = Options {
        via_relaxation: false,
        ripup_growth: 1.0,
        ripup_max_per_pair: 1000,
        ..off
    };
    let r6 = route(&crop(), &a6);
    assert!(r6.connections.iter().all(|c| !c.junction));
    assert_eq!(
        digest(&r6),
        CROP_DIGEST_3CDDCC9,
        "steiner off differs from A6 alone"
    );
}
const CROP_DIGEST_EA1F064: u64 = 0xeb87_2b37_1095_cb1d;
const CROP_DIGEST_3CDDCC9: u64 = 0xf913_eefd_138a_3898;
const EPROM_DIGEST_946530C: u64 = 0x4bbf_8e93_4c97_0d2f;

/// A junction whose final chord meets the sibling below `steiner_min_angle_deg` is refused: the
/// default run has a T below 85 deg; with the limit at 85 every reported junction meets it (that
/// T is gone or replaced by a steeper one), and the routing stays self-check clean.
#[test]
fn junction_angle_below_60_is_rejected() {
    let m = crop();
    let r60 = route(&m, &usb());
    let ts: Vec<&ConnReport> = r60.connections.iter().filter(|c| c.junction).collect();
    assert!(!ts.is_empty());
    assert!(ts
        .iter()
        .all(|c| c.junction_angle_deg.unwrap_or(0.0) >= 60.0));
    const LIM: f64 = 85.0;
    assert!(
        ts.iter().any(|c| c.junction_angle_deg.unwrap() < LIM),
        "the limit must bind: {ts:?}"
    );
    let mut o = usb();
    o.expert.steiner_min_angle_deg = LIM;
    let r = route(&m, &o);
    let shallow: Vec<&ConnReport> = r
        .connections
        .iter()
        .filter(|c| c.junction && c.junction_angle_deg.unwrap_or(0.0) < LIM)
        .collect();
    assert!(shallow.is_empty(), "{shallow:?}");
    clean(&m, &r);
}

/// Threads 1 / 2 / 4 and a repeat give the same result with junctions in play.
#[test]
fn steiner_threads_byte_identical() {
    let m = crop();
    let json = |t: usize| {
        let mut v = serde_json::to_value(route(
            &m,
            &Options {
                threads: t,
                ..usb()
            },
        ))
        .unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("runtime_s");
        o.remove("threads_used");
        serde_json::to_string(&v).unwrap()
    };
    let one = json(1);
    assert!(one.contains("\"junction\":true"));
    assert_eq!(one, json(2));
    assert_eq!(one, json(4));
    assert_eq!(one, json(1));
}

/// M4-D3 net ordering: `Decomposition::net_rank` ranks a net's connections by `est_len` (short
/// spokes first) and the routing order keeps that rank inside every net of one tier and width,
/// so the longer connections of a net come after the siblings they may attach to.
#[test]
fn net_rank_is_shorter_first_within_a_net() {
    let m = crop();
    let mut layers: Vec<u32> = m
        .layers
        .iter()
        .filter(|l| l.routable)
        .map(|l| l.id)
        .collect();
    layers.sort_unstable();
    let dec = decompose(&m, &opts(), &layers);
    assert_eq!(dec.net_rank.len(), dec.connections.len());
    assert_eq!(dec.conn_clusters.len(), dec.connections.len());
    let ord = order(&dec.connections, &[]);
    let mut multi = 0;
    for n in &dec.nets {
        let mut by_rank = n.connections.clone();
        by_rank.sort_by_key(|&c| dec.net_rank[c]);
        for w in by_rank.windows(2) {
            let (a, b) = (&dec.connections[w[0]], &dec.connections[w[1]]);
            assert!(a.est_len <= b.est_len, "{} {:?}", n.name, w);
        }
        let in_order: Vec<usize> = ord
            .iter()
            .copied()
            .filter(|c| n.connections.contains(c))
            .collect();
        let first = &dec.connections[n.connections.first().copied().unwrap_or(0)];
        let one_class = n.connections.iter().all(|&c| {
            let x = &dec.connections[c];
            (x.tier, x.width) == (first.tier, first.width)
        });
        if one_class && n.connections.len() > 1 {
            multi += 1;
            assert_eq!(in_order, by_rank, "{}", n.name);
        }
    }
    assert!(multi > 0);
}
