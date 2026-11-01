//! A2 (M4-D2): fine-pitch pad entry and line-of-sight convergence of the slot search, on the
//! dcsgonebadge USB corner (U1 / U7 UDFN-10 0.5 mm pitch / J1 USB-C 0.5 mm pitch) cropped from the
//! harness model export of snapshot `3e7e66f862a7` (`fixtures/crop_model.py
//! dcsgonebadge--model.json 84 97 100 129`: pads, vias and holes in the window, window outline,
//! no pours / kept copper). Every search runs on the empty graph (static geometry only).
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration, Instant};
use topor_engine::check::Checker;
use topor_engine::decompose::{decompose, Connection, TermKind};
use topor_engine::geom::Vec2;
use topor_engine::graph::{GraphRules, TerminalInfo};
use topor_engine::model::{BoardModel, Options, PadAttr};
use topor_engine::multigraph::MultiGraph;
use topor_engine::negotiate::PRESENT_OVERUSE;
use topor_engine::obstacle::ObstacleSet;
use topor_engine::search::{
    default_expansion_cap, probe_entry, search, tier_weight, via_weight, Goal, SearchOutcome,
    SearchParams, SlotPath,
};
use topor_engine::vias::{via_sites, via_sizes, ViaSize};

const MM: f64 = 1_000_000.0;
const FIXTURE: &str = include_str!("fixtures/dcsgone-usb-crop.json");

struct Env {
    conns: Vec<Connection>,
    obs: ObstacleSet,
    mg: MultiGraph,
    routable: Vec<u32>,
}

fn bits(p: Vec2) -> (u64, u64) {
    (p.x.to_bits(), p.y.to_bits())
}

/// The `route()` setup of `gate_diag` (decompose, terminal vertices, escape via sites,
/// multigraph) on the fixture, every net's clearance set to `clearance` nm when given.
fn env(clearance: Option<i64>) -> Env {
    let mut model = BoardModel::from_json(FIXTURE).expect("fixture");
    if let Some(c) = clearance {
        model.rules.default.clearance = c;
        for r in model.rules.per_net.values_mut() {
            r.clearance = c;
        }
    }
    let opts = Options::default();
    let mut routable: Vec<u32> = model
        .layers
        .iter()
        .filter(|l| l.routable)
        .map(|l| l.id)
        .collect();
    routable.sort_unstable();
    let dec = decompose(&model, &opts, &routable);
    let conns: Vec<Connection> = dec
        .connections
        .iter()
        .filter(|c| !(c.from.layers.is_empty() || c.to.layers.is_empty()))
        .cloned()
        .collect();
    let mut obs = ObstacleSet::build(&model, &opts, &HashSet::new(), &[]);
    let escape: Vec<usize> = conns
        .iter()
        .flat_map(|c| [&c.from, &c.to])
        .filter_map(|t| match t.kind {
            TermKind::Pad(pi) if model.pads[pi].attr == PadAttr::Smd => Some(pi),
            _ => None,
        })
        .collect::<BTreeSet<usize>>()
        .into_iter()
        .collect();
    let mut sizes: Vec<ViaSize> = conns
        .iter()
        .map(|c| c.net)
        .collect::<BTreeSet<i32>>()
        .into_iter()
        .flat_map(|n| via_sizes(&model.rules, n))
        .collect();
    sizes.sort_by_key(|z| std::cmp::Reverse((z.d, z.drill)));
    sizes.dedup();
    let checker = Checker::new(&model, &opts);
    let sites = via_sites(
        &model,
        &obs,
        &checker,
        &routable,
        &escape,
        &sizes,
        opts.margin_nm as f64,
    );
    let mut term_at: BTreeMap<(u64, u64), usize> = BTreeMap::new();
    let mut terms: Vec<(TerminalInfo, Vec<u32>)> = Vec::new();
    for c in &conns {
        for t in [&c.from, &c.to] {
            if matches!(t.kind, TermKind::Pour(_)) {
                continue;
            }
            match term_at.get(&bits(t.at)) {
                Some(&i) => {
                    for l in &t.layers {
                        if !terms[i].1.contains(l) {
                            terms[i].1.push(*l);
                        }
                    }
                }
                None => {
                    term_at.insert(bits(t.at), terms.len());
                    terms.push((
                        TerminalInfo {
                            at: t.at,
                            net: c.net,
                            owner: t.label.clone(),
                            clr: (c.clearance as f64).max(model.rules.min_clearance as f64),
                            r: 0.0,
                        },
                        t.layers.clone(),
                    ));
                }
            }
        }
    }
    let rules = GraphRules {
        eps: opts.margin_nm as f64,
        min_clearance: model.rules.min_clearance as f64,
        edge_clearance: model.rules.edge_clearance as f64,
        hole_clearance: model.rules.hole_clearance as f64,
    };
    let mg = MultiGraph::build_on(&model, &mut obs, sites, &routable, &terms, &rules).expect("mg");
    Env {
        conns,
        obs,
        mg,
        routable,
    }
}

fn conn<'a>(e: &'a Env, from: &str, to: &str) -> &'a Connection {
    e.conns
        .iter()
        .find(|c| {
            (c.from.label == from && c.to.label == to) || (c.from.label == to && c.to.label == from)
        })
        .unwrap_or_else(|| panic!("no connection {from} -> {to}"))
}

fn vertices(e: &Env, at: Vec2, layers: &[u32]) -> Vec<(usize, usize)> {
    e.routable
        .iter()
        .enumerate()
        .filter(|(_, l)| layers.contains(l))
        .filter_map(|(li, _)| e.mg.vertex_of(li, at).map(|v| (li, v)))
        .collect()
}

/// Slot search of `c` alone on the empty graph with at most `max_vias` vias.
fn run(e: &Env, c: &Connection, max_vias: u32) -> SearchOutcome {
    let hist: Vec<Vec<f64>> =
        e.mg.layers
            .iter()
            .map(|l| vec![0.0; l.g.edges.len()])
            .collect();
    let shist = vec![0.0; e.mg.sites.len()];
    let opts = Options::default();
    let p = SearchParams {
        history: &hist,
        site_history: &shist,
        weight: tier_weight(c.tier),
        present_overuse: PRESENT_OVERUSE,
        expansion_cap: default_expansion_cap(&e.mg),
        deadline: Instant::now() + Duration::from_secs(60),
        eps: None,
        via_cost: opts.via_cost_nm as f64 * via_weight(c.tier),
        max_vias,
        strict_unfit_edges: false,
    };
    let start = vertices(e, c.from.at, &c.from.layers);
    let goal = Goal {
        vertices: vertices(e, c.to.at, &c.to.layers),
        pour_sites: Vec::new(),
        pour_layer: None,
    };
    search(&e.mg, &e.obs, c, &start, &goal, &p)
}

fn found(r: SearchOutcome, what: &str) -> SlotPath {
    match r {
        SearchOutcome::Found(p) => p,
        SearchOutcome::Blocked { static_reason, .. } => {
            panic!("{what}: blocked ({static_reason:?})")
        }
        SearchOutcome::Timeout => panic!("{what}: timeout"),
    }
}

fn straight(c: &Connection) -> f64 {
    (c.to.at - c.from.at).len()
}

/// #208 class (M4-D2): leaving a 0.3 mm USB-C pad through its tip with a foreign pad 0.2 mm
/// beside it. Before A2 the state point sat on the own corner, 0.2 mm from the neighbour's
/// corner, and the bend loop of `los` spiralled around the neighbour (apex_len 11.3 mm per
/// tip): J1.A6 -> J1.B6 took 2 escape vias (est 24.9 mm), J1.A7 -> J1.B7 a 0-via 25 mm loop.
/// The hand routes both around A7's / A6's tip in 2.93 mm (0 vias). After: the 0-via path is
/// within 1.3 x 2.93 mm, and every admissible tip exit's string is within 4 x its chord.
#[test]
fn los_nonconverging_bend_loop_is_rejected() {
    let e = env(None);
    let hand = 2.93 * MM;
    for (from, to) in [("J1.A6", "J1.B6"), ("J1.A7", "J1.B7")] {
        let c = conn(&e, from, to);
        let p = found(run(&e, c, 0), &format!("{from} -> {to}"));
        assert_eq!(p.vias(), 0);
        assert!(
            p.est_len <= 1.3 * hand,
            "{from} -> {to}: est {:.3} mm > 1.3 x 2.93 mm",
            p.est_len / MM
        );
        // the default search (vias allowed) takes the direct path too
        let q = found(
            run(&e, c, opts_max_vias()),
            &format!("{from} -> {to} (vias)"),
        );
        assert_eq!(q.vias(), 0, "{from} -> {to}: {} vias", q.vias());
    }
    // the third of the trio: U7.6 -> J1.A6 stays the direct 2.41 mm path
    let c = conn(&e, "U7.6", "J1.A6");
    let p = found(run(&e, c, opts_max_vias()), "U7.6 -> J1.A6");
    assert_eq!(p.vias(), 0);
    assert!(p.est_len <= 1.3 * 2.41 * MM, "est {:.3} mm", p.est_len / MM);
    // every admissible exit of J1.A6 towards B6: apex_len + last chord <= 4 x straight chord
    let c = conn(&e, "J1.A6", "J1.B6");
    let li = 1; // B.Cu
    let g = &e.mg.layers[li].g;
    let obs = e.obs.layer(e.mg.layers[li].layer);
    let v = e.mg.vertex_of(li, c.from.at).expect("A6 vertex");
    let mut tested = 0;
    for &f in g.faces_of_vertex(v) {
        for &ed in &g.faces[f].e {
            let (pt, apex, apex_len, ok) = probe_entry(g, obs, c, v, ed, c.to.at);
            if !ok {
                continue;
            }
            let chord = (pt - c.from.at).len();
            let string = apex_len + (pt - apex).len();
            assert!(
                string <= 4.0 * chord + 1.0,
                "edge {ed}: string {:.3} mm > 4 x chord {:.3} mm",
                string / MM,
                chord / MM
            );
            tested += 1;
        }
    }
    assert!(tested > 0, "no admissible exit of J1.A6");
}

fn opts_max_vias() -> u32 {
    Options::default().max_vias_per_connection
}

/// #201 class: U7 is a UDFN-10 (0.625 x 0.25 mm pads, 0.5 mm pitch) whose middle pads U7.2 /
/// U7.4 / U7.7 / U7.9 sit 0.15 mm from the 0.45 mm GND pads U7.3 / U7.8. A 0.2 mm wire attached
/// at U7.4's centre (90.8875, 121.5) is 0.175 mm from U7.3's copper (the hand board's own DRC
/// flags exactly that: "clearance 0.2000 mm; actual 0.1750 mm"), so at the board's 0.2 mm
/// clearance no exit from the centre is admissible and the search must not report a path the
/// checker rejects (the U7 entry at 0.2 mm needs an off-centre attachment, A6 exit stubs). Before
/// A2 the line of sight of every U7.4 exit ran away around U7.3 / U7.5 (apex_len 5.1-9.4 mm for
/// a 0.2-0.25 mm chord) before `bend_ok` rejected it; now each exit is decided within two bends.
/// At 0.15 mm clearance (the gap the hand layout uses) the entry is feasible: U7.4 -> U7.7 /
/// J1.B7 and U7.2 -> U7.9 route direct with 0 vias within 1.5 x straight, and U1.13 -> U7.4 is
/// found within 1.5 x straight with at most the hand's 2 vias (the hand crosses U1 on F.Cu; the
/// 0-via B.Cu path around U1's pad rows is 1.52 x straight).
#[test]
fn fine_pitch_entry_u7_4_routes_direct() {
    let tight = env(None);
    for (from, to) in [("U7.4", "U7.7"), ("U7.4", "J1.B7"), ("U7.2", "U7.9")] {
        let c = conn(&tight, from, to);
        assert!(
            !matches!(run(&tight, c, 0), SearchOutcome::Found(_)),
            "{from} -> {to} at 0.2 mm clearance: the centre entry is 0.175 mm from U7.3 / U7.8"
        );
    }
    let c = conn(&tight, "U7.4", "U7.7");
    let li = 1; // B.Cu
    let g = &tight.mg.layers[li].g;
    let obs = tight.obs.layer(tight.mg.layers[li].layer);
    let v = tight.mg.vertex_of(li, c.from.at).expect("U7.4 vertex");
    let mut exits = 0;
    for &f in g.faces_of_vertex(v) {
        for &ed in &g.faces[f].e {
            let (pt, apex, apex_len, ok) = probe_entry(g, obs, c, v, ed, c.to.at);
            let chord = (pt - c.from.at).len();
            let string = apex_len + (pt - apex).len();
            if ok {
                assert!(
                    string <= 4.0 * chord + 1.0,
                    "U7.4 edge {ed}: string {:.3} mm for a {:.3} mm chord",
                    string / MM,
                    chord / MM
                );
            } else {
                // decided within LOS_STALL_CAP bends, not after a 5-9 mm spiral
                assert!(
                    apex_len <= 1.0 * MM,
                    "U7.4 edge {ed}: bend loop ran away, apex_len {:.3} mm before rejecting",
                    apex_len / MM
                );
            }
            exits += 1;
        }
    }
    assert!(exits > 0);
    let e = env(Some(150_000));
    for (from, to) in [("U7.4", "U7.7"), ("U7.4", "J1.B7"), ("U7.2", "U7.9")] {
        let c = conn(&e, from, to);
        let p = found(run(&e, c, opts_max_vias()), &format!("{from} -> {to}"));
        assert_eq!(p.vias(), 0, "{from} -> {to}: {} vias", p.vias());
        assert!(
            p.est_len <= 1.5 * straight(c),
            "{from} -> {to}: est {:.3} mm > 1.5 x straight {:.3} mm",
            p.est_len / MM,
            straight(c) / MM
        );
    }
    let c = conn(&e, "U1.13", "U7.4");
    let p = found(run(&e, c, opts_max_vias()), "U1.13 -> U7.4");
    assert!(p.vias() <= 2, "U1.13 -> U7.4: {} vias", p.vias());
    assert!(
        p.est_len <= 1.5 * straight(c),
        "U1.13 -> U7.4: est {:.3} mm > 1.5 x straight {:.3} mm",
        p.est_len / MM,
        straight(c) / MM
    );
}
