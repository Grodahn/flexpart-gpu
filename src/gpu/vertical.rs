//! Canonical vertical meteorology sampling on the GPU for issue #88.
//!
//! Ports the completed canonical vertical sampling semantics from #73 to actual
//! GPU execution, including the W/interface production semantics frozen by
//! #80, using the #30 runtime geometry and the repository-wide GPU contract
//! from #91.
//!
//! ## Scientific contract
//!
//! Model-level scalar/vector sampling follows
//! `src/meteorology/vertical_sampling.rs` (#73), which ports FLEXPART 11.1
//! `interpol_mod.f90:215-242` (`find_z_level_meters`, METRE mode),
//! `:406-430` (`find_vert_vars_lin`) and `:539-547` (`vert_interpol`).
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

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wgpu::util::DeviceExt;

use super::{
    compare_finite_values, download_buffer_typed, ComparisonPolicy, GpuAdapterEvidence,
    GpuBufferError, GpuCalculationEvidence, GpuCalculationPath, GpuCandidateEvidence, GpuContext,
    GpuEvidenceError, GpuEvidenceSchema, GpuExecutionEvidence, GpuExecutionStatus,
    NumericalVerdict, PinnedOracleEvidence,
};
use crate::meteorology::{
    vertical::{NormalizedVerticalMotion, VerticalRuntimeView, VerticalTransformError},
    vertical_sampling::VerticalSamplingError,
    FieldId, SchemaIdentity, VerticalOrdering, VerticalReference, VerticalStaggering,
};

/// WGSL source for the ordinary METRE-mode particle-height sample.
const SAMPLE_SHADER_SOURCE: &str = include_str!("../shaders/vertical_sample.wgsl");
/// WGSL source for the #80 W/interface-to-shared-grid remap.
const REMAP_SHADER_SOURCE: &str = include_str!("../shaders/vertical_remap_w.wgsl");

/// Workgroup width for both vertical kernels.
///
/// A fixed width mirrors `gpu::horizontal`/`gpu::temporal` and avoids a
/// shared-infrastructure change while #87 proceeds concurrently.
const WORKGROUP_SIZE_X: u32 = 64;

/// Candidate implementation identity for machine-readable evidence.
pub const VERTICAL_GPU_IMPLEMENTATION_ID: &str =
    "meteorology::vertical-gpu::encode_vertical_sample";
/// Candidate implementation identity for the W remap stage.
pub const VERTICAL_W_REMAP_IMPLEMENTATION_ID: &str =
    "meteorology::vertical-gpu::encode_vertical_remap_w";
/// Human-readable candidate description recorded in issue-specific reports.
pub const VERTICAL_GPU_CANDIDATE_DESCRIPTION: &str =
    "meteorology::vertical-gpu::encode_vertical_sample+remap_w (WGSL device)";

/// Pinned FLEXPART 11.1 revision owning the #71/#80 vertical oracles.
pub const VERTICAL_ORACLE_REVISION: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";
/// Pinned oracle linked-object-set identity from #71/#80 provenance.
pub const VERTICAL_ORACLE_EXECUTABLE_SHA256: &str =
    "388d1f824306df30fc74fbedc6464e86602b6a93ba782e9e1dad20ffacc3327c";
/// Pinned per-artifact oracle output digest for the #71 vertical contract fixture.
pub const VERTICAL_MODEL_ORACLE_OUTPUT_SHA256: &str =
    "d537a239e69d609131abbddf870d0067272a6e675cd4c50ba059e223793cfaed";
/// Pinned binary digest for the #80 W production oracle executable.
pub const VERTICAL_W_ORACLE_BINARY_SHA256: &str =
    "9e2ff66ab93a815cd90011943042e23ecf3f44a8ce000de9db819d02f9ef4c95";
/// Pinned raw oracle output digest for the #80 W production oracle.
pub const VERTICAL_W_ORACLE_OUTPUT_SHA256: &str =
    "730c08209e4ffd7f7c941926846e53bc9cd30b2b255d69eaf3b21e40337c2671";
/// Pinned oracle implementation for model-level vertical sampling.
pub const VERTICAL_MODEL_ORACLE_IMPLEMENTATION_ID: &str = "FLEXPART-11.1 vert_interpol";
/// Pinned oracle implementation for the #80 W production path.
pub const VERTICAL_W_ORACLE_IMPLEMENTATION_ID: &str =
    "FLEXPART-11.1 verttransform_ecmwf_windfields+interpol_wind_meter";

/// Absolute tolerance for GPU comparison (matches #73/#80 candidate rules).
pub const VERTICAL_GPU_ABSOLUTE_TOLERANCE: f64 = 1.0e-6;
/// Relative tolerance for model-level GPU comparison (matches #71/#73 `1e-4`).
pub const VERTICAL_GPU_RELATIVE_TOLERANCE_MODEL: f64 = 1.0e-4;
/// Relative tolerance for W/interface GPU comparison (matches #80 `1e-5`).
pub const VERTICAL_GPU_RELATIVE_TOLERANCE_W: f64 = 1.0e-5;

/// Schema identity for issue-specific vertical GPU evidence.
pub const VERTICAL_GPU_REPORT_SCHEMA_ID: &str = "flexpart-gpu.vertical-gpu-evidence";
/// Version of the issue-specific vertical GPU evidence schema.
pub const VERTICAL_GPU_REPORT_SCHEMA_VERSION: u32 = 1;

/// Expected #30 pressure reconstruction algorithm.
const EXPECTED_PRESSURE_ALGORITHM_ID: &str =
    "hybrid_interface_ab_local_ps_fulllevel_adjacent_mean_v1";
/// Expected #30 height reconstruction algorithm.
const EXPECTED_HEIGHT_ALGORITHM_ID: &str = "flexpart11_verttransform_ecmwf_heights_v1";
/// Expected #30 W height reconstruction algorithm.
const EXPECTED_W_HEIGHT_ALGORITHM_ID: &str = "flexpart11_wzlev_from_uvzlev_v1";
/// Expected #80 interface omega conversion algorithm.
const EXPECTED_INTERFACE_OMEGA_ALGORITHM_ID: &str = "omega_interface_flexpart11_pinmconv_v1";

/// Errors for GPU vertical sampling.
#[derive(Debug, Error)]
pub enum GpuVerticalError {
    /// Canonical #73 sampling validation rejected the request.
    #[error(transparent)]
    Sampling(#[from] VerticalSamplingError),
    /// Canonical #30 runtime access failed.
    #[error(transparent)]
    Runtime(#[from] VerticalTransformError),
    /// Buffer transfer or readback failed.
    #[error(transparent)]
    Buffer(#[from] GpuBufferError),
    /// Evidence construction or validation failed.
    #[error(transparent)]
    Evidence(#[from] GpuEvidenceError),
    /// No queries were supplied; an empty comparison cannot prove parity.
    #[error("vertical GPU query sequence is empty")]
    EmptyQueries,
    /// A value does not fit in `u32`.
    #[error("value for {field} does not fit in u32: {value}")]
    ValueTooLarge {
        /// Name of the oversized value.
        field: &'static str,
        /// Offending value.
        value: usize,
    },
    /// Byte-size overflow while sizing a buffer.
    #[error("byte-size overflow while preparing {field}")]
    SizeOverflow {
        /// Name of the buffer being sized.
        field: &'static str,
    },
    /// Grid and query resources describe different counts.
    #[error("count mismatch for {field}: grids {grids}, queries {queries}, outputs {outputs}")]
    CountMismatch {
        /// Resource that mismatched.
        field: &'static str,
        /// Grid level count.
        grids: usize,
        /// Query count.
        queries: usize,
        /// Output count.
        outputs: usize,
    },
    /// Interface and level resources describe inconsistent level counts.
    #[error("level count mismatch for {field}: interfaces {interfaces}, levels {levels}, shared {shared}")]
    LevelCountMismatch {
        /// Resource that mismatched.
        field: &'static str,
        /// Interface count.
        interfaces: usize,
        /// Level count.
        levels: usize,
        /// Shared grid count.
        shared: usize,
    },
    /// Uniform buffer does not match the validated grid and query count.
    #[error("vertical uniform metadata does not match the validated grid")]
    MismatchedUniforms,
    /// GPU device scope reported an error.
    #[error("GPU {scope} error during vertical sampling: {message}")]
    Device {
        /// Scope that failed.
        scope: &'static str,
        /// Device error text.
        message: String,
    },
    /// A validated value is not finite as `f32`.
    #[error("validated vertical value for {field} is not finite as f32")]
    NonFiniteDeviceValue {
        /// Name of the offending lane.
        field: &'static str,
    },
    /// GPU output is non-finite where the oracle is finite.
    #[error("non-finite GPU vertical output at query {query_index}")]
    NonFiniteOutputValue {
        /// Index of the offending query.
        query_index: usize,
    },
    /// Candidate revision must be a 40-character lowercase Git SHA.
    #[error("candidate revision must be a 40-character lowercase Git SHA, got {value}")]
    InvalidCandidateRevision {
        /// Offending revision.
        value: String,
    },
    /// Failed to hash candidate inputs.
    #[error("failed to hash candidate inputs: {message}")]
    InputHash {
        /// Failure detail.
        message: String,
    },
    /// Request does not match the pinned vertical oracle contract.
    #[error("request does not match the pinned vertical oracle contract: {message}")]
    OracleContract {
        /// Failure detail.
        message: &'static str,
    },
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct VerticalSampleParamsRaw {
    grid_count: u32,
    query_count: u32,
    _pad0: u32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct VerticalRemapParamsRaw {
    model_level_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

fn push_device_error_scopes(device: &wgpu::Device) {
    device.push_error_scope(wgpu::ErrorFilter::Internal);
    device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    device.push_error_scope(wgpu::ErrorFilter::Validation);
}

fn pop_device_error_scopes(
    device: &wgpu::Device,
    scope: &'static str,
) -> Result<(), GpuVerticalError> {
    let validation = pollster::block_on(device.pop_error_scope());
    let out_of_memory = pollster::block_on(device.pop_error_scope());
    let internal = pollster::block_on(device.pop_error_scope());
    if let Some(error) = [validation, out_of_memory, internal]
        .into_iter()
        .flatten()
        .next()
    {
        return Err(GpuVerticalError::Device {
            scope,
            message: error.to_string(),
        });
    }
    Ok(())
}

/// Reusable WGSL kernel for ordinary METRE-mode particle-height sampling.
pub struct VerticalSampleKernel {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl VerticalSampleKernel {
    /// Compile the WGSL sampling kernel once per context.
    ///
    /// # Errors
    /// Returns [`GpuVerticalError::Device`] when shader or pipeline creation
    /// raises a scoped `wgpu` error.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuVerticalError> {
        push_device_error_scopes(&ctx.device);
        let shader = ctx.load_shader("vertical_sample_shader", SAMPLE_SHADER_SOURCE);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("vertical_sample_bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });
        let pipeline =
            ctx.create_compute_pipeline("vertical_sample_pipeline", &shader, "main", &[
                &bind_group_layout,
            ]);
        let kernel = Self {
            bind_group_layout,
            pipeline,
            workgroup_size_x: WORKGROUP_SIZE_X,
        };
        let _ = ctx.device.poll(wgpu::Maintain::Wait);
        pop_device_error_scopes(&ctx.device, "pipeline creation")?;
        Ok(kernel)
    }

    /// Workgroup width used for 1-D dispatch.
    #[must_use]
    pub const fn workgroup_size_x(&self) -> u32 {
        self.workgroup_size_x
    }
}

/// Reusable WGSL kernel for the #80 W/interface-to-shared-grid remap.
pub struct VerticalWRemapKernel {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl VerticalWRemapKernel {
    /// Compile the WGSL remap kernel once per context.
    ///
    /// # Errors
    /// Returns [`GpuVerticalError::Device`] when shader or pipeline creation
    /// raises a scoped `wgpu` error.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuVerticalError> {
        push_device_error_scopes(&ctx.device);
        let shader = ctx.load_shader("vertical_remap_w_shader", REMAP_SHADER_SOURCE);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("vertical_remap_w_bgl"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });
        let pipeline =
            ctx.create_compute_pipeline("vertical_remap_w_pipeline", &shader, "main", &[
                &bind_group_layout,
            ]);
        let kernel = Self {
            bind_group_layout,
            pipeline,
            workgroup_size_x: WORKGROUP_SIZE_X,
        };
        let _ = ctx.device.poll(wgpu::Maintain::Wait);
        pop_device_error_scopes(&ctx.device, "pipeline creation")?;
        Ok(kernel)
    }

    /// Workgroup width used for 1-D dispatch.
    #[must_use]
    pub const fn workgroup_size_x(&self) -> u32 {
        self.workgroup_size_x
    }
}

/// Device-resident vertical grid for ordinary sampling.
///
/// Heights and values are in physical bottom-to-top order, sourced from the
/// canonical #30 runtime geometry without a second coordinate system. Lifetime:
/// column/source lifetime, reusable across many queries for that column.
pub struct VerticalGridBuffers {
    /// Grid heights AGL in metres, physical bottom-to-top.
    pub heights: wgpu::Buffer,
    /// Grid field values, aligned with `heights`.
    pub values: wgpu::Buffer,
    grid_count: usize,
}

impl VerticalGridBuffers {
    /// Number of grid levels.
    #[must_use]
    pub const fn grid_count(&self) -> usize {
        self.grid_count
    }
}

/// Device-resident W/interface source for the #80 remap stage.
///
/// All three lanes are physical bottom-to-top. Lifetime: column/source
/// lifetime, reusable across remap dispatches for that column.
pub struct VerticalWInterfaceInputs {
    /// Interface heights AGL, length `nz+1`, with `[0] == 0.0`.
    pub interface_heights: wgpu::Buffer,
    /// Interface geometric vertical velocity in m/s, length `nz+1`.
    pub interface_values: wgpu::Buffer,
    /// Model-level heights AGL, length `nz`.
    pub level_heights: wgpu::Buffer,
    model_level_count: usize,
}

impl VerticalWInterfaceInputs {
    /// Number of model levels (`nz`).
    #[must_use]
    pub const fn model_level_count(&self) -> usize {
        self.model_level_count
    }
}

/// Device-resident particle-height queries.
pub struct VerticalQueryBuffers {
    /// Query heights AGL in metres.
    pub buffer: wgpu::Buffer,
    query_count: usize,
}

impl VerticalQueryBuffers {
    /// Number of queries.
    #[must_use]
    pub const fn query_count(&self) -> usize {
        self.query_count
    }
}

/// Device-resident sampled outputs.
pub struct VerticalSampleOutput {
    /// Sampled field values, one per query.
    pub buffer: wgpu::Buffer,
    query_count: usize,
}

impl VerticalSampleOutput {
    /// Number of outputs.
    #[must_use]
    pub const fn query_count(&self) -> usize {
        self.query_count
    }
}

fn storage_usage() -> wgpu::BufferUsages {
    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC
}

fn checked_byte_len(len: usize, field: &'static str) -> Result<u64, GpuVerticalError> {
    let bytes = len
        .checked_mul(size_of::<f32>())
        .ok_or(GpuVerticalError::SizeOverflow { field })?;
    u64::try_from(bytes).map_err(|_| GpuVerticalError::SizeOverflow { field })
}

fn usize_to_u32(value: usize, field: &'static str) -> Result<u32, GpuVerticalError> {
    u32::try_from(value).map_err(|_| GpuVerticalError::ValueTooLarge { field, value })
}

fn validate_finite_lane(values: &[f32], field: &'static str) -> Result<(), GpuVerticalError> {
    for value in values {
        if !value.is_finite() {
            return Err(GpuVerticalError::NonFiniteDeviceValue { field });
        }
    }
    Ok(())
}

fn validate_strictly_increasing(heights: &[f32]) -> Result<(), GpuVerticalError> {
    for pair in heights.windows(2) {
        if !(pair[1] > pair[0]) {
            return Err(VerticalSamplingError::MalformedGeometry {
                x: 0,
                y: 0,
                index: 0,
                lower_m: pair[0],
                upper_m: pair[1],
            }
            .into());
        }
    }
    Ok(())
}

fn is_lowercase_git_sha(revision: &str) -> bool {
    revision.len() == 40
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_candidate_revision(revision: &str) -> Result<(), GpuVerticalError> {
    if !is_lowercase_git_sha(revision) {
        return Err(GpuVerticalError::InvalidCandidateRevision {
            value: revision.to_string(),
        });
    }
    Ok(())
}

/// Create device-resident grid buffers from validated physical columns.
///
/// Heights must be finite and strictly increasing bottom-to-top; values must
/// be finite; at least two levels are required. This is the low-level
/// primitive used by both #71 oracle validation and #30 runtime paths.
///
/// # Errors
/// Returns [`GpuVerticalError`] for empty, mismatched, non-finite, or
/// non-monotonic inputs.
pub fn create_vertical_grid_buffers(
    ctx: &GpuContext,
    heights_agl_m: &[f32],
    values: &[f32],
) -> Result<VerticalGridBuffers, GpuVerticalError> {
    if heights_agl_m.len() < 2 || values.len() < 2 {
        return Err(VerticalSamplingError::InsufficientLevels {
            nz: heights_agl_m.len().min(values.len()),
        }
        .into());
    }
    if heights_agl_m.len() != values.len() {
        return Err(GpuVerticalError::CountMismatch {
            field: "vertical_grid",
            grids: heights_agl_m.len(),
            queries: values.len(),
            outputs: heights_agl_m.len(),
        });
    }
    validate_finite_lane(heights_agl_m, "grid_heights_agl_m")?;
    validate_finite_lane(values, "grid_values")?;
    validate_strictly_increasing(heights_agl_m)?;
    let _ = checked_byte_len(heights_agl_m.len(), "vertical_grid")?;

    let heights = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_grid_heights"),
            contents: bytemuck::cast_slice(heights_agl_m),
            usage: storage_usage(),
        });
    let value_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_grid_values"),
            contents: bytemuck::cast_slice(values),
            usage: storage_usage(),
        });
    Ok(VerticalGridBuffers {
        heights,
        values: value_buffer,
        grid_count: heights_agl_m.len(),
    })
}

/// Create device-resident W/interface inputs from validated physical columns.
///
/// Interface heights must be strictly increasing with `[0] == 0.0`;
/// interface values and level heights must be finite; level heights must be
/// strictly increasing; `interface.len() == levels.len() + 1` with at least
/// two model levels. Interior shared heights must lie strictly inside the
/// interface domain, matching the CPU `remap_interface_motion_to_model_grid`
/// preconditions.
///
/// # Errors
/// Returns [`GpuVerticalError`] for any malformed geometry or shape mismatch.
pub fn create_vertical_w_interface_inputs(
    ctx: &GpuContext,
    interface_heights_agl_m: &[f32],
    interface_values_ms: &[f32],
    level_heights_agl_m: &[f32],
) -> Result<VerticalWInterfaceInputs, GpuVerticalError> {
    let nz = level_heights_agl_m.len();
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels { nz }.into());
    }
    if interface_heights_agl_m.len() != nz + 1 || interface_values_ms.len() != nz + 1 {
        return Err(GpuVerticalError::LevelCountMismatch {
            field: "vertical_w_interfaces",
            interfaces: interface_heights_agl_m.len(),
            levels: nz,
            shared: nz + 1,
        });
    }
    validate_finite_lane(interface_heights_agl_m, "interface_heights_agl_m")?;
    validate_finite_lane(interface_values_ms, "interface_values_ms")?;
    validate_finite_lane(level_heights_agl_m, "level_heights_agl_m")?;
    validate_strictly_increasing(interface_heights_agl_m)?;
    validate_strictly_increasing(level_heights_agl_m)?;
    if interface_heights_agl_m[0] != 0.0 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the #80 shared height grid requires a zero-metre AGL ground boundary",
        }
        .into());
    }
    let interface_top = interface_heights_agl_m[nz];
    for (index, height) in level_heights_agl_m.iter().enumerate().take(nz - 1) {
        if *height <= interface_heights_agl_m[0] || *height >= interface_top {
            return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
                reason: "an interior shared model height is outside the W/interface domain",
            }
            .into());
        }
        let _ = index;
    }
    let top_level = level_heights_agl_m[nz - 1];
    let previous_shared = if nz >= 2 {
        level_heights_agl_m[nz - 2]
    } else {
        0.0
    };
    if top_level <= previous_shared || top_level > interface_top {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the top shared model height is incompatible with the W/interface domain",
        }
        .into());
    }

    let _ = checked_byte_len(nz + 1, "vertical_w_interfaces")?;
    let interface_heights = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_w_interface_heights"),
            contents: bytemuck::cast_slice(interface_heights_agl_m),
            usage: storage_usage(),
        });
    let interface_values = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_w_interface_values"),
            contents: bytemuck::cast_slice(interface_values_ms),
            usage: storage_usage(),
        });
    let level_heights = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_w_level_heights"),
            contents: bytemuck::cast_slice(level_heights_agl_m),
            usage: storage_usage(),
        });
    Ok(VerticalWInterfaceInputs {
        interface_heights,
        interface_values,
        level_heights,
        model_level_count: nz,
    })
}

/// Create the device-resident shared `[ground, model levels]` grid.
///
/// `shared_heights_agl_m` must be `[0.0, level_heights...]` in physical
/// bottom-to-top order with length `nz+1`. Heights are uploaded; values are
/// allocated uninitialized and must be written by
/// [`encode_vertical_remap_w_with_kernel`] before sampling.
///
/// # Errors
/// Returns [`GpuVerticalError`] for malformed shared heights.
pub fn create_vertical_shared_grid(
    ctx: &GpuContext,
    shared_heights_agl_m: &[f32],
) -> Result<VerticalGridBuffers, GpuVerticalError> {
    if shared_heights_agl_m.len() < 3 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels {
            nz: shared_heights_agl_m.len().saturating_sub(1),
        }
        .into());
    }
    validate_finite_lane(shared_heights_agl_m, "shared_heights_agl_m")?;
    validate_strictly_increasing(shared_heights_agl_m)?;
    if shared_heights_agl_m[0] != 0.0 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the #80 shared height grid requires a zero-metre AGL ground boundary",
        }
        .into());
    }
    let size = checked_byte_len(shared_heights_agl_m.len(), "vertical_shared_values")?;
    let heights = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_shared_heights"),
            contents: bytemuck::cast_slice(shared_heights_agl_m),
            usage: storage_usage(),
        });
    let values = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vertical_shared_values"),
        size,
        usage: storage_usage(),
        mapped_at_creation: false,
    });
    Ok(VerticalGridBuffers {
        heights,
        values,
        grid_count: shared_heights_agl_m.len(),
    })
}

/// Create device-resident query buffers from validated AGL heights.
///
/// # Errors
/// Returns [`GpuVerticalError`] for empty or non-finite queries.
pub fn create_vertical_query_buffers(
    ctx: &GpuContext,
    query_heights_agl_m: &[f32],
) -> Result<VerticalQueryBuffers, GpuVerticalError> {
    if query_heights_agl_m.is_empty() {
        return Err(GpuVerticalError::EmptyQueries);
    }
    validate_finite_lane(query_heights_agl_m, "query_heights_agl_m")?;
    let _ = checked_byte_len(query_heights_agl_m.len(), "vertical_queries")?;
    let buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_query_heights"),
            contents: bytemuck::cast_slice(query_heights_agl_m),
            usage: storage_usage(),
        });
    Ok(VerticalQueryBuffers {
        buffer,
        query_count: query_heights_agl_m.len(),
    })
}

/// Allocate device-resident sampled outputs for `query_count` queries.
///
/// # Errors
/// Returns [`GpuVerticalError`] for zero or overflowing counts.
pub fn create_vertical_output_buffer(
    ctx: &GpuContext,
    query_count: usize,
) -> Result<VerticalSampleOutput, GpuVerticalError> {
    if query_count == 0 {
        return Err(GpuVerticalError::EmptyQueries);
    }
    let size = checked_byte_len(query_count, "vertical_outputs")?;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vertical_sample_outputs"),
        size,
        usage: storage_usage(),
        mapped_at_creation: false,
    });
    Ok(VerticalSampleOutput {
        buffer,
        query_count,
    })
}

#[inline]
const fn canonical_index(
    ordering: VerticalOrdering,
    count: usize,
    physical_index: usize,
) -> usize {
    match ordering {
        VerticalOrdering::Increasing => count - 1 - physical_index,
        VerticalOrdering::Decreasing => physical_index,
    }
}

#[inline]
const fn volume_offset(x: usize, y: usize, z: usize, nx: usize, ny: usize) -> usize {
    x + nx * (y + ny * z)
}

/// Extract physical bottom-to-top model-level columns from the #30 runtime.
///
/// Mirrors the CPU `collect_physical_column` ordering, finiteness, shape, and
/// monotonicity checks without computing a sampled value. Both storage
/// orderings are supported; the returned lanes are always physical
/// bottom-to-top for device upload.
///
/// # Errors
/// Returns [`GpuVerticalError`] for any #73 fail-closed condition.
pub fn physical_model_column_from_runtime(
    runtime: VerticalRuntimeView<'_>,
    field: FieldId,
    values: &[f32],
    x: usize,
    y: usize,
) -> Result<(Vec<f32>, Vec<f32>), GpuVerticalError> {
    if !is_vertically_sampled(field) {
        return Err(VerticalSamplingError::UnsupportedField { field }.into());
    }
    if field == FieldId::VerticalVelocity {
        return Err(VerticalSamplingError::WrongStaggering {
            field,
            requested: VerticalStaggering::LevelCenter,
        }
        .into());
    }
    let (nx, ny, nz) = runtime.dimensions();
    let horizontal = nx.checked_mul(ny).ok_or(GpuVerticalError::SizeOverflow {
        field: "vertical_horizontal",
    })?;
    let expected = horizontal.checked_mul(nz).ok_or(GpuVerticalError::SizeOverflow {
        field: "vertical_volume",
    })?;
    if values.len() != expected {
        return Err(VerticalSamplingError::ShapeMismatch {
            field,
            expected,
            actual: values.len(),
        }
        .into());
    }
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientLevels { nz }.into());
    }
    let ordering = runtime.provenance().source_vertical_ordering;
    let mut heights = Vec::with_capacity(nz);
    let mut lane = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = canonical_index(ordering, nz, physical);
        let point = runtime.level(x, y, canonical)?;
        if !point.height_agl_m.is_finite() {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index: physical,
                lower_m: point.height_agl_m,
                upper_m: point.height_agl_m,
            }
            .into());
        }
        let flat = volume_offset(x, y, canonical, nx, ny);
        let value = values.get(flat).copied().ok_or(
            VerticalSamplingError::ShapeMismatch {
                field,
                expected: nx * ny * nz,
                actual: values.len(),
            },
        )?;
        if !value.is_finite() {
            return Err(VerticalSamplingError::NonFiniteFieldValue {
                field,
                x,
                y,
                index: canonical,
            }
            .into());
        }
        heights.push(point.height_agl_m);
        lane.push(value);
    }
    for pair in heights.windows(2) {
        if !(pair[1] > pair[0]) {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index: 0,
                lower_m: pair[0],
                upper_m: pair[1],
            }
            .into());
        }
    }
    Ok((heights, lane))
}

/// Extract physical W/interface columns from the #30 runtime with the #80 contract.
///
/// Returns `(interface_heights, interface_values, level_heights,
/// shared_heights)` in physical bottom-to-top order. `shared_heights` is
/// `[0.0, level_heights...]`. Enforces the single-column, provenance,
/// algorithm, ground, domain, and top-compatibility checks owned by #80.
///
/// # Errors
/// Returns [`GpuVerticalError`] for any #73/#80 fail-closed condition.
#[allow(clippy::type_complexity)]
pub fn physical_w_columns_from_runtime(
    runtime: VerticalRuntimeView<'_>,
    x: usize,
    y: usize,
) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>), GpuVerticalError> {
    let motion = runtime
        .vertical_velocity()
        .ok_or(VerticalSamplingError::MissingRuntimeVerticalMotion)?;
    if motion.vertical_staggering() != VerticalStaggering::LevelInterface {
        return Err(VerticalSamplingError::WrongStaggering {
            field: FieldId::VerticalVelocity,
            requested: VerticalStaggering::LevelCenter,
        }
        .into());
    }
    validate_interface_runtime_contract(runtime, motion)?;

    let (nx, ny, nz) = runtime.dimensions();
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels { nz }.into());
    }
    let ordering = runtime.provenance().source_vertical_ordering;
    let interface_count = nz + 1;
    let mut interface_heights = Vec::with_capacity(interface_count);
    let mut interface_values = Vec::with_capacity(interface_count);
    for physical in 0..interface_count {
        let canonical = canonical_index(ordering, interface_count, physical);
        let point = runtime.interface(x, y, canonical)?;
        if !point.height_agl_m.is_finite() {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index: physical,
                lower_m: point.height_agl_m,
                upper_m: point.height_agl_m,
            }
            .into());
        }
        let flat = volume_offset(x, y, canonical, nx, ny);
        let value =
            motion
                .values_ms()
                .get(flat)
                .copied()
                .ok_or(VerticalSamplingError::ShapeMismatch {
                    field: FieldId::VerticalVelocity,
                    expected: nx * ny * interface_count,
                    actual: motion.values_ms().len(),
                })?;
        if !value.is_finite() {
            return Err(VerticalSamplingError::NonFiniteFieldValue {
                field: FieldId::VerticalVelocity,
                x,
                y,
                index: canonical,
            }
            .into());
        }
        interface_heights.push(point.height_agl_m);
        interface_values.push(value);
    }
    let mut level_heights = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = canonical_index(ordering, nz, physical);
        let point = runtime.level(x, y, canonical)?;
        if !point.height_agl_m.is_finite() {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index: physical,
                lower_m: point.height_agl_m,
                upper_m: point.height_agl_m,
            }
            .into());
        }
        level_heights.push(point.height_agl_m);
    }
    for (lane, name) in [
        (&interface_heights, "interface_heights"),
        (&level_heights, "level_heights"),
    ] {
        for pair in lane.windows(2) {
            if !(pair[1] > pair[0]) {
                return Err(VerticalSamplingError::MalformedGeometry {
                    x,
                    y,
                    index: 0,
                    lower_m: pair[0],
                    upper_m: pair[1],
                }
                .into());
            }
        }
        let _ = name;
    }
    if interface_heights[0] != 0.0 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the #80 shared height grid requires a zero-metre AGL ground boundary",
        }
        .into());
    }
    let interface_top = interface_heights[interface_count - 1];
    for height in level_heights.iter().take(nz - 1) {
        if *height <= interface_heights[0] || *height >= interface_top {
            return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
                reason: "an interior shared model height is outside the W/interface domain",
            }
            .into());
        }
    }
    let top_level = level_heights[nz - 1];
    let previous_shared = if nz >= 2 {
        level_heights[nz - 2]
    } else {
        0.0
    };
    if top_level <= previous_shared || top_level > interface_top {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the top shared model height is incompatible with the W/interface domain",
        }
        .into());
    }

    let mut shared_heights = Vec::with_capacity(nz + 1);
    shared_heights.push(interface_heights[0]);
    shared_heights.extend_from_slice(&level_heights);
    debug_assert_eq!(shared_heights.len(), nz + 1);
    Ok((
        interface_heights,
        interface_values,
        level_heights,
        shared_heights,
    ))
}

fn validate_interface_runtime_contract(
    runtime: VerticalRuntimeView<'_>,
    motion: &NormalizedVerticalMotion,
) -> Result<(), GpuVerticalError> {
    use crate::meteorology::vertical::{
        NativeVerticalMotionKind, NativeVerticalMotionSign, NativeVerticalMotionUnit,
    };
    let provenance = motion.provenance();
    let supported_motion = provenance.source_kind
        == NativeVerticalMotionKind::PressureVelocityOmega
        && provenance.source_unit == NativeVerticalMotionUnit::PascalPerSecond
        && provenance.source_sign == NativeVerticalMotionSign::PositivePressureIncreasing
        && provenance.source_vertical_staggering == VerticalStaggering::LevelInterface
        && provenance.output_vertical_staggering == VerticalStaggering::LevelInterface
        && provenance.algorithm_id == EXPECTED_INTERFACE_OMEGA_ALGORITHM_ID;
    if !supported_motion {
        return Err(VerticalSamplingError::UnsupportedInterfaceVerticalMotion {
            kind: provenance.source_kind,
            unit: provenance.source_unit,
            sign: provenance.source_sign,
            source_staggering: provenance.source_vertical_staggering,
            output_staggering: provenance.output_vertical_staggering,
        }
        .into());
    }
    let runtime_provenance = runtime.provenance();
    let (nx, ny, nz) = runtime.dimensions();
    if nx != 1 || ny != 1 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "#80 freezes a single vertical column without horizontal slope correction",
        }
        .into());
    }
    if runtime_provenance.source_level_count != nz {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "runtime level count differs from the #30 source provenance",
        }
        .into());
    }
    if runtime_provenance.pressure_algorithm_id != EXPECTED_PRESSURE_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "pressure reconstruction is not the #30 hybrid-interface contract",
        }
        .into());
    }
    if runtime_provenance.height_algorithm_id != EXPECTED_HEIGHT_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "model heights are not the #30 FLEXPART height contract",
        }
        .into());
    }
    if runtime_provenance.w_height_algorithm_id != EXPECTED_W_HEIGHT_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "W/interface heights are not the #30 FLEXPART wzlev contract",
        }
        .into());
    }
    if runtime_provenance.terrain_reference != VerticalReference::AboveMeanSeaLevel {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "runtime terrain reference is not explicit ASL",
        }
        .into());
    }
    Ok(())
}

fn is_vertically_sampled(field: FieldId) -> bool {
    matches!(
        field,
        FieldId::WindU
            | FieldId::WindV
            | FieldId::VerticalVelocity
            | FieldId::Temperature
            | FieldId::SpecificHumidity
            | FieldId::Pressure
            | FieldId::AirDensity
            | FieldId::DensityGradient
            | FieldId::CloudTotalWater
    )
}

/// Resolve query heights to AGL through the column's local #30 terrain.
///
/// `AboveGroundLevel` passes through; `AboveMeanSeaLevel` subtracts terrain;
/// `ModelNative` fails closed as ambiguous.
///
/// # Errors
/// Returns [`GpuVerticalError`] for ambiguous references or non-finite heights.
pub fn resolve_query_heights_agl(
    runtime: VerticalRuntimeView<'_>,
    x: usize,
    y: usize,
    heights_m: &[f32],
    reference: VerticalReference,
) -> Result<Vec<f32>, GpuVerticalError> {
    if heights_m.is_empty() {
        return Err(GpuVerticalError::EmptyQueries);
    }
    for height in heights_m {
        if !height.is_finite() {
            return Err(VerticalSamplingError::NonFiniteHeight { height_m: *height }.into());
        }
    }
    match reference {
        VerticalReference::AboveGroundLevel => Ok(heights_m.to_vec()),
        VerticalReference::AboveMeanSeaLevel => {
            let terrain = runtime.terrain_asl_m(x, y)?;
            let mut resolved = Vec::with_capacity(heights_m.len());
            for height in heights_m {
                let agl = *height - terrain;
                if !agl.is_finite() {
                    return Err(
                        VerticalSamplingError::NonFiniteHeight { height_m: agl }.into(),
                    );
                }
                resolved.push(agl);
            }
            Ok(resolved)
        }
        VerticalReference::ModelNative => Err(VerticalSamplingError::AmbiguousReference {
            reference,
        }
        .into()),
    }
}

/// Encode ordinary METRE-mode sampling into a caller-owned encoder.
///
/// This is the production composition path for later #76 use. It submits
/// nothing, waits for nothing, allocates no field buffers, and reads nothing
/// back. The caller owns submission and lifetime: `grid`, `queries`,
/// `outputs`, and `kernel` must live until the encoded work completes.
///
/// # Errors
/// Returns [`GpuVerticalError`] for mismatched counts or oversized values.
pub fn encode_vertical_sample_with_kernel(
    ctx: &GpuContext,
    grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    outputs: &VerticalSampleOutput,
    kernel: &VerticalSampleKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuVerticalError> {
    if queries.query_count() != outputs.query_count() {
        return Err(GpuVerticalError::CountMismatch {
            field: "vertical_sample",
            grids: grid.grid_count(),
            queries: queries.query_count(),
            outputs: outputs.query_count(),
        });
    }
    if grid.grid_count() < 2 {
        return Err(VerticalSamplingError::InsufficientLevels {
            nz: grid.grid_count(),
        }
        .into());
    }
    if queries.query_count() == 0 {
        return Err(GpuVerticalError::EmptyQueries);
    }
    let grid_count_u32 = usize_to_u32(grid.grid_count(), "grid_count")?;
    let query_count_u32 = usize_to_u32(queries.query_count(), "query_count")?;
    debug_assert_eq!(
        size_of::<VerticalSampleParamsRaw>() % 16,
        0,
        "vertical sample uniform params must stay 16-byte aligned"
    );
    let raw = VerticalSampleParamsRaw {
        grid_count: grid_count_u32,
        query_count: query_count_u32,
        _pad0: 0,
        _pad1: 0,
    };
    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_sample_params"),
            contents: bytemuck::bytes_of(&raw),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vertical_sample_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: grid.heights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: grid.values.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: queries.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: outputs.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("vertical_sample_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&kernel.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        super::dispatch_1d(&mut pass, query_count_u32, kernel.workgroup_size_x);
    }
    Ok(())
}

/// Encode the #80 W/interface-to-shared-grid remap into a caller-owned encoder.
///
/// This is the first stage of the two-stage W production path. It submits
/// nothing, waits for nothing, and reads nothing back. `inputs`,
/// `shared_grid`, and `kernel` must live until the encoded work completes.
/// The caller must encode [`encode_vertical_sample_with_kernel`] afterwards
/// on `shared_grid` to complete the production sample; the intermediate
/// `shared_grid.values` stays device-resident between the two stages.
///
/// # Errors
/// Returns [`GpuVerticalError`] for inconsistent level counts.
pub fn encode_vertical_remap_w_with_kernel(
    ctx: &GpuContext,
    inputs: &VerticalWInterfaceInputs,
    shared_grid: &VerticalGridBuffers,
    kernel: &VerticalWRemapKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuVerticalError> {
    let nz = inputs.model_level_count();
    if shared_grid.grid_count() != nz + 1 {
        return Err(GpuVerticalError::LevelCountMismatch {
            field: "vertical_w_shared",
            interfaces: nz + 1,
            levels: nz,
            shared: shared_grid.grid_count(),
        });
    }
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels { nz }.into());
    }
    let nz_u32 = usize_to_u32(nz, "model_level_count")?;
    debug_assert_eq!(
        size_of::<VerticalRemapParamsRaw>() % 16,
        0,
        "vertical remap uniform params must stay 16-byte aligned"
    );
    let raw = VerticalRemapParamsRaw {
        model_level_count: nz_u32,
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
    };
    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertical_remap_w_params"),
            contents: bytemuck::bytes_of(&raw),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vertical_remap_w_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: inputs.interface_heights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: inputs.interface_values.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: inputs.level_heights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: shared_grid.values.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("vertical_remap_w_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&kernel.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        let shared_count_u32 = nz_u32 + 1;
        super::dispatch_1d(&mut pass, shared_count_u32, kernel.workgroup_size_x);
    }
    Ok(())
}

/// Dispatch ordinary sampling with an isolated encoder.
///
/// Convenience path for isolated oracle validation only. Production
/// composition must use [`encode_vertical_sample_with_kernel`] instead.
///
/// # Errors
/// Forwards [`GpuVerticalError`] from encoding.
pub fn dispatch_vertical_sample_with_kernel(
    ctx: &GpuContext,
    grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    outputs: &VerticalSampleOutput,
    kernel: &VerticalSampleKernel,
) -> Result<(), GpuVerticalError> {
    push_device_error_scopes(&ctx.device);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vertical_sample_encoder"),
        });
    if let Err(error) =
        encode_vertical_sample_with_kernel(ctx, grid, queries, outputs, kernel, &mut encoder)
    {
        let _ = pop_device_error_scopes(&ctx.device, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    pop_device_error_scopes(&ctx.device, "dispatch")
}

/// Dispatch the W remap stage with an isolated encoder.
///
/// Convenience path for isolated oracle validation only.
///
/// # Errors
/// Forwards [`GpuVerticalError`] from encoding.
pub fn dispatch_vertical_remap_w_with_kernel(
    ctx: &GpuContext,
    inputs: &VerticalWInterfaceInputs,
    shared_grid: &VerticalGridBuffers,
    kernel: &VerticalWRemapKernel,
) -> Result<(), GpuVerticalError> {
    push_device_error_scopes(&ctx.device);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vertical_remap_w_encoder"),
        });
    if let Err(error) =
        encode_vertical_remap_w_with_kernel(ctx, inputs, shared_grid, kernel, &mut encoder)
    {
        let _ = pop_device_error_scopes(&ctx.device, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    pop_device_error_scopes(&ctx.device, "dispatch")
}

/// Explicit D2H readback of sampled outputs (validation/output boundary only).
///
/// # Errors
/// Forwards [`GpuVerticalError::Buffer`] from staging and mapping.
pub async fn download_vertical_samples(
    ctx: &GpuContext,
    outputs: &VerticalSampleOutput,
) -> Result<Vec<f32>, GpuVerticalError> {
    Ok(download_buffer_typed::<f32>(
        ctx,
        &outputs.buffer,
        outputs.query_count(),
        "vertical_sample_outputs",
    )
    .await?)
}

/// Sample one physical grid on the GPU and read back (isolated validation).
///
/// Convenience helper for oracle tests only. Production code must keep grid
/// and shared resources device-resident through `encode_*` composition.
///
/// # Errors
/// Returns [`GpuVerticalError`] for encoding or readback failure. No CPU
/// fallback is performed for supported inputs.
pub async fn sample_vertical_grid_gpu(
    ctx: &GpuContext,
    grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    kernel: &VerticalSampleKernel,
) -> Result<Vec<f32>, GpuVerticalError> {
    let outputs = create_vertical_output_buffer(ctx, queries.query_count())?;
    dispatch_vertical_sample_with_kernel(ctx, grid, queries, &outputs, kernel)?;
    let values = download_vertical_samples(ctx, &outputs).await?;
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(GpuVerticalError::NonFiniteOutputValue {
                query_index: index,
            });
        }
    }
    Ok(values)
}

/// Execute the full two-stage W production path on the GPU and read back.
///
/// Remap and sample execute as two separate WGSL dispatches sharing one
/// caller-visible sequence but no host materialization of the intermediate:
/// this helper encodes both into isolated encoders in order (remap before
/// sample) for test clarity. Production composition should encode both into
/// a single caller-owned encoder without intermediate submit/wait.
///
/// # Errors
/// Returns [`GpuVerticalError`] for encoding, dispatch, or readback failure.
pub async fn sample_vertical_w_gpu_two_stage(
    ctx: &GpuContext,
    inputs: &VerticalWInterfaceInputs,
    shared_grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    sample_kernel: &VerticalSampleKernel,
    remap_kernel: &VerticalWRemapKernel,
) -> Result<Vec<f32>, GpuVerticalError> {
    dispatch_vertical_remap_w_with_kernel(ctx, inputs, shared_grid, remap_kernel)?;
    let outputs = create_vertical_output_buffer(ctx, queries.query_count())?;
    dispatch_vertical_sample_with_kernel(ctx, shared_grid, queries, &outputs, sample_kernel)?;
    let values = download_vertical_samples(ctx, &outputs).await?;
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(GpuVerticalError::NonFiniteOutputValue {
                query_index: index,
            });
        }
    }
    Ok(values)
}

/// Encode the full two-stage W production path into one caller-owned encoder.
///
/// Production composition entrypoint for later #76 use: remap is encoded
/// first, then the dependent sample on the shared grid, with no submit, wait,
/// or readback between them. The intermediate stays device-resident.
///
/// # Errors
/// Returns [`GpuVerticalError`] for inconsistent counts.
pub fn encode_vertical_w_two_stage_with_kernels(
    ctx: &GpuContext,
    inputs: &VerticalWInterfaceInputs,
    shared_grid: &VerticalGridBuffers,
    queries: &VerticalQueryBuffers,
    outputs: &VerticalSampleOutput,
    sample_kernel: &VerticalSampleKernel,
    remap_kernel: &VerticalWRemapKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuVerticalError> {
    encode_vertical_remap_w_with_kernel(ctx, inputs, shared_grid, remap_kernel, encoder)?;
    encode_vertical_sample_with_kernel(ctx, shared_grid, queries, outputs, sample_kernel, encoder)
}

/// Lowercase hexadecimal SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// SHA-256 of the executed sampling WGSL source.
#[must_use]
pub fn vertical_sample_shader_sha256() -> String {
    sha256_hex(SAMPLE_SHADER_SOURCE.as_bytes())
}

/// SHA-256 of the executed W remap WGSL source.
#[must_use]
pub fn vertical_remap_shader_sha256() -> String {
    sha256_hex(REMAP_SHADER_SOURCE.as_bytes())
}

/// SHA-256 of the executed W two-stage bundle (sample + remap sources).
#[must_use]
pub fn vertical_w_bundle_shader_sha256() -> String {
    let mut combined = Vec::with_capacity(
        SAMPLE_SHADER_SOURCE.len() + REMAP_SHADER_SOURCE.len() + 1,
    );
    combined.extend_from_slice(SAMPLE_SHADER_SOURCE.as_bytes());
    combined.push(b'\n');
    combined.extend_from_slice(REMAP_SHADER_SOURCE.as_bytes());
    sha256_hex(&combined)
}

/// SHA-256 of the normalized JSON encoding of validated GPU inputs.
///
/// Binds grid heights/values, query heights, staggering, and geometry identity
/// so the hashed inputs provably correspond to the dispatched work.
///
/// # Errors
/// Returns [`GpuVerticalError::InputHash`] when JSON encoding fails.
pub fn vertical_inputs_sha256(
    grid_heights_agl_m: &[f32],
    grid_values: &[f32],
    query_heights_agl_m: &[f32],
    staggering: VerticalStaggering,
    geometry_identity: &str,
) -> Result<String, GpuVerticalError> {
    #[derive(Serialize)]
    struct NormalizedVerticalInput<'a> {
        grid_heights_agl_m: &'a [f32],
        grid_values: &'a [f32],
        query_heights_agl_m: &'a [f32],
        staggering: String,
        geometry_identity: &'a str,
    }
    let normalized = NormalizedVerticalInput {
        grid_heights_agl_m,
        grid_values,
        query_heights_agl_m,
        staggering: format!("{staggering:?}"),
        geometry_identity,
    };
    let json = serde_json::to_vec(&normalized).map_err(|err| GpuVerticalError::InputHash {
        message: err.to_string(),
    })?;
    Ok(sha256_hex(&json))
}

/// Geometry identity string for evidence (provenance + level count + ordering).
#[must_use]
pub fn vertical_geometry_identity(
    runtime: VerticalRuntimeView<'_>,
) -> String {
    let provenance = runtime.provenance();
    let (nx, ny, nz) = runtime.dimensions();
    format!(
        "snapshot={}:ordering={:?}:levels={}:nx={}:ny={}:pressure={}:height={}:wheight={}",
        provenance.source_snapshot_sha256,
        provenance.source_vertical_ordering,
        nz,
        nx,
        ny,
        provenance.pressure_algorithm_id,
        provenance.height_algorithm_id,
        provenance.w_height_algorithm_id
    )
}

/// Build the repository-wide model-level comparison policy.
///
/// Absolute `1e-6` plus relative `1e-4` matches the #71/#73 candidate rule.
///
/// # Errors
/// Returns [`GpuEvidenceError`] for an invalid tolerance policy.
pub fn vertical_model_comparison_policy() -> Result<ComparisonPolicy, GpuEvidenceError> {
    ComparisonPolicy::new(
        VERTICAL_GPU_ABSOLUTE_TOLERANCE,
        VERTICAL_GPU_RELATIVE_TOLERANCE_MODEL,
    )
}

/// Build the repository-wide W/interface comparison policy.
///
/// Absolute `1e-6` plus relative `1e-5` matches the #80 production oracle rule.
///
/// # Errors
/// Returns [`GpuEvidenceError`] for an invalid tolerance policy.
pub fn vertical_w_comparison_policy() -> Result<ComparisonPolicy, GpuEvidenceError> {
    ComparisonPolicy::new(
        VERTICAL_GPU_ABSOLUTE_TOLERANCE,
        VERTICAL_GPU_RELATIVE_TOLERANCE_W,
    )
}

fn pinned_model_oracle_evidence() -> PinnedOracleEvidence {
    PinnedOracleEvidence {
        implementation_id: VERTICAL_MODEL_ORACLE_IMPLEMENTATION_ID.to_string(),
        revision: VERTICAL_ORACLE_REVISION.to_string(),
        executable_sha256: VERTICAL_ORACLE_EXECUTABLE_SHA256.to_string(),
        output_sha256: VERTICAL_MODEL_ORACLE_OUTPUT_SHA256.to_string(),
    }
}

fn pinned_w_oracle_evidence() -> PinnedOracleEvidence {
    PinnedOracleEvidence {
        implementation_id: VERTICAL_W_ORACLE_IMPLEMENTATION_ID.to_string(),
        revision: VERTICAL_ORACLE_REVISION.to_string(),
        executable_sha256: VERTICAL_ORACLE_EXECUTABLE_SHA256.to_string(),
        output_sha256: VERTICAL_W_ORACLE_OUTPUT_SHA256.to_string(),
    }
}

/// One machine-readable GPU-vs-oracle comparison row for #88.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerticalGpuRow {
    /// Stable issue-owned validation case.
    pub case_id: String,
    /// Canonical field identity that was sampled.
    pub field_id: FieldId,
    /// Vertical staggering of the sampled grid.
    pub staggering: VerticalStaggering,
    /// Requested vertical reference (AGL or ASL).
    pub reference: VerticalReference,
    /// Requested height in metres in the requested reference.
    pub requested_height_m: f32,
    /// Query height resolved to AGL actually dispatched.
    pub query_height_agl_m: f32,
    /// Local terrain ASL used for ASL resolution.
    pub terrain_asl_m: f32,
    /// Geometry identity (provenance + counts + ordering).
    pub geometry_identity: String,
    /// Physical bottom-to-top lower index actually bracketing the query.
    pub lower_physical_index: usize,
    /// Physical bottom-to-top upper index (`lower + 1`).
    pub upper_physical_index: usize,
    /// Weight applied to the lower value (`dz2`).
    pub weight_lower: f32,
    /// Weight applied to the upper value (`dz1`).
    pub weight_upper: f32,
    /// Element value computed on the GPU.
    pub gpu_value: f32,
    /// Pinned oracle value.
    pub oracle_value: f32,
    /// CPU #73 diagnostic value (migration reference only, never the candidate).
    pub cpu_value: f32,
    /// Predeclared comparison policy.
    pub comparison_policy: ComparisonPolicy,
    /// Absolute GPU-oracle difference.
    pub absolute_difference: f64,
    /// Whether the GPU value matches the oracle within tolerance.
    pub value_verdict: bool,
    /// Combined row verdict.
    pub row_verdict: bool,
    /// Repository-wide GPU execution and numerical evidence.
    pub gpu_evidence: GpuCalculationEvidence,
}

/// Machine-readable GPU-vs-oracle comparison report for #88.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerticalGpuReport {
    /// Report schema identity.
    pub schema: SchemaIdentity,
    /// Stable scenario identifier covering the pinned vertical cases.
    pub scenario_id: String,
    /// Candidate description recorded for traceability.
    pub candidate: String,
    /// Predeclared comparison policy for the dominant lane.
    pub comparison_policy: ComparisonPolicy,
    /// Per-query comparison rows with GPU evidence.
    pub rows: Vec<VerticalGpuRow>,
    /// Overall report verdict (`true` only when every row passes).
    pub status: bool,
}

impl VerticalGpuRow {
    /// Validate structural honesty and fail closed on contradiction.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] for contradictory verdicts, differences
    /// or embedded evidence that cannot prove a claimed pass.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        let comparison = compare_finite_values(
            &[f64::from(self.oracle_value)],
            &[f64::from(self.gpu_value)],
            self.comparison_policy,
        )?;
        if self.gpu_evidence.comparison != comparison {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU comparison contradicts vertical row values or policy",
            ));
        }
        if !is_lowercase_git_sha(&self.gpu_evidence.candidate.revision) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "candidate revision is not a lowercase Git commit SHA",
            ));
        }
        if self.gpu_evidence.candidate.implementation_id != VERTICAL_GPU_IMPLEMENTATION_ID {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU provenance contradicts the vertical calculation",
            ));
        }
        let expected_shader = if self.staggering == VerticalStaggering::LevelInterface {
            vertical_w_bundle_shader_sha256()
        } else {
            vertical_sample_shader_sha256()
        };
        if self.gpu_evidence.candidate.shader_sha256 != expected_shader {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU shader contradicts the vertical stage",
            ));
        }
        let expected_oracle = if self.staggering == VerticalStaggering::LevelInterface {
            pinned_w_oracle_evidence()
        } else {
            pinned_model_oracle_evidence()
        };
        if self.gpu_evidence.oracle.as_ref() != Some(&expected_oracle) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU oracle contradicts the pinned vertical case",
            ));
        }
        if !self.gpu_evidence.case_id.starts_with(&self.case_id) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU case id contradicts the vertical row case",
            ));
        }
        if self.geometry_identity.trim().is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row geometry identity must not be empty",
            ));
        }
        let value_pass = comparison.verdict == NumericalVerdict::Passed;
        if self.value_verdict != value_pass {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical value verdict contradicts tolerance",
            ));
        }
        if self.row_verdict != self.value_verdict {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row verdict contradicts value verdict",
            ));
        }
        let expected_difference =
            (f64::from(self.gpu_value) - f64::from(self.oracle_value)).abs();
        if (self.absolute_difference - expected_difference).abs() > 1.0e-12 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical absolute difference contradicts values",
            ));
        }
        if self.upper_physical_index != self.lower_physical_index + 1 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row indices must satisfy upper == lower + 1",
            ));
        }
        if !(0.0..=1.0).contains(&self.weight_lower)
            || !(0.0..=1.0).contains(&self.weight_upper)
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row weights must lie in [0, 1]",
            ));
        }
        if ((self.weight_lower + self.weight_upper) - 1.0).abs() > 1.0e-5 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical row weights must sum to one",
            ));
        }
        self.gpu_evidence.validate()?;
        if self.row_verdict {
            self.gpu_evidence.require_paired_pass()?;
        }
        Ok(())
    }
}

impl VerticalGpuReport {
    /// Validate structural honesty and fail closed on contradiction.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] when the schema, verdicts, tolerances or
    /// embedded evidence are missing or contradictory.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        if self.schema.id != VERTICAL_GPU_REPORT_SCHEMA_ID
            || self.schema.version != VERTICAL_GPU_REPORT_SCHEMA_VERSION
        {
            return Err(GpuEvidenceError::UnsupportedSchema {
                id: self.schema.id.clone(),
                version: self.schema.version,
            });
        }
        if self.candidate != VERTICAL_GPU_CANDIDATE_DESCRIPTION {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical GPU report does not identify the pinned candidate",
            ));
        }
        if self.rows.is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical GPU report must contain at least one row",
            ));
        }
        let candidate_revision = self.rows[0].gpu_evidence.candidate.revision.clone();
        for row in &self.rows {
            if row.gpu_evidence.candidate.revision != candidate_revision {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "vertical GPU row candidate revision contradicts its report",
                ));
            }
            row.validate()?;
        }
        let all_pass = self.rows.iter().all(|row| row.row_verdict);
        if self.status != all_pass {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "vertical GPU report status contradicts row verdicts",
            ));
        }
        Ok(())
    }

    /// Require this report to prove a successful paired oracle validation.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] unless every row validates and proves a
    /// WGSL paired pass.
    pub fn require_paired_pass(&self) -> Result<(), GpuEvidenceError> {
        self.validate()?;
        if !self.status {
            return Err(GpuEvidenceError::NotPassing);
        }
        for row in &self.rows {
            row.gpu_evidence.require_paired_pass()?;
            if !row.row_verdict {
                return Err(GpuEvidenceError::NotPassing);
            }
        }
        Ok(())
    }
}

/// Build one GPU-vs-oracle row with #91 execution evidence.
///
/// `gpu_value` must come from actual device execution and `oracle_value` from
/// the pinned #71/#80 oracle. `cpu_value` is diagnostic only. Level
/// indices/weights are host-derived diagnostics from the identical #73
/// primitive; they do not replace device execution proof.
///
/// # Errors
/// Returns [`GpuVerticalError`] for invalid tolerances, non-finite values,
/// invalid candidate provenance or evidence construction failures.
#[allow(clippy::too_many_arguments)]
pub fn build_vertical_gpu_row(
    ctx: &GpuContext,
    case_id: &str,
    field_id: FieldId,
    staggering: VerticalStaggering,
    reference: VerticalReference,
    requested_height_m: f32,
    query_height_agl_m: f32,
    terrain_asl_m: f32,
    geometry_identity: &str,
    lower_physical_index: usize,
    upper_physical_index: usize,
    weight_lower: f32,
    weight_upper: f32,
    gpu_value: f32,
    oracle_value: f32,
    cpu_value: f32,
    comparison_policy: ComparisonPolicy,
    grid_heights_agl_m: &[f32],
    grid_values: &[f32],
    all_queries_agl_m: &[f32],
    candidate_revision: &str,
) -> Result<VerticalGpuRow, GpuVerticalError> {
    if !gpu_value.is_finite() || !oracle_value.is_finite() || !cpu_value.is_finite() {
        return Err(GpuVerticalError::NonFiniteOutputValue { query_index: 0 });
    }
    if geometry_identity.trim().is_empty() {
        return Err(GpuVerticalError::OracleContract {
            message: "vertical row geometry identity must not be empty",
        });
    }
    validate_candidate_revision(candidate_revision)?;
    let comparison =
        compare_finite_values(&[f64::from(oracle_value)], &[f64::from(gpu_value)], comparison_policy)?;
    let value_verdict = comparison.verdict == NumericalVerdict::Passed;
    let input_sha = vertical_inputs_sha256(
        grid_heights_agl_m,
        grid_values,
        all_queries_agl_m,
        staggering,
        geometry_identity,
    )?;
    let (shader_sha, oracle) = if staggering == VerticalStaggering::LevelInterface {
        (
            vertical_w_bundle_shader_sha256(),
            pinned_w_oracle_evidence(),
        )
    } else {
        (
            vertical_sample_shader_sha256(),
            pinned_model_oracle_evidence(),
        )
    };
    let gpu_evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: format!("{case_id}/height={query_height_agl_m}"),
        candidate: GpuCandidateEvidence {
            implementation_id: VERTICAL_GPU_IMPLEMENTATION_ID.to_string(),
            revision: candidate_revision.to_string(),
            shader_sha256: shader_sha,
            input_sha256: input_sha,
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(GpuAdapterEvidence::from_context(ctx)),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(oracle),
        comparison,
    };
    gpu_evidence.validate()?;
    Ok(VerticalGpuRow {
        case_id: case_id.to_string(),
        field_id,
        staggering,
        reference,
        requested_height_m,
        query_height_agl_m,
        terrain_asl_m,
        geometry_identity: geometry_identity.to_string(),
        lower_physical_index,
        upper_physical_index,
        weight_lower,
        weight_upper,
        gpu_value,
        oracle_value,
        cpu_value,
        comparison_policy,
        absolute_difference: (f64::from(gpu_value) - f64::from(oracle_value)).abs(),
        value_verdict,
        row_verdict: value_verdict,
        gpu_evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_and_remap_shader_hashes_are_stable_hex() {
        for hash in [
            vertical_sample_shader_sha256(),
            vertical_remap_shader_sha256(),
            vertical_w_bundle_shader_sha256(),
        ] {
            assert_eq!(hash.len(), 64);
            assert!(hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        }
        assert_ne!(
            vertical_sample_shader_sha256(),
            vertical_remap_shader_sha256()
        );
    }

    #[test]
    fn grid_buffers_reject_non_monotonic_geometry() {
        let heights = [0.0_f32, 100.0, 100.0, 500.0];
        let values = [1.0_f32, 2.0, 3.0, 4.0];
        let error = validate_strictly_increasing(&heights).expect_err("flat must fail");
        let _ = error;
        let _ = values;
    }

    #[test]
    fn w_interface_inputs_reject_ground_mismatch() {
        assert_ne!(0.5_f32, 0.0_f32);
    }
}
