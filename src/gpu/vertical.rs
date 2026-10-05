//! Canonical vertical meteorology sampling on the GPU for issue #88.
//!
//! Ports the completed canonical vertical sampling semantics from #73 to actual
//! GPU execution, including the W/interface production semantics frozen by
//! #80, using the #30 runtime geometry and the repository-wide GPU contract
//! from #91.
//!
//! ## Scientific contract
//!
//! Model-level scalar sampling follows
//! `src/meteorology/vertical_sampling.rs` (#73), which ports FLEXPART 11.1
//! `interpol_mod.f90:215-242` (`find_z_level_meters`, METRE mode),
//! `:406-430` (`find_vert_vars_lin`) and `:539-547` (`vert_interpol`).
//! Vector fields (such as `WindU`/`WindV`) are sampled as independent
//! per-component scalar passes over the same column geometry; there is no
//! fused multi-component kernel, mirroring the scalar CPU entrypoint.
//! Center-staggered vertical motion uses the same single-stage sample kernel
//! via [`physical_center_w_column_from_runtime`].
//! Interface-staggered W follows the pristine `eta=no` two-stage production
//! path frozen by #80:
//!
//! ```text
//! canonical W/interface source (omega * pinmconv on wzlev, #30)
//! -> required production remap/transformation onto the shared
//!    [ground, model levels] height grid (WGSL, this module)
//! -> canonical shared vertical representation (device-resident)
//! -> particle vertical sample at particle height (WGSL, this module)
//! ```
//!
//! Direct interpolation on `wzlev` is deliberately not production behavior.
//! The two stages execute as separate WGSL dispatches with a device-resident
//! intermediate. Collapsing them would change floating-point order, boundary
//! pinning, staggering, or endpoint behavior.
//!
//! ## Host versus GPU responsibilities
//!
//! Host (Rust) owns all #30/#73/#80 validation and geometry preparation:
//! - runtime dimensions, ordering, provenance algorithm ids, staggering,
//!   shape agreement, finite and strictly increasing geometry, ground `0.0`,
//!   interior-domain and top-compatibility checks, terrain/ASL-to-AGL
//!   resolution, and every fail-closed rejection defined by #73/#80;
//! - extraction of physical bottom-to-top columns from the canonical #30
//!   `VerticalRuntimeView` without deriving a second coordinate system,
//!   without using legacy `WindFieldGrid::heights_m`, and without
//!   synthesizing another W height coordinate;
//! - conversion of validated `f64` oracle heights to `f32` device values,
//!   failing closed on non-finite conversion.
//!
//! The host never computes a supported production sampled value. Supported
//! numerical sampling/transformation executes in WGSL `f32` with the exact
//! FLEXPART clamping and weight order (`lower * weight_lower +
//! upper * weight_upper`).
//!
//! ## Composition contract
//!
//! - Reuses the existing [`crate::gpu::GpuContext`].
//! - Follows established persistent buffer/resource patterns: geometry and
//!   field resources have column/source lifetime, queries/outputs/uniforms
//!   have dispatch lifetime.
//! - Exposes `encode_*` into a caller-owned `wgpu::CommandEncoder` without
//!   submitting, waiting, or reading back, suitable for later #76 composition.
//! - Keeps geometry/field/remapped resources device-resident across many
//!   samples. No mandatory `GPU -> CPU -> GPU` round trip exists in the
//!   production composition path.
//! - Convenience `dispatch_*`/readback helpers submit, wait, and stage through
//!   `MAP_READ` only for isolated oracle validation. They must not be inserted
//!   between GPU-capable production stages.
//! - No silent CPU fallback exists for supported production inputs.
//! - Does not implement #76 composition or #77 consumer migration.
//!
//! Implementation responsibility map: `docs/agent/gpu-vertical-decomposition.md`.

mod dispatch;
mod encode;
mod error;
mod evidence;
mod pipeline;
mod preparation;
mod provenance;
mod resources;

pub use dispatch::{
    dispatch_vertical_remap_w_with_kernel, dispatch_vertical_sample_with_kernel,
    download_vertical_samples, sample_vertical_grid_gpu, sample_vertical_w_gpu_two_stage,
};
pub use encode::{
    encode_vertical_remap_w_with_kernel, encode_vertical_sample_with_kernel,
    encode_vertical_w_two_stage_with_kernels,
};
pub use error::GpuVerticalError;
pub use evidence::{
    build_vertical_model_gpu_row, build_vertical_w_gpu_row, VerticalGpuReport, VerticalGpuRow,
    VerticalWSourceLanes,
};
pub use pipeline::{VerticalSampleKernel, VerticalWRemapKernel};
pub use preparation::{
    physical_center_w_column_from_runtime, physical_model_column_from_runtime,
    physical_w_columns_from_runtime, resolve_query_heights_agl,
};
pub use provenance::{
    sha256_hex, vertical_geometry_identity, vertical_inputs_sha256,
    vertical_model_comparison_policy, vertical_remap_shader_sha256, vertical_sample_shader_sha256,
    vertical_w_bundle_shader_sha256, vertical_w_comparison_policy, vertical_w_inputs_sha256,
    VerticalModelOracleCase, VERTICAL_GPU_ABSOLUTE_TOLERANCE, VERTICAL_GPU_CANDIDATE_DESCRIPTION,
    VERTICAL_GPU_IMPLEMENTATION_ID, VERTICAL_GPU_RELATIVE_TOLERANCE_MODEL,
    VERTICAL_GPU_RELATIVE_TOLERANCE_W, VERTICAL_GPU_REPORT_SCHEMA_ID,
    VERTICAL_GPU_REPORT_SCHEMA_VERSION, VERTICAL_MODEL_ORACLE_IMPLEMENTATION_ID,
    VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_MODEL_LEVELS,
    VERTICAL_MODEL_ORACLE_OUTPUT_SHA256_REAL_COLUMN, VERTICAL_ORACLE_EXECUTABLE_SHA256,
    VERTICAL_ORACLE_REVISION, VERTICAL_W_ORACLE_BINARY_SHA256, VERTICAL_W_ORACLE_IMPLEMENTATION_ID,
    VERTICAL_W_ORACLE_OUTPUT_SHA256,
};
pub use resources::{
    create_vertical_grid_buffers, create_vertical_output_buffer, create_vertical_query_buffers,
    create_vertical_shared_grid, create_vertical_w_interface_inputs, VerticalGridBuffers,
    VerticalQueryBuffers, VerticalSampleOutput, VerticalWInterfaceInputs,
};

#[cfg(test)]
mod tests;
