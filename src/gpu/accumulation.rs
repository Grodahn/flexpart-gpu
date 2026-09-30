//! GPU accumulated-field interval/reset transformation (issue #90).
//!
//! This module ports the canonical CPU semantics from
//! [`crate::meteorology::accumulation`] (issue #75) to actual GPU execution.
//! It is a separate accumulated-field preprocessing/resource branch, not
//! generic temporal interpolation and not a fourth ordinary interpolation
//! stage:
//!
//! ```text
//! accumulated source observations
//! -> interval/reset interpretation (host, #75)
//! -> interval amount/rate resources (WGSL, this module)
//! -> later sampling/composition owned by #76
//! ```
//!
//! FLEXPART `interpol_rain` (`interpol_mod.f90:1209-1582`) consumes
//! `lsprec`/`convprec` as already-normalized mm/h rates. Deaccumulation is
//! owned here, upstream of that sampling boundary.
//!
//! ## Host versus GPU responsibilities
//!
//! Host (Rust) owns metadata validation and interval-window derivation exactly
//! as defined by #75:
//!
//! - reject empty sequences, non-increasing valid times, backwards resets,
//!   resets not before valid time, mid-window resets, uncovered gaps, negative
//!   or non-finite source amounts, and same-run negative deltas;
//! - derive the canonical interval window `[start, end]`, its strictly
//!   positive duration, and the `reset_applied` flag;
//! - convert validated `f64` amounts and durations to `f32` for device upload,
//!   failing closed on non-finite conversion.
//!
//! The host never computes the production interval amount or rate. Supported
//! numerical transformation of canonical valid inputs executes in WGSL:
//!
//! ```text
//! amount  = reset_applied ? curr : (curr - prev)   [kg/m2]
//! rate_si = amount / duration_seconds               [kg/(m2 s)]
//! rate_mmh = rate_si * 3600.0                       [mm/h]
//! ```
//!
//! ## Composition contract
//!
//! - Reuses the existing [`crate::gpu::GpuContext`].
//! - Follows existing persistent GPU resource patterns: source inputs and
//!   derived amount/rate outputs are device-resident buffers with
//!   bracket/interval lifetime.
//! - Exposes `encode_*` into a caller-owned `wgpu::CommandEncoder` without
//!   submitting, waiting, or reading back, suitable for later #76 composition.
//! - Keeps transformed amount/rate resources device-resident when consumed by
//!   later GPU stages. No mandatory `GPU -> CPU -> GPU` round trip exists in
//!   the production composition path.
//! - Convenience `dispatch_*`/readback helpers submit, wait, and stage through
//!   `MAP_READ` only for isolated oracle validation. They must not be inserted
//!   between GPU-capable production stages.
//! - No silent CPU fallback exists for supported production inputs. A failed
//!   GPU path surfaces as an explicit error.
//!
//! Amount and rate remain distinct outputs throughout. Units remain explicit
//! and unchanged from #75. Source provenance and reset metadata remain
//! authoritative. No provider-specific reset cadence is assumed.

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::meteorology::accumulation::{
    validate_interval_sequence, AccumulatedObservation, AccumulationError, RATE_HANDOFF_UNIT,
    RATE_SI_UNIT, RELATIVE_TOLERANCE, SOURCE_UNIT,
};

use super::{
    download_buffer_typed, ComparisonPolicy, GpuAdapterEvidence, GpuBufferError,
    GpuCalculationEvidence, GpuCalculationPath, GpuCandidateEvidence, GpuContext, GpuEvidenceError,
    GpuExecutionEvidence, GpuExecutionStatus, NumericalVerdict, PinnedOracleEvidence,
};

/// WGSL source for the accumulated-field interval/reset transformation.
const SHADER_SOURCE: &str = include_str!("../shaders/accumulated_interval.wgsl");

/// Workgroup width for the accumulation kernel.
///
/// A fixed width mirrors `gpu::interpolation` and avoids a shared-infrastructure
/// change to `workgroup.rs` while #89 proceeds concurrently.
const WORKGROUP_SIZE_X: u32 = 64;

/// Candidate implementation identity for machine-readable evidence.
pub const ACCUMULATED_GPU_IMPLEMENTATION_ID: &str = "accumulation-interval-reset-gpu-90";

/// Pinned FLEXPART revision owning the #71 interpolation oracle.
pub const PINNED_FLEXPART_REVISION: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";

/// Stable identity of the pinned #71/#75 FLEXPART oracle path.
pub const PINNED_ORACLE_IMPLEMENTATION_ID: &str =
    "FLEXPART-11.1-interpol_rain-canonical-accumulation-71-75";

/// Pinned oracle executable identity (linked object set from #71 provenance).
pub const PINNED_ORACLE_EXECUTABLE_SHA256: &str =
    "388d1f824306df30fc74fbedc6464e86602b6a93ba782e9e1dad20ffacc3327c";

/// Pinned canonical accumulation fixture digest (issues #71/#75 oracle chain).
pub const PINNED_ACCUMULATION_FIXTURE_SHA256: &str =
    "4bf7d901be71210dea9a072069d8551318455b4fe9bace8bcb45b04729517ef1";

/// Relative tolerance inherited from #75 (`1e-6`).
pub const ACCUMULATION_GPU_RELATIVE_TOLERANCE: f64 = RELATIVE_TOLERANCE;

/// Absolute tolerance for GPU `f32` comparison.
///
/// The #75 contract defines only a relative tolerance. A small absolute band
/// handles exact-zero and near-zero interval amounts without weakening the
/// relative requirement for physical precipitation values.
pub const ACCUMULATION_GPU_ABSOLUTE_TOLERANCE: f64 = 1.0e-6;

/// Schema identity for issue-specific accumulated-GPU evidence.
pub const ACCUMULATED_GPU_EVIDENCE_SCHEMA_ID: &str = "flexpart-gpu.accumulation-gpu-evidence";

/// Version of the issue-specific accumulated-GPU evidence schema.
pub const ACCUMULATED_GPU_EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// One pre-validated per-interval transform element uploaded to the GPU.
///
/// `curr_amount_kg_per_square_meter` is the current source total,
/// `prev_amount_kg_per_square_meter` is the previous source total in the same
/// run (zeroed when `reset_applied` is set), `duration_seconds` is the
/// host-validated strictly positive interval length, and `reset_applied` is
/// `1` for a fresh-run amount versus `0` for a within-run delta.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct AccumulatedTransformInput {
    /// Current source accumulated total in kg/m2.
    pub curr_amount_kg_per_square_meter: f32,
    /// Previous source total in the same run in kg/m2.
    pub prev_amount_kg_per_square_meter: f32,
    /// Host-validated interval duration in seconds, strictly positive.
    pub duration_seconds: f32,
    /// `1` when the interval amount is the fresh-run value, else `0`.
    pub reset_applied: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct AccumulatedTransformParamsRaw {
    interval_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// Device-resident source inputs for the accumulation transformation.
///
/// Lifetime: represented interval/bracket lifetime. The buffer is uploaded once
/// when the source observations change, not once per particle query.
pub struct AccumulatedTransformInputs {
    /// Packed per-interval inputs in observation order.
    pub buffer: wgpu::Buffer,
    interval_count: usize,
}

impl AccumulatedTransformInputs {
    /// Validate `observations` with the canonical #75 semantics and upload the
    /// resulting per-interval inputs.
    ///
    /// The canonical interval amount/rate values are not computed here. Only
    /// the control plane (`curr`, `prev`, `duration`, `reset_applied`) is
    /// uploaded. The WGSL kernel computes all production amounts and rates.
    ///
    /// # Errors
    ///
    /// Returns [`GpuAccumulationError`] for empty input, #75 validation
    /// failure, non-finite `f32` conversion, oversized counts, or buffer
    /// allocation failure.
    pub fn from_observations(
        ctx: &GpuContext,
        observations: &[AccumulatedObservation],
    ) -> Result<Self, GpuAccumulationError> {
        let inputs = build_transform_inputs(observations)?;
        Self::from_inputs(ctx, &inputs)
    }

    /// Upload pre-validated transform inputs.
    ///
    /// # Errors
    ///
    /// Returns [`GpuAccumulationError`] for empty input or oversized counts.
    pub fn from_inputs(
        ctx: &GpuContext,
        inputs: &[AccumulatedTransformInput],
    ) -> Result<Self, GpuAccumulationError> {
        if inputs.is_empty() {
            return Err(GpuAccumulationError::EmptySequence);
        }
        validate_transform_inputs(inputs)?;
        let interval_count = inputs.len();
        let _count_u32 =
            u32::try_from(interval_count).map_err(|_| GpuAccumulationError::ValueTooLarge {
                field: "interval_count",
                value: interval_count,
            })?;
        let byte_len = interval_count
            .checked_mul(size_of::<AccumulatedTransformInput>())
            .ok_or(GpuAccumulationError::SizeOverflow {
                field: "accumulated_transform_inputs",
            })?;
        let _byte_len_u64 =
            u64::try_from(byte_len).map_err(|_| GpuAccumulationError::SizeOverflow {
                field: "accumulated_transform_inputs",
            })?;
        push_device_error_scopes(ctx);
        let buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("accumulated_transform_inputs"),
                contents: bytemuck::cast_slice(inputs),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            });
        finish_device_error_scopes(ctx, "input buffer creation")?;
        Ok(Self {
            buffer,
            interval_count,
        })
    }

    /// Number of intervals represented by this resource.
    #[must_use]
    pub const fn interval_count(&self) -> usize {
        self.interval_count
    }
}

/// Device-resident derived interval amount/rate resources.
///
/// Amount and rate remain structurally distinct buffers. Lifetime: the
/// represented interval lifetime, reusable across queries for that interval.
/// Suitable for direct consumption by later #76 composition without host
/// materialization.
pub struct AccumulatedIntervalBuffers {
    /// Interval-integrated amounts in kg/m2.
    pub amount_kg_per_square_meter: wgpu::Buffer,
    /// Interval rates in kg/(m2 s).
    pub rate_si: wgpu::Buffer,
    /// Handoff rates in mm/h.
    pub rate_millimeter_per_hour: wgpu::Buffer,
    interval_count: usize,
}

impl AccumulatedIntervalBuffers {
    /// Allocate device-resident output buffers for `interval_count` intervals.
    ///
    /// # Errors
    ///
    /// Returns [`GpuAccumulationError`] for zero counts, oversized counts, or
    /// byte-size overflow.
    pub fn new(ctx: &GpuContext, interval_count: usize) -> Result<Self, GpuAccumulationError> {
        if interval_count == 0 {
            return Err(GpuAccumulationError::EmptySequence);
        }
        let _count_u32 =
            u32::try_from(interval_count).map_err(|_| GpuAccumulationError::ValueTooLarge {
                field: "interval_count",
                value: interval_count,
            })?;
        let byte_len = interval_count.checked_mul(size_of::<f32>()).ok_or(
            GpuAccumulationError::SizeOverflow {
                field: "accumulated_interval_outputs",
            },
        )?;
        let size = u64::try_from(byte_len).map_err(|_| GpuAccumulationError::SizeOverflow {
            field: "accumulated_interval_outputs",
        })?;
        let usage = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC;
        push_device_error_scopes(ctx);
        let amount_kg_per_square_meter = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accumulated_interval_amounts"),
            size,
            usage,
            mapped_at_creation: false,
        });
        let rate_si = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accumulated_interval_rates_si"),
            size,
            usage,
            mapped_at_creation: false,
        });
        let rate_millimeter_per_hour = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accumulated_interval_rates_mmh"),
            size,
            usage,
            mapped_at_creation: false,
        });
        finish_device_error_scopes(ctx, "output buffer creation")?;
        Ok(Self {
            amount_kg_per_square_meter,
            rate_si,
            rate_millimeter_per_hour,
            interval_count,
        })
    }

    /// Number of intervals represented by this resource.
    #[must_use]
    pub const fn interval_count(&self) -> usize {
        self.interval_count
    }

    /// Download derived amounts (explicit validation-boundary readback).
    ///
    /// # Errors
    ///
    /// Forwards [`GpuBufferError`] from staging and mapping.
    pub async fn download_amounts(&self, ctx: &GpuContext) -> Result<Vec<f32>, GpuBufferError> {
        download_buffer_typed(
            ctx,
            &self.amount_kg_per_square_meter,
            self.interval_count,
            "accumulated_interval_amounts",
        )
        .await
    }

    /// Download derived SI rates (explicit validation-boundary readback).
    ///
    /// # Errors
    ///
    /// Forwards [`GpuBufferError`] from staging and mapping.
    pub async fn download_rates_si(&self, ctx: &GpuContext) -> Result<Vec<f32>, GpuBufferError> {
        download_buffer_typed(
            ctx,
            &self.rate_si,
            self.interval_count,
            "accumulated_interval_rates_si",
        )
        .await
    }

    /// Download derived mm/h rates (explicit validation-boundary readback).
    ///
    /// # Errors
    ///
    /// Forwards [`GpuBufferError`] from staging and mapping.
    pub async fn download_rates_millimeter_per_hour(
        &self,
        ctx: &GpuContext,
    ) -> Result<Vec<f32>, GpuBufferError> {
        download_buffer_typed(
            ctx,
            &self.rate_millimeter_per_hour,
            self.interval_count,
            "accumulated_interval_rates_mmh",
        )
        .await
    }
}

/// Reusable WGSL kernel for the accumulation transformation.
pub struct AccumulatedIntervalKernel {
    /// Bind-group layout for transform inputs, three outputs, and uniforms.
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Compiled compute pipeline executing the WGSL arithmetic.
    pub pipeline: wgpu::ComputePipeline,
    workgroup_size_x: u32,
}

impl AccumulatedIntervalKernel {
    /// Compile the WGSL transformation kernel and surface device validation
    /// failures instead of allowing deferred errors to masquerade as success.
    ///
    /// # Errors
    ///
    /// Returns [`GpuAccumulationError::Device`] when `wgpu` reports a shader,
    /// pipeline, out-of-memory, or internal device error.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuAccumulationError> {
        push_device_error_scopes(ctx);
        let shader = ctx.load_shader("accumulated_interval_shader", SHADER_SOURCE);
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("accumulated_interval_bgl"),
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
        let pipeline = ctx.create_compute_pipeline(
            "accumulated_interval_pipeline",
            &shader,
            "main",
            &[&bind_group_layout],
        );
        let kernel = Self {
            bind_group_layout,
            pipeline,
            workgroup_size_x: WORKGROUP_SIZE_X,
        };
        finish_device_error_scopes(ctx, "kernel creation")?;
        Ok(kernel)
    }
}

/// Errors for GPU accumulated-field transformation.
#[derive(Debug, Error)]
pub enum GpuAccumulationError {
    /// No intervals were supplied.
    #[error("accumulation GPU sequence is empty")]
    EmptySequence,
    /// Canonical #75 validation rejected the source sequence.
    #[error("canonical accumulation validation failed: {0}")]
    Validation(#[from] AccumulationError),
    /// A validated value is not finite in `f32` device representation.
    #[error("validated accumulation value for {field} is not finite as f32")]
    NonFiniteDeviceValue {
        /// Name of the offending input lane.
        field: &'static str,
    },
    /// A validated interval duration is not strictly positive as `f32`.
    #[error("validated interval duration is not positive as f32")]
    NonPositiveDuration,
    /// A supposedly pre-validated raw transform input violates the kernel's
    /// finite, sign, reset-flag, or monotonicity contract.
    #[error("invalid transform input {index}: {reason}")]
    InvalidTransformInput {
        /// Index of the offending transform input.
        index: usize,
        /// Stable description of the violated invariant.
        reason: &'static str,
    },
    /// Input and output resources describe different interval counts.
    #[error("interval count mismatch for {field}: inputs {inputs}, outputs {outputs}")]
    CountMismatch {
        /// Resource that mismatched.
        field: &'static str,
        /// Input interval count.
        inputs: usize,
        /// Output interval count.
        outputs: usize,
    },
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
    /// Buffer transfer or readback failed.
    #[error("buffer operation failed: {0}")]
    Buffer(#[from] GpuBufferError),
    /// Machine-readable GPU evidence could not be constructed or validated.
    #[error("accumulation GPU evidence failed: {0}")]
    Evidence(#[from] GpuEvidenceError),
    /// The WebGPU device reported an explicit scoped error.
    #[error("accumulation GPU {stage} failed: {message}")]
    Device {
        /// Operation during which the device error was observed.
        stage: &'static str,
        /// Backend-provided error detail.
        message: String,
    },
}

fn push_device_error_scopes(ctx: &GpuContext) {
    ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
    ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
}

fn finish_device_error_scopes(
    ctx: &GpuContext,
    stage: &'static str,
) -> Result<(), GpuAccumulationError> {
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    let validation = pollster::block_on(ctx.device.pop_error_scope());
    let out_of_memory = pollster::block_on(ctx.device.pop_error_scope());
    let internal = pollster::block_on(ctx.device.pop_error_scope());
    if let Some(error) = validation.or(out_of_memory).or(internal) {
        return Err(GpuAccumulationError::Device {
            stage,
            message: error.to_string(),
        });
    }
    Ok(())
}

/// Build validated per-interval GPU inputs from canonical observations.
///
/// This function performs host-side metadata validation with the exact #75
/// semantics and derives the control plane only. It does not compute interval
/// amounts or rates; those are computed by the WGSL kernel.
///
/// # Errors
///
/// Returns [`AccumulationError`] when the source sequence violates the frozen
/// #75 fail-closed rules, or [`GpuAccumulationError`] for empty input and
/// non-finite `f32` conversion.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn build_transform_inputs(
    observations: &[AccumulatedObservation],
) -> Result<Vec<AccumulatedTransformInput>, GpuAccumulationError> {
    if observations.is_empty() {
        return Err(GpuAccumulationError::EmptySequence);
    }
    let metadata = validate_interval_sequence(observations)?;
    let mut inputs = Vec::with_capacity(observations.len());
    for (index, observation) in observations.iter().enumerate() {
        let interval = metadata[index];
        let curr = observation.accumulated_amount_kg_per_square_meter;
        let prev = if interval.reset_applied {
            0.0
        } else {
            observations[index - 1].accumulated_amount_kg_per_square_meter
        };
        let duration_f64 = interval.duration_seconds as f64;
        let curr_f32 = curr as f32;
        let prev_f32 = prev as f32;
        let duration_f32 = duration_f64 as f32;
        if !curr_f32.is_finite() {
            return Err(GpuAccumulationError::NonFiniteDeviceValue {
                field: "curr_amount_kg_per_square_meter",
            });
        }
        if !prev_f32.is_finite() {
            return Err(GpuAccumulationError::NonFiniteDeviceValue {
                field: "prev_amount_kg_per_square_meter",
            });
        }
        if !duration_f32.is_finite() || duration_f32 <= 0.0 {
            return Err(GpuAccumulationError::NonPositiveDuration);
        }
        inputs.push(AccumulatedTransformInput {
            curr_amount_kg_per_square_meter: curr_f32,
            prev_amount_kg_per_square_meter: prev_f32,
            duration_seconds: duration_f32,
            reset_applied: u32::from(interval.reset_applied),
        });
    }
    validate_transform_inputs(&inputs)?;
    Ok(inputs)
}

fn validate_transform_inputs(
    inputs: &[AccumulatedTransformInput],
) -> Result<(), GpuAccumulationError> {
    for (index, input) in inputs.iter().enumerate() {
        if !input.curr_amount_kg_per_square_meter.is_finite()
            || !input.prev_amount_kg_per_square_meter.is_finite()
        {
            return Err(GpuAccumulationError::InvalidTransformInput {
                index,
                reason: "source amounts must be finite",
            });
        }
        if input.curr_amount_kg_per_square_meter < 0.0
            || input.prev_amount_kg_per_square_meter < 0.0
        {
            return Err(GpuAccumulationError::InvalidTransformInput {
                index,
                reason: "source amounts must be non-negative",
            });
        }
        if !input.duration_seconds.is_finite() || input.duration_seconds <= 0.0 {
            return Err(GpuAccumulationError::InvalidTransformInput {
                index,
                reason: "duration must be finite and strictly positive",
            });
        }
        if input.reset_applied > 1 {
            return Err(GpuAccumulationError::InvalidTransformInput {
                index,
                reason: "reset_applied must be 0 or 1",
            });
        }
        if input.reset_applied == 0 {
            if input.curr_amount_kg_per_square_meter < input.prev_amount_kg_per_square_meter {
                return Err(GpuAccumulationError::InvalidTransformInput {
                    index,
                    reason: "same-run accumulated amount must not decrease",
                });
            }
        } else if input.prev_amount_kg_per_square_meter != 0.0 {
            return Err(GpuAccumulationError::InvalidTransformInput {
                index,
                reason: "fresh-run inputs must zero the ignored previous amount",
            });
        }
    }
    Ok(())
}

/// Encode the accumulation transformation into a caller-owned encoder.
///
/// This is the production composition path for later #76 use. It submits
/// nothing, waits for nothing, and reads nothing back. The caller owns
/// submission and lifetime: `inputs`, `outputs`, and `kernel` must live until
/// the encoded work completes.
///
/// # Errors
///
/// Returns [`GpuAccumulationError::CountMismatch`] when input and output
/// resources disagree, or sizing errors for oversized counts.
pub fn encode_accumulated_intervals_gpu_with_kernel(
    ctx: &GpuContext,
    inputs: &AccumulatedTransformInputs,
    outputs: &AccumulatedIntervalBuffers,
    kernel: &AccumulatedIntervalKernel,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), GpuAccumulationError> {
    if inputs.interval_count() != outputs.interval_count() {
        return Err(GpuAccumulationError::CountMismatch {
            field: "accumulated_intervals",
            inputs: inputs.interval_count(),
            outputs: outputs.interval_count(),
        });
    }
    if inputs.interval_count() == 0 {
        return Err(GpuAccumulationError::EmptySequence);
    }
    let interval_count_u32 = u32::try_from(inputs.interval_count()).map_err(|_| {
        GpuAccumulationError::ValueTooLarge {
            field: "interval_count",
            value: inputs.interval_count(),
        }
    })?;
    debug_assert_eq!(
        size_of::<AccumulatedTransformParamsRaw>() % 16,
        0,
        "accumulation uniform params must stay 16-byte aligned"
    );
    let raw_params = AccumulatedTransformParamsRaw {
        interval_count: interval_count_u32,
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
    };
    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("accumulated_interval_params"),
            contents: bytemuck::bytes_of(&raw_params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("accumulated_interval_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: inputs.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: outputs.amount_kg_per_square_meter.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: outputs.rate_si.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: outputs.rate_millimeter_per_hour.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });
    {
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("accumulated_interval_pass"),
            timestamp_writes: None,
        });
        compute_pass.set_pipeline(&kernel.pipeline);
        compute_pass.set_bind_group(0, &bind_group, &[]);
        super::dispatch_1d(
            &mut compute_pass,
            interval_count_u32,
            kernel.workgroup_size_x,
        );
    }
    Ok(())
}

/// Dispatch the accumulation transformation with an isolated encoder.
///
/// Convenience path for isolated oracle validation only. It creates an
/// encoder, encodes the WGSL work, submits, and waits. Production composition
/// must use [`encode_accumulated_intervals_gpu_with_kernel`] instead so
/// dependent GPU stages can share one submission without host synchronization.
///
/// # Errors
///
/// Forwards [`GpuAccumulationError`] from encoding.
pub fn dispatch_accumulated_intervals_gpu_with_kernel(
    ctx: &GpuContext,
    inputs: &AccumulatedTransformInputs,
    outputs: &AccumulatedIntervalBuffers,
    kernel: &AccumulatedIntervalKernel,
) -> Result<(), GpuAccumulationError> {
    push_device_error_scopes(ctx);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("accumulated_interval_encoder"),
        });
    if let Err(error) =
        encode_accumulated_intervals_gpu_with_kernel(ctx, inputs, outputs, kernel, &mut encoder)
    {
        let _ = finish_device_error_scopes(ctx, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    finish_device_error_scopes(ctx, "dispatch")
}

/// Dispatch the accumulation transformation with a transient kernel.
///
/// Convenience path for isolated oracle validation only. See
/// [`dispatch_accumulated_intervals_gpu_with_kernel`].
///
/// # Errors
///
/// Forwards [`GpuAccumulationError`] from encoding.
pub fn dispatch_accumulated_intervals_gpu(
    ctx: &GpuContext,
    inputs: &AccumulatedTransformInputs,
    outputs: &AccumulatedIntervalBuffers,
) -> Result<(), GpuAccumulationError> {
    let kernel = AccumulatedIntervalKernel::new(ctx)?;
    dispatch_accumulated_intervals_gpu_with_kernel(ctx, inputs, outputs, &kernel)
}

/// Transform one validated observation sequence on the GPU and read back.
///
/// Convenience helper for isolated oracle validation only. Production code
/// must keep amount/rate resources device-resident through `encode_*`
/// composition instead of reading them back here.
///
/// The returned amounts are in kg/m2, SI rates in kg/(m2 s), and handoff rates
/// in mm/h, in observation order.
///
/// # Errors
///
/// Returns [`GpuAccumulationError`] for #75 validation failure or dispatch
/// failure, or [`GpuBufferError`] for readback failure. No CPU fallback is
/// performed for supported inputs.
pub async fn transform_accumulated_intervals_gpu(
    ctx: &GpuContext,
    observations: &[AccumulatedObservation],
) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>), GpuAccumulationError> {
    let readback = execute_accumulated_intervals_gpu(ctx, observations).await?;
    Ok((readback.amounts, readback.rates_si, readback.rates_mmh))
}

struct AccumulatedGpuReadback {
    transform_inputs: Vec<AccumulatedTransformInput>,
    amounts: Vec<f32>,
    rates_si: Vec<f32>,
    rates_mmh: Vec<f32>,
    adapter: GpuAdapterEvidence,
}

async fn execute_accumulated_intervals_gpu(
    ctx: &GpuContext,
    observations: &[AccumulatedObservation],
) -> Result<AccumulatedGpuReadback, GpuAccumulationError> {
    let transform_inputs = build_transform_inputs(observations)?;
    let inputs = AccumulatedTransformInputs::from_inputs(ctx, &transform_inputs)?;
    let outputs = AccumulatedIntervalBuffers::new(ctx, inputs.interval_count())?;
    let kernel = AccumulatedIntervalKernel::new(ctx)?;
    dispatch_accumulated_intervals_gpu_with_kernel(ctx, &inputs, &outputs, &kernel)?;
    let amounts = outputs.download_amounts(ctx).await?;
    let rates_si = outputs.download_rates_si(ctx).await?;
    let rates_mmh = outputs.download_rates_millimeter_per_hour(ctx).await?;
    Ok(AccumulatedGpuReadback {
        transform_inputs,
        amounts,
        rates_si,
        rates_mmh,
        adapter: GpuAdapterEvidence::from_context(ctx),
    })
}

/// Lowercase hexadecimal SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// SHA-256 of the executed WGSL bundle.
#[must_use]
pub fn accumulated_shader_sha256() -> String {
    sha256_hex(SHADER_SOURCE.as_bytes())
}

/// SHA-256 of the canonical little-endian encoding of validated GPU inputs.
#[must_use]
pub fn accumulated_inputs_sha256(inputs: &[AccumulatedTransformInput]) -> String {
    let mut normalized = Vec::with_capacity(std::mem::size_of_val(inputs));
    for entry in inputs {
        normalized.extend_from_slice(&entry.curr_amount_kg_per_square_meter.to_le_bytes());
        normalized.extend_from_slice(&entry.prev_amount_kg_per_square_meter.to_le_bytes());
        normalized.extend_from_slice(&entry.duration_seconds.to_le_bytes());
        normalized.extend_from_slice(&entry.reset_applied.to_le_bytes());
    }
    sha256_hex(&normalized)
}

/// Candidate revision for machine-readable evidence.
#[must_use]
pub fn accumulated_candidate_revision() -> String {
    ["FLEXPART_GPU_CANDIDATE_REVISION", "GITHUB_SHA"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .filter(|revision| {
            revision.len() == 40
                && revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
        .unwrap_or_else(|| format!("local-uncommitted-{}", env!("CARGO_PKG_VERSION")))
}

/// Build the repository-wide comparison policy owned by #90.
///
/// The relative tolerance is inherited from #75. The absolute tolerance covers
/// exact-zero handling for GPU `f32` without weakening the relative rule.
///
/// # Errors
///
/// Returns [`GpuEvidenceError`] for an invalid tolerance policy.
pub fn accumulated_comparison_policy() -> Result<ComparisonPolicy, GpuEvidenceError> {
    ComparisonPolicy::new(
        ACCUMULATION_GPU_ABSOLUTE_TOLERANCE,
        ACCUMULATION_GPU_RELATIVE_TOLERANCE,
    )
}

/// Build a repository-wide [`GpuCalculationEvidence`] record for one value lane.
///
/// `oracle_values` are the canonical expected values, `candidate_values` are
/// the GPU-derived values, both in the same units. The caller supplies the
/// stable `case_id`.
///
/// # Errors
///
/// Returns [`GpuEvidenceError`] for an invalid tolerance policy.
fn build_accumulation_gpu_calculation_evidence(
    case_id: &str,
    adapter: &GpuAdapterEvidence,
    inputs: &[AccumulatedTransformInput],
    oracle_values: &[f64],
    candidate_values: &[f64],
) -> Result<GpuCalculationEvidence, GpuEvidenceError> {
    let policy = accumulated_comparison_policy()?;
    let comparison = super::compare_finite_values(oracle_values, candidate_values, policy)?;
    let evidence = GpuCalculationEvidence {
        schema: super::GpuEvidenceSchema::default(),
        case_id: case_id.to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: ACCUMULATED_GPU_IMPLEMENTATION_ID.to_string(),
            revision: accumulated_candidate_revision(),
            shader_sha256: accumulated_shader_sha256(),
            input_sha256: accumulated_inputs_sha256(inputs),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(adapter.clone()),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: PINNED_ORACLE_IMPLEMENTATION_ID.to_string(),
            revision: PINNED_FLEXPART_REVISION.to_string(),
            executable_sha256: PINNED_ORACLE_EXECUTABLE_SHA256.to_string(),
            output_sha256: finite_values_sha256(oracle_values),
        }),
        comparison,
    };
    evidence.validate()?;
    Ok(evidence)
}

fn finite_values_sha256(values: &[f64]) -> String {
    let mut normalized = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        normalized.extend_from_slice(&value.to_le_bytes());
    }
    sha256_hex(&normalized)
}

/// One per-interval row of issue-specific GPU evidence.
///
/// The four boolean lanes are independent provenance/verdict facts owned by
/// #75/#90 (reset detection, fresh-run application, amount match, rate match);
/// collapsing them into an enum would hide which fact failed.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccumulatedGpuIntervalEvidence {
    /// Index of the source observation in observation order.
    pub source_index: usize,
    /// Source valid time in epoch seconds.
    pub source_valid_time_epoch_seconds: i64,
    /// Source-declared reset origin in epoch seconds.
    pub source_reset_epoch_seconds: i64,
    /// Source accumulated total in kg/m2.
    pub source_accumulated_amount_kg_per_square_meter: f64,
    /// Canonical interval start in epoch seconds.
    pub interval_start_epoch_seconds: i64,
    /// Canonical interval end in epoch seconds.
    pub interval_end_epoch_seconds: i64,
    /// Whether the source declared a new run relative to the previous row.
    pub detected_reset: bool,
    /// Whether the GPU amount used the fresh-run value.
    pub reset_applied: bool,
    /// GPU-derived interval amount in kg/m2.
    pub derived_gpu_amount_kg_per_square_meter: f64,
    /// GPU-derived SI rate in kg/(m2 s).
    pub derived_gpu_rate_si: f64,
    /// GPU-derived handoff rate in mm/h.
    pub derived_gpu_rate_millimeter_per_hour: f64,
    /// Canonical expected interval amount in kg/m2.
    pub oracle_expected_amount_kg_per_square_meter: f64,
    /// Canonical expected handoff rate in mm/h.
    pub oracle_expected_rate_millimeter_per_hour: f64,
    /// Whether the GPU amount matches the oracle within tolerance.
    pub amount_matches_oracle: bool,
    /// Whether the GPU rate matches the oracle within tolerance.
    pub rate_matches_oracle: bool,
}

/// Machine-readable GPU evidence for one accumulation validation case.
///
/// This record carries the issue-specific fields required by #90 alongside the
/// repository-wide [`GpuCalculationEvidence`] pair (amount lane and rate lane).
/// A passing case requires both generic comparisons to pass and every
/// per-interval row to match within the declared tolerance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccumulatedGpuReport {
    /// Stable schema identity.
    pub schema_id: String,
    /// Schema version.
    pub schema_version: u32,
    /// Stable issue-owned validation case identifier.
    pub case_id: String,
    /// Field identity supplied by the caller.
    pub field_identity: String,
    /// Pinned digest of the canonical #71/#75 accumulation fixture.
    pub oracle_fixture_sha256: String,
    /// Source accumulated unit (`kilogram_per_square_meter`).
    pub source_accumulated_unit: String,
    /// Derived amount unit (`kilogram_per_square_meter`).
    pub derived_amount_unit: String,
    /// Derived SI rate unit.
    pub derived_rate_si_unit: String,
    /// Derived handoff rate unit (`millimeter_per_hour`).
    pub derived_rate_handoff_unit: String,
    /// Absolute tolerance used for the GPU comparison.
    pub absolute_tolerance: f64,
    /// Relative tolerance inherited from #75.
    pub relative_tolerance: f64,
    /// One row per source observation, in observation order.
    pub intervals: Vec<AccumulatedGpuIntervalEvidence>,
    /// Repository-wide evidence for the amount lane.
    pub amount_evidence: GpuCalculationEvidence,
    /// Repository-wide evidence for the handoff-rate lane.
    pub rate_evidence: GpuCalculationEvidence,
    /// Overall numerical verdict (`passed` only when both lanes pass).
    pub verdict: NumericalVerdict,
}

impl AccumulatedGpuReport {
    /// Validate the issue-specific schema and its embedded generic evidence.
    ///
    /// # Errors
    ///
    /// Returns [`GpuEvidenceError`] when provenance, row coverage, units,
    /// tolerances, or verdicts are incomplete or contradictory.
    // Keeping these invariants in one audit path reduces the risk that a
    // future schema change validates only a subset of the evidence record.
    #[allow(clippy::too_many_lines)]
    pub fn validate(&self) -> Result<(), GpuEvidenceError> {
        if self.schema_id != ACCUMULATED_GPU_EVIDENCE_SCHEMA_ID
            || self.schema_version != ACCUMULATED_GPU_EVIDENCE_SCHEMA_VERSION
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "unsupported accumulation GPU evidence schema",
            ));
        }
        if self.case_id.trim().is_empty() || self.field_identity.trim().is_empty() {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence requires case and field identities",
            ));
        }
        if self.oracle_fixture_sha256 != PINNED_ACCUMULATION_FIXTURE_SHA256
            || self.source_accumulated_unit != SOURCE_UNIT
            || self.derived_amount_unit != SOURCE_UNIT
            || self.derived_rate_si_unit != RATE_SI_UNIT
            || self.derived_rate_handoff_unit != RATE_HANDOFF_UNIT
            || self.absolute_tolerance != ACCUMULATION_GPU_ABSOLUTE_TOLERANCE
            || self.relative_tolerance != ACCUMULATION_GPU_RELATIVE_TOLERANCE
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence provenance, units, or tolerances do not match the issue contract",
            ));
        }
        self.amount_evidence.validate()?;
        self.rate_evidence.validate()?;
        let amount_oracle = self.amount_evidence.oracle.as_ref().ok_or(
            GpuEvidenceError::InvalidComparisonState(
                "accumulation amount evidence requires pinned oracle provenance",
            ),
        )?;
        let rate_oracle =
            self.rate_evidence
                .oracle
                .as_ref()
                .ok_or(GpuEvidenceError::InvalidComparisonState(
                    "accumulation rate evidence requires pinned oracle provenance",
                ))?;
        let oracle_identity_is_pinned = [amount_oracle, rate_oracle].iter().all(|oracle| {
            oracle.implementation_id == PINNED_ORACLE_IMPLEMENTATION_ID
                && oracle.revision == PINNED_FLEXPART_REVISION
                && oracle.executable_sha256 == PINNED_ORACLE_EXECUTABLE_SHA256
        });
        if !oracle_identity_is_pinned
            || self.amount_evidence.candidate.implementation_id != ACCUMULATED_GPU_IMPLEMENTATION_ID
            || self.rate_evidence.candidate.implementation_id != ACCUMULATED_GPU_IMPLEMENTATION_ID
            || self.amount_evidence.candidate.shader_sha256 != accumulated_shader_sha256()
            || self.rate_evidence.candidate.shader_sha256 != accumulated_shader_sha256()
            || self.amount_evidence.candidate.input_sha256
                != self.rate_evidence.candidate.input_sha256
            || self.amount_evidence.candidate.revision != self.rate_evidence.candidate.revision
            || self.amount_evidence.execution != self.rate_evidence.execution
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence candidate or oracle provenance is inconsistent",
            ));
        }
        if self.intervals.is_empty()
            || self.amount_evidence.comparison.compared_value_count != self.intervals.len()
            || self.rate_evidence.comparison.compared_value_count != self.intervals.len()
            || self.amount_evidence.case_id != format!("{}/amount", self.case_id)
            || self.rate_evidence.case_id != format!("{}/rate", self.case_id)
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence row coverage does not match the embedded comparisons",
            ));
        }
        let recorded_observations: Vec<AccumulatedObservation> = self
            .intervals
            .iter()
            .map(|row| AccumulatedObservation {
                valid_time_epoch_seconds: row.source_valid_time_epoch_seconds,
                reset_epoch_seconds: row.source_reset_epoch_seconds,
                accumulated_amount_kg_per_square_meter: row
                    .source_accumulated_amount_kg_per_square_meter,
            })
            .collect();
        let expected_metadata =
            validate_interval_sequence(&recorded_observations).map_err(|_| {
                GpuEvidenceError::InvalidComparisonState(
                    "accumulation evidence source rows violate the canonical interval contract",
                )
            })?;
        let expected_inputs = build_transform_inputs(&recorded_observations).map_err(|_| {
            GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence source rows cannot produce canonical GPU inputs",
            )
        })?;
        let expected_input_sha256 = accumulated_inputs_sha256(&expected_inputs);
        if self.amount_evidence.candidate.input_sha256 != expected_input_sha256 {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence source rows do not match the recorded GPU input hash",
            ));
        }
        let rows_pass = self.intervals.iter().enumerate().all(|(index, row)| {
            let metadata = expected_metadata[index];
            let duration_seconds = f64::from(expected_inputs[index].duration_seconds);
            row.source_index == index
                && row.interval_start_epoch_seconds == metadata.interval_start_epoch_seconds
                && row.interval_end_epoch_seconds == metadata.interval_end_epoch_seconds
                && row.detected_reset == metadata.detected_reset
                && row.reset_applied == metadata.reset_applied
                && row
                    .source_accumulated_amount_kg_per_square_meter
                    .is_finite()
                && row.derived_gpu_amount_kg_per_square_meter.is_finite()
                && row.derived_gpu_rate_si.is_finite()
                && row.derived_gpu_rate_millimeter_per_hour.is_finite()
                && row.oracle_expected_amount_kg_per_square_meter.is_finite()
                && row.oracle_expected_rate_millimeter_per_hour.is_finite()
                && amounts_match(
                    row.derived_gpu_amount_kg_per_square_meter / duration_seconds,
                    row.derived_gpu_rate_si,
                )
                && amounts_match(
                    row.derived_gpu_rate_si * 3_600.0,
                    row.derived_gpu_rate_millimeter_per_hour,
                )
                && row.amount_matches_oracle
                && row.rate_matches_oracle
                && row.amount_matches_oracle
                    == amounts_match(
                        row.derived_gpu_amount_kg_per_square_meter,
                        row.oracle_expected_amount_kg_per_square_meter,
                    )
                && row.rate_matches_oracle
                    == amounts_match(
                        row.derived_gpu_rate_millimeter_per_hour,
                        row.oracle_expected_rate_millimeter_per_hour,
                    )
        });
        let oracle_amounts: Vec<f64> = self
            .intervals
            .iter()
            .map(|row| row.oracle_expected_amount_kg_per_square_meter)
            .collect();
        let oracle_rates: Vec<f64> = self
            .intervals
            .iter()
            .map(|row| row.oracle_expected_rate_millimeter_per_hour)
            .collect();
        let candidate_amounts: Vec<f64> = self
            .intervals
            .iter()
            .map(|row| row.derived_gpu_amount_kg_per_square_meter)
            .collect();
        let candidate_rates: Vec<f64> = self
            .intervals
            .iter()
            .map(|row| row.derived_gpu_rate_millimeter_per_hour)
            .collect();
        if amount_oracle.output_sha256 != finite_values_sha256(&oracle_amounts)
            || rate_oracle.output_sha256 != finite_values_sha256(&oracle_rates)
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence oracle hashes do not match the recorded values",
            ));
        }
        let policy = accumulated_comparison_policy()?;
        let expected_amount_comparison =
            super::compare_finite_values(&oracle_amounts, &candidate_amounts, policy)?;
        let expected_rate_comparison =
            super::compare_finite_values(&oracle_rates, &candidate_rates, policy)?;
        if self.amount_evidence.comparison != expected_amount_comparison
            || self.rate_evidence.comparison != expected_rate_comparison
        {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence comparisons do not match the recorded rows",
            ));
        }
        let expected_verdict = if rows_pass
            && self.amount_evidence.comparison.verdict == NumericalVerdict::Passed
            && self.rate_evidence.comparison.verdict == NumericalVerdict::Passed
        {
            NumericalVerdict::Passed
        } else {
            NumericalVerdict::Failed
        };
        if self.verdict != expected_verdict {
            return Err(GpuEvidenceError::InvalidComparisonState(
                "accumulation evidence verdict contradicts its rows or comparisons",
            ));
        }
        Ok(())
    }

    /// Require this report to prove a successful paired GPU validation.
    ///
    /// Both embedded [`GpuCalculationEvidence`] records must satisfy
    /// `require_paired_pass` and the overall verdict must be `passed`.
    ///
    /// # Errors
    ///
    /// Returns [`GpuEvidenceError`] unless both lanes prove a paired pass.
    pub fn require_paired_pass(&self) -> Result<(), GpuEvidenceError> {
        self.validate()?;
        self.amount_evidence.require_paired_pass()?;
        self.rate_evidence.require_paired_pass()?;
        if self.verdict != NumericalVerdict::Passed {
            return Err(GpuEvidenceError::NotPassing);
        }
        Ok(())
    }
}

/// Execute one validated observation sequence and build issue-specific GPU evidence.
///
/// `field_identity` names the resolved field, `case_id` is the stable validation
/// case, and `oracle_amounts`/`oracle_rates_mmh` are the canonical expected
/// values from #71/#75. Candidate values and adapter provenance are obtained
/// only by dispatching and reading back this module's WGSL path; callers cannot
/// supply CPU-produced values while claiming device execution.
///
/// The function compares both lanes with the #90 policy, records per-interval
/// rows, and embeds two repository-wide evidence records. No CPU-derived
/// interval values are used as the candidate; the caller must supply actual
/// WGSL results.
///
/// # Errors
///
/// Returns [`GpuAccumulationError`] for GPU preparation, dispatch, readback,
/// comparison-policy, or evidence-validation failure.
pub async fn build_accumulated_gpu_report(
    case_id: &str,
    field_identity: &str,
    ctx: &GpuContext,
    observations: &[AccumulatedObservation],
    oracle_amounts: &[f64],
    oracle_rates_mmh: &[f64],
) -> Result<AccumulatedGpuReport, GpuAccumulationError> {
    let readback = execute_accumulated_intervals_gpu(ctx, observations).await?;
    Ok(assemble_accumulated_gpu_report(
        case_id,
        field_identity,
        observations,
        &readback,
        oracle_amounts,
        oracle_rates_mmh,
    )?)
}

#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
fn assemble_accumulated_gpu_report(
    case_id: &str,
    field_identity: &str,
    observations: &[AccumulatedObservation],
    readback: &AccumulatedGpuReadback,
    oracle_amounts: &[f64],
    oracle_rates_mmh: &[f64],
) -> Result<AccumulatedGpuReport, GpuEvidenceError> {
    let transform_inputs = &readback.transform_inputs;
    let gpu_amounts = &readback.amounts;
    let gpu_rates_si = &readback.rates_si;
    let gpu_rates_mmh = &readback.rates_mmh;
    let metadata = validate_interval_sequence(observations).map_err(|_| {
        GpuEvidenceError::InvalidComparisonState(
            "GPU evidence requires canonically valid source observations",
        )
    })?;
    let expected_transform_inputs = build_transform_inputs(observations).map_err(|_| {
        GpuEvidenceError::InvalidComparisonState(
            "GPU evidence could not derive canonical transform inputs",
        )
    })?;
    if gpu_amounts.len() != observations.len()
        || gpu_rates_si.len() != observations.len()
        || gpu_rates_mmh.len() != observations.len()
        || oracle_amounts.len() != observations.len()
        || oracle_rates_mmh.len() != observations.len()
    {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "GPU evidence lanes must match the observation count",
        ));
    }
    if transform_inputs != &expected_transform_inputs {
        return Err(GpuEvidenceError::InvalidComparisonState(
            "GPU evidence transform inputs do not match the source observations",
        ));
    }
    let gpu_amounts_f64: Vec<f64> = gpu_amounts.iter().map(|value| f64::from(*value)).collect();
    let gpu_rates_mmh_f64: Vec<f64> = gpu_rates_mmh
        .iter()
        .map(|value| f64::from(*value))
        .collect();
    let amount_evidence = build_accumulation_gpu_calculation_evidence(
        &format!("{case_id}/amount"),
        &readback.adapter,
        transform_inputs,
        oracle_amounts,
        &gpu_amounts_f64,
    )?;
    let rate_evidence = build_accumulation_gpu_calculation_evidence(
        &format!("{case_id}/rate"),
        &readback.adapter,
        transform_inputs,
        oracle_rates_mmh,
        &gpu_rates_mmh_f64,
    )?;
    let mut intervals = Vec::with_capacity(observations.len());
    let mut all_match = true;
    for index in 0..observations.len() {
        let observation = observations[index];
        let interval = metadata[index];
        let amount_matches = amounts_match(gpu_amounts_f64[index], oracle_amounts[index]);
        let rate_matches = amounts_match(gpu_rates_mmh_f64[index], oracle_rates_mmh[index]);
        if !amount_matches || !rate_matches {
            all_match = false;
        }
        intervals.push(AccumulatedGpuIntervalEvidence {
            source_index: index,
            source_valid_time_epoch_seconds: observation.valid_time_epoch_seconds,
            source_reset_epoch_seconds: observation.reset_epoch_seconds,
            source_accumulated_amount_kg_per_square_meter: observation
                .accumulated_amount_kg_per_square_meter,
            interval_start_epoch_seconds: interval.interval_start_epoch_seconds,
            interval_end_epoch_seconds: interval.interval_end_epoch_seconds,
            detected_reset: interval.detected_reset,
            reset_applied: interval.reset_applied,
            derived_gpu_amount_kg_per_square_meter: gpu_amounts_f64[index],
            derived_gpu_rate_si: f64::from(gpu_rates_si[index]),
            derived_gpu_rate_millimeter_per_hour: gpu_rates_mmh_f64[index],
            oracle_expected_amount_kg_per_square_meter: oracle_amounts[index],
            oracle_expected_rate_millimeter_per_hour: oracle_rates_mmh[index],
            amount_matches_oracle: amount_matches,
            rate_matches_oracle: rate_matches,
        });
    }
    let verdict = if all_match
        && amount_evidence.comparison.verdict == NumericalVerdict::Passed
        && rate_evidence.comparison.verdict == NumericalVerdict::Passed
    {
        NumericalVerdict::Passed
    } else {
        NumericalVerdict::Failed
    };
    let report = AccumulatedGpuReport {
        schema_id: ACCUMULATED_GPU_EVIDENCE_SCHEMA_ID.to_string(),
        schema_version: ACCUMULATED_GPU_EVIDENCE_SCHEMA_VERSION,
        case_id: case_id.to_string(),
        field_identity: field_identity.to_string(),
        oracle_fixture_sha256: PINNED_ACCUMULATION_FIXTURE_SHA256.to_string(),
        source_accumulated_unit: SOURCE_UNIT.to_string(),
        derived_amount_unit: SOURCE_UNIT.to_string(),
        derived_rate_si_unit: RATE_SI_UNIT.to_string(),
        derived_rate_handoff_unit: RATE_HANDOFF_UNIT.to_string(),
        absolute_tolerance: ACCUMULATION_GPU_ABSOLUTE_TOLERANCE,
        relative_tolerance: ACCUMULATION_GPU_RELATIVE_TOLERANCE,
        intervals,
        amount_evidence,
        rate_evidence,
        verdict,
    };
    report.validate()?;
    Ok(report)
}

fn amounts_match(candidate: f64, oracle: f64) -> bool {
    let absolute_error = (candidate - oracle).abs();
    if absolute_error <= ACCUMULATION_GPU_ABSOLUTE_TOLERANCE {
        return true;
    }
    let scale = candidate.abs().max(oracle.abs());
    if scale == 0.0 {
        return absolute_error == 0.0;
    }
    absolute_error / scale <= ACCUMULATION_GPU_RELATIVE_TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(valid_time: i64, reset: i64, amount: f64) -> AccumulatedObservation {
        AccumulatedObservation {
            valid_time_epoch_seconds: valid_time,
            reset_epoch_seconds: reset,
            accumulated_amount_kg_per_square_meter: amount,
        }
    }

    #[test]
    fn transform_inputs_derive_control_plane_without_amounts() {
        let observations = vec![
            observation(1_800, 0, 1.0),
            observation(3_600, 1_800, 4.0),
            observation(5_400, 1_800, 7.0),
        ];
        let inputs = build_transform_inputs(&observations).expect("valid sequence builds");
        assert_eq!(inputs.len(), 3);
        assert_eq!(inputs[0].reset_applied, 1);
        assert_eq!(inputs[1].reset_applied, 1);
        assert_eq!(inputs[2].reset_applied, 0);
        assert!((f64::from(inputs[0].duration_seconds) - 1_800.0).abs() < 1.0e-3);
        assert!((f64::from(inputs[0].curr_amount_kg_per_square_meter) - 1.0).abs() < 1.0e-6);
        assert_eq!(
            inputs[0].prev_amount_kg_per_square_meter.to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(
            inputs[1].prev_amount_kg_per_square_meter.to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(
            inputs[2].prev_amount_kg_per_square_meter.to_bits(),
            4.0_f32.to_bits()
        );
    }

    #[test]
    fn transform_inputs_reject_negative_delta_before_dispatch() {
        let observations = vec![observation(1_800, 0, 1.0), observation(3_600, 0, 0.5)];
        let error = build_transform_inputs(&observations).expect_err("negative delta fails");
        assert!(matches!(
            error,
            GpuAccumulationError::Validation(AccumulationError::NegativeDelta { .. })
        ));
    }

    #[test]
    fn transform_inputs_reject_empty_sequence() {
        let error = build_transform_inputs(&[]).expect_err("empty fails");
        assert!(matches!(error, GpuAccumulationError::EmptySequence));
    }

    #[test]
    fn raw_transform_inputs_fail_closed_on_invalid_kernel_values() {
        let valid = AccumulatedTransformInput {
            curr_amount_kg_per_square_meter: 2.0,
            prev_amount_kg_per_square_meter: 1.0,
            duration_seconds: 1_800.0,
            reset_applied: 0,
        };
        validate_transform_inputs(&[valid]).expect("valid raw input passes");

        for invalid in [
            AccumulatedTransformInput {
                reset_applied: 2,
                ..valid
            },
            AccumulatedTransformInput {
                duration_seconds: f32::NAN,
                ..valid
            },
            AccumulatedTransformInput {
                curr_amount_kg_per_square_meter: 0.5,
                ..valid
            },
            AccumulatedTransformInput {
                prev_amount_kg_per_square_meter: 1.0,
                reset_applied: 1,
                ..valid
            },
        ] {
            assert!(matches!(
                validate_transform_inputs(&[invalid]),
                Err(GpuAccumulationError::InvalidTransformInput { .. })
            ));
        }
    }

    #[test]
    fn transform_inputs_reject_unrepresentable_interval_duration() {
        let error = build_transform_inputs(&[observation(i64::MAX, i64::MIN, 1.0)])
            .expect_err("overflowing duration fails closed");
        assert!(matches!(
            error,
            GpuAccumulationError::Validation(AccumulationError::IntervalDurationOutOfRange { .. })
        ));
    }

    #[test]
    fn shader_and_input_hashes_are_stable_sha256() {
        let shader = accumulated_shader_sha256();
        assert_eq!(shader.len(), 64);
        assert!(shader
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        let inputs =
            build_transform_inputs(&[observation(1_800, 0, 1.0)]).expect("single input builds");
        let digest = accumulated_inputs_sha256(&inputs);
        assert_eq!(digest.len(), 64);
    }
}
