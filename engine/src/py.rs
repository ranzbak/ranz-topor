//! PyO3 module `topor_engine`: plain strings / numbers in, plain strings / numbers out.
//! Every function releases the GIL while it computes.

use crate::check::Checker;
use crate::control::{Job, JobError, RunControl};
use crate::geom::prim::Aabb;
use crate::geom::quant::{calc_arc_center, IVec2};
use crate::geom::{Grid, Vec2};
use crate::model::{BoardModel, Item, Options};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;
use std::time::Instant;

#[pyfunction]
fn version() -> &'static str {
    crate::VERSION
}

fn check_impl(model_json: &str, items_json: &str, options_json: &str) -> Result<String, String> {
    let model = BoardModel::from_json(model_json)?;
    let opts = Options::from_json(options_json)?;
    let items = Item::list_from_json(items_json)?;
    let rep = Checker::new(&model, &opts).check_items(&items);
    serde_json::to_string(&rep).map_err(|e| e.to_string())
}

/// `CheckReport` JSON for `items_json` checked against `model_json`.
#[pyfunction]
fn check_items(
    py: Python<'_>,
    model_json: String,
    items_json: String,
    options_json: String,
) -> PyResult<String> {
    py.allow_threads(|| check_impl(&model_json, &items_json, &options_json))
        .map_err(PyValueError::new_err)
}

/// `{"items": [...], "from_arc": [...], "truncated": bool, "attempts": int}` (Items in the
/// KiCad frame); seeded, deterministic, self-checked unless `skip_self_check` is set.
#[pyfunction]
fn fuzz_items(
    py: Python<'_>,
    model_json: String,
    options_json: String,
    seed: u64,
    n: usize,
) -> PyResult<String> {
    py.allow_threads(|| {
        let model = BoardModel::from_json(&model_json)?;
        let opts = Options::from_json(&options_json)?;
        let res = crate::fuzz::fuzz_items(&model, &opts, seed, n);
        serde_json::to_string(&res).map_err(|e| e.to_string())
    })
    .map_err(PyValueError::new_err)
}

/// `RouteResult` JSON: topological routing of the unconnected nets (see `route::route`).
#[pyfunction]
fn route(py: Python<'_>, model_json: String, options_json: String) -> PyResult<String> {
    py.allow_threads(|| {
        let model = BoardModel::from_json(&model_json)?;
        let opts = Options::from_json(&options_json)?;
        let res = crate::route::route(&model, &opts);
        serde_json::to_string(&res).map_err(|e| e.to_string())
    })
    .map_err(PyValueError::new_err)
}

/// The fail-fast probe ([`crate::topo::core::probe`]): the board projection a route builds first,
/// without routing. Returns its JSON (`ok`, `error`, `notes`, `path`, `seconds`, ...).
#[pyfunction]
fn probe_board(py: Python<'_>, model_json: String, options_json: String) -> PyResult<String> {
    py.allow_threads(|| {
        let model = BoardModel::from_json(&model_json)?;
        let opts = Options::from_json(&options_json)?;
        serde_json::to_string(&crate::topo::core::probe(&model, &opts)).map_err(|e| e.to_string())
    })
    .map_err(PyValueError::new_err)
}

/// Options of the engine as JSON: `Options::default()` (every field, nested `expert` included).
#[pyfunction]
fn default_options() -> PyResult<String> {
    serde_json::to_string(&Options::default()).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Via sizes of net `net` as JSON `[[d, drill], ...]` (largest first): `vias::via_sizes` for the
/// `rules` object (the model's `rules` JSON) and `options_json`. Used by the plugin / harness
/// tests that keep the Python pre-flight in agreement with the engine.
#[pyfunction]
fn via_sizes(rules_json: String, net: i32, options_json: String) -> PyResult<String> {
    let rules: crate::model::Rules =
        serde_json::from_str(&rules_json).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let opts = Options::from_json(&options_json).map_err(PyValueError::new_err)?;
    let out: Vec<[i64; 2]> = crate::vias::via_sizes(&rules, net, &opts)
        .into_iter()
        .map(|s| [s.d, s.drill])
        .collect();
    serde_json::to_string(&out).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// A routing run on its own thread: `progress()` (JSON of `control::Progress`), `cancel()`,
/// `reach_limit()` (M2.TL: the caller's clock reached the limit),
/// `set_time_limit(seconds)` / `time_limit()` (the live wall-clock limit, seconds since the job
/// started, `0` / `None` = no limit; `control::TimeLimit`), `done()`, `wait()` (blocks with the GIL
/// released; the `RouteResult` JSON, idempotent).
/// `done()` is also true after the routing thread panicked (gate review B-1, ruling C31); `wait()`
/// then raises `RuntimeError("routing thread panicked: <message>")`; a failed run (result not
/// serialisable) raises `ValueError`. Complexity of every method except `wait`: O(1).
#[pyclass]
struct RouteJob {
    ctl: RunControl,
    job: Job,
}

#[pymethods]
impl RouteJob {
    fn progress(&self) -> PyResult<String> {
        serde_json::to_string(&self.ctl.snapshot())
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    fn cancel(&self) {
        self.ctl.request_cancel();
    }

    /// Change the wall-clock limit while the job runs: seconds since the job started (`0` = no
    /// limit; below the time spent = stop at the next check, keeping the best result, with the
    /// stop reason of the time limit). The interactive plugin's progress window only.
    fn set_time_limit(&self, seconds: f64) {
        self.ctl.set_time_limit(seconds);
    }

    /// M2.TL: the caller's own clock (the plugin's CPU clock) reached its limit: the engine's soft
    /// limit is reached now (the step that runs finishes, the best kept variant is drawn; not a
    /// change of the limit, not a Cancel). O(1).
    fn reach_limit(&self) {
        self.ctl.limit.reach();
    }

    /// F3 `[run] timeout_overrun_s` on the caller's own clock (the plugin's CPU clock): the soft
    /// limit was overrun by the cap: it acts as the hard one from now on (the step that runs is
    /// cut, what exists is drawn, the result's limit report says so). O(1).
    fn harden(&self) {
        self.ctl.limit.reach();
        self.ctl.limit.harden();
    }

    /// The wall-clock limit now (seconds since the job started); `None` = no limit.
    fn time_limit(&self) -> Option<f64> {
        self.ctl.limit.seconds()
    }

    fn done(&self) -> bool {
        self.job.done()
    }

    fn wait(&self, py: Python<'_>) -> PyResult<String> {
        py.allow_threads(|| self.job.wait()).map_err(|e| match e {
            JobError::Failed(m) => PyValueError::new_err(m),
            JobError::Panicked(m) => {
                PyRuntimeError::new_err(format!("routing thread panicked: {m}"))
            }
        })
    }
}

/// Start a routing run on a background thread (see [`RouteJob`]); the model and options are
/// parsed here, so bad JSON raises immediately.
#[pyfunction]
fn route_async(model_json: String, options_json: String) -> PyResult<RouteJob> {
    let model = BoardModel::from_json(&model_json).map_err(PyValueError::new_err)?;
    let opts = Options::from_json(&options_json).map_err(PyValueError::new_err)?;
    let ctl = RunControl::default();
    ctl.limit.init(opts.wall_clock_s); // the clock starts with the job; set_time_limit may change it
    let c2 = ctl.clone();
    let job = Job::spawn(move || {
        let res = crate::route::route_with(&model, &opts, &c2);
        serde_json::to_string(&res).map_err(|e| e.to_string())
    });
    Ok(RouteJob { ctl, job })
}

/// TEST HOOK, not part of the plugin API: `topor` never calls it and nothing in `topor.toml` or the
/// engine `Options` reaches it. A [`RouteJob`] whose thread panics with `message`, so the Python
/// tests drive the REAL job object through a panic (gate review B-1). O(1).
#[pyfunction]
fn _route_async_panicking(message: String) -> RouteJob {
    let job = Job::spawn(move || -> Result<String, String> { panic!("{message}") });
    RouteJob {
        ctl: RunControl::default(),
        job,
    }
}

/// `WireStats` JSON of the hand copper of `model_json` (length gate reference).
#[pyfunction]
fn wire_stats(py: Python<'_>, model_json: String) -> PyResult<String> {
    py.allow_threads(|| {
        let model = BoardModel::from_json(&model_json)?;
        let res = crate::route::wire_stats(&model);
        serde_json::to_string(&res).map_err(|e| e.to_string())
    })
    .map_err(PyValueError::new_err)
}

/// Centre of the circle through three integer points, rounded to nm; `None` if collinear.
#[pyfunction]
fn arc_center_nm(
    py: Python<'_>,
    sx: i32,
    sy: i32,
    mx: i32,
    my: i32,
    ex: i32,
    ey: i32,
) -> Option<(i32, i32)> {
    py.allow_threads(|| {
        calc_arc_center(
            IVec2 { x: sx, y: sy },
            IVec2 { x: mx, y: my },
            IVec2 { x: ex, y: ey },
        )
        .map(|c| (c.x, c.y))
    })
}

fn probe(n: usize, queries: usize, seed: u64) -> HashMap<String, f64> {
    // Constant density: the board side grows with sqrt(n), 2 mm per item.
    let side = (n as f64).sqrt().ceil() * 2_000_000.0;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let rect = |x: f64, y: f64, l: f64| Aabb {
        min: Vec2 { x, y },
        max: Vec2 {
            x: x + l,
            y: y + 200_000.0,
        },
    };
    let mut grid: Grid<u32> = Grid::new(1_000_000.0);
    for i in 0..n {
        let (x, y) = (rng.gen_range(0.0..side), rng.gen_range(0.0..side));
        grid.insert(rect(x, y, 500_000.0), i as u32);
    }
    let qs: Vec<Aabb> = (0..queries)
        .map(|_| {
            rect(
                rng.gen_range(0.0..side),
                rng.gen_range(0.0..side),
                500_000.0,
            )
        })
        .collect();
    let mut best = f64::INFINITY;
    let mut visited = 0usize;
    for _ in 0..5 {
        let t = Instant::now();
        visited = 0;
        for q in &qs {
            visited += grid.query_counted(*q).1;
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    HashMap::from([
        (
            "candidates_per_query".to_string(),
            visited as f64 / queries.max(1) as f64,
        ),
        ("seconds".to_string(), best),
    ])
}

/// `{"candidates_per_query": float, "seconds": float}` for `queries` random grid queries over
/// `n` uniformly scattered items at constant density (best of five passes).
#[pyfunction]
fn grid_probe(py: Python<'_>, n: usize, queries: usize, seed: u64) -> HashMap<String, f64> {
    py.allow_threads(|| probe(n, queries, seed))
}

#[pymodule]
fn topor_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(check_items, m)?)?;
    m.add_function(wrap_pyfunction!(fuzz_items, m)?)?;
    m.add_function(wrap_pyfunction!(route, m)?)?;
    m.add_function(wrap_pyfunction!(route_async, m)?)?;
    m.add_function(wrap_pyfunction!(probe_board, m)?)?;
    m.add_function(wrap_pyfunction!(_route_async_panicking, m)?)?;
    m.add_function(wrap_pyfunction!(default_options, m)?)?;
    m.add_function(wrap_pyfunction!(via_sizes, m)?)?;
    m.add_class::<RouteJob>()?;
    m.add_function(wrap_pyfunction!(wire_stats, m)?)?;
    m.add_function(wrap_pyfunction!(arc_center_nm, m)?)?;
    m.add_function(wrap_pyfunction!(grid_probe, m)?)?;
    Ok(())
}
