//! GPU reconstruction of FLEXPART `numpf=1` precipitation point members (issue #207).
//!
//! #202 decided that a FLEXPART 11.1 `numpf=1` LSP/CP member stored at memory
//! time `T` is not a #90 interval mean. Pinned `flex_extract` 7.1.2 with `RRINT=0`
//! writes `darain(x_{k-1}, x_k, x_{k+1}, x_{k+2})` at the boundary
//! `T = end(x_k)` (`EcFlexpart.py:1180-1211`, `disaggregation.py:106-157`).
//! This module adds exactly that stage on the device:
//!
//! ```text
//! #90 interval-mean rate grids (mm/h, device-resident)
//! -> darain point-rate reconstruction (WGSL, this module)
//! -> typed members at T with support [T - 2Δ, T + 2Δ]
//! -> later #119 temporal sampling (not implemented here)
//! ```
//!
//! ## Supported envelope (#202 decision artifact)
//!
//! `N >= 5` contiguous intervals of equal positive integer-hour length `Δ`, one
//! source/grid/context identity, LSP and CP accumulated since reset and resolved
//! by #90, `flex_extract` `RRINT=0`, `PUREFC=0`, `BASETIME` unset, `numpf=1`.
//! Members exist only at `T_j = s0 + jΔ`, `j = 2..N-2`. The leading persistence
//! value and the BASETIME trailing writes are never produced.
//!
//! ## Composition contract
//!
//! - Reuses [`GpuContext`] and the #90 rate buffers in place; no copy, no
//!   readback, no host arithmetic on rates.
//! - [`PreparedPrecipitationMembers::encode`] records #90 and the reconstruction
//!   into a caller-owned encoder and never submits, polls, maps or reads back.
//! - Output member buffers and a shared fatal status word stay device-resident
//!   for the #119 consumer. Readback exists only at explicit validation
//!   boundaries.
//! - No CPU fallback exists. A failed GPU path is an explicit error.

use std::mem::size_of;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::meteorology::{
    accumulation::PackingDecreaseAdmission,
    accumulation::RATE_HANDOFF_UNIT,
    temporal::{Tolerance, DEFAULT_ABS_TOL, DEFAULT_REL_TOL},
    Calendar, FieldId, FieldTime, HorizontalGrid, SignConvention, Unit,
};

use super::{
    download_buffer_typed,
    meteorology::{
        AccumulatedIntervalMetadataRecord, CanonicalGpuField, IntervalRateGrid,
        MeteorologyCompositionError,
    },
    AccumulatedIntervalKernel, ComparisonPolicy, GpuAccumulationError, GpuAdapterEvidence,
    GpuBufferError, GpuCalculationEvidence, GpuCalculationPath, GpuCandidateEvidence, GpuContext,
    GpuEvidenceError, GpuExecutionEvidence, GpuExecutionStatus, PinnedOracleEvidence,
};

/// WGSL source of the `darain` reconstruction kernel.
const SHADER_SOURCE: &str = include_str!("../shaders/darain_precipitation_member.wgsl");

/// Workgroup width of the reconstruction kernel.
const WORKGROUP_SIZE_X: u32 = 64;

/// Seconds per hour; `flex_extract` uses `DTIME` as integer hours.
const SECONDS_PER_HOUR: i64 = 3_600;

/// Number of consecutive interval means consumed per member (`xa..xd`).
const SUPPORT_INTERVALS: usize = 4;

/// Smallest sequence admitted by the #202 coverage envelope (`N >= 5`).
pub const MINIMUM_INTERVAL_COUNT: usize = 5;

/// Candidate implementation identity for machine-readable evidence.
pub const PRECIPITATION_MEMBER_IMPLEMENTATION_ID: &str = "precipitation-member-darain-gpu-207";

/// Stable identity of the produced representation, as named by the #202 handoff.
pub const PRECIPITATION_MEMBER_PREPARATION: &str = "flex_extract-7.1.2-rrint0-darain";

/// Pinned `flex_extract` revision that owns `darain`.
pub const FLEX_EXTRACT_REVISION: &str = "e0005c99ac81d12faa45a8ff799debbd592b0dc0";

/// Oracle identity of the pinned original routine.
pub const DARAIN_ORACLE_IMPLEMENTATION_ID: &str =
    "flex_extract-7.1.2-disaggregation.darain (Source/Python/Mods/disaggregation.py:106-157)";

/// SHA-256 of the archived pinned `disaggregation.py` (git blob `12ec74a7`).
pub const DARAIN_SOURCE_SHA256: &str =
    "ddfa9276f7cd6c7a5c9380c7ef91f5a11c6790f1b0ea7629b6d226776a0161b6";

/// Status bit: a supporting interval-mean rate was not finite.
pub const STATUS_NON_FINITE_RATE: u32 = 1;

/// Status bit: a reconstructed member was not finite.
pub const STATUS_NON_FINITE_MEMBER: u32 = 2;

/// `flex_extract` disaggregation routine selected for precipitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisaggregationMethod {
    /// `disaggregation.darain` (`RRINT=0` LSP/CP).
    Darain,
    /// `disaggregation.dapoly` (non-precipitation fluxes).
    Dapoly,
    /// `disaggregation.IA3` (`RRINT=1`, Hittmeir et al. 2018).
    Ia3,
}

/// Declared `flex_extract`/FLEXPART preparation the members must represent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecipitationMemberPreparation {
    /// `flex_extract` `RRINT`.
    pub rrint: u8,
    /// `flex_extract` `PUREFC`.
    pub purefc: u8,
    /// `flex_extract` `BASETIME` hour, `None` when unset.
    pub basetime_hour: Option<u8>,
    /// FLEXPART `numpf` of the consumer.
    pub numpf: u8,
    /// Disaggregation routine.
    pub method: DisaggregationMethod,
}

impl PrecipitationMemberPreparation {
    /// The only representation established by #202.
    #[must_use]
    pub const fn flex_extract_rrint0() -> Self {
        Self {
            rrint: 0,
            purefc: 0,
            basetime_hour: None,
            numpf: 1,
            method: DisaggregationMethod::Darain,
        }
    }

    /// Reject every preparation outside the #202 decision.
    ///
    /// # Errors
    /// Returns [`PrecipitationMemberError::UnsupportedPreparation`] naming the
    /// first unsupported setting.
    pub fn validate(&self) -> Result<(), PrecipitationMemberError> {
        let reason = if self.rrint != 0 {
            "RRINT=1 / IA3 sub-members are not numpf=1 members (#212)"
        } else if self.purefc != 0 {
            "PUREFC=1 stamps non-zero stepRange members"
        } else if self.basetime_hour.is_some() {
            "BASETIME trailing boundary records are rejected by #202"
        } else if self.numpf != 1 {
            "only numpf=1 is established by #202"
        } else if self.method != DisaggregationMethod::Darain {
            "only flex_extract darain reconstructs RRINT=0 precipitation"
        } else {
            return Ok(());
        };
        Err(PrecipitationMemberError::UnsupportedPreparation(reason))
    }
}

/// Errors of the #207 reconstruction stage.
#[derive(Debug, Error)]
pub enum PrecipitationMemberError {
    /// Preparation settings outside the #202 decision.
    #[error("unsupported precipitation preparation: {0}")]
    UnsupportedPreparation(&'static str),
    /// Source accumulation convention or field class has no #202 evidence.
    #[error("unsupported precipitation source representation: {0}")]
    UnsupportedRepresentation(&'static str),
    /// LSP/CP fields supplied in the wrong slots.
    #[error("invalid precipitation field ordering: expected {expected:?}, got {actual:?}")]
    FieldOrder {
        /// Required field identity for the slot.
        expected: FieldId,
        /// Supplied field identity.
        actual: FieldId,
    },
    /// Fewer than `N = 5` intervals.
    #[error("insufficient support: {interval_count} intervals, at least {MINIMUM_INTERVAL_COUNT} required")]
    InsufficientSupport {
        /// Supplied interval count.
        interval_count: usize,
    },
    /// Interval `index` does not start where its predecessor ends.
    #[error("interval {index} is not contiguous with its predecessor")]
    GapOrOverlap {
        /// Offending interval index.
        index: usize,
    },
    /// Interval `index` has a different length from the first interval.
    #[error("interval {index} has unequal duration")]
    UnequalDelta {
        /// Offending interval index.
        index: usize,
    },
    /// `Δ` is not a positive integer number of hours.
    #[error("interval duration {seconds} s is not a positive integer number of hours")]
    NonIntegerHourDelta {
        /// Offending duration in seconds.
        seconds: i64,
    },
    /// LSP/CP or context identities differ.
    #[error("identity mismatch: {0}")]
    IdentityMismatch(&'static str),
    /// A raw interval-mean rate is not finite.
    #[error("interval-mean rate {index} is not finite")]
    NonFinite {
        /// Offending flat index.
        index: usize,
    },
    /// No member exists at a requested time outside `[T_first, T_last]`.
    #[error("no precipitation member at {valid_time_epoch_seconds}: outside coverage")]
    OutsideCoverage {
        /// Requested member time.
        valid_time_epoch_seconds: i64,
    },
    /// A requested time inside the coverage is not a member boundary.
    #[error("no precipitation member at {valid_time_epoch_seconds}: not on the Δ member spacing")]
    MemberSpacingMismatch {
        /// Requested member time.
        valid_time_epoch_seconds: i64,
    },
    /// The device reported non-finite rates or members.
    #[error("precipitation member device status {0:#x} is not valid")]
    DeviceStatus(u32),
    /// A count does not fit the `u32` device addressing.
    #[error("{field} does not fit in u32")]
    ValueTooLarge {
        /// Name of the oversized value.
        field: &'static str,
    },
    /// #76/#90 source description or encode failed.
    #[error(transparent)]
    Composition(#[from] MeteorologyCompositionError),
    /// #90 kernel creation failed.
    #[error(transparent)]
    Accumulation(#[from] GpuAccumulationError),
    /// Explicit readback failed.
    #[error(transparent)]
    Buffer(#[from] GpuBufferError),
    /// Evidence could not be built or validated.
    #[error(transparent)]
    Evidence(#[from] GpuEvidenceError),
    /// Metadata could not be serialized for hashing.
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    /// The WebGPU device reported a scoped error.
    #[error("precipitation member GPU {stage} failed: {message}")]
    Device {
        /// Operation during which the error was observed.
        stage: &'static str,
        /// Backend detail.
        message: String,
    },
}

impl PrecipitationMemberError {
    /// The #202 decision-artifact rejection code for this error, when one applies.
    #[must_use]
    pub const fn rejection_code(&self) -> Option<&'static str> {
        match self {
            Self::UnsupportedPreparation(_) | Self::UnsupportedRepresentation(_) => {
                Some("unsupported_preparation")
            }
            Self::FieldOrder { .. } | Self::IdentityMismatch(_) => Some("identity_mismatch"),
            Self::InsufficientSupport { .. } => Some("insufficient_support"),
            Self::GapOrOverlap { .. } => Some("gap_or_overlap"),
            Self::UnequalDelta { .. } | Self::NonIntegerHourDelta { .. } => {
                Some("unequal_or_non_integer_hour_delta")
            }
            Self::NonFinite { .. } | Self::DeviceStatus(_) => Some("non_finite"),
            Self::OutsideCoverage { .. } => Some("outside_coverage"),
            Self::MemberSpacingMismatch { .. } => Some("member_spacing_mismatch"),
            _ => None,
        }
    }
}

/// One reconstructed member: rate at the instant `T` from four supporting intervals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecipitationPointMember {
    /// Plane index in the member buffers: values at `[slot * cell_count + cell]`.
    pub slot: usize,
    /// Member time `T = end(xb) = start(xc)`; FLEXPART memtime of this member.
    pub valid_time_epoch_seconds: i64,
    /// `T - 2Δ`, start of `xa`.
    pub support_start_epoch_seconds: i64,
    /// `T + 2Δ`, end of `xd`.
    pub support_end_epoch_seconds: i64,
    /// `xa, xb, xc, xd` in physical order.
    pub support_intervals: [AccumulatedIntervalMetadataRecord; SUPPORT_INTERVALS],
}

/// Source identity of one precipitation parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecipitationSourceIdentity {
    /// Canonical field identity.
    pub field_id: FieldId,
    /// Canonical accumulated-total unit.
    pub source_unit: Unit,
    /// Canonical sign convention.
    pub source_sign: SignConvention,
    /// Original observation times.
    pub source_times: Vec<FieldTime>,
    /// #216 admissions propagated from #90 (#219); omitted from serialization
    /// when empty so manifests of sources without admissions are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quantization_admissions: Vec<PackingDecreaseAdmission>,
}

/// Immutable typed handoff for #119: everything needed without reconstructing rates or times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrecipitationMemberMetadata {
    /// Declared preparation (validated `RRINT=0` darain).
    pub preparation: PrecipitationMemberPreparation,
    /// Stable representation identity, `flex_extract-7.1.2-rrint0-darain`.
    pub representation: String,
    /// Member value unit, `millimeter_per_hour`.
    pub unit: String,
    /// Shared source calendar.
    pub calendar: Calendar,
    /// Interval length `Δ` in seconds (integer hours).
    pub delta_seconds: i64,
    /// `s0`, start of the first interval.
    pub sequence_start_epoch_seconds: i64,
    /// Shared source horizontal grid; cells are x-fastest.
    pub horizontal_grid: HorizontalGrid,
    /// Cells per member plane.
    pub cell_count: usize,
    /// Shared complete canonical source snapshot hashes in source order.
    pub source_snapshot_sha256: Vec<String>,
    /// Shared validated #75 intervals in physical order.
    pub intervals: Vec<AccumulatedIntervalMetadataRecord>,
    /// Large-scale precipitation source identity.
    pub large_scale: PrecipitationSourceIdentity,
    /// Convective precipitation source identity.
    pub convective: PrecipitationSourceIdentity,
    /// Members in ascending time; `T_first = s0 + 2Δ`, `T_last = s0 + (N-2)Δ`.
    pub members: Vec<PrecipitationPointMember>,
    /// Adapter of the owning [`GpuContext`].
    pub gpu_adapter: GpuAdapterEvidence,
}

impl PrecipitationMemberMetadata {
    /// Member at exactly `valid_time_epoch_seconds`.
    ///
    /// # Errors
    /// `OutsideCoverage` before `T_first` or after `T_last`; `MemberSpacingMismatch`
    /// for a time inside the coverage that is not a member boundary.
    pub fn member_at(
        &self,
        valid_time_epoch_seconds: i64,
    ) -> Result<&PrecipitationPointMember, PrecipitationMemberError> {
        let outside = PrecipitationMemberError::OutsideCoverage {
            valid_time_epoch_seconds,
        };
        let (Some(first), Some(last)) = (self.members.first(), self.members.last()) else {
            return Err(outside);
        };
        if valid_time_epoch_seconds < first.valid_time_epoch_seconds
            || valid_time_epoch_seconds > last.valid_time_epoch_seconds
        {
            return Err(outside);
        }
        self.members
            .iter()
            .find(|m| m.valid_time_epoch_seconds == valid_time_epoch_seconds)
            .ok_or(PrecipitationMemberError::MemberSpacingMismatch {
                valid_time_epoch_seconds,
            })
    }

    /// Byte offset and size of one member plane in either member buffer.
    #[must_use]
    pub fn plane_byte_range(&self, member: &PrecipitationPointMember) -> (u64, u64) {
        let plane = (self.cell_count * size_of::<f32>()) as u64;
        (member.slot as u64 * plane, plane)
    }

    /// SHA-256 of the normalized source manifest (identities and intervals).
    ///
    /// # Errors
    /// Forwards serialization failures.
    pub fn input_sha256(&self) -> Result<String, PrecipitationMemberError> {
        let manifest = serde_json::to_vec(&(
            &self.horizontal_grid,
            &self.source_snapshot_sha256,
            &self.intervals,
            &self.large_scale,
            &self.convective,
        ))?;
        Ok(sha256_hex(&manifest))
    }
}

/// Validated member layout derived from #75 interval metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberSequence {
    /// `Δ` in seconds.
    pub delta_seconds: i64,
    /// `s0`.
    pub sequence_start_epoch_seconds: i64,
    /// Members with complete four-interval support.
    pub members: Vec<PrecipitationPointMember>,
}

/// Validate the #202 interval envelope and derive every admitted member.
///
/// # Errors
/// Rejects `N < 5`, non-integer-hour or unequal `Δ`, and gaps/overlaps.
pub fn derive_member_sequence(
    intervals: &[AccumulatedIntervalMetadataRecord],
) -> Result<MemberSequence, PrecipitationMemberError> {
    if intervals.len() < MINIMUM_INTERVAL_COUNT {
        return Err(PrecipitationMemberError::InsufficientSupport {
            interval_count: intervals.len(),
        });
    }
    let delta = intervals[0].end_epoch_seconds - intervals[0].start_epoch_seconds;
    if delta <= 0 || delta % SECONDS_PER_HOUR != 0 {
        return Err(PrecipitationMemberError::NonIntegerHourDelta { seconds: delta });
    }
    for (index, interval) in intervals.iter().enumerate() {
        if interval.end_epoch_seconds - interval.start_epoch_seconds != delta
            || interval.duration_seconds != delta
        {
            return Err(PrecipitationMemberError::UnequalDelta { index });
        }
        if index > 0 && interval.start_epoch_seconds != intervals[index - 1].end_epoch_seconds {
            return Err(PrecipitationMemberError::GapOrOverlap { index });
        }
    }
    let members = intervals
        .windows(SUPPORT_INTERVALS)
        .enumerate()
        .map(|(slot, support)| PrecipitationPointMember {
            slot,
            valid_time_epoch_seconds: support[1].end_epoch_seconds,
            support_start_epoch_seconds: support[0].start_epoch_seconds,
            support_end_epoch_seconds: support[3].end_epoch_seconds,
            support_intervals: [
                support[0].clone(),
                support[1].clone(),
                support[2].clone(),
                support[3].clone(),
            ],
        })
        .collect();
    Ok(MemberSequence {
        delta_seconds: delta,
        sequence_start_epoch_seconds: intervals[0].start_epoch_seconds,
        members,
    })
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct DarainParamsRaw {
    member_count: u32,
    cell_count: u32,
    _pad0: u32,
    _pad1: u32,
}

/// Compiled `darain` reconstruction pipeline.
pub struct DarainMemberKernel {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl DarainMemberKernel {
    /// Compile the WGSL kernel and surface scoped device errors.
    ///
    /// # Errors
    /// [`PrecipitationMemberError::Device`] for shader or pipeline errors.
    pub fn new(ctx: &GpuContext) -> Result<Self, PrecipitationMemberError> {
        push_device_error_scopes(ctx);
        let shader = ctx.load_shader("darain_precipitation_member_shader", SHADER_SOURCE);
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("darain_precipitation_member_bgl"),
                    entries: &[
                        storage(0, true),
                        storage(1, false),
                        storage(2, false),
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
            "darain_precipitation_member_pipeline",
            &shader,
            "main",
            &[&bind_group_layout],
        );
        finish_device_error_scopes(ctx, "kernel creation")?;
        Ok(Self {
            bind_group_layout,
            pipeline,
        })
    }
}

/// Reusable pipelines for #90 interval rates and #207 reconstruction on one device.
pub struct PrecipitationMemberKernels {
    owner_context: Arc<()>,
    accumulation: AccumulatedIntervalKernel,
    darain: DarainMemberKernel,
}

impl PrecipitationMemberKernels {
    /// Initialize both pipelines once, outside command encoding.
    ///
    /// # Errors
    /// Forwards pipeline creation errors.
    pub fn new(ctx: &GpuContext) -> Result<Self, PrecipitationMemberError> {
        Ok(Self {
            owner_context: Arc::clone(&ctx.identity),
            accumulation: AccumulatedIntervalKernel::new(ctx)?,
            darain: DarainMemberKernel::new(ctx)?,
        })
    }
}

/// A stage actually recorded into the caller's encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrecipitationMemberStage {
    /// Existing #90 interval/reset transform of the named field.
    AccumulatedTransform(FieldId),
    /// #207 darain reconstruction of the named field.
    DarainReconstruction(FieldId),
}

/// Prepared reconstruction for one LSP/CP pair; owns member and status resources.
///
/// Keep it (and the borrowed sources) alive until the last submitted consumer
/// of the encoded member buffers completes.
pub struct PreparedPrecipitationMembers<'a, 'ctx> {
    owner_context: Arc<()>,
    large_scale: &'a CanonicalGpuField<'ctx>,
    convective: &'a CanonicalGpuField<'ctx>,
    large_scale_members: wgpu::Buffer,
    convective_members: wgpu::Buffer,
    status: wgpu::Buffer,
    params: wgpu::Buffer,
    metadata: PrecipitationMemberMetadata,
    stages: Vec<PrecipitationMemberStage>,
}

/// Device-side handoff of encoded members for the #119 temporal consumer.
pub struct EncodedPrecipitationMembers<'a> {
    /// LSP members in mm/h, `[slot * cell_count + cell]`, `STORAGE | COPY_SRC`.
    pub large_scale_mm_per_h: &'a wgpu::Buffer,
    /// CP members in mm/h, same layout.
    pub convective_mm_per_h: &'a wgpu::Buffer,
    /// Shared fatal status word; a consumer must reject any nonzero value.
    pub status: &'a wgpu::Buffer,
    /// Typed member times, supports and identities.
    pub metadata: &'a PrecipitationMemberMetadata,
    /// Stages recorded in order; execution still requires caller submission.
    pub stages: &'a [PrecipitationMemberStage],
}

/// Validate an LSP/CP pair against the #202 envelope and allocate member resources.
///
/// Every check runs before any GPU work is recorded.
///
/// # Errors
/// Fails closed for unsupported preparation, field order, representation,
/// identity, interval envelope, or sizing; see [`PrecipitationMemberError`].
pub fn prepare_precipitation_members<'a, 'ctx>(
    ctx: &GpuContext,
    large_scale: &'a CanonicalGpuField<'ctx>,
    convective: &'a CanonicalGpuField<'ctx>,
    preparation: PrecipitationMemberPreparation,
) -> Result<PreparedPrecipitationMembers<'a, 'ctx>, PrecipitationMemberError> {
    preparation.validate()?;
    if !large_scale.belongs_to(ctx) || !convective.belongs_to(ctx) {
        return Err(PrecipitationMemberError::IdentityMismatch(
            "sources belong to another GPU context",
        ));
    }
    let lsp = describe_source(large_scale, FieldId::LargeScalePrecipitation)?;
    let cp = describe_source(convective, FieldId::ConvectivePrecipitation)?;
    if lsp.horizontal_grid != cp.horizontal_grid || lsp.cell_count != cp.cell_count {
        return Err(PrecipitationMemberError::IdentityMismatch(
            "LSP and CP grids differ",
        ));
    }
    if lsp.calendar != cp.calendar {
        return Err(PrecipitationMemberError::IdentityMismatch(
            "LSP and CP calendars differ",
        ));
    }
    if lsp.source_snapshot_sha256 != cp.source_snapshot_sha256 {
        return Err(PrecipitationMemberError::IdentityMismatch(
            "LSP and CP come from different source snapshots",
        ));
    }
    if lsp.intervals != cp.intervals {
        return Err(PrecipitationMemberError::IdentityMismatch(
            "LSP and CP intervals differ",
        ));
    }
    let sequence = derive_member_sequence(&lsp.intervals)?;
    let member_count = checked_u32(sequence.members.len(), "member count")?;
    let cell_count = checked_u32(lsp.cell_count, "cell count")?;
    checked_u32(sequence.members.len() * lsp.cell_count, "member values")?;
    checked_u32(lsp.intervals.len() * lsp.cell_count, "interval values")?;
    let metadata = PrecipitationMemberMetadata {
        preparation,
        representation: PRECIPITATION_MEMBER_PREPARATION.to_string(),
        unit: RATE_HANDOFF_UNIT.to_string(),
        calendar: lsp.calendar,
        delta_seconds: sequence.delta_seconds,
        sequence_start_epoch_seconds: sequence.sequence_start_epoch_seconds,
        horizontal_grid: lsp.horizontal_grid.clone(),
        cell_count: lsp.cell_count,
        source_snapshot_sha256: lsp.source_snapshot_sha256.clone(),
        intervals: lsp.intervals.clone(),
        large_scale: source_identity(&lsp),
        convective: source_identity(&cp),
        members: sequence.members,
        gpu_adapter: GpuAdapterEvidence::from_context(ctx),
    };
    push_device_error_scopes(ctx);
    let resources = MemberResources::new(ctx, member_count, cell_count);
    finish_device_error_scopes(ctx, "member buffer creation")?;
    Ok(PreparedPrecipitationMembers {
        owner_context: Arc::clone(&ctx.identity),
        large_scale,
        convective,
        large_scale_members: resources.large_scale,
        convective_members: resources.convective,
        status: resources.status,
        params: resources.params,
        metadata,
        stages: Vec::new(),
    })
}

fn describe_source<'a>(
    source: &'a CanonicalGpuField<'_>,
    expected: FieldId,
) -> Result<IntervalRateGrid<'a>, PrecipitationMemberError> {
    let grid = source.interval_rate_grid().map_err(|_| {
        PrecipitationMemberError::UnsupportedRepresentation(
            "LSP/CP must be canonical accumulated-since-reset surface fields resolved by #90",
        )
    })?;
    if grid.field_id != expected {
        return Err(PrecipitationMemberError::FieldOrder {
            expected,
            actual: grid.field_id,
        });
    }
    if grid.source_unit != Unit::KilogramPerSquareMeter
        || grid.source_sign != SignConvention::NonNegative
    {
        return Err(PrecipitationMemberError::UnsupportedRepresentation(
            "LSP/CP accumulations must be non-negative kg/m2",
        ));
    }
    let expected_bytes = (grid.intervals.len() * grid.cell_count * size_of::<f32>()) as u64;
    if grid.rates_mm_per_h.size() != expected_bytes {
        return Err(PrecipitationMemberError::IdentityMismatch(
            "#90 rate grid size differs from its interval metadata",
        ));
    }
    Ok(grid)
}

fn source_identity(grid: &IntervalRateGrid<'_>) -> PrecipitationSourceIdentity {
    PrecipitationSourceIdentity {
        field_id: grid.field_id,
        source_unit: grid.source_unit,
        source_sign: grid.source_sign,
        source_times: grid.source_times.clone(),
        quantization_admissions: grid.quantization_admissions.to_vec(),
    }
}

struct MemberResources {
    large_scale: wgpu::Buffer,
    convective: wgpu::Buffer,
    status: wgpu::Buffer,
    params: wgpu::Buffer,
}

impl MemberResources {
    /// `member_count * cell_count` must already be bounded by `checked_u32`.
    fn new(ctx: &GpuContext, member_count: u32, cell_count: u32) -> Self {
        let members = |label| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(member_count) * u64::from(cell_count) * size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params = DarainParamsRaw {
            member_count,
            cell_count,
            _pad0: 0,
            _pad1: 0,
        };
        Self {
            large_scale: members("precipitation_members_lsp_mmh"),
            convective: members("precipitation_members_cp_mmh"),
            status: ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("precipitation_members_status"),
                size: size_of::<u32>() as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            params: ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("precipitation_members_params"),
                    contents: bytemuck::bytes_of(&params),
                    usage: wgpu::BufferUsages::UNIFORM,
                }),
        }
    }
}

impl PreparedPrecipitationMembers<'_, '_> {
    /// Typed handoff metadata, available before encoding.
    #[must_use]
    pub const fn metadata(&self) -> &PrecipitationMemberMetadata {
        &self.metadata
    }

    /// Encode #90 for LSP and CP, then `darain` for each, into the caller's encoder.
    ///
    /// Resets the status word first. Submits, polls, maps and reads back nothing;
    /// the #90 rate grids are bound in place. On error discard the encoder: no
    /// handoff is returned for partial work.
    ///
    /// # Errors
    /// Rejects another device's kernels or context and forwards encode errors.
    pub fn encode<'s>(
        &'s mut self,
        ctx: &GpuContext,
        kernels: &PrecipitationMemberKernels,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<EncodedPrecipitationMembers<'s>, PrecipitationMemberError> {
        if !Arc::ptr_eq(&self.owner_context, &ctx.identity)
            || !Arc::ptr_eq(&kernels.owner_context, &ctx.identity)
        {
            return Err(PrecipitationMemberError::IdentityMismatch(
                "prepared members or kernels belong to another GPU context",
            ));
        }
        self.stages.clear();
        encoder.clear_buffer(&self.status, 0, None);
        for source in [self.large_scale, self.convective] {
            source.encode_interval_rates(ctx, &kernels.accumulation, encoder)?;
            self.stages
                .push(PrecipitationMemberStage::AccumulatedTransform(
                    source.interval_rate_grid()?.field_id,
                ));
        }
        for (source, members) in [
            (self.large_scale, &self.large_scale_members),
            (self.convective, &self.convective_members),
        ] {
            let grid = source.interval_rate_grid()?;
            encode_darain(
                ctx,
                &kernels.darain,
                &DarainBindings {
                    rates: grid.rates_mm_per_h,
                    members,
                    status: &self.status,
                    params: &self.params,
                },
                self.metadata.members.len() * self.metadata.cell_count,
                encoder,
            )?;
            self.stages
                .push(PrecipitationMemberStage::DarainReconstruction(
                    grid.field_id,
                ));
        }
        Ok(EncodedPrecipitationMembers {
            large_scale_mm_per_h: &self.large_scale_members,
            convective_mm_per_h: &self.convective_members,
            status: &self.status,
            metadata: &self.metadata,
            stages: &self.stages,
        })
    }

    /// Read members and status back at an explicit validation boundary.
    ///
    /// Call only after the encoder from [`Self::encode`] was submitted. Never
    /// insert this between GPU stages.
    ///
    /// # Errors
    /// Forwards mapping errors.
    pub async fn download(
        &self,
        ctx: &GpuContext,
    ) -> Result<PrecipitationMemberReadback, PrecipitationMemberError> {
        let values = self.metadata.members.len() * self.metadata.cell_count;
        Ok(PrecipitationMemberReadback {
            large_scale_mm_per_h: download_buffer_typed(
                ctx,
                &self.large_scale_members,
                values,
                "precipitation_members_lsp_mmh",
            )
            .await?,
            convective_mm_per_h: download_buffer_typed(
                ctx,
                &self.convective_members,
                values,
                "precipitation_members_cp_mmh",
            )
            .await?,
            status: download_status(ctx, &self.status).await?,
            input_sha256: self.metadata.input_sha256()?,
            adapter: GpuAdapterEvidence::from_context(ctx),
        })
    }
}

/// Validation-boundary readback of a production reconstruction.
#[derive(Debug, Clone)]
pub struct PrecipitationMemberReadback {
    large_scale_mm_per_h: Vec<f32>,
    convective_mm_per_h: Vec<f32>,
    status: u32,
    input_sha256: String,
    adapter: GpuAdapterEvidence,
}

impl PrecipitationMemberReadback {
    /// LSP members in mm/h, `[slot * cell_count + cell]`.
    ///
    /// # Errors
    /// [`PrecipitationMemberError::DeviceStatus`] when the device flagged non-finite values.
    pub fn large_scale_mm_per_h(&self) -> Result<&[f32], PrecipitationMemberError> {
        self.require_valid_status()?;
        Ok(&self.large_scale_mm_per_h)
    }

    /// CP members in mm/h, same layout.
    ///
    /// # Errors
    /// [`PrecipitationMemberError::DeviceStatus`] when the device flagged non-finite values.
    pub fn convective_mm_per_h(&self) -> Result<&[f32], PrecipitationMemberError> {
        self.require_valid_status()?;
        Ok(&self.convective_mm_per_h)
    }

    /// Raw device status word.
    #[must_use]
    pub const fn status(&self) -> u32 {
        self.status
    }

    /// Adapter that executed the WGSL.
    #[must_use]
    pub const fn adapter(&self) -> &GpuAdapterEvidence {
        &self.adapter
    }

    /// Reject a nonzero device status.
    ///
    /// # Errors
    /// [`PrecipitationMemberError::DeviceStatus`] for any nonzero status.
    pub const fn require_valid_status(&self) -> Result<(), PrecipitationMemberError> {
        if self.status != 0 {
            return Err(PrecipitationMemberError::DeviceStatus(self.status));
        }
        Ok(())
    }

    /// SHA-256 of the normalized source manifest the members were built from.
    #[must_use]
    pub fn input_sha256(&self) -> &str {
        &self.input_sha256
    }

    /// Repository-wide evidence for one lane against pinned `darain` results.
    ///
    /// `field_id` selects LSP or CP; `oracle` must be in member-buffer order.
    /// Rows must additionally satisfy [`member_matches_oracle`] for a #207 pass.
    ///
    /// # Errors
    /// Rejects a nonzero device status, a field other than LSP/CP, and forwards
    /// comparison/evidence validation failures.
    pub fn evidence(
        &self,
        case_id: &str,
        field_id: FieldId,
        oracle: &[f64],
    ) -> Result<GpuCalculationEvidence, PrecipitationMemberError> {
        let candidate = match field_id {
            FieldId::LargeScalePrecipitation => self.large_scale_mm_per_h()?,
            FieldId::ConvectivePrecipitation => self.convective_mm_per_h()?,
            actual => {
                return Err(PrecipitationMemberError::FieldOrder {
                    expected: FieldId::LargeScalePrecipitation,
                    actual,
                })
            }
        };
        build_lane_evidence(
            case_id,
            &self.adapter,
            &self.input_sha256,
            oracle,
            candidate,
        )
    }
}

struct DarainBindings<'a> {
    rates: &'a wgpu::Buffer,
    members: &'a wgpu::Buffer,
    status: &'a wgpu::Buffer,
    params: &'a wgpu::Buffer,
}

fn encode_darain(
    ctx: &GpuContext,
    kernel: &DarainMemberKernel,
    bindings: &DarainBindings<'_>,
    value_count: usize,
    encoder: &mut wgpu::CommandEncoder,
) -> Result<(), PrecipitationMemberError> {
    let total = checked_u32(value_count, "member values")?;
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("darain_precipitation_member_bg"),
        layout: &kernel.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: bindings.rates.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: bindings.members.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: bindings.status.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: bindings.params.as_entire_binding(),
            },
        ],
    });
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("darain_precipitation_member_pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&kernel.pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    super::dispatch_1d(&mut pass, total, WORKGROUP_SIZE_X);
    Ok(())
}

async fn download_status(ctx: &GpuContext, status: &wgpu::Buffer) -> Result<u32, GpuBufferError> {
    let words: Vec<u32> =
        download_buffer_typed(ctx, status, 1, "precipitation_members_status").await?;
    Ok(words[0])
}

/// Validation-boundary readback of the kernel on raw interval-mean rates.
#[derive(Debug, Clone)]
pub struct DarainRateReadback {
    members_mm_per_h: Vec<f32>,
    status: u32,
    input_sha256: String,
    adapter: GpuAdapterEvidence,
}

impl DarainRateReadback {
    /// Members in mm/h, `[member * cell_count + cell]`.
    #[must_use]
    pub fn members_mm_per_h(&self) -> &[f32] {
        &self.members_mm_per_h
    }

    /// Raw device status word.
    #[must_use]
    pub const fn status(&self) -> u32 {
        self.status
    }

    /// SHA-256 of the uploaded little-endian rates.
    #[must_use]
    pub fn input_sha256(&self) -> &str {
        &self.input_sha256
    }

    /// Adapter that executed the WGSL.
    #[must_use]
    pub const fn adapter(&self) -> &GpuAdapterEvidence {
        &self.adapter
    }

    /// Reject a nonzero device status.
    ///
    /// # Errors
    /// [`PrecipitationMemberError::DeviceStatus`] for any nonzero status.
    pub const fn require_valid_status(&self) -> Result<(), PrecipitationMemberError> {
        if self.status != 0 {
            return Err(PrecipitationMemberError::DeviceStatus(self.status));
        }
        Ok(())
    }

    /// Repository-wide evidence against pinned `darain` results in member order.
    ///
    /// # Errors
    /// Rejects a nonzero device status and forwards comparison/evidence failures.
    pub fn evidence(
        &self,
        case_id: &str,
        oracle: &[f64],
    ) -> Result<GpuCalculationEvidence, PrecipitationMemberError> {
        self.require_valid_status()?;
        build_lane_evidence(
            case_id,
            &self.adapter,
            &self.input_sha256,
            oracle,
            &self.members_mm_per_h,
        )
    }
}

/// Run the reconstruction kernel on validated raw rates and read back (validation only).
///
/// For isolated oracle validation of the kernel arithmetic on declared
/// sequences, including negative rates that #90 never produces. It produces no
/// typed member metadata and is not a production path.
///
/// # Errors
/// Rejects `N < 5`, empty or mismatched sizes and non-finite rates before
/// dispatch; forwards device and readback errors.
pub async fn reconstruct_members_from_rates_gpu(
    ctx: &GpuContext,
    interval_rates_mm_per_h: &[f32],
    interval_count: usize,
    cell_count: usize,
) -> Result<DarainRateReadback, PrecipitationMemberError> {
    if interval_count < MINIMUM_INTERVAL_COUNT {
        return Err(PrecipitationMemberError::InsufficientSupport { interval_count });
    }
    if cell_count == 0 || interval_rates_mm_per_h.len() != interval_count * cell_count {
        return Err(PrecipitationMemberError::IdentityMismatch(
            "rate count differs from interval_count * cell_count",
        ));
    }
    if let Some(index) = interval_rates_mm_per_h.iter().position(|v| !v.is_finite()) {
        return Err(PrecipitationMemberError::NonFinite { index });
    }
    checked_u32(interval_rates_mm_per_h.len(), "interval values")?;
    let member_count = interval_count + 1 - SUPPORT_INTERVALS;
    checked_u32(member_count * cell_count, "member values")?;
    let kernel = DarainMemberKernel::new(ctx)?;
    push_device_error_scopes(ctx);
    let rates = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("darain_validation_rates_mmh"),
            contents: bytemuck::cast_slice(interval_rates_mm_per_h),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let resources = MemberResources::new(
        ctx,
        checked_u32(member_count, "member count")?,
        checked_u32(cell_count, "cell count")?,
    );
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("darain_validation_encoder"),
        });
    encoder.clear_buffer(&resources.status, 0, None);
    let recorded = encode_darain(
        ctx,
        &kernel,
        &DarainBindings {
            rates: &rates,
            members: &resources.large_scale,
            status: &resources.status,
            params: &resources.params,
        },
        member_count * cell_count,
        &mut encoder,
    );
    if let Err(error) = recorded {
        let _ = finish_device_error_scopes(ctx, "dispatch preparation");
        return Err(error);
    }
    ctx.queue.submit(Some(encoder.finish()));
    finish_device_error_scopes(ctx, "dispatch")?;
    Ok(DarainRateReadback {
        members_mm_per_h: download_buffer_typed(
            ctx,
            &resources.large_scale,
            member_count * cell_count,
            "darain_validation_members_mmh",
        )
        .await?,
        status: download_status(ctx, &resources.status).await?,
        input_sha256: rates_sha256(interval_rates_mm_per_h),
        adapter: GpuAdapterEvidence::from_context(ctx),
    })
}

/// Whether a member agrees with the oracle under the #71 rule `|c-o| <= 1e-6 + 1e-5|o|`.
#[must_use]
pub fn member_matches_oracle(candidate: f64, oracle: f64) -> bool {
    Tolerance::new(DEFAULT_ABS_TOL, DEFAULT_REL_TOL).allows(candidate, oracle)
}

/// The #71 tolerance expressed as the repository-wide comparison policy.
///
/// # Errors
/// Forwards an invalid policy.
pub fn precipitation_member_comparison_policy() -> Result<ComparisonPolicy, GpuEvidenceError> {
    ComparisonPolicy::new(DEFAULT_ABS_TOL, DEFAULT_REL_TOL)
}

// Private so that `wgsl_device` evidence can only be built from this module's
// own status-checked readbacks, never from caller-supplied values.
fn build_lane_evidence(
    case_id: &str,
    adapter: &GpuAdapterEvidence,
    input_sha256: &str,
    oracle: &[f64],
    candidate: &[f32],
) -> Result<GpuCalculationEvidence, PrecipitationMemberError> {
    let candidate: Vec<f64> = candidate.iter().map(|v| f64::from(*v)).collect();
    let comparison = super::compare_finite_values(
        oracle,
        &candidate,
        precipitation_member_comparison_policy()?,
    )?;
    let evidence = GpuCalculationEvidence {
        schema: super::GpuEvidenceSchema::default(),
        case_id: case_id.to_string(),
        candidate: GpuCandidateEvidence {
            implementation_id: PRECIPITATION_MEMBER_IMPLEMENTATION_ID.to_string(),
            revision: super::accumulation::accumulated_candidate_revision(),
            shader_sha256: darain_shader_sha256(),
            input_sha256: input_sha256.to_string(),
        },
        execution: GpuExecutionEvidence {
            status: GpuExecutionStatus::Passed,
            calculation_path: GpuCalculationPath::WgslDevice,
            adapter: Some(adapter.clone()),
            failure: None,
            skip_reason: None,
        },
        oracle: Some(PinnedOracleEvidence {
            implementation_id: DARAIN_ORACLE_IMPLEMENTATION_ID.to_string(),
            revision: FLEX_EXTRACT_REVISION.to_string(),
            executable_sha256: DARAIN_SOURCE_SHA256.to_string(),
            output_sha256: f64_values_sha256(oracle),
        }),
        comparison,
    };
    evidence.validate()?;
    Ok(evidence)
}

/// SHA-256 of the executed WGSL.
#[must_use]
pub fn darain_shader_sha256() -> String {
    sha256_hex(SHADER_SOURCE.as_bytes())
}

/// SHA-256 of little-endian `f32` rates.
#[must_use]
pub fn rates_sha256(values: &[f32]) -> String {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    sha256_hex(&bytes)
}

fn f64_values_sha256(values: &[f64]) -> String {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    sha256_hex(&bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn checked_u32(value: usize, field: &'static str) -> Result<u32, PrecipitationMemberError> {
    u32::try_from(value).map_err(|_| PrecipitationMemberError::ValueTooLarge { field })
}

fn push_device_error_scopes(ctx: &GpuContext) {
    ctx.device.push_error_scope(wgpu::ErrorFilter::Internal);
    ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
}

fn finish_device_error_scopes(
    ctx: &GpuContext,
    stage: &'static str,
) -> Result<(), PrecipitationMemberError> {
    let _ = ctx.device.poll(wgpu::Maintain::Wait);
    let validation = pollster::block_on(ctx.device.pop_error_scope());
    let out_of_memory = pollster::block_on(ctx.device.pop_error_scope());
    let internal = pollster::block_on(ctx.device.pop_error_scope());
    if let Some(error) = validation.or(out_of_memory).or(internal) {
        return Err(PrecipitationMemberError::Device {
            stage,
            message: error.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600;

    fn interval(start: i64, end: i64) -> AccumulatedIntervalMetadataRecord {
        AccumulatedIntervalMetadataRecord {
            start_epoch_seconds: start,
            end_epoch_seconds: end,
            duration_seconds: end - start,
            reset_applied: false,
            detected_reset: false,
        }
    }

    fn sequence(count: i64, delta: i64) -> Vec<AccumulatedIntervalMetadataRecord> {
        (0..count)
            .map(|k| interval(k * delta, (k + 1) * delta))
            .collect()
    }

    #[test]
    fn test_precipitation_members_sequence_admits_j_two_to_n_minus_two() {
        let delta = 3 * HOUR;
        let derived = derive_member_sequence(&sequence(16, delta)).expect("16 intervals admit");
        let times: Vec<i64> = derived
            .members
            .iter()
            .map(|m| m.valid_time_epoch_seconds)
            .collect();
        let expected: Vec<i64> = (2..=14).map(|j| j * delta).collect();
        assert_eq!(times, expected);
        for member in &derived.members {
            let t = member.valid_time_epoch_seconds;
            assert_eq!(member.support_start_epoch_seconds, t - 2 * delta);
            assert_eq!(member.support_end_epoch_seconds, t + 2 * delta);
            assert_eq!(member.support_intervals[1].end_epoch_seconds, t);
            assert_eq!(member.support_intervals[2].start_epoch_seconds, t);
        }
    }

    #[test]
    fn test_precipitation_members_sequence_rejects_envelope_violations() {
        let delta = 3 * HOUR;
        assert!(matches!(
            derive_member_sequence(&sequence(4, delta)),
            Err(PrecipitationMemberError::InsufficientSupport { interval_count: 4 })
        ));
        assert!(matches!(
            derive_member_sequence(&sequence(6, 1_800)),
            Err(PrecipitationMemberError::NonIntegerHourDelta { seconds: 1_800 })
        ));
        let mut gap = sequence(6, delta);
        for record in &mut gap[3..] {
            record.start_epoch_seconds += HOUR;
            record.end_epoch_seconds += HOUR;
        }
        assert!(matches!(
            derive_member_sequence(&gap),
            Err(PrecipitationMemberError::GapOrOverlap { index: 3 })
        ));
        let mut unequal = sequence(6, delta);
        unequal[4] = interval(4 * delta, 5 * delta + HOUR);
        unequal[5] = interval(5 * delta + HOUR, 6 * delta + HOUR);
        assert!(matches!(
            derive_member_sequence(&unequal),
            Err(PrecipitationMemberError::UnequalDelta { index: 4 })
        ));
        let mut inconsistent = sequence(6, delta);
        inconsistent[2].duration_seconds = delta + 1;
        assert!(matches!(
            derive_member_sequence(&inconsistent),
            Err(PrecipitationMemberError::UnequalDelta { index: 2 })
        ));
    }

    #[test]
    fn test_precipitation_members_preparation_rejects_unsupported_settings() {
        PrecipitationMemberPreparation::flex_extract_rrint0()
            .validate()
            .expect("RRINT=0 darain is supported");
        let base = PrecipitationMemberPreparation::flex_extract_rrint0();
        for invalid in [
            PrecipitationMemberPreparation { rrint: 1, ..base },
            PrecipitationMemberPreparation { purefc: 1, ..base },
            PrecipitationMemberPreparation {
                basetime_hour: Some(0),
                ..base
            },
            PrecipitationMemberPreparation { numpf: 3, ..base },
            PrecipitationMemberPreparation {
                method: DisaggregationMethod::Ia3,
                ..base
            },
            PrecipitationMemberPreparation {
                method: DisaggregationMethod::Dapoly,
                ..base
            },
        ] {
            let error = invalid.validate().expect_err("unsupported preparation");
            assert_eq!(error.rejection_code(), Some("unsupported_preparation"));
        }
    }

    #[test]
    fn test_precipitation_members_shader_hash_is_lowercase_sha256() {
        let hash = darain_shader_sha256();
        assert_eq!(hash.len(), 64);
        assert!(hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
}
