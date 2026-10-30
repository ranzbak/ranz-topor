pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod cdt;
pub mod check;
pub mod fuzz;
pub mod geom;
pub mod model;
pub mod obstacle;
#[cfg(feature = "python")]
mod py;
pub mod relax;
pub mod sketch;
pub mod taut;
pub mod wires;

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_semver() {
        assert_eq!(VERSION.split('.').count(), 3);
    }
}
