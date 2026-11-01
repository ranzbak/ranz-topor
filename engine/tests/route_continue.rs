//! Continuation rounds (`Options::continue_rounds`): the connections still failed after the
//! first pass are routed again against everything routed so far as fixed copper.
//!
//! Fixture: the dcsgone USB-C crop (`fixtures/dcsgone-usb-crop.json`, see `route_steiner.rs`).

use topor_engine::check::Checker;
use topor_engine::model::{BoardModel, Options};
use topor_engine::route::{route, RouteResult};

const CROP: &str = include_str!("fixtures/dcsgone-usb-crop.json");
const EPROM: &str = include_str!("fixtures/eprom-to-kernal-ripped.json");

fn opts(rounds: u32, threads: usize) -> Options {
    Options {
        wall_clock_s: 600.0,
        threads,
        continue_rounds: rounds,
        ..Options::default()
    }
}

fn strip(r: &RouteResult) -> String {
    let mut v = serde_json::to_value(r).unwrap();
    let o = v.as_object_mut().unwrap();
    o.remove("runtime_s");
    o.remove("threads_used");
    o.remove("profile");
    serde_json::to_string(&v).unwrap()
}

/// Pad pairs of the routed connections (labels, unordered).
fn routed_pairs(r: &RouteResult) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = r
        .connections
        .iter()
        .filter(|c| c.status == "routed")
        .map(|c| {
            let (a, b) = (c.from.label.clone(), c.to.label.clone());
            if a <= b { (a, b) } else { (b, a) }
        })
        .collect();
    v.sort();
    v
}

/// The rounds route more of the crop, every connection routed without them stays routed (the
/// first pass is the same; rounds only add), the warnings name each round, and the result is
/// clean at margin 0.
#[test]
fn continue_rounds_route_more_and_keep_the_first_pass() {
    let m = BoardModel::from_json(CROP).unwrap();
    let off = route(&m, &opts(0, 1));
    let on = route(&m, &opts(2, 1));
    assert!(on.routed > off.routed, "{} -> {}", off.routed, on.routed);
    assert!(on.warnings.iter().any(|w| w.starts_with("continue round 1:")), "{:?}", on.warnings);
    assert!(!off.warnings.iter().any(|w| w.starts_with("continue round")));
    let on_pairs = routed_pairs(&on);
    for p in routed_pairs(&off) {
        assert!(on_pairs.contains(&p), "{p:?} lost");
    }
    let rep = Checker::new(&m, &Options { margin_nm: 0, ..Options::default() }).check_items(&on.items);
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// Threads 1 / 2 / 4: byte-identical with the rounds on; eprom (all routed in the first pass)
/// is unchanged by them.
#[test]
fn continue_rounds_deterministic_and_noop_when_all_routed() {
    let m = BoardModel::from_json(CROP).unwrap();
    let want = strip(&route(&m, &opts(2, 1)));
    for t in [2usize, 4] {
        assert_eq!(strip(&route(&m, &opts(2, t))), want, "threads {t}");
    }
    let e = BoardModel::from_json(EPROM).unwrap();
    assert_eq!(strip(&route(&e, &opts(0, 1))), strip(&route(&e, &opts(2, 1))));
}

/// Three rounds by default (older option JSON too); 0 switches them off.
#[test]
fn continue_rounds_default_three() {
    assert_eq!(Options::default().continue_rounds, 3);
    assert_eq!(Options::from_json("{}").unwrap().continue_rounds, 3);
    assert_eq!(Options::from_json(r#"{ "continue_rounds": 0 }"#).unwrap().continue_rounds, 0);
}
