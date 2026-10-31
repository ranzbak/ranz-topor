//! PyO3 module `topor_engine`: plain strings / numbers in, plain strings / numbers out.
//! Every function releases the GIL while it computes.

use crate::check::Checker;
use crate::geom::prim::Aabb;
use crate::geom::quant::{calc_arc_center, IVec2};
use crate::geom::{Grid, Vec2};
use crate::model::{BoardModel, Item, Options};
use pyo3::exceptions::PyValueError;
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

/// `SmoothResult` JSON: taut smoothing of the hand copper of `model_json` (see `smooth::smooth`).
#[pyfunction]
fn smooth(py: Python<'_>, model_json: String, options_json: String) -> PyResult<String> {
    py.allow_threads(|| {
        let model = BoardModel::from_json(&model_json)?;
        let opts = Options::from_json(&options_json)?;
        let res = crate::smooth::smooth(&model, &opts);
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
    m.add_function(wrap_pyfunction!(smooth, m)?)?;
    m.add_function(wrap_pyfunction!(route, m)?)?;
    m.add_function(wrap_pyfunction!(wire_stats, m)?)?;
    m.add_function(wrap_pyfunction!(arc_center_nm, m)?)?;
    m.add_function(wrap_pyfunction!(grid_probe, m)?)?;
    Ok(())
}
