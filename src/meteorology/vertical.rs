//! Stable canonical #30 facade from immutable meteorology Snapshots to runtime geometry.
//!
//! Private owners separate pressure, model levels, W/interfaces, motion and terrain.
//! All consumers use this boundary; no interpolation or sampling occurs here.
//! The opaque derived state is serialized for evidence and borrowed for sampling.
//! See `docs/vertical-transform.md` and `docs/agent/vertical-geometry-decomposition.md`.

mod error;
mod eta_dot;
mod interfaces;
mod layout;
mod model_levels;
mod motion;
mod pressure;
mod provenance;
mod runtime;
mod terrain;

#[cfg(test)]
mod test_support;

pub use error::VerticalTransformError;
pub use eta_dot::{
    eta_dot_to_pressure_velocity, EtaDotPressureVelocity,
    FLEX_EXTRACT_CALC_ETADOT_REFERENCE_PRESSURE_PA,
};
pub use model_levels::reconstruct_vertical_geometry;
pub use motion::{
    reconstruct_vertical_geometry_with_motion, NativeVerticalMotion, NativeVerticalMotionKind,
    NativeVerticalMotionProvenance, NativeVerticalMotionSign, NativeVerticalMotionUnit,
    NormalizedVerticalMotion, NormalizedVerticalMotionProvenance,
};
pub use pressure::{reconstruct_hybrid_pressure, HybridPressureGrid};
pub use provenance::VerticalTransformProvenance;
pub use runtime::{
    VerticalInterfacePoint, VerticalLevelPoint, VerticalRuntimeView, VerticalTransformResult,
};
pub use terrain::{
    height_agl_to_asl, height_asl_to_agl, resolve_release_height, resolve_release_height_at_column,
    resolve_release_height_range, resolve_release_height_range_at_column, ResolvedReleaseHeight,
    ResolvedReleaseHeightRange,
};
