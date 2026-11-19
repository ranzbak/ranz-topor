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

/// The loop's `plan metrics` block "loop".
fn loop_json(m: &BoardModel, o: &Options) -> serde_json::Value {
    let r = route_with(m, o, &RunControl::new());
    let line = r.warnings.iter().find(|w| w.starts_with("plan metrics:")).expect("plan metrics");
    let v: serde_json::Value = serde_json::from_str(line.trim_start_matches("plan metrics:").trim()).expect("JSON");
    v["loop"].clone()
}

/// Part B, `[optimize] pattern_share` (docs/dev/design/tw7-moves.md §11): a graded work budget.
/// At y 12.4 every U-clinch move is tried and undone, so the budget alone decides how many run:
/// without a budget one per cycle and none waits; a small share runs fewer, the found rest waits;
/// a larger share runs at least as many as a smaller one (graded, no cutoff); the measured share
/// stays near the asked one (one move of overdraw at most). The per-kind counters add up to the
/// totals, and the copper stays as without moves (every move undone exactly).
#[test]
fn pattern_share_bounds_the_moves_graded() {
    let m = u_clinch_at(12.4);
    let r0 = route_with(&m, &opts(&[], 0), &RunControl::new());
    let run = |share: f64| {
        let o = Options { pattern_share: share, ..opts(&["u_clinch"], 0) };
        let r = route_with(&m, &o, &RunControl::new());
        assert_eq!(format!("{:?}", r.items), format!("{:?}", r0.items), "share {share}: undone moves leave the copper");
        loop_json(&m, &o)
    };
    let all = run(1.0);
    let pm = |j: &serde_json::Value, i: usize| j["pattern_moves"][i].as_u64().unwrap();
    let uc = |j: &serde_json::Value, i: usize| j["pattern_kinds"]["u_clinch"][i].as_u64().unwrap();
    assert!(pm(&all, 1) >= 3, "one move per cycle: {}", all["pattern_moves"]);
    assert_eq!(all["pattern_share"]["waited"], 0, "no budget: nothing waits");
    assert_eq!(all["pattern_share"]["retried"].as_u64().unwrap(), pm(&all, 1) - 1, "one pattern, every try fruitless: each try after the first is a retry");
    assert_eq!(all["pattern_share"]["stuck_patterns"], 1);
    assert_eq!((uc(&all, 2), uc(&all, 1), uc(&all, 0)), (pm(&all, 0), pm(&all, 1), pm(&all, 2)), "per kind = totals (one kind on)");
    assert_eq!(all["pattern_kinds"]["clinch"], serde_json::json!([0, 0, 0]));
    let low = run(0.02);
    let mid = run(0.3);
    assert!(pm(&low, 1) >= 1, "the first move runs on the first cycle's other work");
    assert!(pm(&low, 1) < pm(&all, 1), "a small share runs fewer moves: {} vs {}", low["pattern_moves"], all["pattern_moves"]);
    assert!(low["pattern_share"]["waited"].as_u64().unwrap() > 0, "the found rest waits: {}", low["pattern_share"]);
    assert!(pm(&mid, 1) >= pm(&low, 1) && pm(&mid, 1) <= pm(&all, 1), "graded: {} {} {}", low["pattern_moves"], mid["pattern_moves"], all["pattern_moves"]);
    let meas = |j: &serde_json::Value| j["pattern_share"]["measured"].as_f64().unwrap();
    assert!(meas(&low) < meas(&all), "{} vs {}", low["pattern_share"], all["pattern_share"]);
    let w = |j: &serde_json::Value, k: &str| j["pattern_share"][k].as_f64().unwrap();
    let per_move = w(&low, "work") / pm(&low, 1) as f64;
    assert!(w(&low, "work") <= 0.02 / 0.98 * w(&low, "other_work") + per_move + 1e-6, "at most one move of overdraw: {}", low["pattern_share"]);
    assert_eq!(low["pattern_share"]["share"], 0.02);
}

/// Part B with a budget: the first found pattern still runs, so the careless U-clinch is left
/// at a small share too (N straight, no via); the report line names the budget.
#[test]
fn pattern_share_still_takes_the_best_move() {
    let m = u_clinch_board();
    let o = Options { pattern_share: 0.05, ..opts(&["u_clinch", "evasion"], 0) };
    let r = route_with(&m, &o, &RunControl::new());
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    assert_eq!(vias(&r.items), 0);
    assert!(net_len(&r.items, 1) < 4.5, "N {}", net_len(&r.items, 1));
    assert!(r.warnings.iter().any(|w| w.contains("pattern_share = 0.05: the moves took")), "{:?}", r.warnings);
}

/// Part B, `[optimize] pattern_stuck` (docs/dev/design/tw7-moves.md §12): two u_clinch boards
/// side by side, A (nets 1, 2; M's end at y 12.4: every move undone) and B (nets 3, 4; y 13.3:
/// the move is kept, after it B's reversed pattern, M round N's pad, has a little excess and its
/// moves are undone). After B's kept move A has the most excess (about 0.5 against B's reversed
/// 0.02). At share 0.25 (about one move per cycle, 9 cycles) with the excess alone (weight 0) A's
/// fruitless pattern takes the budget every cycle and B's reversed one is never tried; with
/// weight 3 A falls below it after a few fruitless tries (0.5 / (1 + 3 k)), so both get tried
/// (graded order, no cutoff; weight 1 needs more cycles than this run has). B is laid straight
/// either way.
#[test]
fn pattern_stuck_lets_a_fresh_pattern_past_a_fruitless_one() {
    let m = board(
        56.0,
        22.0,
        &[(1, 12.0, 11.0, 1.0), (1, 16.0, 11.0, 1.0), (2, 14.0, 2.5, 1.0), (2, 14.0, 12.4, 1.0), (3, 40.0, 11.0, 1.0), (3, 44.0, 11.0, 1.0), (4, 42.0, 2.5, 1.0), (4, 42.0, M2Y, 1.0)],
        &[0.25, 0.5, 0.25, 0.5],
    );
    let run = |w: f64| {
        let o = Options { pattern_share: 0.25, pattern_stuck: w, ..opts(&["u_clinch"], 0) };
        let r = route_with(&m, &o, &RunControl::new());
        assert_eq!(r.failed, 0, "{:?}", r.warnings);
        assert!(net_len(&r.items, 3) < 4.5, "weight {w}: B's N straight: {}", net_len(&r.items, 3));
        loop_json(&m, &o)
    };
    let plain = run(0.0);
    let graded = run(3.0);
    assert_eq!(plain["pattern_share"]["stuck_patterns"], 1, "excess alone: only A's pattern is retried: {}", plain["pattern_share"]);
    assert_eq!(graded["pattern_share"]["stuck_patterns"], 2, "graded: B's reversed pattern gets its tries too: {}", graded["pattern_share"]);
}

/// Part C (`[optimize] pattern_moves` "via_trade", Luzin 2015 pp. 9-10 fig. 21b; Luzin, Popov &
/// Popov 2013 p. 134 step 8.1) on the approved u_clinch board: careless N (one layer, round M's
/// end) is a via trade with one mover, M (it crosses N's straight line, not N). The move lays N
/// first, held to its layer, then M round N's pad: N straight, no via, as the U-clinch move; the
/// report counts it under via_trade.
#[test]
fn via_trade_lays_the_wire_straight_and_leads_the_crosser_round() {
    let m = u_clinch_board();
    for seed in [0u64, 1] {
        let o = opts(&["via_trade"], seed);
        let r = route_with(&m, &o, &RunControl::new());
        assert_eq!(r.failed, 0, "seed {seed}: {:?}", r.warnings);
        assert_eq!(vias(&r.items), 0, "seed {seed}");
        assert!(net_len(&r.items, 1) < 4.5, "seed {seed}: N {} (straight)", net_len(&r.items, 1));
        assert!(net_len(&r.items, 2) < 15.0, "seed {seed}: M {} (round N's pad)", net_len(&r.items, 2));
        let j = loop_json(&m, &o);
        let vt = &j["pattern_kinds"]["via_trade"];
        assert!(vt[2].as_u64().unwrap() >= 1 && vt[0].as_u64().unwrap() >= 1, "seed {seed}: the via trade found and kept: {}", j["pattern_kinds"]);
        assert_eq!(j["pattern_kinds"]["u_clinch"], serde_json::json!([0, 0, 0]), "seed {seed}: only the listed kind");
        let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
        assert!(rep.violations.is_empty(), "seed {seed}: {:?}", rep.violations.first());
    }
}

/// Part C, the detector alone: on the careless u_clinch state N (round M's end, not crossing it)
/// has the one mover M; M (nearly straight) has N's wire across its line? No: N goes round M's
/// end, so N's projection does not cross M's straight line and M has no via trade. A subject the
/// caller refuses has none; a wire that crosses the line and the subject too is no mover.
#[test]
fn via_trades_are_found_on_the_careless_state() {
    let m = u_clinch_board();
    let b = board::build(&m, &BuildOpts { steiner_mm: 0.0, ..BuildOpts::default() }).unwrap();
    let (st, w) = routed_x(&b, &m, &[2, 1], Some(1e9));
    let (wm, wn) = (w[0], w[1]);
    assert_eq!(xrules::via_trades(&st, &|_| true), vec![(wn, vec![wm])]);
    assert!(xrules::via_trades(&st, &|x| x != wn).is_empty(), "the caller's subject filter");
    // N laid across M (the crossing priced away): M crosses N's line and N itself, so no mover. The
    // subject must be one (excess length > 0, else the case tests nothing: N laid dead straight has
    // none, verify tw7-moves r4): M one pitch lower and laid after N, N bends a little and still
    // crosses M.
    let m3 = u_clinch_at(11.0);
    let b3 = board::build(&m3, &BuildOpts { steiner_mm: 0.0, ..BuildOpts::default() }).unwrap();
    let (st3, w3) = routed_x(&b3, &m3, &[1, 2], Some(0.0));
    assert!(st3.crossings(w3[0]).iter().any(|x| x.other == w3[1]), "N crosses M");
    assert!(xrules::detour_amount(&st3, w3[0]) > 0.0, "the subject has excess length: {}", xrules::detour_amount(&st3, w3[0]));
    assert!(xrules::via_trades(&st3, &|_| true).is_empty(), "N crosses M: M is no mover: {:?}", xrules::via_trades(&st3, &|_| true));
    assert_eq!(Kind::parse("via_trade"), Some(Kind::ViaTrade));
    assert_eq!(Kind::parse("pour"), Some(Kind::PourCut));
    assert_eq!(Kind::known(), "clinch, double, unlayerable, u_clinch, evasion, via_trade, pour");
}

/// A straight track piece `(a, b, width)` in mm.
type Piece = ((f64, f64), (f64, f64), f64);

/// Net `net`'s drawn track pieces as straight pieces `(a, b, width)` in mm (KiCad frame; an arc as
/// 16 chords of its circle through start, mid and end).
fn pieces(items: &[Item], net: i32) -> Vec<Piece> {
    let mm = |v: topor_engine::geom::IVec2| (v.x as f64 / MM, v.y as f64 / MM);
    let mut out = Vec::new();
    for it in items {
        let Item::Track(t) = it else { continue };
        if t.net != net {
            continue;
        }
        let w = t.width as f64 / MM;
        match t.prim {
            TrackPrim::Seg { a, b } => out.push((mm(a), mm(b), w)),
            TrackPrim::Arc { start, mid, end } => {
                let (p0, p1, p2) = (mm(start), mm(mid), mm(end));
                let d = 2.0 * (p0.0 * (p1.1 - p2.1) + p1.0 * (p2.1 - p0.1) + p2.0 * (p0.1 - p1.1));
                let sq = |p: (f64, f64)| p.0 * p.0 + p.1 * p.1;
                let cx = (sq(p0) * (p1.1 - p2.1) + sq(p1) * (p2.1 - p0.1) + sq(p2) * (p0.1 - p1.1)) / d;
                let cy = (sq(p0) * (p2.0 - p1.0) + sq(p1) * (p0.0 - p2.0) + sq(p2) * (p1.0 - p0.0)) / d;
                let r = (p0.0 - cx).hypot(p0.1 - cy);
                let ang = |p: (f64, f64)| (p.1 - cy).atan2(p.0 - cx);
                let (a0, a1, a2) = (ang(p0), ang(p1), ang(p2));
                let norm = |x: f64| x.rem_euclid(std::f64::consts::TAU);
                let sweep = if norm(a1 - a0) < norm(a2 - a0) { norm(a2 - a0) } else { -norm(a0 - a2) };
                let pt = |k: usize| {
                    let a = a0 + sweep * k as f64 / 16.0;
                    (cx + r * a.cos(), cy + r * a.sin())
                };
                for k in 0..16 {
                    out.push((pt(k), pt(k + 1), w));
                }
            }
        }
    }
    out
}

fn piece_len(p: &[Piece]) -> f64 {
    p.iter().map(|(a, b, _)| (b.0 - a.0).hypot(b.1 - a.1)).sum()
}

/// The smallest distance (mm) between the points of the straight pieces `a`-`b` and `c`-`d`.
fn seg_seg(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> f64 {
    let pd = |p: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = dx * dx + dy * dy;
        let t = if l > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l).clamp(0.0, 1.0) } else { 0.0 };
        (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
    };
    let o = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0);
    if o(a, b, c) * o(a, b, d) < 0.0 && o(c, d, a) * o(c, d, b) < 0.0 {
        return 0.0;
    }
    pd(a, c, d).min(pd(b, c, d)).min(pd(c, a, b)).min(pd(d, a, b))
}

/// The smallest copper gap (mm, edge to edge) between two nets' drawn tracks.
fn track_gap(items: &[Item], n1: i32, n2: i32) -> f64 {
    let (p, q) = (pieces(items, n1), pieces(items, n2));
    p.iter().flat_map(|x| q.iter().map(move |y| seg_seg(x.0, x.1, y.0, y.1) - x.2 / 2.0 - y.2 / 2.0)).fold(f64::MAX, f64::min)
}

/// The smallest gap (mm) from net `net`'s drawn tracks to the square pad of side `s` centred at `c`.
fn pad_gap(items: &[Item], net: i32, c: (f64, f64), s: f64) -> f64 {
    let h = s / 2.0;
    let k = [(c.0 - h, c.1 - h), (c.0 + h, c.1 - h), (c.0 + h, c.1 + h), (c.0 - h, c.1 + h)];
    pieces(items, net).iter().flat_map(|x| (0..4).map(move |i| seg_seg(x.0, x.1, k[i], k[(i + 1) % 4]) - x.2 / 2.0)).fold(f64::MAX, f64::min)
}

/// The user-approved `detour_trade` pattern (2026-11-19; tools/tw7_sketch.py `p_detour_trade`,
/// picture hz-runs/tw7s2-17228bd/sketch/detour_trade.png; Luzin 2015 pp. 9-10 fig. 21a-c; Luzin,
/// Popov & Popov 2013 p. 134 step 8.1): two 0.5 mm rails B1 (net 2, y 14) and B2 (net 3, y 15.5),
/// x 10-50, across the line of a 0.25 mm wire A (net 1) from A1 (30, 5) to A2 (30, 19); 1 mm pads.
/// Expected = panel (c): A straight (14 mm), no via; B1 and B2 dip round A2 together, packed
/// at clearance (B1-B2 copper gap 0.20 mm at the V), B1 at the 0.20 mm clearance from A2, B1 at
/// most 44.93 mm with the sketch's 45 degree legs (a drawn dip may be shorter, never longer). Seed 1 of the defaults ends careless (A round the rails, about 49 mm); part C
/// (`pattern_moves` = ["via_trade"], alone and with the other kinds) must reach (c), seeds 0 and 1.
///
/// Pitch (user 2026-11-19, replaces the sketch's constant 1.5 mm pitch): the bundle is packed at
/// clearance round the bend (rule "bundles tight-packed"): B2 lies against B1 at the V with a
/// copper gap of the 0.20 mm clearance (B2 about 44.27 mm), not the rails' 1.0 mm.
#[test]
fn detour_trade_pattern_reaches_fig_21c() {
    let m = board(
        60.0,
        30.0,
        &[(2, 10.0, 14.0, 1.0), (2, 50.0, 14.0, 1.0), (3, 10.0, 15.5, 1.0), (3, 50.0, 15.5, 1.0), (1, 30.0, 5.0, 1.0), (1, 30.0, 19.0, 1.0)],
        &[0.25, 0.5, 0.5],
    );
    let r0 = route_with(&m, &Options { seed: 1, ..Options::default() }, &RunControl::new());
    assert!(piece_len(&pieces(&r0.items, 1)) > 20.0, "moves off, seed 1: careless A round the rails: {}", piece_len(&pieces(&r0.items, 1)));
    let mut loose = Vec::new();
    for (name, kinds) in [("via_trade", &["via_trade"][..]), ("all kinds + via_trade", &["clinch", "double", "unlayerable", "u_clinch", "evasion", "via_trade"][..])] {
        for seed in [0u64, 1] {
            let o = Options { seed, pattern_moves: kinds.iter().map(|k| k.to_string()).collect(), ..Options::default() };
            let r = route_with(&m, &o, &RunControl::new());
            let tag = format!("{name} seed {seed}");
            let (la, l1, l2) = (piece_len(&pieces(&r.items, 1)), piece_len(&pieces(&r.items, 2)), piece_len(&pieces(&r.items, 3)));
            let (g12, g1a) = (track_gap(&r.items, 2, 3), pad_gap(&r.items, 2, (30.0, 19.0), 1.0));
            eprintln!("detour_trade {tag}: A {la:.3} B1 {l1:.3} B2 {l2:.3} gap B1-B2 {g12:.3} B1-A2 {g1a:.3} vias {}", vias(&r.items));
            assert_eq!((r.failed, vias(&r.items)), (0, 0), "{tag}: {:?}", r.warnings);
            assert!(la < 14.01, "{tag}: A straight: {la}");
            assert!(l1 < 44.93 + 0.05 && l2 < 44.93 + 0.05, "{tag}: the rails no longer than the sketch's dip: {l1} {l2}");
            assert!(l1 > 40.5 && l2 > 40.5, "{tag}: the rails go round A2: {l1} {l2}");
            assert!(g1a > 0.2 - 0.01 && g1a < 0.2 + 0.1, "{tag}: B1 at the clearance from A2: {g1a}");
            let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
            assert!(rep.violations.is_empty(), "{tag}: {:?}", rep.violations.first());
            if !(g12 > 0.2 - 0.01 && g12 < 0.2 + 0.05) {
                loose.push(format!("{tag}: gap {g12:.3}"));
            }
        }
    }
    assert!(loose.is_empty(), "bundle packed at clearance at the V (B1-B2 copper gap = the 0.20 mm clearance): {loose:?}");
}

/// PROPOSED variant of the approved `detour_trade` (round 4; not user-approved, so ignored and not
/// a regression test): the rails shortened to x 22-38. Unlike the approved geometry (which the
/// loop solves without moves) this is a local minimum of single re-lays: without moves A goes
/// round the rails' ends (25.1 mm), 0 vias; the U-clinch move (one rail at a time) cannot leave
/// it. The via trade (A first, held, then both rails) leads both rails round A2 (about +4.6 mm
/// each): A straight, 0 vias, less length in all.
#[test]
#[ignore = "PROPOSED variant of detour_trade (rails x 22-38): not user-approved"]
fn proposed_detour_trade_short_rails_needs_the_via_trade() {
    let m = board(
        60.0,
        30.0,
        &[(2, 22.0, 14.0, 1.0), (2, 38.0, 14.0, 1.0), (3, 22.0, 15.5, 1.0), (3, 38.0, 15.5, 1.0), (1, 30.0, 5.0, 1.0), (1, 30.0, 19.0, 1.0)],
        &[0.25, 0.5, 0.5],
    );
    let r0 = route_with(&m, &opts(&[], 0), &RunControl::new());
    let u = route_with(&m, &opts(&["u_clinch", "evasion"], 0), &RunControl::new());
    let r = route_with(&m, &opts(&["via_trade"], 0), &RunControl::new());
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    let all = |r: &topor_engine::route::RouteResult| net_len(&r.items, 1) + net_len(&r.items, 2) + net_len(&r.items, 3);
    assert!(net_len(&r0.items, 1) > 20.0 && vias(&r0.items) == 0, "careless without the move: A {} vias {}", net_len(&r0.items, 1), vias(&r0.items));
    assert!(net_len(&u.items, 1) > 20.0, "the U-clinch move cannot leave it: A {}", net_len(&u.items, 1));
    assert_eq!(vias(&r.items), 0);
    assert!(net_len(&r.items, 1) < 14.5, "A straight: {}", net_len(&r.items, 1));
    assert!(net_len(&r.items, 2) < 22.0 && net_len(&r.items, 3) < 22.0, "rails round A2: {} {}", net_len(&r.items, 2), net_len(&r.items, 3));
    assert!(all(&r) < all(&r0), "less length in all: {} vs {}", all(&r), all(&r0));
    let rep = Checker::new(&m, &Options::default()).check_items(&r.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations.first());
}


/// The pour board of `pourplan`'s tests as a routable board: 50 x 40 mm, GND pour on B.Cu over
/// [2, 32] x [2, 32] mm listing U1.1 (4, 15), U2.1 (4, 25) and U3.1 (20, 15), U4.1 (20, 25),
/// U5.1 (25, 20); SIG pads A (7, 2.3), B (7, 31.7); every pad SMD on B.Cu (1 mm square).
fn pour_board() -> BoardModel {
    const M: i64 = 1_000_000;
    let pad = |r: &str, net: i32, x: i64, y: i64| {
        format!(
            r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{x},{y}],"rotation_deg":0.0,"layers":[31],
                "shapes":{{"31":{{"kind":"rect","w":1000000,"h":1000000}}}},"drill":null}}"#
        )
    };
    let pads = [pad("U1", 1, 4 * M, 15 * M), pad("U2", 1, 4 * M, 25 * M), pad("U3", 1, 20 * M, 15 * M), pad("U4", 1, 20 * M, 25 * M), pad("U5", 1, 25 * M, 20 * M), pad("A", 2, 7 * M, 2_300_000), pad("B", 2, 7 * M, 31_700_000)];
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":31,"name":"B.Cu","kind":"signal"}}],
            "nets":[{{"code":1,"name":"GND"}},{{"code":2,"name":"SIG"}}],
            "outline":[{{"kind":"seg","a":[0,0],"b":[50000000,0]}},{{"kind":"seg","a":[50000000,0],"b":[50000000,40000000]}},
                       {{"kind":"seg","a":[50000000,40000000],"b":[0,40000000]}},{{"kind":"seg","a":[0,40000000],"b":[0,0]}}],
            "pads":[{}],
            "pours":[{{"uuid":"zz","net":1,"layers":[31],"pads":["U1.1","U2.1","U3.1","U4.1","U5.1"],"vias":[],"clearance":200000,
                       "min_thickness":254000,"thermal_gap":500000,"spoke_width":500000,"pad_connection":1,
                       "fills":[{{"layer":31,"outer":[[2000000,2000000],[32000000,2000000],[32000000,32000000],[2000000,32000000]]}}]}}],
            "rules":{{"min_clearance":200000,"min_track_width":200000,"min_via_diameter":450000,"min_via_drill":250000,
                     "min_annular":100000,"hole_to_hole":250000,"edge_clearance":500000,
                     "default":{{"clearance":200000,"width":250000,"via_d":600000,"via_drill":300000}},"per_net":{{}}}}}}"#,
        pads.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

/// Part E (`[optimize] pattern_moves` "pour", tw4-r2 (3); needs `[capacity] pour_cuts`): SIG's
/// straight plan (x = 7 mm, on B.Cu, its pads' layer) cuts U1.1 and U2.1 off the pour, a predicted
/// violation with the cut priced. The pour move is found and tried, and lays SIG again with the
/// cut's price on its passes near the cut-off piece. Round-4 finding (kept as the test's record,
/// not as the wanted answer): the move is kept on the plan's pour check (plan eval 1279.4 ->
/// 30.9, TOPO_LOOPDBG), but the drawn copper is the same straight track: the relaid plan only
/// changes the triangles SIG passes, not its way round U1/U2, and the plan's check (on the pass
/// points, not the taut path) no longer sees the cut. Without pour_cuts the kind finds nothing
/// and the report says so.
#[test]
fn pour_move_is_found_and_tried_and_needs_pour_cuts() {
    let m = pour_board();
    let o = Options { pour_cuts: true, ..opts(&["pour"], 0) };
    let j = loop_json(&m, &o);
    let pk = &j["pattern_kinds"]["pour"];
    assert!(pk[0].as_u64().unwrap() >= 1 && pk[1].as_u64().unwrap() >= 1, "found and tried: {}", j["pattern_kinds"]);
    assert!(pk[2].as_u64().unwrap() >= 1, "and kept (plan eval 1279.4 -> 30.9; kills e_no_bump): {}", j["pattern_kinds"]);
    let r = route_with(&m, &o, &RunControl::new());
    assert_eq!(r.failed, 0, "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("[optimize] pattern_moves = pour") && !w.contains("finds nothing")), "{:?}", r.warnings);
    let off = Options { pour_cuts: false, ..opts(&["pour"], 0) };
    let ro = route_with(&m, &off, &RunControl::new());
    assert!(ro.warnings.iter().any(|w| w.contains("\"pour\" finds nothing without [capacity] pour_cuts = true")), "{:?}", ro.warnings);
    assert_eq!(loop_json(&m, &off)["pattern_kinds"]["pour"], serde_json::json!([0, 0, 0]));
}

/// Verifier round 3, F1 (reports/verify-tw7-moves-data/terminal_clinch_test.rs): the user-approved
/// `terminal_clinch` pattern at the sketch's geometry (tools/tw7_sketch.py p_terminal_clinch;
/// Luzin 2015 p. 9 fig. 18): M (0.5 mm) runs straight past N's terminal b1. Expected answer: N
/// straight (15 mm), M round b1 (about 16.8 mm drawn), no via. Seed 1 without pattern moves ends
/// careless (N about 26.4 mm round M's end); with the moves the U-clinch move reaches the answer.
#[test]
fn terminal_clinch_pattern_reaches_the_expected_answer() {
    let m = board(26.0, 20.0, &[(1, 20.0, 10.0, 1.0), (1, 5.0, 10.0, 1.0), (2, 18.8, 2.5, 1.0), (2, 18.8, 17.5, 1.0)], &[0.25, 0.5]);
    let all = ["clinch", "double", "unlayerable", "u_clinch", "evasion"];
    let base = Options { seed: 1, ..Options::default() };
    let r0 = route_with(&m, &base, &RunControl::new());
    assert!(net_len(&r0.items, 1) > 20.0, "moves off, seed 1: careless N {}", net_len(&r0.items, 1));
    for seed in [0u64, 1] {
        let o = Options { seed, pattern_moves: all.iter().map(|s| s.to_string()).collect(), ..Options::default() };
        let r = route_with(&m, &o, &RunControl::new());
        assert_eq!((r.failed, vias(&r.items)), (0, 0), "seed {seed}: {:?}", r.warnings);
        assert!(net_len(&r.items, 1) < 15.5, "seed {seed}: N {} (straight)", net_len(&r.items, 1));
        assert!(net_len(&r.items, 2) < 17.5, "seed {seed}: M {} (round b1)", net_len(&r.items, 2));
        assert!(Checker::new(&m, &Options::default()).check_items(&r.items).violations.is_empty(), "seed {seed}");
    }
}

/// Verify tw7-moves r3/r4 (survivor vt_one_mover_dup): with u_clinch and via_trade both listed, the
/// careless u_clinch state's one-mover via trade (N, [M]) is the U-clinch's pair and is not counted
/// as a via trade too.
#[test]
fn the_u_clinch_pair_is_not_found_as_a_via_trade_too() {
    let m = u_clinch_board();
    let both = loop_json(&m, &opts(&["u_clinch", "via_trade"], 0));
    assert_eq!(both["pattern_kinds"]["via_trade"][0], 0, "the U-clinch pair is not a via trade too: {}", both["pattern_kinds"]);
}

/// Verify tw7-moves r4 (survivor e_no_bump): the pour move is kept, so the cut's price on the
/// passes matters (the same assert as in pour_move_is_found_and_tried_and_needs_pour_cuts, here
/// alone and with the pour kind only).
#[test]
fn the_pour_move_is_kept_with_pour_cuts() {
    let m = pour_board();
    let o = Options { pour_cuts: true, ..opts(&["pour"], 0) };
    let j = loop_json(&m, &o);
    assert!(j["pattern_kinds"]["pour"][2].as_u64().unwrap() >= 1, "kept: {}", j["pattern_kinds"]);
}
