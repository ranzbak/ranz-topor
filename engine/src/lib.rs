pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod cdt;
pub mod check;
pub mod control;
pub mod decompose;
pub mod fuzz;
pub mod geom;
pub mod lengths;
pub mod model;
pub mod obstacle;
pub mod pourcheck;
#[cfg(feature = "python")]
mod py;
pub mod route;
pub mod sketch;
pub mod taut;
pub mod stubs;
pub mod topo;
pub mod vias;
pub mod wires;

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_semver() {
        assert_eq!(VERSION.split('.').count(), 3);
    }
}
