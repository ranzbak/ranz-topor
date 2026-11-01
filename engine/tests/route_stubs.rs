//! A6 + NX: pad exit stubs (off-centre starts, necking) on the dcsgonebadge USB corner (U1 / U7
//! UDFN-10 0.5 mm pitch / J1 USB-C), cropped from the harness model export of snapshot
//! `3e7e66f862a7` (`fixtures/crop_model.py`, see `search_entry.rs`).
//!
//! The U7 middle pads (0.625 × 0.25 mm, 0.15 mm from the 0.45 mm GND pads U7.3 / U7.8) cannot be
//! left from their centre at 0.2 mm clearance (A2 "Key finding"); the DRC-clean exit is a stub
//! whose centre line runs 30–70 µm off the pad centre (KiCad y ∈ [121.530, 121.570] for U7.4).
use topor_engine::check::Checker;
use topor_engine::decompose::{decompose, Connection, TermKind};
use topor_engine::model::{BoardModel, Item, Options, PadAttr, Scope};
use topor_engine::route::{route, RouteResult};
use topor_engine::stubs::{plan_stubs, ExitStub, StubPlan};

const FIXTURE: &str = include_str!("fixtures/dcsgone-usb-crop.json");

fn model() -> BoardModel {
    BoardModel::from_json(FIXTURE).expect("fixture")
}

fn routable(m: &BoardModel) -> Vec<u32> {
    let mut v: Vec<u32> = m
        .layers
        .iter()
        .filter(|l| l.routable)
        .map(|l| l.id)
        .collect();
    v.sort_unstable();
    v
}

fn conns(m: &BoardModel, opts: &Options) -> Vec<Connection> {
    decompose(m, opts, &routable(m))
        .connections
        .into_iter()
        .filter(|c| !(c.from.layers.is_empty() || c.to.layers.is_empty()))
        .collect()
}

fn plan(m: &BoardModel, opts: &Options) -> StubPlan {
    plan_stubs(m, opts, &conns(m, opts), &routable(m), &|| false)
}

fn pad_index(m: &BoardModel, label: &str) -> usize {
    m.pads
        .iter()
        .position(|p| format!("{}.{}", p.ref_, p.number) == label)
        .unwrap_or_else(|| panic!("no pad {label}"))
}

fn stub_of<'a>(m: &BoardModel, p: &'a StubPlan, label: &str) -> Option<&'a ExitStub> {
    p.by_pad.get(&pad_index(m, label)).map(|&i| &p.stubs[i])
}

/// The stub (and its full-width end disc) against the board at margin 0: no violation.
fn assert_clean(m: &BoardModel, s: &ExitStub) {
    let checker = Checker::new(
        m,
        &Options {
            margin_nm: 0,
            ..Options::default()
        },
    );
    let rep = checker.check_items(&[s.item(None)]);
    assert!(
        rep.violations.is_empty(),
        "{}: {:?}",
        s.label,
        rep.violations
    );
}

fn width_of(m: &mut BoardModel, net: &str, w: i64) {
    let code = m.nets.iter().find(|n| n.name == net).expect("net").code;
    m.rules
        .per_net
        .entry(code)
        .or_insert_with(|| m.rules.default.clone())
        .width = w;
}

fn usb_scope(nets: &[&str]) -> Options {
    Options {
        scope: Scope {
            nets: nets.iter().map(|s| s.to_string()).collect(),
            ..Scope::default()
        },
        wall_clock_s: 60.0,
        threads: 1,
        ..Options::default()
    }
}

fn conn_report<'a>(
    r: &'a RouteResult,
    from: &str,
    to: &str,
) -> &'a topor_engine::route::ConnReport {
    r.connections
        .iter()
        .find(|c| {
            (c.from.label == from && c.to.label == to) || (c.from.label == to && c.to.label == from)
        })
        .unwrap_or_else(|| panic!("no connection {from} -> {to}"))
}

/// Every output item against the board (margin 0) and against each other.
fn assert_items_clean(m: &BoardModel, r: &RouteResult) {
    let checker = Checker::new(
        m,
        &Options {
            margin_nm: 0,
            ..Options::default()
        },
    );
    let rep = checker.check_items(&r.items);
    assert!(
        rep.violations.is_empty(),
        "violations: {:?}",
        rep.violations
    );
}

#[test]
fn stub_generated_for_unfit_fine_pitch_pad() {
    let m = model();
    let opts = Options::default();
    assert_eq!(opts.exit_stubs, "auto");
    let p = plan(
        &m,
        &Options {
            pad_bridge_max_nm: 0,
            ..opts.clone()
        },
    );
    // U7.4 (USB-) and U7.2 (J1-CC2) and their partners U7.7 / U7.9 across the package: centre
    // exit 0.175 mm from the GND pads -> off-centre stub out of the row.
    for label in ["U7.4", "U7.2", "U7.7", "U7.9"] {
        let s =
            stub_of(&m, &p, label).unwrap_or_else(|| panic!("{label}: no stub ({:?})", p.warnings));
        assert_eq!(s.width, 200_000, "{label}: 0.2 mm net, not necked");
        assert!(!s.necked());
        let off = s.offset.abs();
        assert!(
            (30_000.0..=70_000.0).contains(&off),
            "{label}: offset {off} nm outside the corridor"
        );
        assert_clean(&m, s);
        // the end lies outside the pad row: x beyond the pad tip (UDFN columns at x 90.1 / 90.9)
        let pad = &m.pads[s.pad];
        let c = pad.center.to_vec2();
        assert!(
            (s.end - c).len() > 312_500.0 + 200_000.0,
            "{label}: end inside the row"
        );
    }
    // KiCad frame: U7.4's stub centre line at y in [121.530, 121.570] mm.
    let s = stub_of(&m, &p, "U7.4").expect("U7.4");
    let y = -s.end.y;
    assert!(
        (121_530_000.0..=121_570_000.0).contains(&y),
        "U7.4 stub at y {y}"
    );
    assert!(-s.start.y >= 121_530_000.0 && -s.start.y <= 121_570_000.0);
    // THT / roomy pads get none in auto mode.
    assert!(
        stub_of(&m, &p, "U1.13").is_none(),
        "U1.13 exits from its centre"
    );

    // Routed through the stubs: the connections the pad centre could not start (A2: BLOCKED at
    // 0.2 mm) now route, DRC-clean, and report their stub length.
    let r = route(&m, &usb_scope(&["USB-", "Net-(J1-CC2)"]));
    for (a, b) in [
        ("U7.4", "U7.7"),
        ("U7.2", "U7.9"),
        ("U7.9", "J1.B5"),
        ("U1.13", "U7.4"),
    ] {
        let c = conn_report(&r, a, b);
        assert_eq!(c.status, "routed", "{a} -> {b}: {}", c.reason);
        assert!(c.stub_nm > 0, "{a} -> {b}: no stub length");
        assert!(c.length_nm >= c.stub_nm);
    }
    // the stub items are on the board once, starting inside their pads
    let stub_items = r
        .items
        .iter()
        .filter(|it| match it {
            Item::Track(t) => r.stubs.iter().any(|s| {
                let pr = t.prim;
                s.width == t.width
                    && pr.start().x as i64 == s.start[0]
                    && pr.start().y as i64 == s.start[1]
                    && pr.end().x as i64 == s.end[0]
                    && pr.end().y as i64 == s.end[1]
            }),
            _ => false,
        })
        .count();
    let used = r.stubs.iter().filter(|s| s.used).count();
    assert!(used >= 4, "stubs used {used}");
    assert_eq!(stub_items, used, "every used stub exactly once");
    assert_items_clean(&m, &r);
}

#[test]
fn stub_skipped_when_infeasible() {
    // USB+ at 0.5 mm: J1.A6 (0.3 mm pad, 0.2 mm from A7 / B7) cannot be left at full width,
    // and without necking no stub is feasible either.
    let mut m = model();
    width_of(&mut m, "USB+", 500_000);
    let opts = Options {
        neck_to_pad: false,
        ..Options::default()
    };
    let p = plan(&m, &opts);
    assert!(stub_of(&m, &p, "J1.A6").is_none());
    assert!(
        p.warnings.iter().any(|w| w == "stub infeasible: J1.A6"),
        "warnings {:?}",
        p.warnings
    );
    // the connection keeps its pad terminal (M3), and the reason surfaces in the route result
    let r = route(
        &m,
        &Options {
            neck_to_pad: false,
            ..usb_scope(&["USB+"])
        },
    );
    assert!(
        r.warnings.iter().any(|w| w == "stub infeasible: J1.A6"),
        "{:?}",
        r.warnings
    );
    for c in r
        .connections
        .iter()
        .filter(|c| c.from.label == "J1.A6" || c.to.label == "J1.A6")
    {
        assert_eq!(c.stub_nm, 0);
    }
}

#[test]
fn bridge_joins_same_net_pads_of_a_footprint_off_centre() {
    // U7.4 -> U7.7 (USB-) and U7.2 -> U7.9 (CC2) face each other across the UDFN; the centre
    // line passes 0.175 mm from U7.3 / U7.8 (GND), the line 50 µm off it is clean: one track.
    let m = model();
    let p = plan(&m, &Options::default());
    let cs = conns(&m, &Options::default());
    for (a, b) in [("U7.4", "U7.7"), ("U7.2", "U7.9")] {
        let c = cs
            .iter()
            .find(|c| {
                (c.from.label == a && c.to.label == b) || (c.from.label == b && c.to.label == a)
            })
            .expect("connection");
        let &k = p
            .bridges
            .get(&c.id)
            .unwrap_or_else(|| panic!("{a} -> {b}: no bridge"));
        let s = &p.stubs[k];
        assert!(s.bridge.is_some());
        assert!(
            (30_000.0..=70_000.0).contains(&s.offset.abs()),
            "offset {}",
            s.offset
        );
        assert!((s.len() - 775_000.0).abs() < 1.0, "length {}", s.len());
        assert_clean(&m, s);
    }
    // U7.5 -> U7.6 / U7.1 -> U7.10 leave from their centres (T0 clean): no bridge in auto mode
    assert_eq!(p.bridges.len(), 2, "bridges {:?}", p.bridges);
    // off: none
    let p0 = plan(
        &m,
        &Options {
            pad_bridge_max_nm: 0,
            ..Options::default()
        },
    );
    assert!(p0.bridges.is_empty());
    // the routed result: the bridged connections are that one track, 0 vias, ratio 1
    let r = route(&m, &usb_scope(&["USB-", "Net-(J1-CC2)"]));
    for (a, b) in [("U7.4", "U7.7"), ("U7.2", "U7.9")] {
        let c = conn_report(&r, a, b);
        assert_eq!(
            (c.status.as_str(), c.vias, c.length_nm),
            ("routed", 0, 775_000)
        );
        assert_eq!(c.ghost_len_nm, Some(775_000));
    }
    assert!(r
        .stubs
        .iter()
        .filter(|s| s.bridge.is_some())
        .all(|s| s.used));
    assert_items_clean(&m, &r);
}

#[test]
fn touching_same_net_pads_are_one_cluster() {
    // J1.A4 / J1.B9 (and A9 / B4) of the USB-C footprint are the same copper: no connection
    // between them (before: a zero-length connection that failed "sketch: inconsistent slot path").
    let m = model();
    let cs = conns(&m, &Options::default());
    for (a, b) in [("J1.A4", "J1.B9"), ("J1.A9", "J1.B4")] {
        assert!(
            !cs.iter().any(|c| (c.from.label == a && c.to.label == b)
                || (c.from.label == b && c.to.label == a)),
            "{a} -> {b} still a connection"
        );
    }
    assert!(cs
        .iter()
        .all(|c| (c.to.at - c.from.at).len() > 0.0 || c.from.label == c.to.label));
}

#[test]
fn neck_to_pad_narrows_wide_net_at_fine_pitch_pad() {
    let mut m = model();
    width_of(&mut m, "USB+", 500_000);
    let p = plan(&m, &Options::default());
    let s = stub_of(&m, &p, "J1.A6").unwrap_or_else(|| panic!("no neck ({:?})", p.warnings));
    assert!(s.necked());
    assert_eq!(s.net_width, 500_000);
    assert!(
        s.width >= m.rules.min_track_width && s.width < 500_000,
        "neck {}",
        s.width
    );
    // what fits between A7 / B7 (0.2 mm gaps to a 0.3 mm pad): wider than the pad, < 0.3 + 2 * 0
    assert!(
        s.width >= 250_000 && s.width <= 300_000,
        "neck width {}",
        s.width
    );
    assert!(
        s.len() <= 2.0 * 2.0 * 1_450_000.0,
        "neck {} nm long",
        s.len()
    );
    assert_eq!(s.width % 1_000, 0, "whole µm");
    assert_clean(&m, s);
    // a custom-rule minimum above what fits forbids the neck
    let code = m.nets.iter().find(|n| n.name == "USB+").expect("net").code;
    m.rules.per_net.get_mut(&code).expect("rule").min_width = 350_000;
    let p = plan(&m, &Options::default());
    assert!(stub_of(&m, &p, "J1.A6").is_none_or(|s| s.width >= 350_000));
    // neck_max caps the necked length
    m.rules.per_net.get_mut(&code).expect("rule").min_width = 0;
    let p = plan(
        &m,
        &Options {
            neck_max_nm: 100_000,
            ..Options::default()
        },
    );
    assert!(
        stub_of(&m, &p, "J1.A6").is_none(),
        "neck longer than neck_max"
    );
}

#[test]
fn exit_stubs_off_matches_previous() {
    // Golden digest of the pre-A6 engine (main 946530c) on this fixture and options: FNV-1a 64
    // of the canonical JSON (sorted keys, no spaces) of `items`, plus the connection outcomes.
    // The features merged after A6 that act on this fixture are off as well (TE): A5 Steiner
    // T-junctions (U7.6 -> J1.A6 ends on a sibling, 1.39 mm instead of 2.41 mm pad to pad) and
    // A13 via relaxation (moves the vias of connections 1 and 6). Until TE the A5 T here failed
    // in its metrization and the default-option result still equalled this golden.
    let m = model();
    let opts = Options {
        exit_stubs: "off".into(),
        steiner: false,
        via_relaxation: false,
        ..usb_scope(&["USB-", "USB+", "Net-(J1-CC2)", "Net-(J1-CC1)"])
    };
    let p = plan(&m, &opts);
    assert!(p.is_empty() && p.warnings.is_empty());
    let r = route(&m, &opts);
    assert!(r.stubs.is_empty());
    assert!(r
        .connections
        .iter()
        .all(|c| c.stub_nm == 0 && c.neck_nm == 0));
    let items =
        serde_json::to_string(&serde_json::to_value(&r.items).expect("json")).expect("json");
    let outcome: Vec<String> = r
        .connections
        .iter()
        .map(|c| format!("{}:{}:{}", c.id, c.status, c.length_nm))
        .collect();
    let digest = fnv(items.as_bytes());
    assert_eq!(
        (digest, r.routed, r.vias, outcome.join(",")),
        (
            GOLDEN_OFF.0,
            GOLDEN_OFF.1,
            GOLDEN_OFF.2,
            GOLDEN_OFF.3.to_string()
        ),
        "items {} bytes",
        items.len()
    );
}

/// Pre-A6 engine result of `exit_stubs_off_matches_previous` (computed with main 946530c).
const GOLDEN_OFF: (u64, usize, usize, &str) = (
    0xe349_cfb9_38ae_6cde,
    7,
    4,
    "0:routed:775000,1:routed:5331310,2:failed:0,3:failed:0,4:failed:0,5:routed:2710952,\
     6:routed:24075109,7:routed:775000,8:routed:2408927,9:routed:2710952,10:failed:0,11:failed:0",
);

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h ^= u64::from(x);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[test]
fn exit_stubs_all_on_every_smd_pad() {
    let m = model();
    let opts = Options {
        exit_stubs: "all".into(),
        ..Options::default()
    };
    let cs = conns(&m, &opts);
    let p = plan_stubs(&m, &opts, &cs, &routable(&m), &|| false);
    let mut pads: Vec<usize> = cs
        .iter()
        .flat_map(|c| [&c.from, &c.to])
        .filter_map(|t| match t.kind {
            TermKind::Pad(pi) if m.pads[pi].attr == PadAttr::Smd => Some(pi),
            _ => None,
        })
        .collect();
    pads.sort_unstable();
    pads.dedup();
    assert!(pads.len() >= 20, "{} SMD terminal pads", pads.len());
    let mut stubbed = 0;
    for &pi in &pads {
        let pad = &m.pads[pi];
        let label = format!("{}.{}", pad.ref_, pad.number);
        match p.by_pad.get(&pi) {
            Some(&k) => {
                stubbed += 1;
                assert_clean(&m, &p.stubs[k]);
            }
            None => {
                // only a pad without a long axis (round / square) or an infeasible one has none
                let sq = pad.shapes.values().all(|s| match s {
                    topor_engine::model::PadShape::Circle { .. } => true,
                    topor_engine::model::PadShape::Rect { w, h }
                    | topor_engine::model::PadShape::RoundRect { w, h, .. }
                    | topor_engine::model::PadShape::Oval { w, h } => (w - h).abs() <= 1_000,
                    _ => false,
                });
                let warned = p
                    .warnings
                    .iter()
                    .any(|w| *w == format!("stub infeasible: {label}"));
                // every connection of the pad is a bridge (routed by that one track)
                let bridged = cs
                    .iter()
                    .filter(|c| c.from.label == label || c.to.label == label)
                    .all(|c| p.bridges.contains_key(&c.id));
                assert!(sq || warned || bridged, "{label}: no stub and no warning");
                if bridged {
                    stubbed += 1;
                }
            }
        }
    }
    assert!(
        stubbed * 2 > pads.len(),
        "{stubbed} of {} SMD pads stubbed ({:?})",
        pads.len(),
        p.warnings
    );
    // auto stubs a strict subset
    let auto = plan(&m, &Options::default());
    assert!(auto.stubs.len() < p.stubs.len());
    // the routing pass uses them: every connection between two stubbed pads starts at a stub end
    let r = route(
        &m,
        &Options {
            exit_stubs: "all".into(),
            ..usb_scope(&["USB-"])
        },
    );
    let c = conn_report(&r, "U7.4", "U7.7");
    assert_eq!(c.status, "routed", "{}", c.reason);
    assert!(c.stub_nm > 0);
    assert_items_clean(&m, &r);
}

#[test]
fn stubs_threads_byte_identical() {
    let m = model();
    let run = |t: usize| {
        let o = Options {
            threads: t,
            ..usb_scope(&["USB-", "USB+", "Net-(J1-CC2)", "Net-(J1-CC1)"])
        };
        let mut v = serde_json::to_value(route(&m, &o)).expect("json");
        let obj = v.as_object_mut().expect("object");
        obj.remove("runtime_s");
        obj.remove("threads_used");
        serde_json::to_string(&v).expect("json")
    };
    let a = run(1);
    assert!(
        a.contains("\"bridge\":\"U7.7\""),
        "bridge expected in the result"
    );
    assert_eq!(a, run(2));
    assert_eq!(a, run(4));
    assert_eq!(a, run(4));
}
