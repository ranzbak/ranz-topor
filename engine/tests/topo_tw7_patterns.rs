//! TW7 (docs/dev/design/tw7-moves.md): the pattern detectors (part A) and the targeted pattern
//! moves of the TopoR loop (part B, `[optimize] pattern_moves`).
//!
//! The board is the user-approved `u_clinch` pattern (2026-11-19; tools/tw7_sketch.py,
//! Luzin 2015 p. 8 b, fig. 19; Luzin, Popov & Popov 2012 p. 117 sign 2, p. 118 removal) with M's
//! lower pad at y 13.3 (sketch: 19.5; closer, so N's way round its end, +4.6 mm, is cheaper than the
//! crossing the initial routing would price): a 0.5 mm rail M (net 2) runs between the two pads of the
//! short 0.25 mm wire N (net 1). The initial routing lays M (wider) first and N round M's lower
//! end (careless, N 8.65 mm); no single re-lay leaves it (M cannot leave the U N makes round its
//! end without a via pair, N cannot cross M without one). Expected: N straight (4 mm), M round
//! N's pad (14 mm), no via: the U-clinch move (N first, then M) reaches it.

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
/// (16, 11); M (net 2, 0.5 mm) pads (14, 2.5), (14, M2Y); 1 mm pads; 28 x 22 mm.
const M2Y: f64 = 13.3;

fn u_clinch_board() -> BoardModel {
    u_clinch_at(M2Y)
}

fn u_clinch_at(y: f64) -> BoardModel {
    board(28.0, 22.0, &[(1, 12.0, 11.0, 1.0), (1, 16.0, 11.0, 1.0), (2, 14.0, 2.5, 1.0), (2, 14.0, y, 1.0)], &[0.25, 0.5])
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
/// by the plain search; `cross` = the crossing price (mm; 1e9 = the second wire must go round).
fn routed_x<'p>(b: &'p board::Board, m: &BoardModel, order: &[i32], cross: Option<f64>) -> (State<'p>, Vec<WireId>) {
    let reg = Registry::standard();
    let conns = b.connections(m);
    let mut st = State::new(&b.proj);
    st.taut_len = true;
    let ws = order
        .iter()
        .map(|&net| {
            let c = conn_of(&conns, net);
            let mut wt = InitWeights { max_expansions: 2_000_000, ..InitWeights::from_registry(&reg, &WireCtx { net, tier: 3 }) };
            if let Some(x) = cross {
                wt.cross_mm = x;
            }
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
    let (st, w) = routed_x(&b, &m, &[2, 1], Some(1e9));
    let (wm, wn) = (w[0], w[1]);
    assert!(st.crossings(wn).iter().all(|x| x.other != wm), "N goes round M, no crossing");
    let uc = xrules::u_clinches(&st, &|_| false);
    let pl = |x: WireId| st.polyline(x).iter().map(|p| format!("({:.2},{:.2})", p.x / MM, p.y / MM)).collect::<Vec<_>>().join(" ");
    assert_eq!(uc, vec![Pair { first: wn, second: wm, kind: Kind::UClinch }], "N {} / M {} / ends N {:?} {:?}", pl(wn), pl(wm), st.proj.pos[st.wires[wn].a], st.proj.pos[st.wires[wn].b]);
    assert!(xrules::u_clinches(&st, &|x| x == wn).is_empty(), "a skipped wire is neither side");
    assert!(xrules::detour_amount(&st, wn) > 0.5, "N round M's end, about twice its straight distance: {}", xrules::detour_amount(&st, wn));
    assert!(xrules::detour_amount(&st, wm) < 0.01, "M nearly straight (its projection bends at the pass points): {}", xrules::detour_amount(&st, wm));
    assert!(xrules::mutual_evasions(&st, &uc).is_empty());
    let (st2, w2) = routed_x(&b, &m, &[1, 2], Some(1e9));
    let (wn2, wm2) = (w2[0], w2[1]);
    assert_eq!(xrules::u_clinches(&st2, &|_| false), vec![Pair { first: wm2, second: wn2, kind: Kind::UClinch }]);
    assert!(xrules::detour_amount(&st2, wm2) < xrules::detour_amount(&st, wn) / 4.0, "M round N's pad: {}", xrules::detour_amount(&st2, wm2));
}

/// Part A, mutual evasion (sign 4) = a U-clinch both ways; the shorter wire first. Same-net loops
/// (fig. 15) = same-net projections crossing. Kind names round-trip.
#[test]
fn evasion_loops_and_kind_names() {
    let m = u_clinch_board();
    let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, ..BuildOpts::default() }).unwrap();
    let (mut st, w) = routed_x(&b, &m, &[2, 1], Some(1e9));
    let (wm, wn) = (w[0], w[1]);
    let both = [Pair { first: wn, second: wm, kind: Kind::UClinch }, Pair { first: wm, second: wn, kind: Kind::UClinch }];
    let len = |x: WireId| st.wire_len(x);
    let (f, g) = if len(wn) <= len(wm) { (wn, wm) } else { (wm, wn) };
    assert_eq!(xrules::mutual_evasions(&st, &both), vec![Pair { first: f, second: g, kind: Kind::MutualEvasion }], "the shorter wire first");
    let both_r = [both[1], both[0]];
    assert_eq!(xrules::mutual_evasions(&st, &both_r), xrules::mutual_evasions(&st, &both), "order of the input does not matter");
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
    assert!(xrules::u_clinches(&st3, &|_| false).is_empty(), "a wire that crosses is not a U-clinch");
    assert!(xrules::same_net_loops(&st3).is_empty());
    st3.wires[w3[1]].net = 2;
    assert_eq!(xrules::same_net_loops(&st3), vec![(w3[0].min(w3[1]), w3[0].max(w3[1]))]);
    // A pass point exactly on the line counts once (half-open); a touch from one side counts an
    // even number (the parity, which the U-clinch reads, is that of no crossing).
    let v = |x: f64, y: f64| topor_engine::geom::Vec2 { x, y };
    let (a, b) = (v(0.0, 0.0), v(4.0, 0.0));
    assert_eq!(xrules::line_crossings(&[v(2.0, 3.0), v(2.0, 0.0), v(2.0, -3.0)], a, b), 1);
    assert_eq!(xrules::line_crossings(&[v(1.0, 3.0), v(2.0, 0.0), v(3.0, 3.0)], a, b) % 2, 0);
    assert_eq!(xrules::line_crossings(&[v(1.0, -3.0), v(2.0, 0.0), v(3.0, -3.0)], a, b), 0);
    assert_eq!(xrules::line_crossings(&[v(5.0, 3.0), v(5.0, -3.0)], a, b), 0, "beyond the segment");
    assert_eq!(xrules::line_crossings(&[v(1.0, 3.0), v(1.0, -3.0), v(3.0, -3.0), v(3.0, 3.0)], a, b), 2);
    for k in [Kind::TerminalClinch, Kind::DoubleCrossing, Kind::Unlayerable, Kind::UClinch, Kind::MutualEvasion] {
        assert_eq!(Kind::parse(k.name()), Some(k));
    }
    assert_eq!(Kind::parse("loop"), None);
}

/// Part B: with the batch crossing pass off the loop stays careless (N about 8.65 mm); with
/// `pattern_moves = ["u_clinch", "evasion"]` one move (N first, then M) is kept: N straight, M
/// round N's pad, no via, clean copper, the run report counts it.
#[test]
fn u_clinch_move_lays_the_clinched_wire_straight() {
    let m = u_clinch_board();
    for seed in [0u64, 1] {
        let r0 = route_with(&m, &opts(&[], seed), &RunControl::new());
        assert_eq!(r0.failed, 0, "seed {seed}: {:?}", r0.warnings);
        assert!(net_len(&r0.items, 1) > 6.5, "seed {seed}, moves off: N {} (careless)", net_len(&r0.items, 1));
        let r = route_with(&m, &opts(&["u_clinch", "evasion"], seed), &RunControl::new());
        assert_eq!(r.failed, 0, "seed {seed}: {:?}", r.warnings);
        assert_eq!(vias(&r.items), 0, "seed {seed}");
        assert!(net_len(&r.items, 1) < 4.5, "seed {seed}: N {} (straight: 3 mm between the pad edges)", net_len(&r.items, 1));
        assert!(net_len(&r.items, 2) < 15.0, "seed {seed}: M {} (round N's pad, about 14)", net_len(&r.items, 2));
        assert!(net_len(&r.items, 1) + net_len(&r.items, 2) < net_len(&r0.items, 1) + net_len(&r0.items, 2) - 1.0, "seed {seed}: shorter in all");
        assert!(r.warnings.iter().any(|w| w.contains("[optimize] pattern_moves = u_clinch, evasion") && !w.contains("): 0 of")), "seed {seed}: {:?}", r.warnings);
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
/// path): a connection's length counts dearer at its next re-lay by 1 + k (r - 1)^2. Careless N
/// (r about 2.2) then prefers the straight way under M (two vias) at its re-lay, after which M's
/// re-lay goes round N's pad: the loop leaves the careless state without any pattern move. The
/// archive still decides the pick (no via, shorter). 0 = off exactly.
#[test]
fn detour_coefficient_leaves_the_careless_state() {
    let m = u_clinch_board();
    for seed in [0u64, 1] {
        let r = route_with(&m, &Options { detour_k: 100.0, ..opts(&[], seed) }, &RunControl::new());
        assert_eq!(r.failed, 0, "seed {seed}: {:?}", r.warnings);
        assert_eq!(vias(&r.items), 0, "seed {seed}");
        assert!(net_len(&r.items, 1) < 4.5, "seed {seed}: N {}", net_len(&r.items, 1));
        let off = variants(&m, &opts(&[], seed));
        assert_eq!(variants(&m, &Options { detour_k: 0.0, ..opts(&[], seed) }), off, "seed {seed}: 0 = off");
    }
}

/// Part B, the judge: with M's lower pad at y 12.4 the way round its end costs N only 2.85 mm,
/// less than M's way round N's pad, so every U-clinch move is tried and none kept (each restored
/// exactly: the drawn copper equals the run without moves).
#[test]
fn u_clinch_moves_that_do_not_gain_are_undone() {
    let m = u_clinch_at(12.4);
    let r0 = route_with(&m, &opts(&[], 0), &RunControl::new());
    let r = route_with(&m, &opts(&["u_clinch"], 0), &RunControl::new());
    let line = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).expect("plan metrics");
    let v: serde_json::Value = serde_json::from_str(line.trim_start_matches("plan metrics:").trim()).expect("JSON");
    let pm = &v["loop"]["pattern_moves"];
    assert!(pm[1].as_u64().unwrap() > 0 && pm[0].as_u64().unwrap() == 0, "tried, none kept: {pm}");
    assert_eq!(format!("{:?}", r.items), format!("{:?}", r0.items), "undone moves leave the copper as without them");
}
