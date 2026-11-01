/// Build margin default (23 §4.1), nm.
pub const EPS_BUILD_NM: f64 = 5000.0;
/// Arc sagitta below which an arc is emitted as a segment (23 §4.3), nm.
pub const MIN_SAGITTA_NM: f64 = 5.0;
/// Arc chord below which an arc is emitted as a segment (23 §4.3), nm.
pub const MIN_CHORD_NM: f64 = 10.0;

pub mod dist;
pub mod grid;
pub mod prim;
pub mod quant;
pub mod tangent;
pub mod vec2;

pub use dist::{
    arc_arc, copper_gap, copper_gap_skeleton, dist_prim, dist_prim_skeleton, point_arc,
    point_circle, point_seg, point_skeleton, seg_arc, seg_seg, skeleton_closest_point,
    skeleton_gap, Skeleton, Track,
};
pub use grid::{Grid, GridError};
pub use prim::{Aabb, Arc, Circle, Prim, Seg};
pub use quant::{
    calc_arc_center, dequantise, is_degenerate_arc, outline_prims, quantise, quantise_chain,
    round_nm, IVec2, Nm, TrackPrim, OUTLINE_SAGITTA_NM,
};
pub use tangent::{all_bitangents, bitangent, point_circle_tangent, GeomError};
pub use vec2::{Orient, Vec2};
