//! GPU gravitational settling velocity for issue #35.
//!
//! Ports the FLEXPART 11.1 spherical settling formulation to actual
//! WGSL/device execution:
//! - `settling_mod.f90:get_settling` (sphere branch, `ishape == 0`)
//! - `settling_mod.f90:viscosity` (Sutherland dynamic viscosity)
//! - `drydepo_mod.f90:part0` (single-bin Cunningham slip and Stokes init)
//! - `par_mod.f90:61` (`ga = 9.81`)
//! - `readoptions_mod.f90` species initialization (`dquer` m-to-um,
//!   `part0` call, `vsetaver = -vset`, `cunningham` weighting)
//!
//! Scientific references: Naeslund and Thaning (1991) for the iterative
//! Reynolds/settling loop; Sutherland (1893) for temperature-dependent
//! viscosity; Clift and Gauvin (1971) for the sphere drag coefficient.
//!
//! The host owns all validation with the exact #10 carrier semantics:
//! per-species `PDENSITY`/`PDIA` from [`crate::config::SpeciesConfig`],
//! spherical shape only (`PSHAPE == 0`), finite positive meteorology, and
//! declared valid-domain bounds. Supported velocity combination executes in
//! WGSL. No CPU settling implementation exists on this path; CPU code is
//! limited to fixture/oracle preparation and test diagnostics.
//!
//! ## Composition contract
//!
//! - Query/input resources have dispatch lifetime.
//! - `encode_settling_velocities` records into a caller-owned encoder without
//!   submitting, polling, mapping or reading back, so later #37 production
//!   composition can consume device-resident velocities without
//!   `GPU -> CPU -> GPU`.
//! - `dispatch_*`/readback helpers submit, wait and stage through `MAP_READ`
//!   only for isolated oracle validation and must not sit between composed
//!   GPU stages.
//! - Settling alone never removes mass; the kernel writes velocities only.
//! - No dry-deposition resistance, ground interaction, mass removal, or
//!   gridding is implemented here (owned by #36/#37 and later issues).

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wgpu::util::DeviceExt;

use super::{
    compare_finite_values, download_buffer_typed, render_shader_with_workgroup_size,
    ComparisonPolicy, GpuAdapterEvidence, GpuBufferError, GpuCalculationEvidence,
    GpuCalculationPath, GpuCandidateEvidence, GpuContext, GpuEvidenceError, GpuEvidenceSchema,
    GpuExecutionEvidence, GpuExecutionStatus, NumericalVerdict, PinnedOracleEvidence,
};
use crate::config::SpeciesConfig;

/// WGSL source for spherical gravitational settling.
const SHADER_SOURCE: &str = include_str!("../shaders/settling_velocity.wgsl");

/// Workgroup width for the settling kernel.
const WORKGROUP_SIZE_X: u32 = 64;

/// Candidate implementation identity for machine-readable evidence.
pub const SETTLING_GPU_IMPLEMENTATION_ID: &str =
    "physics::settling-gpu::encode_settling_velocities";

/// Human-readable candidate description recorded in issue-specific reports.
pub const SETTLING_GPU_CANDIDATE_DESCRIPTION: &str =
    "physics::settling-gpu::encode_settling_velocities (WGSL device)";

/// Pinned FLEXPART routine owning the settling oracle (sphere branch).
pub const SETTLING_ORACLE_IMPLEMENTATION_ID: &str =
    "FLEXPART-11.1 settling_mod::get_settling (sphere)";

/// Pinned FLEXPART 11.1 revision owning the settling oracle.
pub const SETTLING_ORACLE_REVISION: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";

/// Absolute tolerance [m/s] for GPU-oracle comparison (near-zero regime).
pub const SETTLING_GPU_ABSOLUTE_TOLERANCE: f64 = 1.0e-9;

/// Relative tolerance [-] for GPU-oracle comparison (1% per #35).
pub const SETTLING_GPU_RELATIVE_TOLERANCE: f64 = 0.01;

/// Declared valid particle diameter domain [um] (spherical carriers only).
pub const SETTLING_DIAMETER_MIN_UM: f32 = 0.1;
/// Declared valid particle diameter domain [um] (spherical carriers only).
pub const SETTLING_DIAMETER_MAX_UM: f32 = 100.0;

/// Declared valid particle density domain [kg/m3].
pub const SETTLING_DENSITY_MIN_KG_M3: f32 = 500.0;
/// Declared valid particle density domain [kg/m3].
pub const SETTLING_DENSITY_MAX_KG_M3: f32 = 3000.0;

/// Declared valid air temperature domain [K].
pub const SETTLING_TEMPERATURE_MIN_K: f32 = 200.0;
/// Declared valid air temperature domain [K].
pub const SETTLING_TEMPERATURE_MAX_K: f32 = 320.0;

/// Declared valid moist-air density domain [kg/m3].
pub const SETTLING_AIR_DENSITY_MIN_KG_M3: f32 = 0.4;
/// Declared valid moist-air density domain [kg/m3].
pub const SETTLING_AIR_DENSITY_MAX_KG_M3: f32 = 1.6;

/// Schema identity for the issue-specific settling GPU report.
pub const SETTLING_GPU_REPORT_SCHEMA_ID: &str = "flexpart-gpu.settling-gpu-evidence";

/// Version of the issue-specific settling GPU report schema.
pub const SETTLING_GPU_REPORT_SCHEMA_VERSION: u32 = 1;

/// Errors for GPU gravitational settling.
#[derive(Debug, Error)]
pub enum GpuSettlingError {
    /// Carrier/meteorology input violates the declared valid domain.
    #[error("invalid settling input for {field}: {message}")]
    InvalidInput {
        /// Name of the offending input.
        field: &'static str,
        /// Why the input was rejected.
        message: String,
    },
    /// Species carrier cannot produce a settling velocity.
    #[error("species `{species}` cannot settle: {message}")]
    InvalidCarrier {
        /// Species name that was rejected.
        species: String,
        /// Why the carrier was rejected.
        message: String,
    },
    /// No queries were supplied; an empty comparison cannot prove parity.
    #[error("settling GPU query sequence is empty")]
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
    /// Buffer transfer or readback failed.
    #[error(transparent)]
    Buffer(#[from] GpuBufferError),
    /// Evidence construction or validation failed.
    #[error(transparent)]
    Evidence(#[from] GpuEvidenceError),
    /// GPU device scope reported an error.
    #[error("GPU {scope} error during settling: {message}")]
    Device {
        /// Scope that failed.
        scope: &'static str,
        /// Device error text.
        message: String,
    },
    /// A requested storage buffer exceeds the device's resource limits.
    #[error("settling buffer {field} needs {bytes} bytes, exceeding device limit {limit}")]
    BufferLimit {
        /// Resource whose size was rejected.
        field: &'static str,
        /// Required storage bytes.
        bytes: u64,
        /// Maximum bytes allowed by this device.
        limit: u64,
    },
    /// GPU output is non-finite where the oracle is finite.
    #[error("non-finite GPU settling output")]
    NonFiniteOutputValue,
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
    /// Request does not match the pinned settling oracle contract.
    #[error("request does not match the pinned settling oracle contract: {message}")]
    OracleContract {
        /// Failure detail.
        message: &'static str,
    },
}

/// Per-species spherical carrier properties for gravitational settling.
///
/// Sourced from #10 [`SpeciesConfig`] (`PDENSITY` [kg/m3], `PDIA` [um]).
/// Only `PSHAPE == 0` spheres are supported; any other shape fails closed
/// during conversion and never reaches the WGSL kernel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettlingCarrier {
    /// Particle material density [kg/m3].
    pub particle_density_kg_m3: f32,
    /// Volume-equivalent diameter [um].
    pub diameter_um: f32,
}

impl SettlingCarrier {
    /// Build a settling carrier from one #10 species configuration.
    ///
    /// # Errors
    /// Returns [`GpuSettlingError`] when the species is not a spherical
    /// aerosol carrier (gas, passive tracer, non-positive size/density,
    /// out-of-domain, or `PSHAPE != 0` in the raw species map).
    pub fn from_species_config(species: &SpeciesConfig) -> Result<Self, GpuSettlingError> {
        let name = species.name.clone();
        let invalid = |message: String| GpuSettlingError::InvalidCarrier {
            species: name.clone(),
            message,
        };
        // Non-spherical shapes are never approximated silently. `SpeciesConfig`
        // validation already rejects `PSHAPE != 0`; re-check the raw map here
        // so a direct carrier conversion cannot bypass that guardrail.
        if let Some(shape) = species.raw.iter().find_map(|(key, value)| {
            if key.eq_ignore_ascii_case("pshape") || key.eq_ignore_ascii_case("shape") {
                value.parse::<f64>().ok()
            } else {
                None
            }
        }) {
            if shape != 0.0 {
                return Err(invalid(format!(
                    "unsupported non-spherical PSHAPE={shape} (only PSHAPE=0 spheres)"
                )));
            }
        }
        let density = species.particle_density_kg_m3.ok_or_else(|| {
            invalid("missing PDENSITY (settling requires an aerosol carrier)".to_string())
        })?;
        let diameter = species.mean_diameter_um.ok_or_else(|| {
            invalid("missing PDIA (settling requires a positive diameter)".to_string())
        })?;
        if !density.is_finite() || !diameter.is_finite() {
            return Err(invalid("PDENSITY/PDIA must be finite".to_string()));
        }
        #[allow(clippy::cast_possible_truncation)]
        let carrier = Self {
            particle_density_kg_m3: density as f32,
            diameter_um: diameter as f32,
        };
        carrier.validate()?;
        Ok(carrier)
    }

    /// Validate the carrier against the declared #35 valid domain.
    ///
    /// # Errors
    /// Returns [`GpuSettlingError::InvalidCarrier`] for non-finite,
    /// non-positive, or out-of-domain size/density.
    pub fn validate(&self) -> Result<(), GpuSettlingError> {
        let invalid = |message: String| GpuSettlingError::InvalidCarrier {
            species: "<direct>".to_string(),
            message,
        };
        if !self.diameter_um.is_finite()
            || self.diameter_um < SETTLING_DIAMETER_MIN_UM
            || self.diameter_um > SETTLING_DIAMETER_MAX_UM
        {
            return Err(invalid(format!(
                "diameter_um {} outside [{}, {}]",
                self.diameter_um, SETTLING_DIAMETER_MIN_UM, SETTLING_DIAMETER_MAX_UM
            )));
        }
        if !self.particle_density_kg_m3.is_finite()
            || self.particle_density_kg_m3 < SETTLING_DENSITY_MIN_KG_M3
            || self.particle_density_kg_m3 > SETTLING_DENSITY_MAX_KG_M3
        {
            return Err(invalid(format!(
                "particle_density_kg_m3 {} outside [{}, {}]",
                self.particle_density_kg_m3, SETTLING_DENSITY_MIN_KG_M3, SETTLING_DENSITY_MAX_KG_M3
            )));
        }
        Ok(())
    }
}

/// One settling query: spherical carrier plus local air state.
///
/// `diameter_um`/`particle_density_kg_m3` are per-species #10 properties;
/// `temperature_k`/`air_density_kg_m3` are the local meteorology consumed by
/// `settling_mod::get_settling` in addition to carrier properties.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SettlingQuery {
    /// Volume-equivalent diameter [um].
    pub diameter_um: f32,
    /// Particle material density [kg/m3].
    pub particle_density_kg_m3: f32,
    /// Local air temperature [K].
    pub temperature_k: f32,
    /// Local moist-air density [kg/m3].
    pub air_density_kg_m3: f32,
}

impl SettlingQuery {
    /// Build a validated query from a carrier and local air state.
    ///
    /// # Errors
    /// Returns [`GpuSettlingError::InvalidInput`] for non-finite or
    /// out-of-domain meteorology.
    pub fn new(
        carrier: SettlingCarrier,
        temperature_k: f32,
        air_density_kg_m3: f32,
    ) -> Result<Self, GpuSettlingError> {
        carrier.validate().map_err(|err| match err {
            GpuSettlingError::InvalidCarrier { message, .. } => GpuSettlingError::InvalidInput {
                field: "carrier",
                message,
            },
            other => other,
        })?;
        if !temperature_k.is_finite()
            || temperature_k < SETTLING_TEMPERATURE_MIN_K
            || temperature_k > SETTLING_TEMPERATURE_MAX_K
        {
            return Err(GpuSettlingError::InvalidInput {
                field: "temperature_k",
                message: format!(
                    "temperature_k {temperature_k} outside [{SETTLING_TEMPERATURE_MIN_K}, {SETTLING_TEMPERATURE_MAX_K}]"
                ),
            });
        }
        if !air_density_kg_m3.is_finite()
            || air_density_kg_m3 < SETTLING_AIR_DENSITY_MIN_KG_M3
            || air_density_kg_m3 > SETTLING_AIR_DENSITY_MAX_KG_M3
        {
            return Err(GpuSettlingError::InvalidInput {
                field: "air_density_kg_m3",
                message: format!(
                    "air_density_kg_m3 {air_density_kg_m3} outside [{SETTLING_AIR_DENSITY_MIN_KG_M3}, {SETTLING_AIR_DENSITY_MAX_KG_M3}]"
                ),
            });
        }
        Ok(Self {
            diameter_um: carrier.diameter_um,
            particle_density_kg_m3: carrier.particle_density_kg_m3,
            temperature_k,
            air_density_kg_m3,
        })
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct SettlingParamsRaw {
    query_count: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

fn push_device_error_scopes(device: &wgpu::Device) {
    device.push_error_scope(wgpu::ErrorFilter::Internal);
    device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    device.push_error_scope(wgpu::ErrorFilter::Validation);
}

fn pop_device_error_scopes(
    device: &wgpu::Device,
    scope: &'static str,
) -> Result<(), GpuSettlingError> {
    let validation = pollster::block_on(device.pop_error_scope());
    let out_of_memory = pollster::block_on(device.pop_error_scope());
    let internal = pollster::block_on(device.pop_error_scope());
    if let Some(error) = [validation, out_of_memory, internal]
        .into_iter()
        .flatten()
        .next()
    {
        return Err(GpuSettlingError::Device {
            scope,
            message: error.to_string(),
        });
    }
    Ok(())
}

/// Reusable settling velocity dispatch kernel.
pub struct SettlingVelocityKernel {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl SettlingVelocityKernel {
    /// Compile the WGSL settling kernel once per context.
    ///
    /// # Errors
    /// Returns [`GpuSettlingError::Device`] when shader or pipeline creation
    /// raises a scoped `wgpu` error.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuSettlingError> {
        push_device_error_scopes(&ctx.device);
        let shader_source = render_shader_with_workgroup_size(SHADER_SOURCE, WORKGROUP_SIZE_X);
        let shader = ctx.load_shader("settling_velocity_shader", &shader_source);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("settling_velocity_bgl"),
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
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
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
            "settling_velocity_pipeline",
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

/// Device-resident settling queries for one dispatch.
///
/// Lifetime: dispatch lifetime. Built only from validated carriers and air
/// state; invalid inputs fail closed on the host and never reach the device.
#[derive(Debug)]
pub struct SettlingQueryBuffers {
    /// Packed queries.
    pub buffer: wgpu::Buffer,
    query_count: usize,
}

impl SettlingQueryBuffers {
    /// Number of queries in this resource.
    #[must_use]
    pub const fn query_count(&self) -> usize {
        self.query_count
    }
}

/// Device-resident settling velocity outputs.
///
/// Lifetime: dispatch lifetime, reusable as a device-resident input for later
/// #37 production composition. Host readback is allowed only at explicit
/// validation or output boundaries. Settling alone never modifies particle
/// mass; mass buffers are not bound here.
#[derive(Debug)]
pub struct SettlingVelocityOutput {
    /// Settling velocities [m/s], one per query (negative, downward).
    pub buffer: wgpu::Buffer,
    query_count: usize,
}

impl SettlingVelocityOutput {
    /// Number of velocities in this resource.
    #[must_use]
    pub const fn query_count(&self) -> usize {
        self.query_count
    }
}

/// Typed uniform resource bound to one query count.
#[derive(Debug)]
pub struct SettlingUniforms {
    buffer: wgpu::Buffer,
    params: SettlingParamsRaw,
}

fn storage_usage() -> wgpu::BufferUsages {
    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC
}

fn validate_storage_size(
    ctx: &GpuContext,
    bytes: u64,
    field: &'static str,
) -> Result<(), GpuSettlingError> {
    let limits = ctx.device.limits();
    let limit = limits
        .max_buffer_size
        .min(u64::from(limits.max_storage_buffer_binding_size));
    if bytes > limit {
        return Err(GpuSettlingError::BufferLimit {
            field,
            bytes,
            limit,
        });
    }
    Ok(())
}

fn is_lowercase_git_sha(revision: &str) -> bool {
    revision.len() == 40
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_candidate_revision(revision: &str) -> Result<(), GpuSettlingError> {
    if !is_lowercase_git_sha(revision) {
        return Err(GpuSettlingError::InvalidCandidateRevision {
            value: revision.to_string(),
        });
    }
    Ok(())
}

/// Build validated device queries from host queries.
///
/// # Errors
/// Returns [`GpuSettlingError`] for empty input, oversized counts, or
/// device-limit violations. Per-query domain validation must already have
/// succeeded in [`SettlingQuery::new`]; this function never invents inputs.
pub fn create_settling_query_buffers(
    ctx: &GpuContext,
    queries: &[SettlingQuery],
) -> Result<SettlingQueryBuffers, GpuSettlingError> {
    if queries.is_empty() {
        return Err(GpuSettlingError::EmptyQueries);
    }
    let _count_u32 = u32::try_from(queries.len()).map_err(|_| GpuSettlingError::ValueTooLarge {
        field: "query_count",
        value: queries.len(),
    })?;
    let byte_len = queries
        .len()
        .checked_mul(size_of::<SettlingQuery>())
        .ok_or(GpuSettlingError::SizeOverflow {
            field: "settling_queries",
        })?;
    let byte_len_u64 = u64::try_from(byte_len).map_err(|_| GpuSettlingError::SizeOverflow {
        field: "settling_queries",
    })?;
    validate_storage_size(ctx, byte_len_u64, "settling_queries")?;
    push_device_error_scopes(&ctx.device);
    let buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("settling_queries"),
            contents: bytemuck::cast_slice(queries),
            usage: storage_usage(),
        });
    pop_device_error_scopes(&ctx.device, "query upload")?;
    Ok(SettlingQueryBuffers {
        buffer,
        query_count: queries.len(),
    })
}

/// Create a GPU-resident settling output buffer (explicit allocation, no H2D).
///
/// # Errors
/// Returns [`GpuSettlingError`] for zero or overflowing query counts.
pub fn create_settling_output_buffer(
    ctx: &GpuContext,
    query_count: usize,
) -> Result<SettlingVelocityOutput, GpuSettlingError> {
    if query_count == 0 {
        return Err(GpuSettlingError::EmptyQueries);
    }
    let _count_u32 = u32::try_from(query_count).map_err(|_| GpuSettlingError::ValueTooLarge {
        field: "query_count",
        value: query_count,
    })?;
    let bytes =
        query_count
            .checked_mul(size_of::<f32>())
            .ok_or(GpuSettlingError::SizeOverflow {
                field: "settling_output",
            })?;
    let size = u64::try_from(bytes).map_err(|_| GpuSettlingError::SizeOverflow {
        field: "settling_output",
    })?;
    validate_storage_size(ctx, size, "settling_output")?;
    push_device_error_scopes(&ctx.device);
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("settling_output"),
        size,
        usage: storage_usage(),
        mapped_at_creation: false,
    });
    pop_device_error_scopes(&ctx.device, "output allocation")?;
    Ok(SettlingVelocityOutput {
        buffer,
        query_count,
    })
}

/// Create an explicit uniform buffer for one query count.
///
/// # Errors
/// Returns [`GpuSettlingError`] for zero queries or oversized counts.
pub fn create_settling_uniform_buffer(
    ctx: &GpuContext,
    query_count: usize,
) -> Result<SettlingUniforms, GpuSettlingError> {
    if query_count == 0 {
        return Err(GpuSettlingError::EmptyQueries);
    }
    let query_count_u32 =
        u32::try_from(query_count).map_err(|_| GpuSettlingError::ValueTooLarge {
            field: "query_count",
            value: query_count,
        })?;
    debug_assert_eq!(
        size_of::<SettlingParamsRaw>() % 16,
        0,
        "settling uniform params must stay 16-byte aligned"
    );
    let params = SettlingParamsRaw {
        query_count: query_count_u32,
        _pad0: 0.0,
        _pad1: 0.0,
        _pad2: 0.0,
    };
    push_device_error_scopes(&ctx.device);
    let buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("settling_params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    pop_device_error_scopes(&ctx.device, "uniform upload")?;
    Ok(SettlingUniforms { buffer, params })
}

/// Encode settling velocity computation into a caller-provided encoder.
///
/// Production composition boundary: records GPU work without submitting,
/// waiting, transferring or allocating query buffers. The caller owns
/// submission and lifetime: `queries`, `output`, `uniforms` and `kernel`
/// must live until the encoded work completes.
///
/// # Errors
/// Returns [`GpuSettlingError`] for mismatched query counts or oversized counts.
pub fn encode_settling_velocities(
    ctx: &GpuContext,
    queries: &SettlingQueryBuffers,
    output: &SettlingVelocityOutput,
    uniforms: &SettlingUniforms,
    kernel: &SettlingVelocityKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuSettlingError> {
    if queries.query_count != output.query_count {
        return Err(GpuSettlingError::CountMismatch {
            field: "settling_velocities",
            queries: queries.query_count,
            outputs: output.query_count,
        });
    }
    if queries.query_count == 0 {
        return Err(GpuSettlingError::EmptyQueries);
    }
    let query_count_u32 =
        u32::try_from(queries.query_count).map_err(|_| GpuSettlingError::ValueTooLarge {
            field: "query_count",
            value: queries.query_count,
        })?;
    let expected = SettlingParamsRaw {
        query_count: query_count_u32,
        _pad0: 0.0,
        _pad1: 0.0,
        _pad2: 0.0,
    };
    if uniforms.params != expected {
        return Err(GpuSettlingError::OracleContract {
            message: "uniform query count does not match validated queries",
        });
    }
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("settling_velocity_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: queries.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("settling_velocity_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&kernel.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        super::dispatch_1d(&mut pass, query_count_u32, kernel.workgroup_size_x);
    }
    Ok(())
}

/// Dispatch settling velocities and wait for completion (no readback).
///
/// Convenience path for isolated oracle validation only. Production
/// composition must use [`encode_settling_velocities`] so dependent GPU
/// stages can share one submission without host synchronization.
///
/// # Errors
/// Forwards [`GpuSettlingError`] from encoding and scoped device errors.
pub fn dispatch_settling_velocities_and_wait(
    ctx: &GpuContext,
    queries: &SettlingQueryBuffers,
    output: &SettlingVelocityOutput,
    uniforms: &SettlingUniforms,
    kernel: &SettlingVelocityKernel,
) -> Result<(), GpuSettlingError> {
    push_device_error_scopes(&ctx.device);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("settling_velocity_encoder"),
        });
    if let Err(error) =
        encode_settling_velocities(ctx, queries, output, uniforms, kernel, &mut encoder)
    {
        let _ = pop_device_error_scopes(&ctx.device, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    pop_device_error_scopes(&ctx.device, "dispatch")
}

/// Explicit D2H readback of settling velocities.
///
/// Allowed only at validation/output boundaries. Fails closed on non-finite
/// values; the length check itself never hides a device failure.
///
/// # Errors
/// Forwards [`GpuSettlingError::Buffer`] from staging and mapping, or
/// [`GpuSettlingError::NonFiniteOutputValue`] for non-finite outputs.
pub async fn download_settling_velocities(
    ctx: &GpuContext,
    output: &SettlingVelocityOutput,
) -> Result<Vec<f32>, GpuSettlingError> {
    let values =
        download_buffer_typed::<f32>(ctx, &output.buffer, output.query_count, "settling_output")
            .await?;
    for value in &values {
        if !value.is_finite() {
            return Err(GpuSettlingError::NonFiniteOutputValue);
        }
    }
    Ok(values)
}

/// Compute settling velocities with GPU-executed arithmetic.
///
/// Isolated oracle-verification workflow. Validates nothing beyond the
/// already-validated [`SettlingQuery`] inputs, executes the WGSL kernel,
/// reads back at an explicit validation boundary and fails closed on
/// non-finite outputs. No CPU fallback exists: any GPU failure surfaces as
/// an error.
///
/// # Errors
/// Returns [`GpuSettlingError`] for empty queries, buffer/GPU failures or
/// non-finite outputs.
pub async fn compute_settling_velocities_gpu(
    ctx: &GpuContext,
    queries: &[SettlingQuery],
    kernel: &SettlingVelocityKernel,
) -> Result<Vec<f32>, GpuSettlingError> {
    let query_buffers = create_settling_query_buffers(ctx, queries)?;
    let output = create_settling_output_buffer(ctx, query_buffers.query_count)?;
    let uniforms = create_settling_uniform_buffer(ctx, query_buffers.query_count)?;
    dispatch_settling_velocities_and_wait(ctx, &query_buffers, &output, &uniforms, kernel)?;
    download_settling_velocities(ctx, &output).await
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
pub fn settling_shader_sha256() -> String {
    sha256_hex(SHADER_SOURCE.as_bytes())
}

/// SHA-256 of the normalized JSON encoding of dispatched GPU inputs.
///
/// Binds per-query diameter, density, temperature and air density so evidence
/// cannot claim a different input than what was dispatched.
///
/// # Errors
/// Returns [`GpuSettlingError::InputHash`] when JSON encoding fails.
pub fn settling_inputs_sha256(queries: &[SettlingQuery]) -> Result<String, GpuSettlingError> {
    #[derive(Serialize)]
    struct NormalizedSettlingInput {
        queries: Vec<[f64; 4]>,
    }
    let normalized = NormalizedSettlingInput {
        queries: queries
            .iter()
            .map(|query| {
                [
                    f64::from(query.diameter_um),
                    f64::from(query.particle_density_kg_m3),
                    f64::from(query.temperature_k),
                    f64::from(query.air_density_kg_m3),
                ]
            })
            .collect(),
    };
    let json = serde_json::to_vec(&normalized).map_err(|err| GpuSettlingError::InputHash {
        message: err.to_string(),
    })?;
    Ok(sha256_hex(&json))
}

/// Build the repository-wide comparison policy owned by #35.
///
/// Absolute `1e-9` m/s covers the near-zero regime; relative `0.01` (1%)
/// covers the declared valid domain per the #35 proof obligation and the #91
/// generic finite comparator (`absolute <= A OR relative <= R`).
///
/// # Errors
/// Returns [`GpuSettlingError::Evidence`] for an invalid tolerance policy.
pub fn default_comparison_policy() -> Result<ComparisonPolicy, GpuSettlingError> {
    Ok(ComparisonPolicy::new(
        SETTLING_GPU_ABSOLUTE_TOLERANCE,
        SETTLING_GPU_RELATIVE_TOLERANCE,
    )?)
}

/// One machine-readable GPU-vs-oracle comparison row for #35.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettlingGpuRow {
    /// Stable canonical vector identifier (e.g. `SETTLE-001`).
    pub vector_id: String,
    /// Species name from the #10 carrier that produced this row.
    pub species_name: String,
    /// Volume-equivalent diameter [um].
    pub diameter_um: f32,
    /// Particle material density [kg/m3].
    pub particle_density_kg_m3: f32,
    /// Local air temperature [K].
    pub temperature_k: f32,
    /// Local moist-air density [kg/m3].
    pub air_density_kg_m3: f32,
    /// Element value computed on the GPU [m/s] (negative, downward).
    pub gpu_settling_velocity_m_s: f32,
    /// Pinned oracle value [m/s] (negative, downward).
    pub oracle_settling_velocity_m_s: f32,
    /// Predeclared comparison policy.
    pub comparison_policy: ComparisonPolicy,
    /// Absolute GPU-oracle difference [m/s].
    pub absolute_difference: f64,
    /// Relative GPU-oracle difference [-].
    pub relative_difference: f64,
    /// Combined row verdict.
    pub row_verdict: bool,
    /// Repository-wide GPU execution and numerical evidence.
    pub gpu_evidence: GpuCalculationEvidence,
}

/// Machine-readable GPU-vs-oracle comparison report for #35.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettlingGpuReport {
    /// Report schema identity.
    pub schema: SettlingReportSchema,
    /// Stable scenario identifier covering all pinned settling vectors.
    pub scenario_id: String,
    /// Candidate description recorded for traceability.
    pub candidate: String,
    /// Physical units for every recorded field.
    pub units: SettlingReportUnits,
    /// Predeclared comparison policy.
    pub comparison_policy: ComparisonPolicy,
    /// Per-vector comparison rows with GPU evidence.
    pub rows: Vec<SettlingGpuRow>,
    /// Overall report verdict (`true` only when every row passes).
    pub status: bool,
}

/// Physical units for the issue-specific settling comparison output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlingReportUnits {
    /// Diameter unit.
    pub diameter_um: String,
    /// Particle density unit.
    pub particle_density_kg_m3: String,
    /// Air temperature unit.
    pub temperature_k: String,
    /// Air density unit.
    pub air_density_kg_m3: String,
    /// Settling velocity unit (negative, downward).
    pub settling_velocity_m_s: String,
    /// Absolute difference unit.
    pub absolute_difference_m_s: String,
    /// Relative difference unit (dimensionless).
    pub relative_difference: String,
}

impl Default for SettlingReportUnits {
    fn default() -> Self {
        Self {
            diameter_um: "micrometre".to_string(),
            particle_density_kg_m3: "kilogram_per_cubic_metre".to_string(),
            temperature_k: "kelvin".to_string(),
            air_density_kg_m3: "kilogram_per_cubic_metre".to_string(),
            settling_velocity_m_s: "metre_per_second (negative, downward)".to_string(),
            absolute_difference_m_s: "metre_per_second".to_string(),
            relative_difference: "dimensionless".to_string(),
        }
    }
}

/// Schema identity for the issue-specific settling report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlingReportSchema {
    /// Stable schema name.
    pub id: String,
    /// Schema version.
    pub version: u32,
}

impl Default for SettlingReportSchema {
    fn default() -> Self {
        Self {
            id: SETTLING_GPU_REPORT_SCHEMA_ID.to_string(),
            version: SETTLING_GPU_REPORT_SCHEMA_VERSION,
        }
    }
}

impl SettlingGpuRow {
    /// Validate structural honesty and fail closed on contradiction.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] for contradictory verdicts, differences
    /// or embedded evidence that cannot prove a claimed pass.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        self.validate_identity()?;
        self.validate_values()?;
        self.validate_evidence_consistency()?;
        self.gpu_evidence.validate()?;
        self.gpu_evidence.require_paired_pass()?;
        Ok(())
    }

    fn validate_identity(&self) -> Result<(), GpuEvidenceError> {
        let invalid = || {
            GpuEvidenceError::InvalidComparisonState(
                "settling row contradicts normalized inputs or facet verdicts",
            )
        };
        if self.vector_id.trim().is_empty() || self.species_name.trim().is_empty() {
            return Err(invalid());
        }
        if self.gpu_evidence.case_id != format!("settling-gpu/{}", self.vector_id) {
            return Err(invalid());
        }
        if self.gpu_evidence.candidate.implementation_id != SETTLING_GPU_IMPLEMENTATION_ID {
            return Err(invalid());
        }
        if self.gpu_evidence.candidate.shader_sha256 != settling_shader_sha256() {
            return Err(invalid());
        }
        if !is_lowercase_git_sha(&self.gpu_evidence.candidate.revision) {
            return Err(invalid());
        }
        let oracle = self.gpu_evidence.oracle.as_ref().ok_or_else(invalid)?;
        if oracle.implementation_id != SETTLING_ORACLE_IMPLEMENTATION_ID {
            return Err(invalid());
        }
        if oracle.revision != SETTLING_ORACLE_REVISION {
            return Err(invalid());
        }
        Ok(())
    }

    fn validate_values(&self) -> Result<(), GpuEvidenceError> {
        let invalid = || {
            GpuEvidenceError::InvalidComparisonState(
                "settling row contradicts normalized inputs or facet verdicts",
            )
        };
        if !self.gpu_settling_velocity_m_s.is_finite()
            || !self.oracle_settling_velocity_m_s.is_finite()
            || !self.absolute_difference.is_finite()
            || !self.relative_difference.is_finite()
        {
            return Err(invalid());
        }
        // Sign convention: settling is strictly downward (negative) for all
        // valid spherical carriers. Zero or upward values contradict the
        // pinned formulation and fail closed here.
        if self.gpu_settling_velocity_m_s >= 0.0 || self.oracle_settling_velocity_m_s >= 0.0 {
            return Err(invalid());
        }
        let expected_absolute = (f64::from(self.gpu_settling_velocity_m_s)
            - f64::from(self.oracle_settling_velocity_m_s))
        .abs();
        if (self.absolute_difference - expected_absolute).abs() > 1.0e-12 {
            return Err(invalid());
        }
        let scale = f64::from(self.gpu_settling_velocity_m_s)
            .abs()
            .max(f64::from(self.oracle_settling_velocity_m_s).abs());
        let expected_relative = if scale == 0.0 {
            0.0
        } else {
            expected_absolute / scale
        };
        if (self.relative_difference - expected_relative).abs() > 1.0e-12 {
            return Err(invalid());
        }
        let comparison = compare_finite_values(
            &[f64::from(self.oracle_settling_velocity_m_s)],
            &[f64::from(self.gpu_settling_velocity_m_s)],
            self.comparison_policy,
        )?;
        let expected_verdict = comparison.verdict == NumericalVerdict::Passed;
        if self.row_verdict != expected_verdict {
            return Err(invalid());
        }
        if self.gpu_evidence.comparison.verdict != comparison.verdict {
            return Err(invalid());
        }
        Ok(())
    }

    fn validate_evidence_consistency(&self) -> Result<(), GpuEvidenceError> {
        let invalid = || {
            GpuEvidenceError::InvalidComparisonState(
                "settling row contradicts normalized inputs or facet verdicts",
            )
        };
        let default_policy = default_comparison_policy().map_err(|_| invalid())?;
        if self.comparison_policy != default_policy {
            return Err(invalid());
        }
        let evidence_policy = self
            .gpu_evidence
            .comparison
            .policy
            .ok_or_else(invalid)?;
        if evidence_policy != self.comparison_policy {
            return Err(invalid());
        }
        let evidence_max_abs = self
            .gpu_evidence
            .comparison
            .max_absolute_error
            .ok_or_else(invalid)?;
        if (evidence_max_abs - self.absolute_difference).abs() > 1.0e-12 {
            return Err(invalid());
        }
        let evidence_max_rel = self
            .gpu_evidence
            .comparison
            .max_relative_error
            .ok_or_else(invalid)?;
        if (evidence_max_rel - self.relative_difference).abs() > 1.0e-12 {
            return Err(invalid());
        }
        if self.gpu_evidence.comparison.oracle_value_count != 1
            || self.gpu_evidence.comparison.candidate_value_count != 1
            || self.gpu_evidence.comparison.compared_value_count != 1
        {
            return Err(invalid());
        }
        let query = SettlingQuery {
            diameter_um: self.diameter_um,
            particle_density_kg_m3: self.particle_density_kg_m3,
            temperature_k: self.temperature_k,
            air_density_kg_m3: self.air_density_kg_m3,
        };
        let expected_input_sha =
            settling_inputs_sha256(std::slice::from_ref(&query)).map_err(|_| invalid())?;
        if self.gpu_evidence.candidate.input_sha256 != expected_input_sha {
            return Err(invalid());
        }
        Ok(())
    }
}

impl SettlingGpuReport {
    /// Validate every row and the overall status.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] when any row is dishonest or the overall
    /// status contradicts the row verdicts.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        if self.schema.id != SETTLING_GPU_REPORT_SCHEMA_ID
            || self.schema.version != SETTLING_GPU_REPORT_SCHEMA_VERSION
        {
            return Err(GpuEvidenceError::UnsupportedSchema {
                id: self.schema.id.clone(),
                version: self.schema.version,
            });
        }
        if self.rows.is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "settling report contains no rows",
            ));
        }
        if self.scenario_id.trim().is_empty() || self.candidate.trim().is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "settling report scenario_id and candidate must be non-empty",
            ));
        }
        let default_policy = default_comparison_policy()
            .map_err(|_| GpuEvidenceError::InvalidComparisonState("invalid default policy"))?;
        if self.comparison_policy != default_policy {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "settling report comparison policy must match the issue-owned default",
            ));
        }
        if self.units != SettlingReportUnits::default() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "settling report units must match the declared defaults",
            ));
        }
        for row in &self.rows {
            row.validate()?;
        }
        let expected_status = self.rows.iter().all(|row| row.row_verdict);
        if self.status != expected_status {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "settling report status contradicts row verdicts",
            ));
        }
        Ok(())
    }

    /// Require a passing paired validation for every row.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError::NotPassing`] unless every row proves a
    /// successful WGSL execution and pinned-oracle comparison.
    pub fn require_paired_pass(&self) -> Result<(), GpuEvidenceError> {
        self.validate()?;
        if !self.status {
            return Err(GpuEvidenceError::NotPassing);
        }
        Ok(())
    }
}

/// Build one machine-readable GPU-vs-oracle comparison row.
///
/// The GPU value must come from actual WGSL/device execution; the oracle
/// value must come from the pinned FLEXPART 11.1 oracle fixture. No Rust CPU
/// settling implementation participates in this comparison.
///
/// # Errors
/// Returns [`GpuSettlingError`] for non-finite values, invalid candidate
/// provenance, or evidence validation failures.
#[allow(clippy::too_many_arguments)]
pub fn build_settling_gpu_row(
    ctx: &GpuContext,
    vector_id: &str,
    species_name: &str,
    query: SettlingQuery,
    gpu_value: f32,
    oracle_value: f32,
    comparison_policy: ComparisonPolicy,
    candidate_revision: &str,
    oracle: PinnedOracleEvidence,
) -> Result<SettlingGpuRow, GpuSettlingError> {
    if vector_id.trim().is_empty() {
        return Err(GpuSettlingError::OracleContract {
            message: "vector_id must not be empty",
        });
    }
    if species_name.trim().is_empty() {
        return Err(GpuSettlingError::OracleContract {
            message: "species_name must not be empty",
        });
    }
    if !gpu_value.is_finite() || !oracle_value.is_finite() {
        return Err(GpuSettlingError::NonFiniteOutputValue);
    }
    validate_candidate_revision(candidate_revision)?;
    let comparison = compare_finite_values(
        &[f64::from(oracle_value)],
        &[f64::from(gpu_value)],
        comparison_policy,
    )?;
    let row_verdict = comparison.verdict == NumericalVerdict::Passed;
    let absolute_difference = (f64::from(gpu_value) - f64::from(oracle_value)).abs();
    let scale = f64::from(gpu_value)
        .abs()
        .max(f64::from(oracle_value).abs());
    let relative_difference = if scale == 0.0 {
        0.0
    } else {
        absolute_difference / scale
    };
    let input_sha = settling_inputs_sha256(std::slice::from_ref(&query))?;
    let case_id = format!("settling-gpu/{vector_id}");
    let gpu_evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id,
        candidate: GpuCandidateEvidence {
            implementation_id: SETTLING_GPU_IMPLEMENTATION_ID.to_string(),
            revision: candidate_revision.to_string(),
            shader_sha256: settling_shader_sha256(),
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
    Ok(SettlingGpuRow {
        vector_id: vector_id.to_string(),
        species_name: species_name.to_string(),
        diameter_um: query.diameter_um,
        particle_density_kg_m3: query.particle_density_kg_m3,
        temperature_k: query.temperature_k,
        air_density_kg_m3: query.air_density_kg_m3,
        gpu_settling_velocity_m_s: gpu_value,
        oracle_settling_velocity_m_s: oracle_value,
        comparison_policy,
        absolute_difference,
        relative_difference,
        row_verdict,
        gpu_evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settling_carrier_rejects_gas_species() {
        use std::collections::BTreeMap;
        let gas = SpeciesConfig {
            name: "gas".to_string(),
            version: None,
            molecular_weight: None,
            dry_deposition_velocity: None,
            decay_constant: None,
            half_life_s: None,
            wet_a_gas: None,
            wet_b_gas: None,
            crain_aero: None,
            csnow_aero: None,
            ccn_aero: None,
            in_aero: None,
            relative_diffusivity: Some(0.8),
            henry: None,
            surface_reactivity_f0: None,
            particle_density_kg_m3: None,
            mean_diameter_um: None,
            diameter_sigma: None,
            source_file: None,
            raw: BTreeMap::new(),
        };
        assert!(SettlingCarrier::from_species_config(&gas).is_err());
    }

    #[test]
    fn settling_query_rejects_out_of_domain_meteo() {
        let carrier = SettlingCarrier {
            particle_density_kg_m3: 1000.0,
            diameter_um: 10.0,
        };
        assert!(SettlingQuery::new(carrier, f32::NAN, 1.2).is_err());
        assert!(SettlingQuery::new(carrier, 293.15, 0.0).is_err());
        assert!(SettlingQuery::new(carrier, 50.0, 1.2).is_err());
    }

    #[test]
    fn settling_shader_is_nonempty_and_hashable() {
        assert!(!SHADER_SOURCE.trim().is_empty());
        let hash = settling_shader_sha256();
        assert_eq!(hash.len(), 64);
    }
}
