//! TW7 (docs/dev/design/tw7-moves.md): the pattern detectors (part A) and the targeted pattern
//! moves of the TopoR loop (part B, `[optimize] pattern_moves`).
//!
//! The board is the user-approved `u_clinch` pattern (2026-11-19; tools/tw7_sketch.py,
//! Luzin 2015 p. 8 b, fig. 19; Luzin, Popov & Popov 2012 p. 117 sign 2, p. 118 removal): a 0.5 mm
//! rail M (net 2, laid first: wider) runs between the two pads of the short 0.25 mm wire N
//! (net 1). N cannot cross M without two vias, so the initial routing lays it round M's lower
//! end (x 5). No single re-lay helps: M alone cannot leave the U N makes round its end, N alone
//! meets the same M. The U-clinch move lays N first (straight), then M round N's pad.

use topor_engine::check::Checker;
use topor_engine::control::RunControl;
use topor_engine::geom::quant::TrackPrim;
use topor_engine::model::{BoardModel, Item, Options};
use topor_engine::route::route_with;
use topor_engine::topo::board::{self, BuildOpts};
use topor_engine::topo::factors::{Registry, WireCtx};
use topor_engine::topo::init::{Conn, InitWeights};
use topor_engine::topo::state::{State, WireId};
use topor_engine::topo::xrules::{self, Kind, Pair};

const MM: f64 = 1e6;
const WS0: f64 = 450_000.0;

/// One SMD pad on F.Cu: net code, centre (mm, KiCad frame), square size (mm).
type P = (i32, f64, f64, f64);

/// 2-layer board `w` x `h` mm with SMD pads `pads`; net `k` has track width `widths[k-1]` (mm).
fn board(w: f64, h: f64, pads: &[P], widths: &[f64]) -> BoardModel {
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
        .enumerate()
        .map(|(i, &(net, x, y, s))| {
            format!(
                r#"{{"uuid":"p{i}","ref":"U{i}","number":"1","net":{net},"attr":"smd","center":[{},{}],
                    "rotation_deg":0.0,"layers":[0],"shapes":{{"0":{{"kind":"rect","w":{},"h":{}}}}},"drill":null}}"#,
                p(x),
                p(y),
                p(s),
                p(s)
            )
        })
        .collect();
    let nets: Vec<String> = (1..=4).map(|k| format!(r#"{{"code":{k},"name":"N{k}"}}"#)).collect();
    let per: Vec<String> = widths
        .iter()
        .enumerate()
        .map(|(i, &wd)| format!(r#""{}":{{"clearance":200000,"width":{},"via_d":600000,"via_drill":300000,"netclass":"C{i}","width_source":"netclass"}}"#, i + 1, p(wd)))
        .collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{}],"pads":[{}],"outline":[{}],
            "rules":{{"min_clearance":200000,"min_track_width":200000,"min_via_diameter":450000,"min_via_drill":250000,
                     "min_annular":100000,"hole_to_hole":250000,"edge_clearance":500000,
                     "default":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000}},
                     "per_net":{{{}}}}}}}"#,
        nets.join(","),
        pj.join(","),
        segs.join(","),
        per.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// The u_clinch pattern (tools/tw7_sketch.py `p_u_clinch`): N (net 1, 0.25 mm) pads (12, 11),
/// (16, 11); M (net 2, 0.5 mm) pads (14, 2.5), (14, 19.5); 1 mm pads; 28 x 22 mm.
fn u_clinch_board() -> BoardModel {
    board(28.0, 22.0, &[(1, 12.0, 11.0, 1.0), (1, 16.0, 11.0, 1.0), (2, 14.0, 2.5, 1.0), (2, 14.0, 19.5, 1.0)], &[0.25, 0.5])
}

fn opts(kinds: &[&str], seed: u64) -> Options {
    Options {
        opt_loop: "topor".into(),
        search_length: "taut".into(),
        steiner: false,
        crossing_rules: false,
        pattern_moves: kinds.iter().map(|k| k.to_string()).collect(),
        seed,
        ..Options::default()
    }
}

/// Drawn length (mm) of net `net`'s straight segments.
fn net_len(items: &[Item], net: i32) -> f64 {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) if t.net == net => match t.prim {
                TrackPrim::Seg { a, b } => Some(((b.x - a.x) as f64).hypot((b.y - a.y) as f64) / MM),
                _ => None,
            },
            _ => None,
        })
        .sum()
}

fn vias(items: &[Item]) -> usize {
    items.iter().filter(|i| matches!(i, Item::Via(_))).count()
}

fn conn_of(conns: &[Conn], net: i32) -> Conn {
    *conns.iter().find(|c| c.net == net).expect("a connection of the net")
}

/// The state of the u_clinch board with the connections of `order` routed one after the other
/// by the plain search (crossings priced, as the initial routing).
fn routed<'p>(b: &'p board::Board, m: &BoardModel, order: &[i32]) -> (State<'p>, Vec<WireId>) {
    let reg = Registry::standard();
    let conns = b.connections(m);
    let mut st = State::new(&b.proj);
    let ws = order
        .iter()
        .map(|&net| {
            let c = conn_of(&conns, net);
            let wt = InitWeights { max_expansions: 2_000_000, ..InitWeights::from_registry(&reg, &WireCtx { net, tier: 3 }) };
            st.route_conn(&c, &wt, (c.w + c.s) / WS0).expect("routes").0
        })
        .collect();
    (st, ws)
}

/// Part A, U-clinch (sign 2) on the careless state: M laid first (straight), N round M's end; N
/// does not cross M but M crosses N's straight pad-to-pad line once: the pattern (N, M), N first.
/// The expected state (N first, straight; M round N's pad) has the U-clinch the other way
/// round, with much less excess length. Detour amounts are graded.
#[test]
fn u_clinch_is_found_on_the_careless_state_with_the_clinched_wire_first() {
    let m = u_clinch_board();
    let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, ..BuildOpts::default() }).unwrap();
    let (st, w) = routed(&b, &m, &[2, 1]);
    let (wm, wn) = (w[0], w[1]);
    assert!(st.crossings(wn).iter().all(|x| x.other != wm), "N goes round M, no crossing");
    let uc = xrules::u_clinches(&st, &|_| false);
    assert_eq!(uc, vec![Pair { first: wn, second: wm, kind: Kind::UClinch }]);
    assert!(xrules::u_clinches(&st, &|x| x == wn).is_empty(), "a skipped wire is neither side");
    assert!(xrules::detour_amount(&st, wn) > 1.0, "N about 5x its straight distance: {}", xrules::detour_amount(&st, wn));
    assert!(xrules::detour_amount(&st, wm) < 1e-6);
    assert!(xrules::mutual_evasions(&st, &uc).is_empty());
    let (st2, w2) = routed(&b, &m, &[1, 2]);
    let (wn2, wm2) = (w2[0], w2[1]);
    assert_eq!(xrules::u_clinches(&st2, &|_| false), vec![Pair { first: wm2, second: wn2, kind: Kind::UClinch }]);
    assert!(xrules::detour_amount(&st2, wm2) < 0.05, "M round N's pad: {}", xrules::detour_amount(&st2, wm2));
}

/// Part A, mutual evasion (sign 4) = a U-clinch both ways; the shorter wire first. Same-net loops
/// (fig. 15) = same-net projections crossing. Kind names round-trip.
#[test]
fn evasion_loops_and_kind_names() {
    let m = u_clinch_board();
    let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, ..BuildOpts::default() }).unwrap();
    let (mut st, w) = routed(&b, &m, &[2, 1]);
    let (wm, wn) = (w[0], w[1]);
    let both = [Pair { first: wn, second: wm, kind: Kind::UClinch }, Pair { first: wm, second: wn, kind: Kind::UClinch }];
    assert_eq!(xrules::mutual_evasions(&st, &both), vec![Pair { first: wm, second: wn, kind: Kind::MutualEvasion }], "M (17 mm) is shorter than N round it");
    assert!(xrules::mutual_evasions(&st, &both[..1]).is_empty());
    assert!(xrules::same_net_loops(&st).is_empty());
    // N's projection crosses M's? No (it goes round): no loop even as one net.
    st.wires[wn].net = 2;
    assert!(xrules::same_net_loops(&st).is_empty());
    st.wires[wn].net = 1;
    // N laid straight across M (the crossing priced away): one net = a loop, two nets = none.
    let (mut st3, w3) = {
        let reg = Registry::standard();
        let conns = b.connections(&m);
        let mut st = State::new(&b.proj);
        let mut ws = Vec::new();
        for net in [2, 1] {
            let c = conn_of(&conns, net);
            let mut wt = InitWeights { max_expansions: 2_000_000, ..InitWeights::from_registry(&reg, &WireCtx { net, tier: 3 }) };
            wt.cross_mm = 0.0;
            ws.push(st.route_conn(&c, &wt, (c.w + c.s) / WS0).expect("routes").0);
        }
        (st, ws)
    };
    assert!(st3.crossings(w3[1]).iter().any(|x| x.other == w3[0]), "N crosses M");
    assert!(xrules::same_net_loops(&st3).is_empty());
    st3.wires[w3[1]].net = 2;
    assert_eq!(xrules::same_net_loops(&st3), vec![(w3[0].min(w3[1]), w3[0].max(w3[1]))]);
    for k in [Kind::TerminalClinch, Kind::DoubleCrossing, Kind::Unlayerable, Kind::UClinch, Kind::MutualEvasion] {
        assert_eq!(Kind::parse(k.name()), Some(k));
    }
    assert_eq!(Kind::parse("loop"), None);
}

/// Part B: with the batch crossing pass off, the loop stays in the careless state (N about five
/// times its 4 mm); `pattern_moves = ["u_clinch"]` removes the clinch: N straight, M round N's
/// pad, no via, clean copper, the run report counts the kept move.
#[test]
fn u_clinch_move_lays_the_clinched_wire_straight() {
    let m = u_clinch_board();
    for seed in [0u64, 1] {
        let r0 = route_with(&m, &opts(&[], seed), &RunControl::new());
        assert_eq!(r0.failed, 0, "seed {seed}: {:?}", r0.warnings);
        assert!(net_len(&r0.items, 1) > 10.0, "seed {seed}, moves off: N {} (the careless state)", net_len(&r0.items, 1));
        let r = route_with(&m, &opts(&["u_clinch"], seed), &RunControl::new());
        assert_eq!(r.failed, 0, "seed {seed}: {:?}", r.warnings);
        assert_eq!(vias(&r.items), 0, "seed {seed}");
        assert!(net_len(&r.items, 1) < 4.5, "seed {seed}: N {} (straight: 3 mm between the pad edges)", net_len(&r.items, 1));
        assert!(net_len(&r.items, 2) < 19.5, "seed {seed}: M {} (round N's pad, about 18)", net_len(&r.items, 2));
        assert!(r.warnings.iter().any(|w| w.contains("[optimize] pattern_moves = u_clinch") && !w.contains("): 0 of")), "seed {seed}: {:?}", r.warnings);
        let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
        assert!(rep.violations.is_empty(), "seed {seed}: {:?}", rep.violations.first());
    }
}

/// Part B: an unknown entry is ignored with a warning naming it; the known ones still run.
#[test]
fn unknown_pattern_kind_is_named() {
    let m = u_clinch_board();
    let r = route_with(&m, &opts(&["u_clinch", "spiral"], 0), &RunControl::new());
    assert!(r.warnings.iter().any(|w| w.contains("unknown entries ignored: spiral")), "{:?}", r.warnings);
    assert!(net_len(&r.items, 1) < 4.5);
}

/// The loop's `plan metrics` block "loop" -> "variants_table" rows `(vias, length_mm)`.
fn variants(m: &BoardModel, o: &Options) -> Vec<(u64, f64)> {
    let r = route_with(m, o, &RunControl::new());
    let line = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).unwrap_or_else(|| panic!("plan metrics line: {:?}", r.warnings));
    let v: serde_json::Value = serde_json::from_str(line.trim_start_matches("plan metrics:").trim()).expect("plan metrics JSON");
    v["loop"]["variants_table"].as_array().expect("variants table").iter().map(|row| (row["vias"].as_u64().unwrap(), row["length_mm"].as_f64().unwrap())).collect()
}

/// Part D (`[factors] detour`, Polubasov 2010: per-wire coefficients from the quality of its
/// path): off, N's re-lays keep the way round M (two vias cost more than its 15 mm); with a
/// detour coefficient N's length counts dearer by 1 + k (r - 1)^2 (r about 5) at its next re-lay,
/// so the loop also visits the short plan that dives under M (two vias): a new trajectory, the
/// archive (not the coefficient) still decides the pick.
#[test]
fn detour_coefficient_makes_the_loop_visit_the_short_plan() {
    let m = u_clinch_board();
    let off = variants(&m, &opts(&[], 0));
    assert!(off.iter().all(|&(v, _)| v == 0), "detour 0: only the way round: {off:?}");
    let on = variants(&m, &Options { detour_k: 10.0, ..opts(&[], 0) });
    let short = on.iter().filter(|&&(v, _)| v > 0).map(|&(_, l)| l).fold(f64::MAX, f64::min);
    let round = off.iter().map(|&(_, l)| l).fold(f64::MAX, f64::min);
    assert!(short < round - 5.0, "detour 10: a variant with vias, shorter: {on:?} vs {off:?}");
}
