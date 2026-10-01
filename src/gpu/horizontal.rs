//! Canonical horizontal meteorology interpolation on the GPU for issue #87.
//!
//! Ports the #72 CPU semantics (frozen #71 FLEXPART 11.1 `find_grid_indices`,
//! `find_grid_distances` and `hor_interpol_4d`, `interpol_mod.f90:128-188` and
//! `:481-504`) to actual WGSL/device execution. Reuses `GpuContext`, explicit
//! storage-buffer conventions and the #91 `GpuCalculationEvidence` model.
//! Production composition uses `encode_horizontal_samples` (caller-owned
//! encoder, no submit/wait/readback/field allocation). Convenience
//! dispatch/readback helpers exist for isolated oracle validation only. No
//! silent CPU fallback exists for supported inputs.
//!
//! ## Host versus GPU responsibilities
//!
//! Host (Rust) owns canonical validation exactly as defined by #72:
//! - grid metadata, periodicity (`nx * dx == 360`), staggering (`CellCenter`
//!   only), field shape (`nx * ny` in x-fastest order), longitude convention
//!   and supported-domain checks;
//! - geographic `lon/lat -> xt/yt` mapping with exact-endpoint snapping, so
//!   provider metadata never enters the kernel;
//! - conversion of validated `f64` coordinates to `f32` device queries, failing
//!   closed on non-finite conversion;
//! - derivation of expected indices/weights for evidence diagnostics only.
//!
//! The host never computes the production sampled value. Supported bilinear
//! combination executes in WGSL:
//! ```text
//! p1 = (1-ddx)*(1-ddy), p2 = ddx*(1-ddy), p3 = (1-ddx)*ddy, p4 = ddx*ddy
//! value = p1*f(ix,jy) + p2*f(ixp,jy) + p3*f(ix,jyp) + p4*f(ixp,jyp)
//! ```
//! with x-fastest `offset = x + nx * y`, periodic ghost `nx -> 0` and exact
//! last-row/column collapse exactly as #72.
//!
//! ## Composition contract
//!
//! - Field resources are device-resident with bracket/source lifetime, uploaded
//!   once when the source changes, not once per query.
//! - Query/output/uniform resources have dispatch lifetime.
//! - `encode_*` records into a caller-owned encoder without submitting,
//!   polling, mapping or reading back, so later #76 composition can consume
//!   device-resident outputs without `GPU -> CPU -> GPU`.
//! - `dispatch_*`/readback helpers submit, wait and stage through `MAP_READ`
//!   only for isolated oracle validation and must not sit between composed
//!   GPU stages.
//! - No provider-specific decoding enters this layer.

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
use crate::meteorology::horizontal::{
    sample_horizontal, sample_horizontal_geographic, validate_horizontal_grid, HorizontalError,
    HorizontalSample,
};
use crate::meteorology::{HorizontalGrid, HorizontalStaggering, SchemaIdentity};

/// WGSL source for canonical horizontal bilinear sampling.
const SHADER_SOURCE: &str = include_str!("../shaders/horizontal_interpolation.wgsl");

/// Workgroup width for the horizontal kernel.
///
/// A fixed width mirrors `gpu::temporal` and avoids a shared-infrastructure
/// change while #88 proceeds concurrently.
const WORKGROUP_SIZE_X: u32 = 64;

/// Candidate implementation identity for machine-readable evidence.
pub const HORIZONTAL_GPU_IMPLEMENTATION_ID: &str =
    "meteorology::horizontal-gpu::encode_horizontal_samples";

/// Human-readable candidate description recorded in issue-specific reports.
pub const HORIZONTAL_GPU_CANDIDATE_DESCRIPTION: &str =
    "meteorology::horizontal-gpu::encode_horizontal_samples (WGSL device)";

/// Pinned FLEXPART routine owning the horizontal oracle.
pub const HORIZONTAL_ORACLE_IMPLEMENTATION_ID: &str = "FLEXPART-11.1 hor_interpol_4d";

/// Pinned FLEXPART 11.1 revision owning the #71 horizontal oracle.
pub const HORIZONTAL_ORACLE_REVISION: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";

/// Pinned oracle linked-object-set identity from #71 provenance.
pub const HORIZONTAL_ORACLE_EXECUTABLE_SHA256: &str =
    "388d1f824306df30fc74fbedc6464e86602b6a93ba782e9e1dad20ffacc3327c";

/// Pinned per-case oracle output digests from #71 provenance.
pub const HORIZONTAL_ORACLE_OUTPUT_SHA256_INTERIOR: &str =
    "9bfc39be93f5b743f1b2c46c5ea589c7cd2c9a6847dc9af24f638bc8abc68662";
/// Pinned per-case oracle output digest for the periodic seam case.
pub const HORIZONTAL_ORACLE_OUTPUT_SHA256_PERIODIC: &str =
    "4da71b7aeded048f40d35ccccb2eafb09226c23fab40d8cf27f105b381061003";
/// Pinned per-case oracle output digest for the geographic case.
pub const HORIZONTAL_ORACLE_OUTPUT_SHA256_GEOGRAPHIC: &str =
    "1858607f1f5a976af7476f8754fd74bd84b5986f2eeeb32241378edf115fec6e";

/// Absolute tolerance for GPU comparison (matches #72 candidate rule).
pub const HORIZONTAL_GPU_ABSOLUTE_TOLERANCE: f64 = 1.0e-6;

/// Relative tolerance for GPU comparison (matches #72 candidate rule).
pub const HORIZONTAL_GPU_RELATIVE_TOLERANCE: f64 = 1.0e-5;

/// Schema identity for issue-specific horizontal GPU evidence.
pub const HORIZONTAL_GPU_REPORT_SCHEMA_ID: &str = "flexpart-gpu.horizontal-gpu-evidence";

/// Version of the issue-specific horizontal GPU evidence schema.
pub const HORIZONTAL_GPU_REPORT_SCHEMA_VERSION: u32 = 1;

/// Errors for GPU horizontal interpolation.
#[derive(Debug, Error)]
pub enum GpuHorizontalError {
    /// Canonical #72 validation rejected the grid, coordinates or field.
    #[error(transparent)]
    Horizontal(#[from] HorizontalError),
    /// Buffer transfer or readback failed.
    #[error(transparent)]
    Buffer(#[from] GpuBufferError),
    /// Evidence construction or validation failed.
    #[error(transparent)]
    Evidence(#[from] GpuEvidenceError),
    /// No queries were supplied; an empty comparison cannot prove parity.
    #[error("horizontal GPU query sequence is empty")]
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
    /// Input and output resources describe different query counts.
    #[error("query count mismatch for {field}: queries {queries}, outputs {outputs}")]
    CountMismatch {
        /// Resource that mismatched.
        field: &'static str,
        /// Query count.
        queries: usize,
        /// Output count.
        outputs: usize,
    },
    /// Field and uniform resources describe different grids.
    #[error("grid mismatch between field and uniforms")]
    MismatchedGrid,
    /// Uniform buffer does not match the validated grid and query count.
    #[error("horizontal uniform metadata does not match the validated grid")]
    MismatchedUniforms,
    /// GPU device scope reported an error.
    #[error("GPU {scope} error during horizontal interpolation: {message}")]
    Device {
        /// Scope that failed.
        scope: &'static str,
        /// Device error text.
        message: String,
    },
    /// A validated coordinate is not finite as `f32`.
    #[error("validated horizontal coordinate for {field} is not finite as f32")]
    NonFiniteDeviceCoordinate {
        /// Name of the offending lane.
        field: &'static str,
    },
    /// GPU output is non-finite where the oracle is finite.
    #[error("non-finite GPU horizontal output at query {query_index}")]
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
    /// Request does not match the pinned #71 horizontal oracle contract.
    #[error("request does not match the pinned #71 horizontal oracle contract: {message}")]
    OracleContract {
        /// Failure detail.
        message: &'static str,
    },
}

/// One device query in canonical grid-index space.
///
/// `xt`/`yt` are the #72 grid indices uploaded as `f32`. Padding keeps the
/// 16-byte storage stride required for `array<HorizontalQuery>`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct HorizontalSampleQuery {
    /// Canonical x grid index as `f32`.
    pub xt: f32,
    /// Canonical y grid index as `f32`.
    pub yt: f32,
    _pad0: f32,
    _pad1: f32,
}

impl HorizontalSampleQuery {
    /// Build one device query from validated `f32` coordinates.
    #[must_use]
    pub const fn new(xt: f32, yt: f32) -> Self {
        Self {
            xt,
            yt,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct HorizontalParamsRaw {
    nx: u32,
    ny: u32,
    query_count: u32,
    is_periodic_x: u32,
}

fn push_device_error_scopes(device: &wgpu::Device) {
    device.push_error_scope(wgpu::ErrorFilter::Internal);
    device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    device.push_error_scope(wgpu::ErrorFilter::Validation);
}

fn pop_device_error_scopes(
    device: &wgpu::Device,
    scope: &'static str,
) -> Result<(), GpuHorizontalError> {
    let validation = pollster::block_on(device.pop_error_scope());
    let out_of_memory = pollster::block_on(device.pop_error_scope());
    let internal = pollster::block_on(device.pop_error_scope());
    if let Some(error) = [validation, out_of_memory, internal]
        .into_iter()
        .flatten()
        .next()
    {
        return Err(GpuHorizontalError::Device {
            scope,
            message: error.to_string(),
        });
    }
    Ok(())
}

/// Reusable horizontal interpolation dispatch kernel.
pub struct HorizontalInterpolationKernel {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl HorizontalInterpolationKernel {
    /// Compile the WGSL horizontal kernel once per context.
    ///
    /// # Errors
    /// Returns [`GpuHorizontalError::Device`] when shader or pipeline creation
    /// raises a scoped `wgpu` error.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuHorizontalError> {
        push_device_error_scopes(&ctx.device);
        let shader = ctx.load_shader("horizontal_interpolation_shader", SHADER_SOURCE);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("horizontal_interpolation_bgl"),
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
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
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
        let pipeline = ctx.create_compute_pipeline(
            "horizontal_interpolation_pipeline",
            &shader,
            "main",
            &[&bind_group_layout],
        );
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

/// Device-resident canonical field for horizontal sampling.
///
/// Lifetime: source bracket/resource lifetime. Uploaded once when the source
/// field changes, not once per query.
pub struct HorizontalFieldBuffers {
    /// Field values in x-fastest order (`offset = x + nx * y`).
    pub buffer: wgpu::Buffer,
    nx: usize,
    ny: usize,
    is_periodic_x: bool,
    element_count: usize,
}

impl HorizontalFieldBuffers {
    /// Validate `grid`/`field_values` with the exact #72 semantics and upload.
    ///
    /// Only [`HorizontalStaggering::CellCenter`] is supported; face staggering
    /// fails closed. Shape must be exactly `nx * ny`. Non-finite values are
    /// allowed here so unsampled regions behave exactly as #72 (per-query
    /// corner checks fail closed before dispatch); sampled-corner finiteness
    /// is enforced when queries are built.
    ///
    /// # Errors
    /// Returns [`GpuHorizontalError`] for malformed grids, unsupported
    /// staggering, shape mismatches or oversized dimensions.
    pub fn from_grid_and_values(
        ctx: &GpuContext,
        grid: &HorizontalGrid,
        field_values: &[f32],
        staggering: HorizontalStaggering,
    ) -> Result<Self, GpuHorizontalError> {
        if staggering != HorizontalStaggering::CellCenter {
            return Err(GpuHorizontalError::Horizontal(
                HorizontalError::UnsupportedStaggering { staggering },
            ));
        }
        let is_periodic_x = validate_horizontal_grid(grid)?;
        let (nx, ny) = (grid.nx, grid.ny);
        let expected = nx
            .checked_mul(ny)
            .ok_or(GpuHorizontalError::Horizontal(
                HorizontalError::MalformedDimensions { nx, ny },
            ))?;
        if field_values.len() != expected {
            return Err(GpuHorizontalError::Horizontal(
                HorizontalError::ShapeMismatch {
                    expected,
                    actual: field_values.len(),
                },
            ));
        }
        let _nx_u32 =
            u32::try_from(nx).map_err(|_| GpuHorizontalError::ValueTooLarge {
                field: "nx",
                value: nx,
            })?;
        let _ny_u32 =
            u32::try_from(ny).map_err(|_| GpuHorizontalError::ValueTooLarge {
                field: "ny",
                value: ny,
            })?;
        let byte_len = field_values
            .len()
            .checked_mul(size_of::<f32>())
            .ok_or(GpuHorizontalError::SizeOverflow {
                field: "horizontal_field",
            })?;
        let _byte_len_u64 =
            u64::try_from(byte_len).map_err(|_| GpuHorizontalError::SizeOverflow {
                field: "horizontal_field",
            })?;
        if expected == 0 {
            return Err(GpuHorizontalError::Horizontal(
                HorizontalError::MalformedDimensions { nx, ny },
            ));
        }
        let buffer =
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("horizontal_field_values"),
                    contents: bytemuck::cast_slice(field_values),
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC,
                });
        Ok(Self {
            buffer,
            nx,
            ny,
            is_periodic_x,
            element_count: expected,
        })
    }

    /// Horizontal dimension.
    #[must_use]
    pub const fn nx(&self) -> usize {
        self.nx
    }

    /// Vertical dimension.
    #[must_use]
    pub const fn ny(&self) -> usize {
        self.ny
    }

    /// Whether the grid is periodic in x.
    #[must_use]
    pub const fn is_periodic_x(&self) -> bool {
        self.is_periodic_x
    }

    /// Number of field elements (`nx * ny`).
    #[must_use]
    pub const fn element_count(&self) -> usize {
        self.element_count
    }
}

/// Device-resident query coordinates for one dispatch.
///
/// Lifetime: dispatch lifetime. Built from validated `f64` grid indices; each
/// coordinate is validated with the exact #72 supported-domain and corner
/// rules before upload so fail-closed behavior never depends on `f32`
/// rounding in the shader.
pub struct HorizontalQueryBuffers {
    /// Packed `xt`/`yt` queries.
    pub buffer: wgpu::Buffer,
    query_count: usize,
}

impl HorizontalQueryBuffers {
    /// Number of queries in this resource.
    #[must_use]
    pub const fn query_count(&self) -> usize {
        self.query_count
    }
}

/// Device-resident horizontal sample outputs.
///
/// Lifetime: dispatch lifetime, reusable as a device-resident input for later
/// #76 composition. Host readback is allowed only at explicit validation or
/// output boundaries.
pub struct HorizontalSampleOutput {
    /// Sampled values, one per query.
    pub buffer: wgpu::Buffer,
    query_count: usize,
}

impl HorizontalSampleOutput {
    /// Number of sampled values in this resource.
    #[must_use]
    pub const fn query_count(&self) -> usize {
        self.query_count
    }
}

/// Typed uniform resource bound to one validated grid and query count.
pub struct HorizontalUniforms {
    buffer: wgpu::Buffer,
    params: HorizontalParamsRaw,
    nx: usize,
    ny: usize,
    is_periodic_x: bool,
}

fn storage_usage() -> wgpu::BufferUsages {
    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC
}

fn checked_f32_byte_len(len: usize, field: &'static str) -> Result<u64, GpuHorizontalError> {
    let bytes = len
        .checked_mul(size_of::<f32>())
        .ok_or(GpuHorizontalError::SizeOverflow { field })?;
    u64::try_from(bytes).map_err(|_| GpuHorizontalError::SizeOverflow { field })
}

fn f64_to_f32_coordinate(value: f64, field: &'static str) -> Result<f32, GpuHorizontalError> {
    if !value.is_finite() {
        return Err(GpuHorizontalError::NonFiniteDeviceCoordinate { field });
    }
    #[allow(clippy::cast_possible_truncation)]
    let converted = value as f32;
    if !converted.is_finite() {
        return Err(GpuHorizontalError::NonFiniteDeviceCoordinate { field });
    }
    Ok(converted)
}

fn is_lowercase_git_sha(revision: &str) -> bool {
    revision.len() == 40
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_candidate_revision(revision: &str) -> Result<(), GpuHorizontalError> {
    if !is_lowercase_git_sha(revision) {
        return Err(GpuHorizontalError::InvalidCandidateRevision {
            value: revision.to_string(),
        });
    }
    Ok(())
}

/// Build validated device queries from canonical grid-index coordinates.
///
/// Each `(xt, yt)` pair is validated with the exact #72
/// [`sample_horizontal`] rules (supported domain, staggering, shape and
/// sampled-corner finiteness) before `f32` conversion, so out-of-domain,
/// malformed and non-finite inputs fail closed on the host and never reach
/// device execution. Returns the query resource plus the diagnostic
/// [`HorizontalSample`] records (indices/weights) used for evidence only; the
/// sampled values themselves are never computed here.
///
/// # Errors
/// Returns [`GpuHorizontalError`] for empty input, #72 validation failure or
/// non-finite `f32` conversion.
pub fn create_horizontal_query_buffers(
    ctx: &GpuContext,
    grid: &HorizontalGrid,
    field_values: &[f32],
    staggering: HorizontalStaggering,
    coordinates: &[(f64, f64)],
) -> Result<(HorizontalQueryBuffers, Vec<HorizontalSample>), GpuHorizontalError> {
    if coordinates.is_empty() {
        return Err(GpuHorizontalError::EmptyQueries);
    }
    if coordinates.len() > u32::MAX as usize {
        return Err(GpuHorizontalError::ValueTooLarge {
            field: "query_count",
            value: coordinates.len(),
        });
    }
    let mut diagnostics = Vec::with_capacity(coordinates.len());
    let mut device_queries = Vec::with_capacity(coordinates.len());
    for (xt, yt) in coordinates {
        let sample = sample_horizontal(grid, field_values, staggering, *xt, *yt)?;
        let xt_f32 = f64_to_f32_coordinate(*xt, "xt")?;
        let yt_f32 = f64_to_f32_coordinate(*yt, "yt")?;
        diagnostics.push(sample);
        device_queries.push(HorizontalSampleQuery::new(xt_f32, yt_f32));
    }
    let _ = checked_f32_byte_len(0, "horizontal_queries")?;
    let byte_len = device_queries
        .len()
        .checked_mul(size_of::<HorizontalSampleQuery>())
        .ok_or(GpuHorizontalError::SizeOverflow {
            field: "horizontal_queries",
        })?;
    let _byte_len_u64 =
        u64::try_from(byte_len).map_err(|_| GpuHorizontalError::SizeOverflow {
            field: "horizontal_queries",
        })?;
    let buffer =
        ctx.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("horizontal_sample_queries"),
                contents: bytemuck::cast_slice(&device_queries),
                usage: storage_usage(),
            });
    Ok((
        HorizontalQueryBuffers {
            buffer,
            query_count: device_queries.len(),
        },
        diagnostics,
    ))
}

/// Build validated device queries from geographic coordinates.
///
/// Mapping follows the pinned `point_mod::coordtrafo` convention owned by
/// #72 (`xt = (lon - xlon0) / dx`, `yt = (lat - ylat0) / dy`) with exact
/// endpoint snapping. Longitude-convention and latitude checks fail closed on
/// the host; the kernel only ever sees canonical grid indices.
///
/// # Errors
/// Returns [`GpuHorizontalError`] for empty input, #72 validation failure or
/// non-finite `f32` conversion.
pub fn create_horizontal_query_buffers_geographic(
    ctx: &GpuContext,
    grid: &HorizontalGrid,
    field_values: &[f32],
    staggering: HorizontalStaggering,
    lonlat: &[(f64, f64)],
) -> Result<(HorizontalQueryBuffers, Vec<HorizontalSample>), GpuHorizontalError> {
    if lonlat.is_empty() {
        return Err(GpuHorizontalError::EmptyQueries);
    }
    if lonlat.len() > u32::MAX as usize {
        return Err(GpuHorizontalError::ValueTooLarge {
            field: "query_count",
            value: lonlat.len(),
        });
    }
    let mut diagnostics = Vec::with_capacity(lonlat.len());
    let mut device_queries = Vec::with_capacity(lonlat.len());
    for (lon_deg, lat_deg) in lonlat {
        let sample =
            sample_horizontal_geographic(grid, field_values, staggering, *lon_deg, *lat_deg)?;
        let xt_f32 = f64_to_f32_coordinate(sample.xt, "xt")?;
        let yt_f32 = f64_to_f32_coordinate(sample.yt, "yt")?;
        diagnostics.push(sample);
        device_queries.push(HorizontalSampleQuery::new(xt_f32, yt_f32));
    }
    let byte_len = device_queries
        .len()
        .checked_mul(size_of::<HorizontalSampleQuery>())
        .ok_or(GpuHorizontalError::SizeOverflow {
            field: "horizontal_queries",
        })?;
    let _byte_len_u64 =
        u64::try_from(byte_len).map_err(|_| GpuHorizontalError::SizeOverflow {
            field: "horizontal_queries",
        })?;
    let buffer =
        ctx.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("horizontal_sample_queries"),
                contents: bytemuck::cast_slice(&device_queries),
                usage: storage_usage(),
            });
    Ok((
        HorizontalQueryBuffers {
            buffer,
            query_count: device_queries.len(),
        },
        diagnostics,
    ))
}

/// Create a GPU-resident sample output buffer (explicit allocation, no H2D).
///
/// # Errors
/// Returns [`GpuHorizontalError`] for zero or overflowing query counts.
pub fn create_horizontal_output_buffer(
    ctx: &GpuContext,
    query_count: usize,
) -> Result<HorizontalSampleOutput, GpuHorizontalError> {
    if query_count == 0 {
        return Err(GpuHorizontalError::EmptyQueries);
    }
    let _count_u32 =
        u32::try_from(query_count).map_err(|_| GpuHorizontalError::ValueTooLarge {
            field: "query_count",
            value: query_count,
        })?;
    let size = checked_f32_byte_len(query_count, "horizontal_output")?;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("horizontal_sample_output"),
        size,
        usage: storage_usage(),
        mapped_at_creation: false,
    });
    Ok(HorizontalSampleOutput {
        buffer,
        query_count,
    })
}

/// Create an explicit uniform buffer from a validated grid and query count.
///
/// # Errors
/// Returns [`GpuHorizontalError`] for invalid grids, zero queries or oversized
/// dimensions.
pub fn create_horizontal_uniform_buffer(
    ctx: &GpuContext,
    grid: &HorizontalGrid,
    query_count: usize,
) -> Result<HorizontalUniforms, GpuHorizontalError> {
    if query_count == 0 {
        return Err(GpuHorizontalError::EmptyQueries);
    }
    let is_periodic_x = validate_horizontal_grid(grid)?;
    let nx_u32 = u32::try_from(grid.nx).map_err(|_| GpuHorizontalError::ValueTooLarge {
        field: "nx",
        value: grid.nx,
    })?;
    let ny_u32 = u32::try_from(grid.ny).map_err(|_| GpuHorizontalError::ValueTooLarge {
        field: "ny",
        value: grid.ny,
    })?;
    let query_count_u32 =
        u32::try_from(query_count).map_err(|_| GpuHorizontalError::ValueTooLarge {
            field: "query_count",
            value: query_count,
        })?;
    debug_assert_eq!(
        size_of::<HorizontalParamsRaw>() % 16,
        0,
        "horizontal uniform params must stay 16-byte aligned"
    );
    let params = HorizontalParamsRaw {
        nx: nx_u32,
        ny: ny_u32,
        query_count: query_count_u32,
        is_periodic_x: u32::from(is_periodic_x),
    };
    let buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("horizontal_sample_params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    Ok(HorizontalUniforms {
        buffer,
        params,
        nx: grid.nx,
        ny: grid.ny,
        is_periodic_x,
    })
}

/// Encode horizontal sampling into a caller-provided command encoder.
///
/// Production composition boundary: records GPU work without submitting,
/// waiting, transferring or allocating field buffers. The caller owns
/// submission and lifetime: `fields`, `queries`, `output`, `uniforms` and
/// `kernel` must live until the encoded work completes.
///
/// # Errors
/// Returns [`GpuHorizontalError`] for mismatched query counts, grid/uniform
/// disagreement or oversized counts.
pub fn encode_horizontal_samples(
    ctx: &GpuContext,
    fields: &HorizontalFieldBuffers,
    queries: &HorizontalQueryBuffers,
    output: &HorizontalSampleOutput,
    uniforms: &HorizontalUniforms,
    kernel: &HorizontalInterpolationKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuHorizontalError> {
    if queries.query_count != output.query_count {
        return Err(GpuHorizontalError::CountMismatch {
            field: "horizontal_samples",
            queries: queries.query_count,
            outputs: output.query_count,
        });
    }
    if queries.query_count == 0 {
        return Err(GpuHorizontalError::EmptyQueries);
    }
    if fields.nx != uniforms.nx || fields.ny != uniforms.ny || fields.is_periodic_x != uniforms.is_periodic_x
    {
        return Err(GpuHorizontalError::MismatchedGrid);
    }
    let query_count_u32 =
        u32::try_from(queries.query_count).map_err(|_| GpuHorizontalError::ValueTooLarge {
            field: "query_count",
            value: queries.query_count,
        })?;
    let expected = HorizontalParamsRaw {
        nx: u32::try_from(fields.nx).map_err(|_| GpuHorizontalError::ValueTooLarge {
            field: "nx",
            value: fields.nx,
        })?,
        ny: u32::try_from(fields.ny).map_err(|_| GpuHorizontalError::ValueTooLarge {
            field: "ny",
            value: fields.ny,
        })?,
        query_count: query_count_u32,
        is_periodic_x: u32::from(fields.is_periodic_x),
    };
    if uniforms.params != expected {
        return Err(GpuHorizontalError::MismatchedUniforms);
    }
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("horizontal_interpolation_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: fields.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: queries.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: output.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: uniforms.buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("horizontal_interpolation_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&kernel.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        super::dispatch_1d(&mut pass, query_count_u32, kernel.workgroup_size_x);
    }
    Ok(())
}

/// Dispatch horizontal sampling and wait for completion (no readback).
///
/// Convenience path for isolated oracle validation only. Production
/// composition must use [`encode_horizontal_samples`] so dependent GPU stages
/// can share one submission without host synchronization.
///
/// # Errors
/// Forwards [`GpuHorizontalError`] from encoding and scoped device errors.
pub fn dispatch_horizontal_samples_and_wait(
    ctx: &GpuContext,
    fields: &HorizontalFieldBuffers,
    queries: &HorizontalQueryBuffers,
    output: &HorizontalSampleOutput,
    uniforms: &HorizontalUniforms,
    kernel: &HorizontalInterpolationKernel,
) -> Result<(), GpuHorizontalError> {
    push_device_error_scopes(&ctx.device);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("horizontal_interpolation_encoder"),
        });
    if let Err(error) = encode_horizontal_samples(
        ctx, fields, queries, output, uniforms, kernel, &mut encoder,
    ) {
        let _ = pop_device_error_scopes(&ctx.device, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    pop_device_error_scopes(&ctx.device, "dispatch")
}

/// Explicit D2H readback of horizontal sample outputs.
///
/// Allowed only at validation/output boundaries. Fails closed on non-finite
/// values where the caller expects finite oracle parity; the length check
/// itself never hides a device failure.
///
/// # Errors
/// Forwards [`GpuHorizontalError::Buffer`] from staging and mapping, or
/// [`GpuHorizontalError::NonFiniteOutputValue`] for non-finite outputs.
pub async fn download_horizontal_output(
    ctx: &GpuContext,
    output: &HorizontalSampleOutput,
) -> Result<Vec<f32>, GpuHorizontalError> {
    let values = download_buffer_typed::<f32>(
        ctx,
        &output.buffer,
        output.query_count,
        "horizontal_sample_output",
    )
    .await?;
    for (query_index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(GpuHorizontalError::NonFiniteOutputValue { query_index });
        }
    }
    Ok(values)
}

/// Sample one canonical field with GPU-executed arithmetic.
///
/// Isolated oracle-verification workflow. Validates every coordinate with the
/// exact #72 rules before dispatch, executes the WGSL kernel, reads back at
/// an explicit validation boundary and fails closed on non-finite outputs.
/// The returned diagnostics carry host-derived indices/weights for evidence;
/// the sampled values themselves always come from the device. No CPU fallback
/// exists: any GPU failure surfaces as an error.
///
/// # Errors
/// Returns [`GpuHorizontalError`] for #72 fail-closed cases, empty queries,
/// buffer/GPU failures or non-finite outputs.
pub async fn sample_horizontal_gpu(
    ctx: &GpuContext,
    grid: &HorizontalGrid,
    field_values: &[f32],
    staggering: HorizontalStaggering,
    coordinates: &[(f64, f64)],
    kernel: &HorizontalInterpolationKernel,
) -> Result<(Vec<f32>, Vec<HorizontalSample>), GpuHorizontalError> {
    let fields = HorizontalFieldBuffers::from_grid_and_values(ctx, grid, field_values, staggering)?;
    let (queries, diagnostics) =
        create_horizontal_query_buffers(ctx, grid, field_values, staggering, coordinates)?;
    let output = create_horizontal_output_buffer(ctx, queries.query_count)?;
    let uniforms = create_horizontal_uniform_buffer(ctx, grid, queries.query_count)?;
    dispatch_horizontal_samples_and_wait(ctx, &fields, &queries, &output, &uniforms, kernel)?;
    let values = download_horizontal_output(ctx, &output).await?;
    Ok((values, diagnostics))
}

/// Geographic variant of [`sample_horizontal_gpu`].
///
/// # Errors
/// Returns [`GpuHorizontalError`] under the same conditions as
/// [`sample_horizontal_gpu`], plus longitude-convention and impossible
/// coordinate failures owned by #72.
pub async fn sample_horizontal_geographic_gpu(
    ctx: &GpuContext,
    grid: &HorizontalGrid,
    field_values: &[f32],
    staggering: HorizontalStaggering,
    lonlat: &[(f64, f64)],
    kernel: &HorizontalInterpolationKernel,
) -> Result<(Vec<f32>, Vec<HorizontalSample>), GpuHorizontalError> {
    let fields = HorizontalFieldBuffers::from_grid_and_values(ctx, grid, field_values, staggering)?;
    let (queries, diagnostics) =
        create_horizontal_query_buffers_geographic(ctx, grid, field_values, staggering, lonlat)?;
    let output = create_horizontal_output_buffer(ctx, queries.query_count)?;
    let uniforms = create_horizontal_uniform_buffer(ctx, grid, queries.query_count)?;
    dispatch_horizontal_samples_and_wait(ctx, &fields, &queries, &output, &uniforms, kernel)?;
    let values = download_horizontal_output(ctx, &output).await?;
    Ok((values, diagnostics))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        hex.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    hex
}

/// SHA-256 of the executed WGSL bundle.
#[must_use]
pub fn horizontal_shader_sha256() -> String {
    sha256_hex(SHADER_SOURCE.as_bytes())
}

fn grid_to_normalized(grid: &HorizontalGrid) -> serde_json::Value {
    serde_json::json!({
        "nx": grid.nx,
        "ny": grid.ny,
        "xlon0_deg": grid.xlon0_deg,
        "ylat0_deg": grid.ylat0_deg,
        "dx_deg": grid.dx_deg,
        "dy_deg": grid.dy_deg,
        "longitude_domain": format!("{:?}", grid.longitude_domain),
    })
}

/// SHA-256 of the normalized JSON encoding of dispatched GPU inputs.
///
/// Binds grid metadata, field values and `f32` query coordinates so evidence
/// cannot claim a different input than what was dispatched.
///
/// # Errors
/// Returns [`GpuHorizontalError::InputHash`] when JSON encoding fails.
pub fn horizontal_inputs_sha256(
    grid: &HorizontalGrid,
    field_values: &[f32],
    queries: &[HorizontalSampleQuery],
) -> Result<String, GpuHorizontalError> {
    #[derive(Serialize)]
    struct NormalizedHorizontalInput<'a> {
        grid: serde_json::Value,
        field_values: &'a [f32],
        queries: Vec<[f32; 2]>,
        staggering: String,
    }
    let normalized = NormalizedHorizontalInput {
        grid: grid_to_normalized(grid),
        field_values,
        queries: queries
            .iter()
            .map(|query| [query.xt, query.yt])
            .collect(),
        staggering: format!("{:?}", HorizontalStaggering::CellCenter),
    };
    let json = serde_json::to_vec(&normalized).map_err(|err| GpuHorizontalError::InputHash {
        message: err.to_string(),
    })?;
    Ok(sha256_hex(&json))
}

/// Build the repository-wide comparison policy owned by #87.
///
/// Absolute `1e-6` plus relative `1e-5` matches the #72 candidate rule and the
/// #91 generic finite comparator (`absolute <= A OR relative <= R`).
///
/// # Errors
/// Returns [`GpuHorizontalError::Evidence`] for an invalid tolerance policy.
pub fn default_comparison_policy() -> Result<ComparisonPolicy, GpuHorizontalError> {
    Ok(ComparisonPolicy::new(
        HORIZONTAL_GPU_ABSOLUTE_TOLERANCE,
        HORIZONTAL_GPU_RELATIVE_TOLERANCE,
    )?)
}

fn pinned_oracle_evidence_for_case(case_id: &str) -> Result<PinnedOracleEvidence, GpuHorizontalError> {
    let output_sha256 = match case_id {
        "horizontal-interior" => HORIZONTAL_ORACLE_OUTPUT_SHA256_INTERIOR,
        "horizontal-periodic-wrap" => HORIZONTAL_ORACLE_OUTPUT_SHA256_PERIODIC,
        "horizontal-geographic-interior" => HORIZONTAL_ORACLE_OUTPUT_SHA256_GEOGRAPHIC,
        _ => {
            return Err(GpuHorizontalError::OracleContract {
                message: "unknown pinned horizontal oracle case",
            });
        }
    };
    Ok(PinnedOracleEvidence {
        implementation_id: HORIZONTAL_ORACLE_IMPLEMENTATION_ID.to_string(),
        revision: HORIZONTAL_ORACLE_REVISION.to_string(),
        executable_sha256: HORIZONTAL_ORACLE_EXECUTABLE_SHA256.to_string(),
        output_sha256: output_sha256.to_string(),
    })
}

/// One machine-readable GPU-vs-oracle comparison row for #87.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HorizontalGpuRow {
    /// Pinned oracle case that produced this row.
    pub case_id: String,
    /// Canonical grid that was sampled.
    pub grid: HorizontalGrid,
    /// Whether the grid is periodic in x.
    pub is_periodic_x: bool,
    /// Requested x grid index (host `f64`).
    pub sample_xt: f64,
    /// Requested y grid index (host `f64`).
    pub sample_yt: f64,
    /// Device `f32` x coordinate actually dispatched.
    pub device_xt: f32,
    /// Device `f32` y coordinate actually dispatched.
    pub device_yt: f32,
    /// Host-derived lower/upper indices `[ix, jy, ixp, jyp]` for diagnostics.
    pub candidate_indices: [usize; 4],
    /// Oracle indices `[ix, jy, ixp, jyp]`.
    pub oracle_indices: [usize; 4],
    /// Host-derived bilinear weights `[p1, p2, p3, p4]` for diagnostics.
    pub candidate_weights: [f64; 4],
    /// Oracle weights `[p1, p2, p3, p4]`.
    pub oracle_weights: [f64; 4],
    /// Element value computed on the GPU.
    pub gpu_value: f32,
    /// Pinned oracle value.
    pub oracle_value: f32,
    /// CPU #72 diagnostic value (migration reference only, never the candidate).
    pub cpu_value: f32,
    /// Predeclared comparison policy.
    pub comparison_policy: ComparisonPolicy,
    /// Absolute GPU-oracle difference.
    pub absolute_difference: f64,
    /// Whether the GPU value matches the oracle within tolerance.
    pub value_verdict: bool,
    /// Whether host indices match the oracle.
    pub indices_verdict: bool,
    /// Whether host weights match the oracle within tolerance.
    pub weights_verdict: bool,
    /// Combined row verdict.
    pub row_verdict: bool,
    /// Repository-wide GPU execution and numerical evidence.
    pub gpu_evidence: GpuCalculationEvidence,
}

/// Machine-readable GPU-vs-oracle comparison report for #87.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HorizontalGpuReport {
    /// Report schema identity.
    pub schema: SchemaIdentity,
    /// Stable scenario identifier covering all pinned horizontal cases.
    pub scenario_id: String,
    /// Candidate description recorded for traceability.
    pub candidate: String,
    /// Predeclared comparison policy.
    pub comparison_policy: ComparisonPolicy,
    /// Per-query comparison rows with GPU evidence.
    pub rows: Vec<HorizontalGpuRow>,
    /// Overall report verdict (`true` only when every row passes).
    pub status: bool,
}

impl HorizontalGpuRow {
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
                "embedded GPU comparison contradicts horizontal row values or policy",
            ));
        }
        if !is_lowercase_git_sha(&self.gpu_evidence.candidate.revision) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "candidate revision is not a lowercase Git commit SHA",
            ));
        }
        if self.gpu_evidence.candidate.implementation_id != HORIZONTAL_GPU_IMPLEMENTATION_ID
            || self.gpu_evidence.candidate.shader_sha256 != horizontal_shader_sha256()
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU provenance contradicts the horizontal calculation",
            ));
        }
        let expected_oracle = pinned_oracle_evidence_for_case(&self.case_id).map_err(|_| {
            GpuEvidenceError::InvalidComparisonState(
                "horizontal row case does not identify a pinned oracle case",
            )
        })?;
        if self.gpu_evidence.oracle.as_ref() != Some(&expected_oracle) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU oracle contradicts the pinned horizontal case",
            ));
        }
        let expected_case_prefix = format!("horizontal-gpu/{}", self.case_id);
        if !self.gpu_evidence.case_id.starts_with(&expected_case_prefix) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU case id contradicts the horizontal row case",
            ));
        }
        let value_pass = comparison.verdict == NumericalVerdict::Passed;
        if self.value_verdict != value_pass {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "horizontal value verdict contradicts tolerance",
            ));
        }
        let expected_row = self.value_verdict && self.indices_verdict && self.weights_verdict;
        if self.row_verdict != expected_row {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "horizontal row verdict contradicts facet verdicts",
            ));
        }
        let expected_difference = (f64::from(self.gpu_value) - f64::from(self.oracle_value)).abs();
        if (self.absolute_difference - expected_difference).abs() > 1.0e-12 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "horizontal absolute difference contradicts values",
            ));
        }
        self.gpu_evidence.validate()?;
        if self.row_verdict {
            self.gpu_evidence.require_paired_pass()?;
        }
        Ok(())
    }
}

impl HorizontalGpuReport {
    /// Validate structural honesty and fail closed on contradiction.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] when the schema, verdicts, tolerances or
    /// embedded evidence are missing or contradictory.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        if self.schema.id != HORIZONTAL_GPU_REPORT_SCHEMA_ID
            || self.schema.version != HORIZONTAL_GPU_REPORT_SCHEMA_VERSION
        {
            return Err(GpuEvidenceError::UnsupportedSchema {
                id: self.schema.id.clone(),
                version: self.schema.version,
            });
        }
        if self.scenario_id != "horizontal-oracle-coverage"
            || self.candidate != HORIZONTAL_GPU_CANDIDATE_DESCRIPTION
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "horizontal GPU report does not identify the pinned coverage scenario",
            ));
        }
        if self.rows.is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "horizontal GPU report must contain at least one row",
            ));
        }
        let candidate_revision = self.rows[0].gpu_evidence.candidate.revision.clone();
        for row in &self.rows {
            if row.comparison_policy != self.comparison_policy
                || row.gpu_evidence.candidate.revision != candidate_revision
            {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "horizontal GPU row policy or candidate revision contradicts its report",
                ));
            }
            row.validate()?;
        }
        let all_pass = self.rows.iter().all(|row| row.row_verdict);
        if self.status != all_pass {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "horizontal GPU report status contradicts row verdicts",
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
/// `diagnostic` carries host-derived indices/weights from the exact #72
/// validation; `gpu_value` must come from actual device execution and
/// `oracle_*` from the pinned #71 fixture. `cpu_value` is diagnostic only.
///
/// # Errors
/// Returns [`GpuHorizontalError`] for invalid tolerances, non-finite values,
/// invalid candidate provenance or evidence construction failures.
#[allow(clippy::too_many_arguments)]
pub fn build_horizontal_gpu_row(
    ctx: &GpuContext,
    case_id: &str,
    grid: &HorizontalGrid,
    diagnostic: &HorizontalSample,
    device_query: &HorizontalSampleQuery,
    gpu_value: f32,
    oracle_indices: [usize; 4],
    oracle_weights: [f64; 4],
    oracle_value: f32,
    cpu_value: f32,
    comparison_policy: ComparisonPolicy,
    field_values: &[f32],
    candidate_revision: &str,
) -> Result<HorizontalGpuRow, GpuHorizontalError> {
    if !gpu_value.is_finite() || !oracle_value.is_finite() || !cpu_value.is_finite() {
        return Err(GpuHorizontalError::NonFiniteOutputValue { query_index: 0 });
    }
    validate_candidate_revision(candidate_revision)?;
    let comparison = compare_finite_values(
        &[f64::from(oracle_value)],
        &[f64::from(gpu_value)],
        comparison_policy,
    )?;
    let value_verdict = comparison.verdict == NumericalVerdict::Passed;
    let candidate_indices = [
        diagnostic.ix,
        diagnostic.jy,
        diagnostic.ixp,
        diagnostic.jyp,
    ];
    let indices_verdict = candidate_indices == oracle_indices;
    let weights_verdict = diagnostic
        .weights
        .iter()
        .zip(oracle_weights)
        .all(|(candidate, oracle)| {
            let absolute = (candidate - oracle).abs();
            let scale = candidate.abs().max(oracle.abs());
            let relative = if scale == 0.0 { 0.0 } else { absolute / scale };
            absolute <= comparison_policy.absolute_tolerance
                || relative <= comparison_policy.relative_tolerance
        });
    let row_verdict = value_verdict && indices_verdict && weights_verdict;
    let input_sha = horizontal_inputs_sha256(grid, field_values, std::slice::from_ref(device_query))?;
    let case_evidence_id = format!(
        "horizontal-gpu/{case_id}:xt={:.6}:yt={:.6}",
        diagnostic.xt, diagnostic.yt
    );
    let gpu_evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id: case_evidence_id,
        candidate: GpuCandidateEvidence {
            implementation_id: HORIZONTAL_GPU_IMPLEMENTATION_ID.to_string(),
            revision: candidate_revision.to_string(),
            shader_sha256: horizontal_shader_sha256(),
            input_sha256: input_sha,
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(GpuAdapterEvidence::from_context(ctx)),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(pinned_oracle_evidence_for_case(case_id)?),
        comparison,
    };
    gpu_evidence.validate()?;
    Ok(HorizontalGpuRow {
        case_id: case_id.to_string(),
        grid: grid.clone(),
        is_periodic_x: diagnostic.is_periodic_x,
        sample_xt: diagnostic.xt,
        sample_yt: diagnostic.yt,
        device_xt: device_query.xt,
        device_yt: device_query.yt,
        candidate_indices,
        oracle_indices,
        candidate_weights: diagnostic.weights,
        oracle_weights,
        gpu_value,
        oracle_value,
        cpu_value,
        comparison_policy,
        absolute_difference: (f64::from(gpu_value) - f64::from(oracle_value)).abs(),
        value_verdict,
        indices_verdict,
        weights_verdict,
        row_verdict,
        gpu_evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_source_hash_is_stable_hex() {
        let hash = horizontal_shader_sha256();
        assert_eq!(hash.len(), 64);
        assert!(hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }

    #[test]
    fn uniform_params_stay_16_byte_aligned() {
        assert_eq!(size_of::<HorizontalParamsRaw>() % 16, 0);
        assert_eq!(size_of::<HorizontalSampleQuery>() % 16, 0);
    }

    #[test]
    fn empty_queries_fail_closed() {
        assert!(matches!(
            validate_candidate_revision("0.1.0"),
            Err(GpuHorizontalError::InvalidCandidateRevision { .. })
        ));
    }
}
