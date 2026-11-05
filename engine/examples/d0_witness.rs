//! D0 drawing witness on one board: the hand topology drawn by our drawing.
//!
//!     cargo run --release --example d0_witness -- <hand.json> [--variant A|B|C] [--out d0.json]
//!         [--audit] [--tight] [--margin <nm>] [--via-hole <nm>]
//!
//! `hand.json` is the untouched board from `harness.g0 export`. Prints wires in / drawn /
//! rejected by reason; `--audit` adds the neighbour audit (packer positions against the drawn
//! crossings and copper gaps); `--tight` turns on the D0 tight-packing experiment (`Drawer::tight`); `--margin`
//! sets the drawing's clearance margin (`Options.margin_nm`); `--via-hole` the placed vias' drill
//! clearance (`Rules::via_hole_clearance`, for hand exports without it); `--out` writes the full
//! result.

use topor_engine::model::BoardModel;
use topor_engine::topo::dwitness::{run, DwOpts, PairAudit, Variant};

fn q(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn audit_line(name: &str, a: &[&PairAudit]) {
    let mm = |x: f64| x / 1e6;
    let mut slack: Vec<f64> = a.iter().map(|x| x.slack_nm).collect();
    let mut drawn: Vec<f64> = a.iter().map(|x| x.drawn_nm - x.req_nm).collect();
    let mut nom: Vec<f64> = a.iter().map(|x| x.nom_nm - x.req_nm).collect();
    let mut infl: Vec<f64> = a.iter().map(|x| x.req_nm - x.req_flat_nm).collect();
    let tight = a.iter().filter(|x| x.slack_nm < 10_000.0).count();
    println!(
        "  audit {name}: {} pairs; copper slack med {:+.3} p10 {:+.3} mm, tight(<10um) {:.1} %; drawn spacing - req med {:+.3}; nominal spacing - req med {:+.3}; oblique extra med {:+.3} mm",
        a.len(),
        mm(q(&mut slack, 0.5)),
        mm(q(&mut slack, 0.1)),
        100.0 * tight as f64 / a.len().max(1) as f64,
        mm(q(&mut drawn, 0.5)),
        mm(q(&mut nom, 0.5)),
        mm(q(&mut infl, 0.5)),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut hand, mut out, mut variant, mut show_audit, mut tight) = (None, None, Variant::A, false, false);
    let mut margin: Option<i64> = None;
    let mut via_hole: Option<i64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                out = Some(args[i + 1].clone());
                i += 1;
            }
            "--variant" => {
                variant = match args[i + 1].as_str() {
                    "A" | "a" => Variant::A,
                    "B" | "b" => Variant::B,
                    "C" | "c" => Variant::C,
                    v => panic!("variant {v}: A, B or C"),
                };
                i += 1;
            }
            "--audit" => show_audit = true,
            "--tight" => tight = true,
            "--margin" => {
                margin = Some(args[i + 1].parse::<i64>().expect("margin nm"));
                i += 1;
            }
            "--via-hole" => {
                via_hole = Some(args[i + 1].parse::<i64>().expect("via hole clearance nm"));
                i += 1;
            }
            a => hand = Some(a.to_string()),
        }
        i += 1;
    }
    let hp = hand.expect("hand.json");
    let s = std::fs::read_to_string(&hp).unwrap_or_else(|e| panic!("{hp}: {e}"));
    let mut m = BoardModel::from_json(&s).unwrap_or_else(|e| panic!("{hp}: {e}"));
    if let Some(v) = via_hole {
        // Hand exports older than D1 carry no via hole clearance (it then falls back to the PTH
        // floor in `hole_clearance`): set it as the run would (`toporlib.rules`).
        m.rules.via_hole_clearance = v;
    }
    let mut o = DwOpts { variant, tight, ..DwOpts::default() };
    if let Some(m) = margin {
        o.draw.margin_nm = m;
    }
    let r = run(&m, &o).unwrap_or_else(|e| panic!("d0: {e}"));
    println!(
        "{} variant {:?}: wires in {} (R0 excluded {}), drawn {}, rejected {}; vias ok {} failed {}; runs exact {} fallback {}; {:.1} s",
        r.board, variant, r.wires_in, r.excluded, r.drawn, r.rejected, r.vias_ok, r.vias_failed, r.exact_ok, r.fallback_ok, r.runtime_s
    );
    for (k, n) in &r.reasons {
        println!("  rejected {k}: {n}");
    }
    if show_audit {
        let mut pl: Vec<f64> = r.infl.iter().map(|x| x.0).collect();
        let mut hd: Vec<f64> = r.infl.iter().map(|x| x.1).collect();
        let big = r.infl.iter().filter(|x| x.0 > 1.5 * x.1).count();
        println!(
            "  oblique factor of {} passes on shared edge-layers: planned med {:.2} p90 {:.2}, hand direction med {:.2} p90 {:.2}; planned > 1.5 x hand: {} ({:.1} %); over-full edge-layers {} (with the hand's directions {}, {} of them with same-net neighbours)",
            r.infl.len(),
            q(&mut pl, 0.5),
            q(&mut pl, 0.9),
            q(&mut hd, 0.5),
            q(&mut hd, 0.9),
            big,
            100.0 * big as f64 / r.infl.len().max(1) as f64,
            r.over_lanes,
            r.over_lanes_hand_dir,
            r.over_lanes_hand_dir_same_net
        );
        let all: Vec<&PairAudit> = r.audit.iter().collect();
        audit_line("all", &all);
        let close: Vec<&PairAudit> = r.audit.iter().filter(|x| x.slack_nm < 0.6e6).collect();
        audit_line("gap<0.6mm", &close);
    }
    if let Some(p) = out {
        std::fs::write(&p, serde_json::to_string(&r).expect("json")).expect("write out");
    }
}
