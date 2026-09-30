//! Canonical instantaneous temporal interpolation on the GPU for issue #89.
//!
//! Ports #74 semantics (frozen #71 FLEXPART 11.1 `find_time_vars` +
//! `temporal_interpolation`, `interpol_mod.f90:190-198` and `:531-537`) to
//! actual WGSL/device execution. Reuses `GpuContext`, explicit storage-buffer
//! conventions, and the #91 `GpuCalculationEvidence` model. Production
//! composition uses `encode_temporal_blend` (caller-owned encoder, no
//! submit/wait/readback/allocation). `sample_field_gpu` is an isolated
//! oracle-verification workflow with explicit H2D/D2H. No silent CPU fallback.

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
use crate::meteorology::temporal::{
    self, RequestedSampleTime, SourceTimestamps, TemporalApplication, TemporalBracket,
    TemporalError, TemporalWeights, Verdict, DEFAULT_ABS_TOL, DEFAULT_REL_TOL,
};
use crate::meteorology::{Calendar, FieldId, SchemaIdentity, Snapshot};

const SHADER_SOURCE: &str = include_str!("../shaders/temporal_interpolation.wgsl");
const WORKGROUP_SIZE_X: u32 = 64;
pub const TEMPORAL_GPU_IMPLEMENTATION_ID: &str = "meteorology::temporal-gpu::encode_temporal_blend";
pub const TEMPORAL_ORACLE_IMPLEMENTATION_ID: &str = "FLEXPART-11.1 temporal_interpolation";
pub const TEMPORAL_ORACLE_REVISION: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";
pub const TEMPORAL_ORACLE_EXECUTABLE_SHA256: &str =
    "388d1f824306df30fc74fbedc6464e86602b6a93ba782e9e1dad20ffacc3327c";
pub const TEMPORAL_ORACLE_OUTPUT_SHA256: &str =
    "ae44f1e5dba664011bb51029d2a58693d39f5aa975a00214baf6aa9cb5f92085";
pub const TEMPORAL_GPU_REPORT_SCHEMA_ID: &str = "flexpart-gpu.temporal-gpu-evidence";
pub const TEMPORAL_GPU_REPORT_SCHEMA_VERSION: u32 = 1;
pub const TEMPORAL_GPU_CANDIDATE_DESCRIPTION: &str =
    "meteorology::temporal-gpu::encode_temporal_blend (WGSL device)";
const TEMPORAL_ORACLE_SCENARIO_ID: &str = "oracle-temporal-bilinear";

#[derive(Debug, Error)]
pub enum GpuTemporalError {
    #[error(transparent)]
    Temporal(#[from] TemporalError),
    #[error(transparent)]
    Buffer(#[from] GpuBufferError),
    #[error(transparent)]
    Evidence(#[from] GpuEvidenceError),
    #[error("value for {field} does not fit in u32: {value}")]
    ValueTooLarge { field: &'static str, value: usize },
    #[error("byte-size overflow while preparing {field}")]
    SizeOverflow { field: &'static str },
    #[error("temporal field has no elements to interpolate")]
    EmptyField,
    #[error("bracket element count {bracket} disagrees with resource element count {resource}")]
    MismatchedElementCount { bracket: usize, resource: usize },
    #[error("non-finite or out-of-range temporal blend weight: lower={w_lower}, upper={w_upper}")]
    InvalidBlendWeight { w_lower: f32, w_upper: f32 },
    #[error("invalid temporal bracket metadata: {message}")]
    InvalidBracketMetadata { message: &'static str },
    #[error(
        "temporal bracket timestamps do not match: expected [{expected_lower}, {expected_upper}], got [{actual_lower}, {actual_upper}]"
    )]
    MismatchedBracketTimestamps {
        expected_lower: i64,
        expected_upper: i64,
        actual_lower: i64,
        actual_upper: i64,
    },
    #[error("temporal uniform metadata does not match the requested bracket")]
    MismatchedUniforms,
    #[error("GPU {scope} error during temporal interpolation: {message}")]
    Device {
        scope: &'static str,
        message: String,
    },
    #[error("non-finite value in temporal bracket buffer at element {element_index}")]
    NonFiniteBufferValue { element_index: usize },
    #[error("non-finite GPU temporal output at element {element_index}")]
    NonFiniteOutputValue { element_index: usize },
    #[error("candidate revision must be a 40-character lowercase Git SHA, got {value}")]
    InvalidCandidateRevision { value: String },
    #[error("failed to hash candidate inputs: {message}")]
    InputHash { message: String },
    #[error("request does not match the pinned #71 temporal oracle contract: {message}")]
    OracleContract { message: &'static str },
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct TemporalBlendParamsRaw {
    dt1: f32,
    dt2: f32,
    dtt: f32,
    element_count: u32,
}

impl TemporalBlendParamsRaw {
    /// # Errors
    /// Returns [`GpuTemporalError::InvalidBlendWeight`] for non-finite or
    /// out-of-range weights, or [`GpuTemporalError::ValueTooLarge`] when the
    /// element count does not fit in `u32`.
    fn from_bracket(bracket: &TemporalBracket) -> Result<Self, GpuTemporalError> {
        if bracket.request.calendar != bracket.calendar
            || bracket.source_timestamps.lower_epoch_seconds
                >= bracket.source_timestamps.upper_epoch_seconds
            || bracket.request.epoch_seconds < bracket.source_timestamps.lower_epoch_seconds
            || bracket.request.epoch_seconds > bracket.source_timestamps.upper_epoch_seconds
        {
            return Err(GpuTemporalError::InvalidBracketMetadata {
                message: "calendar, ordered timestamps, and request coverage must agree",
            });
        }
        let dt1_i128 = i128::from(bracket.request.epoch_seconds)
            - i128::from(bracket.source_timestamps.lower_epoch_seconds);
        let dt2_i128 = i128::from(bracket.source_timestamps.upper_epoch_seconds)
            - i128::from(bracket.request.epoch_seconds);
        #[allow(clippy::cast_precision_loss)]
        let expected_dt1 = dt1_i128 as f64;
        #[allow(clippy::cast_precision_loss)]
        let expected_dt2 = dt2_i128 as f64;
        let expected_inverse_span = 1.0 / (expected_dt1 + expected_dt2);
        if bracket.weights.dt1_seconds.to_bits() != expected_dt1.to_bits()
            || bracket.weights.dt2_seconds.to_bits() != expected_dt2.to_bits()
            || bracket.weights.inverse_span_per_second.to_bits() != expected_inverse_span.to_bits()
        {
            return Err(GpuTemporalError::InvalidBracketMetadata {
                message: "reported DTS weights must be derived from the bracket timestamps",
            });
        }
        let (w_lower_f64, w_upper_f64) = bracket.weights.normalized();
        if !w_lower_f64.is_finite() || !w_upper_f64.is_finite() {
            return Err(GpuTemporalError::InvalidBlendWeight {
                w_lower: 0.0,
                w_upper: 0.0,
            });
        }
        #[allow(clippy::cast_possible_truncation)]
        let w_lower = w_lower_f64 as f32;
        #[allow(clippy::cast_possible_truncation)]
        let w_upper = w_upper_f64 as f32;
        if !w_lower.is_finite()
            || !w_upper.is_finite()
            || w_lower < 0.0
            || w_lower > 1.0
            || w_upper < 0.0
            || w_upper > 1.0
        {
            return Err(GpuTemporalError::InvalidBlendWeight { w_lower, w_upper });
        }
        if bracket.element_count == 0 {
            return Err(GpuTemporalError::EmptyField);
        }
        let element_count =
            u32::try_from(bracket.element_count).map_err(|_| GpuTemporalError::ValueTooLarge {
                field: "element_count",
                value: bracket.element_count,
            })?;
        #[allow(clippy::cast_possible_truncation)]
        let dt1 = bracket.weights.dt1_seconds as f32;
        #[allow(clippy::cast_possible_truncation)]
        let dt2 = bracket.weights.dt2_seconds as f32;
        #[allow(clippy::cast_possible_truncation)]
        let dtt = bracket.weights.inverse_span_per_second as f32;
        if !dt1.is_finite() || !dt2.is_finite() || !dtt.is_finite() {
            return Err(GpuTemporalError::InvalidBlendWeight { w_lower, w_upper });
        }
        Ok(Self {
            dt1,
            dt2,
            dtt,
            element_count,
        })
    }
}

fn push_device_error_scopes(device: &wgpu::Device) {
    device.push_error_scope(wgpu::ErrorFilter::Internal);
    device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    device.push_error_scope(wgpu::ErrorFilter::Validation);
}

fn pop_device_error_scopes(
    device: &wgpu::Device,
    scope: &'static str,
) -> Result<(), GpuTemporalError> {
    let validation = pollster::block_on(device.pop_error_scope());
    let out_of_memory = pollster::block_on(device.pop_error_scope());
    let internal = pollster::block_on(device.pop_error_scope());
    if let Some(error) = [validation, out_of_memory, internal]
        .into_iter()
        .flatten()
        .next()
    {
        return Err(GpuTemporalError::Device {
            scope,
            message: error.to_string(),
        });
    }
    Ok(())
}

/// Reusable temporal interpolation dispatch kernel.
pub struct TemporalInterpolationKernel {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl TemporalInterpolationKernel {
    /// Prepare the WGSL pipeline and bind-group layout once per context.
    /// # Errors
    /// Returns [`GpuTemporalError::Device`] when shader or pipeline creation
    /// raises a scoped `wgpu` error.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuTemporalError> {
        push_device_error_scopes(&ctx.device);
        let shader = ctx.load_shader("temporal_interpolation_shader", SHADER_SOURCE);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("temporal_interpolation_bgl"),
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
            "temporal_interpolation_pipeline",
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

/// GPU-resident lower/upper bracket field values.
pub struct TemporalBracketBuffers {
    /// Lower-bracket field values, device-resident.
    pub lower_buffer: wgpu::Buffer,
    /// Upper-bracket field values, device-resident.
    pub upper_buffer: wgpu::Buffer,
    element_count: usize,
    lower_epoch_seconds: i64,
    upper_epoch_seconds: i64,
}

impl TemporalBracketBuffers {
    /// Number of field elements in each bracket buffer.
    #[must_use]
    pub const fn element_count(&self) -> usize {
        self.element_count
    }

    /// Lower bracket timestamp.
    #[must_use]
    pub const fn lower_epoch_seconds(&self) -> i64 {
        self.lower_epoch_seconds
    }

    /// Upper bracket timestamp.
    #[must_use]
    pub const fn upper_epoch_seconds(&self) -> i64 {
        self.upper_epoch_seconds
    }
}

/// GPU-resident temporal blend output.
pub struct TemporalBlendOutput {
    /// Blended field values, device-resident.
    pub buffer: wgpu::Buffer,
    element_count: usize,
}

/// Typed uniform resource bound to one validated temporal bracket.
pub struct TemporalBlendUniforms {
    buffer: wgpu::Buffer,
    params: TemporalBlendParamsRaw,
    source_timestamps: SourceTimestamps,
}

impl TemporalBlendOutput {
    /// Number of field elements in the output buffer.
    #[must_use]
    pub const fn element_count(&self) -> usize {
        self.element_count
    }
}

fn storage_usage() -> wgpu::BufferUsages {
    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC
}

fn checked_byte_len(len: usize, field: &'static str) -> Result<u64, GpuTemporalError> {
    let bytes = len
        .checked_mul(size_of::<f32>())
        .ok_or(GpuTemporalError::SizeOverflow { field })?;
    u64::try_from(bytes).map_err(|_| GpuTemporalError::SizeOverflow { field })
}

fn validate_finite_values(values: &[f32]) -> Result<(), GpuTemporalError> {
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(GpuTemporalError::NonFiniteBufferValue {
                element_index: index,
            });
        }
    }
    Ok(())
}

fn validate_finite_output_values(values: &[f32]) -> Result<(), GpuTemporalError> {
    for (element_index, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(GpuTemporalError::NonFiniteOutputValue { element_index });
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

fn validate_candidate_revision(revision: &str) -> Result<(), GpuTemporalError> {
    if !is_lowercase_git_sha(revision) {
        return Err(GpuTemporalError::InvalidCandidateRevision {
            value: revision.to_string(),
        });
    }
    Ok(())
}

/// Create GPU-resident bracket buffers from validated host values (explicit H2D).
///
/// # Errors
/// Returns [`GpuTemporalError`] for empty, mismatched-length or non-finite inputs.
pub fn create_temporal_bracket_buffers(
    ctx: &GpuContext,
    lower_values: &[f32],
    upper_values: &[f32],
    lower_epoch_seconds: i64,
    upper_epoch_seconds: i64,
) -> Result<TemporalBracketBuffers, GpuTemporalError> {
    if lower_values.is_empty() || upper_values.is_empty() {
        return Err(GpuTemporalError::EmptyField);
    }
    if lower_values.len() != upper_values.len() {
        return Err(GpuTemporalError::MismatchedElementCount {
            bracket: lower_values.len(),
            resource: upper_values.len(),
        });
    }
    if lower_epoch_seconds >= upper_epoch_seconds {
        return Err(GpuTemporalError::InvalidBracketMetadata {
            message: "lower bracket timestamp must precede upper bracket timestamp",
        });
    }
    validate_finite_values(lower_values)?;
    validate_finite_values(upper_values)?;
    let _ = checked_byte_len(lower_values.len(), "temporal_bracket")?;

    let lower_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("temporal_bracket_lower"),
            contents: bytemuck::cast_slice(lower_values),
            usage: storage_usage(),
        });
    let upper_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("temporal_bracket_upper"),
            contents: bytemuck::cast_slice(upper_values),
            usage: storage_usage(),
        });
    Ok(TemporalBracketBuffers {
        lower_buffer,
        upper_buffer,
        element_count: lower_values.len(),
        lower_epoch_seconds,
        upper_epoch_seconds,
    })
}

/// Create a GPU-resident blend output buffer (explicit allocation, no H2D).
///
/// # Errors
/// Returns [`GpuTemporalError`] for zero or overflowing element counts.
pub fn create_temporal_output_buffer(
    ctx: &GpuContext,
    element_count: usize,
) -> Result<TemporalBlendOutput, GpuTemporalError> {
    if element_count == 0 {
        return Err(GpuTemporalError::EmptyField);
    }
    let size = checked_byte_len(element_count, "temporal_output")?;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("temporal_blend_output"),
        size,
        usage: storage_usage(),
        mapped_at_creation: false,
    });
    Ok(TemporalBlendOutput {
        buffer,
        element_count,
    })
}

/// Create an explicit uniform buffer from a validated temporal bracket.
///
/// # Errors
/// Returns [`GpuTemporalError`] for invalid brackets or blend weights.
pub fn create_temporal_uniform_buffer(
    ctx: &GpuContext,
    bracket: &TemporalBracket,
) -> Result<TemporalBlendUniforms, GpuTemporalError> {
    let params = TemporalBlendParamsRaw::from_bracket(bracket)?;
    let buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("temporal_blend_params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    Ok(TemporalBlendUniforms {
        buffer,
        params,
        source_timestamps: bracket.source_timestamps,
    })
}

/// Encode temporal blending into a caller-provided command encoder.
///
/// Production composition boundary: records GPU work without submitting,
/// waiting, transferring or allocating field buffers.
///
/// # Errors
/// Returns [`GpuTemporalError`] for invalid brackets, mismatched element
/// counts or invalid blend weights.
pub fn encode_temporal_blend(
    ctx: &GpuContext,
    bracket_buffers: &TemporalBracketBuffers,
    output: &TemporalBlendOutput,
    uniforms: &TemporalBlendUniforms,
    bracket: &TemporalBracket,
    kernel: &TemporalInterpolationKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuTemporalError> {
    if bracket.element_count == 0 {
        return Err(GpuTemporalError::EmptyField);
    }
    if bracket_buffers.element_count != bracket.element_count {
        return Err(GpuTemporalError::MismatchedElementCount {
            bracket: bracket.element_count,
            resource: bracket_buffers.element_count,
        });
    }
    if output.element_count != bracket.element_count {
        return Err(GpuTemporalError::MismatchedElementCount {
            bracket: bracket.element_count,
            resource: output.element_count,
        });
    }
    if bracket_buffers.lower_epoch_seconds != bracket.source_timestamps.lower_epoch_seconds
        || bracket_buffers.upper_epoch_seconds != bracket.source_timestamps.upper_epoch_seconds
    {
        return Err(GpuTemporalError::MismatchedBracketTimestamps {
            expected_lower: bracket.source_timestamps.lower_epoch_seconds,
            expected_upper: bracket.source_timestamps.upper_epoch_seconds,
            actual_lower: bracket_buffers.lower_epoch_seconds,
            actual_upper: bracket_buffers.upper_epoch_seconds,
        });
    }
    let expected_params = TemporalBlendParamsRaw::from_bracket(bracket)?;
    if uniforms.params != expected_params || uniforms.source_timestamps != bracket.source_timestamps
    {
        return Err(GpuTemporalError::MismatchedUniforms);
    }

    let element_count_u32 =
        u32::try_from(bracket.element_count).map_err(|_| GpuTemporalError::ValueTooLarge {
            field: "element_count",
            value: bracket.element_count,
        })?;

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("temporal_interpolation_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: bracket_buffers.lower_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: bracket_buffers.upper_buffer.as_entire_binding(),
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
            label: Some("temporal_interpolation_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&kernel.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        super::dispatch_1d(&mut pass, element_count_u32, kernel.workgroup_size_x);
    }
    Ok(())
}

/// Dispatch temporal blending and wait for completion (convenience, no readback).
///
/// # Errors
/// Forwards [`GpuTemporalError`] from encoding.
pub fn dispatch_temporal_blend_and_wait(
    ctx: &GpuContext,
    bracket_buffers: &TemporalBracketBuffers,
    output: &TemporalBlendOutput,
    uniforms: &TemporalBlendUniforms,
    bracket: &TemporalBracket,
    kernel: &TemporalInterpolationKernel,
) -> Result<(), GpuTemporalError> {
    push_device_error_scopes(&ctx.device);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("temporal_interpolation_encoder"),
        });
    if let Err(error) = encode_temporal_blend(
        ctx,
        bracket_buffers,
        output,
        uniforms,
        bracket,
        kernel,
        &mut encoder,
    ) {
        let _ = pop_device_error_scopes(&ctx.device, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    pop_device_error_scopes(&ctx.device, "dispatch")
}

/// Explicit D2H readback of a temporal blend output (validation/output boundary only).
///
/// # Errors
/// Forwards [`GpuTemporalError::Buffer`] from staging and mapping.
pub async fn download_temporal_output(
    ctx: &GpuContext,
    output: &TemporalBlendOutput,
) -> Result<Vec<f32>, GpuTemporalError> {
    Ok(download_buffer_typed::<f32>(
        ctx,
        &output.buffer,
        output.element_count,
        "temporal_blend_output",
    )
    .await?)
}

/// GPU sample with device-computed values and host-validated metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct TemporalGpuSample {
    /// Canonical field identity that was sampled.
    pub field_id: FieldId,
    /// Request that produced the sample.
    pub request: RequestedSampleTime,
    /// How the requested time relates to the source series.
    pub application: TemporalApplication,
    /// Active bracketing source timestamps.
    pub source_timestamps: SourceTimestamps,
    /// Temporal weights in `DTS`-aligned form.
    pub weights: TemporalWeights,
    /// Elementwise blended values computed on the GPU.
    pub values: Vec<f32>,
    /// Validated bracket resolution.
    pub bracket: TemporalBracket,
}

fn find_field_values(
    snapshot: &Snapshot,
    field_id: FieldId,
    snapshot_index: usize,
) -> Result<&[f32], GpuTemporalError> {
    snapshot
        .fields
        .iter()
        .find(|field| field.id == field_id)
        .map(|field| field.values.as_slice())
        .ok_or(TemporalError::MissingFieldSnapshot {
            snapshot_index,
            field_id,
        })
        .map_err(GpuTemporalError::Temporal)
}

/// Sample one canonical instantaneous field with GPU-executed arithmetic.
///
/// Isolated oracle-verification workflow (test-only, not production).
///
/// # Errors
/// Returns [`GpuTemporalError`] for #74 fail-closed cases, empty fields,
/// buffer/GPU failures or readback failures.
pub async fn sample_field_gpu(
    ctx: &GpuContext,
    field_id: FieldId,
    snapshots: &[&Snapshot],
    request: RequestedSampleTime,
    kernel: &TemporalInterpolationKernel,
) -> Result<TemporalGpuSample, GpuTemporalError> {
    let bracket = temporal::resolve_temporal_bracket(field_id, snapshots, request)?;
    let lower_values = find_field_values(
        snapshots[bracket.lower_index],
        field_id,
        bracket.lower_index,
    )?;
    let upper_values = find_field_values(
        snapshots[bracket.upper_index],
        field_id,
        bracket.upper_index,
    )?;

    let bracket_buffers = create_temporal_bracket_buffers(
        ctx,
        lower_values,
        upper_values,
        bracket.source_timestamps.lower_epoch_seconds,
        bracket.source_timestamps.upper_epoch_seconds,
    )?;
    let output = create_temporal_output_buffer(ctx, bracket.element_count)?;
    let uniforms = create_temporal_uniform_buffer(ctx, &bracket)?;

    dispatch_temporal_blend_and_wait(ctx, &bracket_buffers, &output, &uniforms, &bracket, kernel)?;
    let values = download_temporal_output(ctx, &output).await?;
    validate_finite_output_values(&values)?;

    Ok(TemporalGpuSample {
        field_id,
        request,
        application: bracket.application,
        source_timestamps: bracket.source_timestamps,
        weights: bracket.weights,
        values,
        bracket,
    })
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

fn shader_sha256() -> String {
    sha256_hex(SHADER_SOURCE.as_bytes())
}

fn input_sha256_hex(
    field_id: FieldId,
    bracket: &TemporalBracket,
    lower_values: &[f32],
    upper_values: &[f32],
) -> Result<String, GpuTemporalError> {
    #[derive(Serialize)]
    struct NormalizedTemporalInput<'a> {
        field_id: String,
        lower_epoch_seconds: i64,
        upper_epoch_seconds: i64,
        requested_epoch_seconds: i64,
        lower_values: &'a [f32],
        upper_values: &'a [f32],
        dt1_seconds: f64,
        dt2_seconds: f64,
        inverse_span_per_second: f64,
    }
    let normalized = NormalizedTemporalInput {
        field_id: format!("{field_id:?}"),
        lower_epoch_seconds: bracket.source_timestamps.lower_epoch_seconds,
        upper_epoch_seconds: bracket.source_timestamps.upper_epoch_seconds,
        requested_epoch_seconds: bracket.request.epoch_seconds,
        lower_values,
        upper_values,
        dt1_seconds: bracket.weights.dt1_seconds,
        dt2_seconds: bracket.weights.dt2_seconds,
        inverse_span_per_second: bracket.weights.inverse_span_per_second,
    };
    let json = serde_json::to_vec(&normalized).map_err(|err| GpuTemporalError::InputHash {
        message: err.to_string(),
    })?;
    Ok(sha256_hex(&json))
}

fn pinned_oracle_evidence() -> PinnedOracleEvidence {
    PinnedOracleEvidence {
        implementation_id: TEMPORAL_ORACLE_IMPLEMENTATION_ID.to_string(),
        revision: TEMPORAL_ORACLE_REVISION.to_string(),
        executable_sha256: TEMPORAL_ORACLE_EXECUTABLE_SHA256.to_string(),
        output_sha256: TEMPORAL_ORACLE_OUTPUT_SHA256.to_string(),
    }
}

/// One machine-readable GPU-vs-oracle comparison row for #89.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalGpuRow {
    /// Canonical field identity that was sampled.
    pub field_id: FieldId,
    /// Compared element index.
    pub element_index: usize,
    /// Requested validity time in epoch seconds.
    pub requested_time_epoch_seconds: i64,
    /// Active bracketing source timestamps.
    pub source_timestamps: SourceTimestamps,
    /// Temporal application classification of the request.
    pub application: TemporalApplication,
    /// Candidate temporal weights in `DTS`-aligned form.
    pub weights: TemporalWeights,
    /// Element value computed on the GPU.
    pub gpu_value: f32,
    /// Independent oracle/expected element value.
    pub oracle_value: f32,
    /// Predeclared repository-wide absolute-or-relative comparison policy.
    pub comparison_policy: ComparisonPolicy,
    /// Absolute GPU-oracle difference.
    pub absolute_difference: f64,
    /// Whether the value comparison passes.
    pub value_verdict: Verdict,
    /// Whether the oracle `DTS` weights match.
    pub weights_verdict: Verdict,
    /// Whether the expected application classification matches.
    pub application_verdict: Verdict,
    /// Combined temporal verdict of this row.
    pub row_verdict: Verdict,
    /// Repository-wide GPU execution and numerical evidence.
    pub gpu_evidence: GpuCalculationEvidence,
}

/// Machine-readable GPU-vs-oracle comparison report for #89.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalGpuReport {
    /// Report schema identity.
    pub schema: SchemaIdentity,
    /// Scenario identifier that produced the report.
    pub scenario_id: String,
    /// Candidate description recorded for traceability.
    pub candidate: String,
    /// Canonical field identity under test.
    pub field_id: FieldId,
    /// Validated series calendar.
    pub calendar: Calendar,
    /// Full ordered series of source timestamps.
    pub source_timestamps_epoch_seconds: Vec<i64>,
    /// Predeclared repository-wide absolute-or-relative comparison policy.
    pub comparison_policy: ComparisonPolicy,
    /// Per-query comparison rows with GPU evidence.
    pub rows: Vec<TemporalGpuRow>,
    /// Overall report verdict.
    pub status: Verdict,
}

impl TemporalGpuReport {
    /// Validate structural honesty and fail closed on missing/contradictory evidence.
    ///
    /// # Errors
    /// Returns [`GpuEvidenceError`] when the schema, verdicts, tolerances or
    /// embedded [`GpuCalculationEvidence`] are missing, contradictory or
    /// claim a GPU pass from CPU replacement/skipped execution.
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        if self.schema.id != TEMPORAL_GPU_REPORT_SCHEMA_ID
            || self.schema.version != TEMPORAL_GPU_REPORT_SCHEMA_VERSION
        {
            return Err(GpuEvidenceError::UnsupportedSchema {
                id: self.schema.id.clone(),
                version: self.schema.version,
            });
        }
        if self.scenario_id != TEMPORAL_ORACLE_SCENARIO_ID
            || self.candidate != TEMPORAL_GPU_CANDIDATE_DESCRIPTION
            || self.field_id != FieldId::WindU
            || self.source_timestamps_epoch_seconds != [0, 3600]
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "temporal GPU report does not identify the pinned #71 case",
            ));
        }
        if self.rows.len() != 3 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "temporal GPU report must contain all three pinned oracle rows",
            ));
        }
        let candidate_revision = &self.rows[0].gpu_evidence.candidate.revision;
        for (row, expected_time) in self.rows.iter().zip([0, 1800, 3600]) {
            if row.field_id != self.field_id
                || row.comparison_policy != self.comparison_policy
                || row.requested_time_epoch_seconds != expected_time
                || row.element_index != 0
                || row.gpu_evidence.candidate.revision != *candidate_revision
                || !self
                    .source_timestamps_epoch_seconds
                    .contains(&row.source_timestamps.lower_epoch_seconds)
                || !self
                    .source_timestamps_epoch_seconds
                    .contains(&row.source_timestamps.upper_epoch_seconds)
            {
                return Err(GpuEvidenceError::InvalidComparisonState(
                    "temporal GPU row metadata or candidate revision contradicts its report",
                ));
            }
            row.validate()?;
        }
        let all_pass = self.rows.iter().all(|row| row.row_verdict.is_pass());
        if self.status.is_pass() != all_pass {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "temporal GPU report status contradicts row verdicts",
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
        if !self.status.is_pass() {
            return Err(GpuEvidenceError::NotPassing);
        }
        for row in &self.rows {
            row.gpu_evidence.require_paired_pass()?;
            if !row.row_verdict.is_pass() {
                return Err(GpuEvidenceError::NotPassing);
            }
        }
        Ok(())
    }
}

impl TemporalGpuRow {
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
                "embedded GPU comparison contradicts temporal row values or policy",
            ));
        }
        if !is_lowercase_git_sha(&self.gpu_evidence.candidate.revision) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "candidate revision is not a lowercase Git commit SHA",
            ));
        }
        let expected_case_id = format!(
            "temporal-gpu/wind_u:requested={}:element={}",
            self.requested_time_epoch_seconds, self.element_index
        );
        if self.gpu_evidence.case_id != expected_case_id
            || self.gpu_evidence.candidate.implementation_id != TEMPORAL_GPU_IMPLEMENTATION_ID
            || self.gpu_evidence.candidate.shader_sha256 != shader_sha256()
            || self.gpu_evidence.oracle.as_ref() != Some(&pinned_oracle_evidence())
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "embedded GPU provenance contradicts the pinned temporal calculation",
            ));
        }
        if self.value_verdict.is_pass() != (comparison.verdict == NumericalVerdict::Passed) {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "temporal GPU value verdict contradicts tolerance",
            ));
        }
        let expected_row = self.value_verdict.is_pass()
            && self.weights_verdict.is_pass()
            && self.application_verdict.is_pass();
        if self.row_verdict.is_pass() != expected_row {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "temporal GPU row verdict contradicts facet verdicts",
            ));
        }
        let expected_difference = (f64::from(self.gpu_value) - f64::from(self.oracle_value)).abs();
        if (self.absolute_difference - expected_difference).abs() > 1.0e-12 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "temporal GPU absolute difference contradicts values",
            ));
        }
        self.gpu_evidence.validate()?;
        if self.row_verdict.is_pass() {
            self.gpu_evidence.require_paired_pass()?;
        }
        Ok(())
    }
}

/// Build one GPU-vs-oracle row with #91 execution evidence.
///
/// # Errors
/// Returns [`GpuTemporalError`] for invalid tolerances or evidence
/// construction failures.
#[allow(clippy::too_many_arguments)]
fn build_temporal_gpu_row(
    ctx: &GpuContext,
    field_id: FieldId,
    element_index: usize,
    bracket: &TemporalBracket,
    gpu_value: f32,
    oracle_value: f32,
    oracle_dts: Option<temporal::OracleDts>,
    expected_application: Option<TemporalApplication>,
    comparison_policy: ComparisonPolicy,
    lower_values: &[f32],
    upper_values: &[f32],
    candidate_revision: &str,
) -> Result<TemporalGpuRow, GpuTemporalError> {
    if !gpu_value.is_finite() || !oracle_value.is_finite() {
        return Err(GpuTemporalError::Temporal(TemporalError::NonFiniteResult));
    }
    validate_candidate_revision(candidate_revision)?;
    let comparison = compare_finite_values(
        &[f64::from(oracle_value)],
        &[f64::from(gpu_value)],
        comparison_policy,
    )?;
    let value_verdict = Verdict::from_bool(comparison.verdict == NumericalVerdict::Passed);
    let weights_verdict = match oracle_dts {
        Some(oracle_dts) => {
            let weights = compare_finite_values(
                &[
                    oracle_dts.dt1_seconds,
                    oracle_dts.dt2_seconds,
                    oracle_dts.inverse_span_per_second,
                ],
                &[
                    bracket.weights.dt1_seconds,
                    bracket.weights.dt2_seconds,
                    bracket.weights.inverse_span_per_second,
                ],
                comparison_policy,
            )?;
            Verdict::from_bool(weights.verdict == NumericalVerdict::Passed)
        }
        None => Verdict::Pass,
    };
    let application_verdict = match expected_application {
        Some(expected) => Verdict::from_bool(bracket.application == expected),
        None => Verdict::Pass,
    };
    let row_verdict = Verdict::from_bool(
        value_verdict.is_pass() && weights_verdict.is_pass() && application_verdict.is_pass(),
    );

    let input_sha = input_sha256_hex(field_id, bracket, lower_values, upper_values)?;
    let case_id = format!(
        "temporal-gpu/{}:requested={}:element={}",
        match field_id {
            FieldId::WindU => "wind_u",
            FieldId::WindV => "wind_v",
            FieldId::VerticalVelocity => "vertical_velocity",
            FieldId::Temperature => "temperature",
            _ => "field",
        },
        bracket.request.epoch_seconds,
        element_index
    );
    let gpu_evidence = GpuCalculationEvidence {
        schema: GpuEvidenceSchema::default(),
        case_id,
        candidate: GpuCandidateEvidence {
            implementation_id: TEMPORAL_GPU_IMPLEMENTATION_ID.to_string(),
            revision: candidate_revision.to_string(),
            shader_sha256: shader_sha256(),
            input_sha256: input_sha,
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(GpuAdapterEvidence::from_context(ctx)),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(pinned_oracle_evidence()),
        comparison,
    };
    gpu_evidence.validate()?;

    Ok(TemporalGpuRow {
        field_id,
        element_index,
        requested_time_epoch_seconds: bracket.request.epoch_seconds,
        source_timestamps: bracket.source_timestamps,
        application: bracket.application,
        weights: bracket.weights,
        gpu_value,
        oracle_value,
        comparison_policy,
        absolute_difference: (f64::from(gpu_value) - f64::from(oracle_value)).abs(),
        value_verdict,
        weights_verdict,
        application_verdict,
        row_verdict,
        gpu_evidence,
    })
}

fn validate_pinned_oracle_contract(
    scenario_id: &str,
    field_id: FieldId,
    snapshots: &[&Snapshot],
    queries: &[temporal::OracleQuery],
) -> Result<(), GpuTemporalError> {
    if scenario_id != TEMPORAL_ORACLE_SCENARIO_ID || field_id != FieldId::WindU {
        return Err(GpuTemporalError::OracleContract {
            message: "scenario and field must identify the temporal-bilinear WindU fixture",
        });
    }
    if snapshots.len() != 2 {
        return Err(GpuTemporalError::OracleContract {
            message: "the pinned fixture contains exactly two source snapshots",
        });
    }
    let lower = find_field_values(snapshots[0], field_id, 0)?;
    let upper = find_field_values(snapshots[1], field_id, 1)?;
    let lower_time = snapshots[0]
        .fields
        .iter()
        .find(|field| field.id == field_id)
        .map(|field| field.time.valid_time_epoch_seconds);
    let upper_time = snapshots[1]
        .fields
        .iter()
        .find(|field| field.id == field_id)
        .map(|field| field.time.valid_time_epoch_seconds);
    if lower_time != Some(0)
        || upper_time != Some(3600)
        || lower != [10.0_f32]
        || upper != [20.0_f32]
    {
        return Err(GpuTemporalError::OracleContract {
            message: "source timestamps and values must match the pinned fixture inputs",
        });
    }

    let expected = [
        (
            0,
            10.0_f32,
            temporal::OracleDts {
                dt1_seconds: 0.0,
                dt2_seconds: 3600.0,
                inverse_span_per_second: 0.000_277_777_784_503_996_37,
            },
            TemporalApplication::FirstEndpoint,
        ),
        (
            1800,
            15.0_f32,
            temporal::OracleDts {
                dt1_seconds: 1800.0,
                dt2_seconds: 1800.0,
                inverse_span_per_second: 0.000_277_777_784_503_996_37,
            },
            TemporalApplication::LinearInterior,
        ),
        (
            3600,
            20.0_f32,
            temporal::OracleDts {
                dt1_seconds: 3600.0,
                dt2_seconds: 0.0,
                inverse_span_per_second: 0.000_277_777_784_503_996_37,
            },
            TemporalApplication::LastEndpoint,
        ),
    ];
    if queries.len() != expected.len()
        || queries.iter().zip(expected).any(|(query, expected)| {
            query.requested_time_epoch_seconds != expected.0
                || query.element_index != 0
                || query.oracle_value.to_bits() != expected.1.to_bits()
                || query.oracle_dts != Some(expected.2)
                || query.expected_application != Some(expected.3)
        })
    {
        return Err(GpuTemporalError::OracleContract {
            message: "queries must exactly match all pinned temporal-bilinear oracle rows",
        });
    }
    Ok(())
}

/// Build a machine-readable GPU-vs-oracle report for one scenario.
///
/// # Errors
/// Returns [`GpuTemporalError`] for empty queries, out-of-range elements,
/// invalid candidate provenance, validation failures or GPU/evidence failures.
pub async fn build_temporal_gpu_report(
    ctx: &GpuContext,
    scenario_id: &str,
    candidate_revision: &str,
    field_id: FieldId,
    snapshots: &[&Snapshot],
    comparison_policy: ComparisonPolicy,
    queries: &[temporal::OracleQuery],
    kernel: &TemporalInterpolationKernel,
) -> Result<TemporalGpuReport, GpuTemporalError> {
    validate_candidate_revision(candidate_revision)?;
    if queries.is_empty() {
        return Err(GpuTemporalError::Temporal(
            TemporalError::EmptyOracleQueries,
        ));
    }
    if scenario_id.trim().is_empty() {
        return Err(GpuTemporalError::InputHash {
            message: "scenario_id must not be empty".to_string(),
        });
    }
    validate_pinned_oracle_contract(scenario_id, field_id, snapshots, queries)?;
    let first_calendar = snapshots
        .first()
        .and_then(|snapshot| {
            snapshot
                .fields
                .iter()
                .find(|field| field.id == field_id)
                .map(|field| field.time.calendar)
        })
        .ok_or(TemporalError::MissingFieldSnapshot {
            snapshot_index: 0,
            field_id,
        })
        .map_err(GpuTemporalError::Temporal)?;
    let first_bracket = temporal::resolve_temporal_bracket(
        field_id,
        snapshots,
        RequestedSampleTime::new(first_calendar, queries[0].requested_time_epoch_seconds),
    )?;
    let calendar = first_bracket.calendar;
    let source_timestamps_epoch_seconds = first_bracket.series_timestamps_epoch_seconds.clone();

    let mut rows = Vec::with_capacity(queries.len());
    for query in queries {
        let request = RequestedSampleTime::new(calendar, query.requested_time_epoch_seconds);
        let bracket = temporal::resolve_temporal_bracket(field_id, snapshots, request)?;
        let sample = sample_field_gpu(ctx, field_id, snapshots, request, kernel).await?;
        if query.element_index >= sample.values.len() {
            return Err(GpuTemporalError::Temporal(
                TemporalError::OutOfBoundsElement {
                    element_index: query.element_index,
                    length: sample.values.len(),
                },
            ));
        }
        let lower_values = find_field_values(
            snapshots[bracket.lower_index],
            field_id,
            bracket.lower_index,
        )?;
        let upper_values = find_field_values(
            snapshots[bracket.upper_index],
            field_id,
            bracket.upper_index,
        )?;
        rows.push(build_temporal_gpu_row(
            ctx,
            field_id,
            query.element_index,
            &bracket,
            sample.values[query.element_index],
            query.oracle_value,
            query.oracle_dts,
            query.expected_application,
            comparison_policy,
            lower_values,
            upper_values,
            candidate_revision,
        )?);
    }

    let status = Verdict::from_bool(rows.iter().all(|row| row.row_verdict.is_pass()));
    let report = TemporalGpuReport {
        schema: SchemaIdentity {
            id: TEMPORAL_GPU_REPORT_SCHEMA_ID.to_string(),
            version: TEMPORAL_GPU_REPORT_SCHEMA_VERSION,
        },
        scenario_id: scenario_id.to_string(),
        candidate: TEMPORAL_GPU_CANDIDATE_DESCRIPTION.to_string(),
        field_id,
        calendar,
        source_timestamps_epoch_seconds,
        comparison_policy,
        rows,
        status,
    };
    report.validate()?;
    Ok(report)
}

/// Default #74 comparison tolerance as a #91 comparison policy.
///
/// # Errors
/// Returns [`GpuTemporalError::Evidence`] for invalid tolerance values.
pub fn default_comparison_policy() -> Result<ComparisonPolicy, GpuTemporalError> {
    Ok(ComparisonPolicy::new(DEFAULT_ABS_TOL, DEFAULT_REL_TOL)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_params_reject_empty_bracket() {
        let bracket = TemporalBracket {
            field_id: FieldId::WindU,
            request: RequestedSampleTime::new(Calendar::Gregorian, 0),
            application: TemporalApplication::FirstEndpoint,
            source_timestamps: SourceTimestamps {
                lower_epoch_seconds: 0,
                upper_epoch_seconds: 3600,
            },
            weights: TemporalWeights {
                dt1_seconds: 0.0,
                dt2_seconds: 3600.0,
                inverse_span_per_second: 1.0 / 3600.0,
            },
            lower_index: 0,
            upper_index: 1,
            calendar: Calendar::Gregorian,
            element_count: 0,
            series_timestamps_epoch_seconds: vec![0, 3600],
        };
        assert!(matches!(
            TemporalBlendParamsRaw::from_bracket(&bracket),
            Err(GpuTemporalError::EmptyField)
        ));
    }

    #[test]
    fn shader_source_hash_is_stable_hex() {
        let hash = shader_sha256();
        assert_eq!(hash.len(), 64);
        assert!(hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }
}
