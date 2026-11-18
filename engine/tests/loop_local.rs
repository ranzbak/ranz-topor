//! loop-local tests (docs/dev/design/loop-local.md).
//!
//! Step A: the per-wire stretch cache of the layerings ([`layers::StretchCache`]) gives the same
//! report bit for bit as no cache, over a sequence of states and labellings in which each of the
//! inputs it keys on changes alone (labels, a wire's path, a neighbour's polyline, the crossed
//! chords of an end site, the parameters).

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use topor_engine::model::BoardModel;
use topor_engine::topo::board::{self, Board, BuildOpts};
use topor_engine::topo::init::{Conn, InitWeights};
use topor_engine::topo::layers::{self, LayerParams, LayerReport, StretchCache};
use topor_engine::topo::state::State;

const MM: f64 = 1e6;

/// Two-layer board `w` x `h` mm with SMD rect pads `(ref, net, x, y, w, h, layer)` (layer 0 =
/// F.Cu, 2 = B.Cu, 99 = both) and nets 1..=`nets` (as tests/topo_d6b.rs).
fn board(w: f64, h: f64, nets: usize, pads: &[(&str, i32, f64, f64, f64, f64, u32)]) -> BoardModel {
    let p = |v: f64| (v * MM) as i64;
    let c = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let segs: Vec<String> = (0..4)
        .map(|i| {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            format!(r#"{{"kind":"seg","a":[{},{}],"b":[{},{}]}}"#, p(a.0), p(a.1), p(b.0), p(b.1))
        })
        .collect();
    let pads: Vec<String> = pads
        .iter()
        .map(|&(r, net, x, y, w, h, l)| {
            let ls: Vec<u32> = if l == 99 { vec![0, 2] } else { vec![l] };
            let shape = format!(r#"{{"kind":"rect","w":{},"h":{}}}"#, p(w), p(h));
            let shapes: Vec<String> = ls.iter().map(|l| format!(r#""{l}":{shape}"#)).collect();
            let ls: Vec<String> = ls.iter().map(|l| l.to_string()).collect();
            format!(
                r#"{{"uuid":"{r}","ref":"{r}","number":"1","net":{net},"attr":"smd","center":[{},{}],"rotation_deg":0.0,
                    "layers":[{}],"shapes":{{{}}}}}"#,
                p(x),
                p(y),
                ls.join(","),
                shapes.join(",")
            )
        })
        .collect();
    let nets: Vec<String> = (1..=nets).map(|i| format!(r#"{{"code":{i},"name":"N{i}"}}"#)).collect();
    let s = format!(
        r#"{{"units":"nm","board":"t",
            "layers":[{{"id":0,"name":"F.Cu","kind":"signal"}},{{"id":2,"name":"B.Cu","kind":"signal"}}],
            "nets":[{}],
            "pads":[{}],"outline":[{}],
            "rules":{{"default":{{"clearance":200000,"width":250000}},"min_connection":127000,"max_error":5000,"per_net":{{}}}}}}"#,
        nets.join(","),
        pads.join(","),
        segs.join(",")
    );
    BoardModel::from_json(&s).unwrap()
}

fn ob(m: &BoardModel, b: &Board, r: &str) -> usize {
    let i = m.pads.iter().position(|p| p.ref_ == r).unwrap();
    b.pad_of.iter().position(|&x| x == i).unwrap()
}

fn conn(m: &BoardModel, b: &Board, net: i32, ra: &str, rb: &str) -> Conn {
    let (pa, pb) = (ob(m, b, ra), ob(m, b, rb));
    Conn { net, pa, pb, a: b.centre[pa].unwrap(), b: b.centre[pb].unwrap(), w: 0.25 * MM, s: 0.2 * MM }
}

/// The parts of a report the stretch pass writes, as exact text (Debug of f64 round-trips).
fn stretch_text(r: &LayerReport) -> String {
    format!("{:?}|{:?}|{:?}|{:?}|{}|{}", r.stretches, r.events, r.via_pts, r.slots, r.vias, r.vias_no_site)
}

/// Random labels on the events of `base` (seeded): every event gets layer 0 or 1.
fn shuffled(base: &LayerReport, rng: &mut ChaCha8Rng, flip: f64) -> Vec<Vec<(usize, f64, i32)>> {
    base.events.iter().map(|ev| ev.iter().map(|&(k, s, l)| (k, s, if rng.gen_bool(flip) { 1 - l } else { l })).collect()).collect()
}

/// A crossing fan: three west-east nets and three north-south nets on a 2-layer board, plus a
/// later net D that shares the edges of the A wires (their polylines shift when it is laid).
fn fan() -> (BoardModel, Board) {
    let mut pads = vec![
        ("A1", 1, 2.0, 6.0, 1.0, 1.0, 0),
        ("A2", 1, 28.0, 6.0, 1.0, 1.0, 0),
        ("C1", 3, 2.0, 14.0, 1.0, 1.0, 2),
        ("C2", 3, 28.0, 14.0, 1.0, 1.0, 0),
        ("E1", 5, 2.0, 10.0, 1.0, 1.0, 0),
        ("E2", 5, 28.0, 11.0, 1.0, 1.0, 2),
        ("B1", 2, 9.0, 1.5, 1.0, 1.0, 2),
        ("B2", 2, 10.0, 18.5, 1.0, 1.0, 2),
        ("F1", 6, 16.0, 1.5, 1.0, 1.0, 0),
        ("F2", 6, 15.0, 18.5, 1.0, 1.0, 2),
        ("G1", 7, 22.0, 1.5, 1.0, 1.0, 0),
        ("G2", 7, 23.0, 18.5, 1.0, 1.0, 0),
        ("D1", 4, 2.0, 7.5, 1.0, 1.0, 0),
        ("D2", 4, 28.0, 7.5, 1.0, 1.0, 0),
    ];
    // A foreign pad on each layer in the middle (no via site there).
    pads.push(("K0", 8, 12.5, 10.0, 2.0, 2.0, 0));
    pads.push(("K1", 8, 19.5, 8.0, 2.0, 2.0, 2));
    let m = board(30.0, 20.0, 8, &pads);
    let b = board::build(&m, &BuildOpts { existing_vias: true, keepouts: true, copper_shapes: true, ..BuildOpts::default() }).unwrap();
    (m, b)
}

/// Step A (Rule 9b): over a sequence of states (wires laid one by one, a wire taken out and laid
/// again, a wire laid along the A wires' edges, a snapshot restored and a wire laid under the same
/// id along another path) and, per state, random labellings (also the previous state's own
/// labels, so a polyline changes under unchanged events), both `between_crossings` settings and
/// two via sizes far apart, the cached stretch pass equals the uncached one bit for bit, and the
/// cache is both hit (untouched wires) and missed (changed ones).
#[test]
fn stretch_cache_equals_no_cache_over_mutations() {
    let (m, b) = fan();
    let wt = InitWeights::default();
    let mut st = State::new(&b.proj);
    let cache = StretchCache::default();
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let smd = |o: usize| b.smd_layer(o);
    let mut checked = 0usize;
    let mut prev: Vec<Vec<(usize, f64, i32)>> = Vec::new();
    let mut check = |st: &State, rng: &mut ChaCha8Rng, prev: &mut Vec<Vec<(usize, f64, i32)>>| {
        for (between, r) in [(true, 0.5e6), (false, 0.5e6), (true, 0.05e6), (true, 3.0e6), (true, 0.5e6)] {
            let p = LayerParams { via_between: between, via_site_r: r, ..LayerParams::default() };
            let base = layers::assign(st, &smd, &p);
            let cached = layers::assign_cached(st, &smd, &p, None, Some(&cache));
            assert_eq!(stretch_text(&base), stretch_text(&cached), "assign: between {between} r {r}");
            // The previous state's labels where they still fit the wire (same pass count).
            let old: Vec<Vec<(usize, f64, i32)>> = (0..st.wires.len())
                .map(|w| match prev.get(w) {
                    Some(e) if st.wires[w].alive && e.iter().all(|x| x.0 <= st.wires[w].passes.len()) => e.clone(),
                    _ => base.events[w].clone(),
                })
                .collect();
            let mut lists = vec![old];
            for flip in [0.0, 0.2, 0.5, 0.5] {
                lists.push(shuffled(&base, rng, flip));
            }
            for evl in &lists {
                let (mut a, mut c) = (base.clone(), base.clone());
                layers::relabel(st, evl, &p, &mut a);
                layers::relabel_cached(st, evl, &p, &mut c, Some(&cache));
                assert_eq!(stretch_text(&a), stretch_text(&c), "relabel: between {between} r {r}");
                // The counts agree with the lists (every via has its point and slot).
                assert_eq!((a.vias, a.via_pts.len()), (a.slots.len(), a.slots.len()));
                assert!(a.vias_no_site <= a.vias);
                checked += 1;
            }
            *prev = base.events.clone();
        }
    };
    let order = [(1, "A1", "A2"), (2, "B1", "B2"), (3, "C1", "C2"), (6, "F1", "F2"), (5, "E1", "E2"), (7, "G1", "G2")];
    for &(net, ra, rb) in &order {
        st.route_conn(&conn(&m, &b, net, ra, rb), &wt, 1.0).expect("routed");
        check(&st, &mut rng, &mut prev);
    }
    // Take B out and lay it again after G (its triangles and the others' polylines change).
    let wb = (0..st.wires.len()).find(|&w| st.wires[w].alive && st.wires[w].net == 2).unwrap();
    topor_engine::topo::tree::remove_wire(&mut st, wb);
    check(&st, &mut rng, &mut prev);
    st.route_conn(&conn(&m, &b, 2, "B1", "B2"), &wt, 3.0).expect("B again");
    check(&st, &mut rng, &mut prev);
    // D along the A wire: A's passes move on the shared edges, A itself is not touched.
    let t0 = st.topology();
    st.route_conn(&conn(&m, &b, 4, "D1", "D2"), &wt, 1.0).expect("D routed");
    check(&st, &mut rng, &mut prev);
    // Back to before D, and D laid again under the same wire id along a costlier path.
    st.restore(t0);
    check(&st, &mut rng, &mut prev);
    st.route_conn(&conn(&m, &b, 4, "D1", "D2"), &wt, 6.0).expect("D again");
    check(&st, &mut rng, &mut prev);
    let (hits, misses) = cache.stats();
    assert!(checked >= 200, "{checked} labellings checked");
    assert!(hits > 0 && misses > 0, "hits {hits} misses {misses}");
}

/// Step A (Rule 9b): each input the cache keys on, changed alone on one wire of a state the cache
/// has seen (its net, one triangle, a turn, one label, the polyline under unchanged events, the
/// via size), changes the uncached result for at least one wire, and the cached pass then still
/// equals the uncached one (so the key holds that input).
#[test]
fn stretch_cache_sees_each_input_alone() {
    let (m, b) = fan();
    let wt = InitWeights::default();
    let mut st = State::new(&b.proj);
    for (net, ra, rb) in [(1, "A1", "A2"), (2, "B1", "B2"), (3, "C1", "C2"), (6, "F1", "F2"), (5, "E1", "E2"), (7, "G1", "G2")] {
        st.route_conn(&conn(&m, &b, net, ra, rb), &wt, 1.0).expect("routed");
    }
    let smd = |o: usize| b.smd_layer(o);
    let p = LayerParams { via_between: true, via_site_r: 0.5e6, ..LayerParams::default() };
    let base = layers::assign(&st, &smd, &p);
    let mut rng = ChaCha8Rng::seed_from_u64(11);
    let evl = shuffled(&base, &mut rng, 0.5);
    let live: Vec<usize> = (0..st.wires.len()).filter(|&w| st.wires[w].alive).collect();
    // One check: warm the cache on (st, evl, p), then the changed input (st2, evl2, p2) must give
    // the uncached result; whether that result differs from the unchanged one.
    // `p0`: the parameters of the warm-up (the changed input's own, except for the params check).
    let differs_full = |evl0: &[Vec<(usize, f64, i32)>], st2: &State, evl2: &[Vec<(usize, f64, i32)>], p0: &LayerParams, p2: &LayerParams| -> bool {
        let cache = StretchCache::default();
        let (mut w0, mut a, mut c) = (base.clone(), base.clone(), base.clone());
        layers::relabel_cached(&st, evl0, p0, &mut w0, Some(&cache));
        layers::relabel(st2, evl2, p2, &mut a);
        layers::relabel_cached(st2, evl2, p2, &mut c, Some(&cache));
        assert_eq!(stretch_text(&a), stretch_text(&c), "cached != uncached after a change");
        stretch_text(&a) != stretch_text(&w0)
    };
    let differs_at = |st2: &State, evl2: &[Vec<(usize, f64, i32)>], p0: &LayerParams, p2: &LayerParams| differs_full(&evl, st2, evl2, p0, p2);
    let differs = |st2: &State, evl2: &[Vec<(usize, f64, i32)>], p2: &LayerParams| differs_at(st2, evl2, &p, p2);
    let mut found: Vec<&str> = Vec::new();
    // Net: another net's obstacles become own ones (the free radius changes).
    let radii = [0.05e6, 0.2e6, 0.5e6, 1.0e6, 1.5e6, 2.0e6, 3.0e6];
    if live.iter().any(|&w| [8, 1, 2, 3].iter().any(|&n| n != st.wires[w].net && radii.iter().any(|&r| {
        let mut s2 = st.clone();
        s2.wires[w].net = n;
        let pr = LayerParams { via_site_r: r, ..p };
        differs_at(&s2, &evl, &pr, &pr)
    }))) {
        found.push("net");
    }
    // One triangle of the wire (a site centroid, an end-site triangle).
    let nt = b.proj.tris.len();
    if live.iter().any(|&w| (0..st.wires[w].tris.len()).any(|k| (1..4).any(|d| {
        let mut s2 = st.clone();
        s2.wires[w].tris[k] = (s2.wires[w].tris[k] + d * 7) % nt;
        differs(&s2, &evl, &p)
    }))) {
        found.push("tris");
    }
    // A turn in a traversal (an end site skips it): a via between two events inside neighbouring
    // traversals (no traversal strictly between, so its site is an end site).
    if live.iter().any(|&w| (0..st.wires[w].tris.len().saturating_sub(1)).any(|k| {
        let mut e = evl.clone();
        e[w] = vec![(k, 0.5, 0), (k + 1, 0.5, 1)];
        let mut s2 = st.clone();
        s2.wires[w].turns = vec![(k, st.wires[w].a), (k + 1, st.wires[w].a)];
        let pr = LayerParams { via_site_r: 0.05e6, ..p };
        differs_full(&e, &s2, &e, &pr, &pr)
    })) {
        found.push("turns");
    }
    // One label.
    if live.iter().any(|&w| (0..evl[w].len()).any(|i| {
        let mut e2 = evl.clone();
        e2[w][i].2 = 1 - e2[w][i].2;
        differs(&st, &e2, &p)
    })) {
        found.push("ev");
    }
    // The via size (the cache's parameters).
    if [0.05e6, 3.0e6].iter().any(|&r| differs(&st, &evl, &LayerParams { via_site_r: r, ..p })) {
        found.push("params");
    }
    // The polyline under unchanged events: D laid along A moves A's passes.
    {
        let mut s2 = st.clone();
        s2.route_conn(&conn(&m, &b, 4, "D1", "D2"), &wt, 1.0).expect("D routed");
        let mut e2 = evl.clone();
        e2.resize(s2.wires.len(), Vec::new());
        if differs(&s2, &e2, &p) {
            found.push("poly");
        }
    }
    assert_eq!(found, ["net", "tris", "turns", "ev", "params", "poly"], "inputs whose change was seen");
}
