//! Field-specific canonical meteorology composition for issue #76.
//!
//! The scientific operations remain owned by #87–#90. This module only prepares
//! their resources and records their existing encode APIs. In particular,
//! instantaneous surface fields use horizontal -> temporal sampling; model
//! fields use horizontal -> vertical -> temporal sampling; single-column
//! interface W uses remap -> vertical -> temporal sampling; accumulated fields
//! use interval transformation -> horizontal sampling of an explicit product.
//! Static class fields use horizontal sampling alone.
//!
//! Horizontal/vertical order follows FLEXPART `interpol_mod.f90:1651-1703`.
//! Differing height profiles across an active horizontal stencil are unsupported:
//! #88 owns column sampling, not a new horizontal geometry interpolation rule.
//! Source upload and query preparation are explicit allocation boundaries.
//! [`PreparedMeteorologySample::encode`] never submits, waits, polls, or reads back.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{
    accumulation::{
        build_transform_inputs, encode_accumulated_intervals_gpu_with_kernel,
        AccumulatedIntervalBuffers, AccumulatedIntervalKernel, AccumulatedTransformInputs,
        GpuAccumulationError,
    },
    horizontal::{
        create_horizontal_output_buffer, create_horizontal_query_buffers,
        create_horizontal_uniform_buffer, encode_horizontal_samples, GpuHorizontalError,
        HorizontalFieldBuffers, HorizontalInterpolationKernel, HorizontalQueryBuffers,
        HorizontalSampleOutput, HorizontalUniforms,
    },
    temporal::{
        create_temporal_bracket_buffers, create_temporal_output_buffer,
        create_temporal_uniform_buffer, encode_temporal_blend, GpuTemporalError,
        TemporalBlendOutput, TemporalBlendUniforms, TemporalBracketBuffers,
        TemporalInterpolationKernel,
    },
    vertical::{
        create_vertical_grid_buffers, create_vertical_output_buffer, create_vertical_query_buffers,
        create_vertical_shared_grid, create_vertical_w_interface_inputs,
        encode_vertical_remap_w_with_kernel, encode_vertical_sample_with_kernel,
        physical_center_w_column_from_runtime, physical_model_column_from_runtime,
        physical_w_columns_from_runtime, resolve_query_heights_agl, vertical_geometry_identity,
        GpuVerticalError, VerticalGridBuffers, VerticalQueryBuffers, VerticalSampleKernel,
        VerticalSampleOutput, VerticalWInterfaceInputs, VerticalWRemapKernel,
    },
    GpuContext,
};

const VALUE_BYTES: u64 = std::mem::size_of::<f32>() as u64;
use crate::meteorology::{
    accumulation::{
        validate_interval_sequence, AccumulatedIntervalMetadata, AccumulatedObservation,
        AccumulationError,
    },
    horizontal::{horizontal_sample_geometry, HorizontalError, HorizontalSampleGeometry},
    temporal::{resolve_temporal_bracket, RequestedSampleTime, TemporalBracket, TemporalError},
    vertical::{
        NormalizedVerticalMotionProvenance, VerticalRuntimeView, VerticalTransformError,
        VerticalTransformProvenance,
    },
    ContractError, Field, FieldId, FieldTime, HorizontalGrid, HorizontalStaggering, Requirements,
    SignConvention, Snapshot, TemporalKind, VerticalReference, VerticalStaggering,
};

/// One error boundary for canonical validation, unsupported composition and GPU stages.
#[derive(Debug, Error)]
pub enum MeteorologyCompositionError {
    /// Canonical schema, dimensions, metadata or values are invalid.
    #[error(transparent)]
    Contract(#[from] ContractError),
    /// Horizontal control-plane validation failed.
    #[error(transparent)]
    HorizontalGeometry(#[from] HorizontalError),
    /// Horizontal GPU resource preparation or encoding failed.
    #[error(transparent)]
    Horizontal(#[from] GpuHorizontalError),
    /// Vertical GPU resource preparation or encoding failed.
    #[error(transparent)]
    Vertical(#[from] GpuVerticalError),
    /// Runtime geometry access failed.
    #[error(transparent)]
    Geometry(#[from] VerticalTransformError),
    /// Instantaneous time coverage or bracketing failed.
    #[error(transparent)]
    Time(#[from] TemporalError),
    /// Temporal GPU resource preparation or encoding failed.
    #[error(transparent)]
    Temporal(#[from] GpuTemporalError),
    /// Accumulated-field GPU preparation or encoding failed.
    #[error(transparent)]
    Accumulation(#[from] GpuAccumulationError),
    /// Explicit interval/reset metadata failed validation.
    #[error(transparent)]
    Interval(#[from] AccumulationError),
    /// The requested combination has no stage-owned scientific contract.
    #[error("unsupported canonical composition: {0}")]
    Unsupported(&'static str),
    /// Source metadata or resource identity does not match its owner.
    #[error("incompatible canonical composition: {0}")]
    Incompatible(&'static str),
    /// A required source, dimension, query or runtime is absent.
    #[error("missing canonical composition input: {0}")]
    Missing(&'static str),
    /// Provenance cannot be serialized.
    #[error(transparent)]
    Provenance(#[from] serde_json::Error),
}

/// Select an accumulated interval product without confusing amounts with rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntervalQuantity {
    /// Interval-integrated water-equivalent amount in kg/m2.
    Amount,
    /// Interval rate in kg/(m2 s).
    RateSi,
    /// Explicit #90 handoff rate in mm/h; no `interpol_rain` time sampling is implied.
    RateMillimeterPerHour,
}

/// Field-specific time selection. Intervals cannot masquerade as instantaneous brackets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeteorologyTimeSelection {
    /// Resolve the existing #89 instantaneous bracket semantics.
    Instantaneous(RequestedSampleTime),
    /// Time-invariant ancillary fields have no time interpolation stage.
    Static,
    /// Select an explicit #90 interval by both endpoints, without inventing time weights.
    AccumulatedInterval {
        /// Exact declared interval start.
        start_epoch_seconds: i64,
        /// Exact declared interval end.
        end_epoch_seconds: i64,
        /// Physically distinct interval product.
        quantity: IntervalQuantity,
    },
    /// Select a canonical interval-total amount, already normalized upstream.
    IntervalTotal {
        /// Exact canonical interval start.
        start_epoch_seconds: i64,
        /// Exact canonical interval end.
        end_epoch_seconds: i64,
    },
    /// Select a canonical interval-mean surface flux, already normalized upstream.
    IntervalMean {
        /// Exact canonical interval start.
        start_epoch_seconds: i64,
        /// Exact canonical interval end.
        end_epoch_seconds: i64,
    },
}

/// A height query in metres with explicit AGL/ASL semantics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeteorologyHeight {
    /// Requested height in metres.
    pub meters: f32,
    /// Height reference; model-native requests fail closed in #88.
    pub reference: VerticalReference,
}

/// One canonical grid-position query; geographic mapping belongs to #87's control plane.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeteorologySampleRequest {
    /// Canonical x grid coordinate, cell-center anchored.
    pub xt: f64,
    /// Canonical y grid coordinate, cell-center anchored.
    pub yt: f64,
    /// Required for model fields and forbidden for surface/class fields.
    pub height: Option<MeteorologyHeight>,
    /// Explicit field-class time semantics.
    pub time: MeteorologyTimeSelection,
}

/// An executed scientific stage; buffer copies are recorded separately as device transfers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeteorologyStage {
    /// Existing #87 bilinear kernel.
    Horizontal,
    /// Existing #88 interface-to-shared-grid W preprocessing kernel.
    WRemap,
    /// Existing #88 metric-height kernel.
    Vertical,
    /// Existing #89 instantaneous time kernel.
    Temporal,
    /// Existing #90 interval/reset preprocessing kernel.
    AccumulatedTransform,
}

/// One successfully encoded stage, with its source and plane identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeteorologyStageRecord {
    /// Stage actually passed to the existing encode API.
    pub stage: MeteorologyStage,
    /// Original source-series index, if applicable.
    pub source_index: Option<usize>,
    /// Physical bottom-to-top level or class index, if applicable.
    pub plane_index: Option<usize>,
}

/// Immutable downstream handoff identity, separate from numerical execution evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeteorologySampleMetadata {
    /// Canonical field identity.
    pub field_id: FieldId,
    /// Output unit (including the existing #90 derived-product unit).
    pub unit: String,
    /// Preserved canonical sign convention.
    pub sign: SignConvention,
    /// Source horizontal grid, including origin, spacing and longitude convention.
    pub horizontal_grid: HorizontalGrid,
    /// Preserved source staggering; W remapping does not erase source identity.
    pub horizontal_staggering: HorizontalStaggering,
    /// Canonical source center/interface staggering.
    pub vertical_staggering: VerticalStaggering,
    /// Exact query with time and height references.
    pub request: MeteorologySampleRequest,
    /// Resolved AGL query per selected runtime source, in metres.
    pub resolved_heights_agl_m: Vec<f32>,
    /// Number of output f32 values (one scalar or the canonical class lanes).
    pub value_count: usize,
    /// Original field time metadata for the selected source(s).
    pub source_times: Vec<FieldTime>,
    /// Hashes of the selected complete canonical snapshots.
    pub source_snapshot_sha256: Vec<String>,
    /// #30 runtime geometry identities for selected sources, when applicable.
    pub geometry_identity: Vec<String>,
    /// Complete #30 geometry provenance; consumers need not reconstruct it.
    pub geometry_provenance: Vec<VerticalTransformProvenance>,
    /// Runtime motion provenance for canonical W.
    pub motion_provenance: Vec<NormalizedVerticalMotionProvenance>,
    /// Existing instantaneous bracketing record, including original series indices.
    pub temporal_bracket: Option<TemporalBracketMetadata>,
    /// Validated #75 interval/reset identity, only for accumulated products.
    pub interval: Option<AccumulatedIntervalMetadataRecord>,
}

/// Serializable view of the existing #89 control-plane bracket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalBracketMetadata {
    /// Original source-series index of the lower member.
    pub lower_index: usize,
    /// Original source-series index of the upper member.
    pub upper_index: usize,
    /// Source timestamps and existing endpoint/interior semantics.
    pub source_timestamps: crate::meteorology::temporal::SourceTimestamps,
    /// Existing #89 temporal application.
    pub application: crate::meteorology::temporal::TemporalApplication,
    /// Existing DTS weights.
    pub weights: crate::meteorology::temporal::TemporalWeights,
}

/// Serializable view of #75's interval metadata, without recomputing its science.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccumulatedIntervalMetadataRecord {
    /// Exact interval start in epoch seconds.
    pub start_epoch_seconds: i64,
    /// Exact interval end in epoch seconds.
    pub end_epoch_seconds: i64,
    /// Validated duration in seconds.
    pub duration_seconds: i64,
    /// Fresh-run versus within-run delta branch.
    pub reset_applied: bool,
    /// Whether this interval begins a new declared run.
    pub detected_reset: bool,
}

impl From<AccumulatedIntervalMetadata> for AccumulatedIntervalMetadataRecord {
    fn from(value: AccumulatedIntervalMetadata) -> Self {
        Self {
            start_epoch_seconds: value.interval_start_epoch_seconds,
            end_epoch_seconds: value.interval_end_epoch_seconds,
            duration_seconds: value.duration_seconds,
            reset_applied: value.reset_applied,
            detected_reset: value.detected_reset,
        }
    }
}

/// Explicit reusable stage initialization; uses the existing runtime and kernels.
pub struct MeteorologyCompositionKernels<'ctx> {
    context: &'ctx GpuContext,
    horizontal: HorizontalInterpolationKernel,
    vertical: VerticalSampleKernel,
    w_remap: VerticalWRemapKernel,
    temporal: TemporalInterpolationKernel,
    accumulation: AccumulatedIntervalKernel,
}

impl<'ctx> MeteorologyCompositionKernels<'ctx> {
    /// Initialize existing pipelines once, outside command encoding.
    ///
    /// # Errors
    /// Propagates the owning stage's pipeline/adapter errors.
    pub fn new(ctx: &'ctx GpuContext) -> Result<Self, MeteorologyCompositionError> {
        Ok(Self {
            context: ctx,
            horizontal: HorizontalInterpolationKernel::new(ctx)?,
            vertical: VerticalSampleKernel::new(ctx)?,
            w_remap: VerticalWRemapKernel::new(ctx)?,
            temporal: TemporalInterpolationKernel::new(ctx)?,
            accumulation: AccumulatedIntervalKernel::new(ctx)?,
        })
    }
}

struct RuntimeColumns<'ctx> {
    view: VerticalRuntimeView<'ctx>,
    heights: Vec<Vec<f32>>,
    terrain: Vec<f32>,
    identity: String,
    provenance: VerticalTransformProvenance,
    motion: Option<NormalizedVerticalMotionProvenance>,
}

struct SourceMember<'ctx> {
    snapshot: Snapshot,
    snapshot_sha256: String,
    field: Field,
    planes: Vec<HorizontalFieldBuffers>,
    plane_values: Vec<Vec<f32>>,
    runtime: Option<RuntimeColumns<'ctx>>,
    w_inputs: Option<VerticalWInterfaceInputs>,
    shared_heights: Option<Vec<f32>>,
}

struct AccumulatedSource {
    inputs: AccumulatedTransformInputs,
    outputs: AccumulatedIntervalBuffers,
    intervals: Vec<AccumulatedIntervalMetadata>,
}

/// Canonical source resources with source/bracket lifetime, explicitly uploaded once.
///
/// Host copies retain validation/provenance only. No CPU interpolation or interval
/// arithmetic produces the sampled output. Resources are private so metadata cannot
/// be recombined with another field's buffers. Recreate this owner when sources change.
pub struct CanonicalGpuField<'ctx> {
    context: &'ctx GpuContext,
    field_id: FieldId,
    members: Vec<SourceMember<'ctx>>,
    accumulation: Option<AccumulatedSource>,
}

fn snapshot_hash(snapshot: &Snapshot) -> Result<String, serde_json::Error> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(snapshot)?)
    ))
}

impl<'ctx> CanonicalGpuField<'ctx> {
    /// Validate canonical sources and #30 identity, then explicitly upload stage resources.
    ///
    /// `runtimes` is one optional #30 view per snapshot. Model fields require it;
    /// surface/class fields reject it. W values must equal the normalized runtime
    /// values, preventing an independently supplied W lane from replacing #30 motion.
    ///
    /// # Errors
    /// Fails closed on schema/metadata/shape errors, missing fields/runtime, changing
    /// grids/staggering/coordinates, incompatible runtime identity or unsupported paths.
    pub fn upload(
        ctx: &'ctx GpuContext,
        field_id: FieldId,
        snapshots: &[&Snapshot],
        runtimes: &[Option<VerticalRuntimeView<'ctx>>],
    ) -> Result<Self, MeteorologyCompositionError> {
        if snapshots.is_empty() || snapshots.len() != runtimes.len() {
            return Err(MeteorologyCompositionError::Missing(
                "sources or matching runtime slots",
            ));
        }
        let requirements = Requirements {
            required_fields: [field_id].into_iter().collect(),
        };
        let mut members = Vec::with_capacity(snapshots.len());
        for (snapshot, runtime) in snapshots.iter().zip(runtimes) {
            snapshot.validate(&requirements)?;
            let field = snapshot
                .fields
                .iter()
                .find(|f| f.id == field_id)
                .ok_or(MeteorologyCompositionError::Missing("canonical field"))?;
            if field.horizontal_staggering != HorizontalStaggering::CellCenter {
                return Err(MeteorologyCompositionError::Unsupported(
                    "#87 only supports cell-center horizontal fields",
                ));
            }
            if snapshot.horizontal_grid != snapshots[0].horizontal_grid
                || snapshot.vertical_coordinate != snapshots[0].vertical_coordinate
            {
                return Err(MeteorologyCompositionError::Incompatible(
                    "source grid or vertical coordinate changes",
                ));
            }
            if let Some(first) = members.first() {
                let first: &SourceMember = first;
                if field.shape != first.field.shape
                    || field.vertical_staggering != first.field.vertical_staggering
                    || field.time.kind != first.field.time.kind
                    || field.time.calendar != first.field.time.calendar
                {
                    return Err(MeteorologyCompositionError::Incompatible(
                        "field class, shape, staggering or calendar changes",
                    ));
                }
            }
            members.push(upload_member(ctx, snapshot, field, *runtime)?);
        }
        match members[0].field.time.kind {
            TemporalKind::Instantaneous => {
                let request = RequestedSampleTime::new(
                    members[0].field.time.calendar,
                    members[0].field.time.valid_time_epoch_seconds,
                );
                resolve_temporal_bracket(field_id, snapshots, request)?;
            }
            TemporalKind::Static if members.len() != 1 => {
                return Err(MeteorologyCompositionError::Incompatible(
                    "static sources require exactly one snapshot",
                ));
            }
            TemporalKind::IntervalMean
            | TemporalKind::IntervalTotal
            | TemporalKind::AccumulatedSinceReset
            | TemporalKind::Static => {}
        }
        let accumulation = if members[0].field.time.kind == TemporalKind::AccumulatedSinceReset {
            Some(upload_accumulation(ctx, &members)?)
        } else {
            None
        };
        Ok(Self {
            context: ctx,
            field_id,
            members,
            accumulation,
        })
    }

    /// Prepare one query using existing stage resources, outside command encoding.
    ///
    /// The returned plan borrows this source owner, and owns query/intermediate/output
    /// resources. Keep it alive through the last submitted consumer. Class fields return
    /// canonical class-index order; all other supported fields return one scalar.
    ///
    /// # Errors
    /// Rejects unsupported time/height combinations, absent coverage, nonuniform active
    /// geometry, mismatched runtime/device identity and stage preparation failures.
    pub fn prepare_sample(
        &self,
        ctx: &GpuContext,
        request: MeteorologySampleRequest,
    ) -> Result<PreparedMeteorologySample<'_, 'ctx>, MeteorologyCompositionError> {
        if !std::ptr::eq(self.context, ctx) {
            return Err(MeteorologyCompositionError::Incompatible(
                "source belongs to another GPU device",
            ));
        }
        let (indices, bracket, interval_index, quantity) = self.select_sources(request.time)?;
        let mut spatial = Vec::with_capacity(indices.len());
        for index in &indices {
            spatial.push(prepare_spatial(
                ctx,
                &self.members[*index],
                request,
                quantity,
            )?);
        }
        let value_count = spatial[0].value_count();
        let temporal = if let Some(bracket) = &bracket {
            let mut device_bracket = bracket.clone();
            device_bracket.element_count = value_count;
            // These are private device-copy destinations, never usable until both
            // spatial producers are encoded. No sampled host value is uploaded here.
            let buffers = create_temporal_bracket_buffers(
                ctx,
                &vec![0.0; value_count],
                &vec![0.0; value_count],
                bracket.source_timestamps.lower_epoch_seconds,
                bracket.source_timestamps.upper_epoch_seconds,
            )?;
            Some(TemporalResources {
                buffers,
                uniforms: create_temporal_uniform_buffer(ctx, &device_bracket)?,
                output: create_temporal_output_buffer(ctx, value_count)?,
                bracket: device_bracket,
            })
        } else {
            None
        };
        let metadata = self.handoff_metadata(
            request,
            &indices,
            bracket.as_ref(),
            interval_index,
            quantity,
            &spatial,
        )?;
        Ok(PreparedMeteorologySample {
            source: self,
            indices,
            spatial,
            temporal,
            interval_index,
            quantity,
            metadata,
            stages: Vec::new(),
            device_copies: 0,
        })
    }
    fn handoff_metadata(
        &self,
        request: MeteorologySampleRequest,
        indices: &[usize],
        bracket: Option<&TemporalBracket>,
        interval_index: Option<usize>,
        quantity: Option<IntervalQuantity>,
        spatial: &[SpatialResources],
    ) -> Result<MeteorologySampleMetadata, MeteorologyCompositionError> {
        let first = &self.members[indices[0]];
        let value_count = spatial[0].value_count();
        let unit = match quantity {
            Some(IntervalQuantity::Amount) => {
                crate::meteorology::accumulation::SOURCE_UNIT.to_string()
            }
            Some(IntervalQuantity::RateSi) => {
                crate::meteorology::accumulation::RATE_SI_UNIT.to_string()
            }
            Some(IntervalQuantity::RateMillimeterPerHour) => {
                crate::meteorology::accumulation::RATE_HANDOFF_UNIT.to_string()
            }
            None => serde_json::to_value(first.field.unit)?
                .as_str()
                .ok_or(MeteorologyCompositionError::Incompatible(
                    "canonical unit encoding",
                ))?
                .to_string(),
        };
        let selected: Vec<_> = indices.iter().map(|i| &self.members[*i]).collect();
        Ok(MeteorologySampleMetadata {
            field_id: self.field_id,
            unit,
            sign: first.field.sign,
            horizontal_grid: first.snapshot.horizontal_grid.clone(),
            horizontal_staggering: first.field.horizontal_staggering,
            vertical_staggering: first.field.vertical_staggering,
            request,
            resolved_heights_agl_m: spatial.iter().filter_map(|s| s.query_agl_m).collect(),
            value_count,
            source_times: selected.iter().map(|s| s.field.time.clone()).collect(),
            // Interval deltas depend on the complete declared observation sequence.
            source_snapshot_sha256: if quantity.is_some() {
                self.members
                    .iter()
                    .map(|s| s.snapshot_sha256.clone())
                    .collect()
            } else {
                selected.iter().map(|s| s.snapshot_sha256.clone()).collect()
            },
            geometry_identity: selected
                .iter()
                .filter_map(|s| s.runtime.as_ref().map(|r| r.identity.clone()))
                .collect(),
            geometry_provenance: selected
                .iter()
                .filter_map(|s| s.runtime.as_ref().map(|r| r.provenance.clone()))
                .collect(),
            motion_provenance: selected
                .iter()
                .filter_map(|s| s.runtime.as_ref().and_then(|r| r.motion.clone()))
                .collect(),
            temporal_bracket: bracket.as_ref().map(|b| TemporalBracketMetadata {
                lower_index: b.lower_index,
                upper_index: b.upper_index,
                source_timestamps: b.source_timestamps,
                application: b.application,
                weights: b.weights,
            }),
            interval: interval_index
                .map(|i| {
                    self.accumulation
                        .as_ref()
                        .ok_or(MeteorologyCompositionError::Missing("interval resources"))?
                        .intervals
                        .get(i)
                        .copied()
                        .map(Into::into)
                        .ok_or(MeteorologyCompositionError::Missing("selected interval"))
                })
                .transpose()?,
        })
    }
}

fn upload_member<'ctx>(
    ctx: &GpuContext,
    snapshot: &Snapshot,
    field: &Field,
    runtime: Option<VerticalRuntimeView<'ctx>>,
) -> Result<SourceMember<'ctx>, MeteorologyCompositionError> {
    let hash = snapshot_hash(snapshot)?;
    let grid = &snapshot.horizontal_grid;
    let horizontal_count = grid.nx * grid.ny;
    let mut columns = None;
    let mut w_inputs = None;
    let mut shared_heights = None;
    let plane_values = if field.vertical_staggering == VerticalStaggering::NotApplicable {
        if runtime.is_some() {
            return Err(MeteorologyCompositionError::Incompatible(
                "surface/class fields do not use runtime vertical geometry",
            ));
        }
        field
            .values
            .chunks_exact(horizontal_count)
            .map(<[f32]>::to_vec)
            .collect::<Vec<_>>()
    } else {
        let runtime =
            runtime.ok_or(MeteorologyCompositionError::Missing("#30 runtime geometry"))?;
        validate_runtime_binding(snapshot, field, runtime, &hash)?;
        let mut heights = Vec::with_capacity(horizontal_count);
        let mut terrain = Vec::with_capacity(horizontal_count);
        let mut values = vec![Vec::with_capacity(horizontal_count); runtime.dimensions().2];
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                let (column_heights, lane) =
                    if field.vertical_staggering == VerticalStaggering::LevelInterface {
                        let (interfaces, motion, levels, shared) =
                            physical_w_columns_from_runtime(runtime, x, y)?;
                        w_inputs = Some(create_vertical_w_interface_inputs(
                            ctx,
                            &interfaces,
                            &motion,
                            &levels,
                        )?);
                        shared_heights = Some(shared.clone());
                        (shared, Vec::new())
                    } else if field.id == FieldId::VerticalVelocity {
                        physical_center_w_column_from_runtime(runtime, x, y)?
                    } else {
                        physical_model_column_from_runtime(runtime, field.id, &field.values, x, y)?
                    };
                heights.push(column_heights);
                terrain.push(runtime.terrain_asl_m(x, y)?);
                for (plane, value) in values.iter_mut().zip(lane) {
                    plane.push(value);
                }
            }
        }
        columns = Some(RuntimeColumns {
            view: runtime,
            heights,
            terrain,
            identity: vertical_geometry_identity(runtime),
            provenance: runtime.provenance().clone(),
            motion: runtime.vertical_velocity().map(|v| v.provenance().clone()),
        });
        if w_inputs.is_some() {
            Vec::new()
        } else {
            values
        }
    };
    let planes = plane_values
        .iter()
        .map(|values| {
            HorizontalFieldBuffers::from_grid_and_values(
                ctx,
                grid,
                values,
                field.horizontal_staggering,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SourceMember {
        snapshot: snapshot.clone(),
        snapshot_sha256: hash,
        field: field.clone(),
        planes,
        plane_values,
        runtime: columns,
        w_inputs,
        shared_heights,
    })
}

fn validate_runtime_binding(
    snapshot: &Snapshot,
    field: &Field,
    runtime: VerticalRuntimeView<'_>,
    hash: &str,
) -> Result<(), MeteorologyCompositionError> {
    let grid = &snapshot.horizontal_grid;
    if runtime.provenance().source_snapshot_sha256 != hash
        || runtime.dimensions()
            != (
                grid.nx,
                grid.ny,
                snapshot.vertical_coordinate.level_values.len(),
            )
    {
        return Err(MeteorologyCompositionError::Incompatible(
            "runtime is not derived from this canonical snapshot",
        ));
    }
    if field.id == FieldId::VerticalVelocity {
        let motion = runtime
            .vertical_velocity()
            .ok_or(MeteorologyCompositionError::Missing(
                "normalized #30 vertical motion",
            ))?;
        if field.vertical_staggering != motion.vertical_staggering()
            || field.values != motion.values_ms()
        {
            return Err(MeteorologyCompositionError::Incompatible(
                "canonical W differs from normalized runtime motion",
            ));
        }
    }
    Ok(())
}

fn upload_accumulation(
    ctx: &GpuContext,
    members: &[SourceMember],
) -> Result<AccumulatedSource, MeteorologyCompositionError> {
    let cells = members[0].field.values.len();
    let mut inputs = vec![Vec::with_capacity(cells); members.len()];
    let mut intervals = Vec::new();
    for cell in 0..cells {
        let observations = members
            .iter()
            .map(|member| {
                let reset = member.field.time.accumulation.as_ref().ok_or(
                    MeteorologyCompositionError::Missing("accumulation reset metadata"),
                )?;
                Ok(AccumulatedObservation {
                    valid_time_epoch_seconds: member.field.time.valid_time_epoch_seconds,
                    reset_epoch_seconds: reset.reset_epoch_seconds,
                    accumulated_amount_kg_per_square_meter: f64::from(member.field.values[cell]),
                })
            })
            .collect::<Result<Vec<_>, MeteorologyCompositionError>>()?;
        let metadata = validate_interval_sequence(&observations)?;
        if cell == 0 {
            intervals = metadata;
        } else if intervals != metadata {
            return Err(MeteorologyCompositionError::Incompatible(
                "interval metadata differs across cells",
            ));
        }
        for (plane, input) in inputs
            .iter_mut()
            .zip(build_transform_inputs(&observations)?)
        {
            plane.push(input);
        }
    }
    let inputs: Vec<_> = inputs.into_iter().flatten().collect();
    Ok(AccumulatedSource {
        outputs: AccumulatedIntervalBuffers::new(ctx, inputs.len())?,
        inputs: AccumulatedTransformInputs::from_inputs(ctx, &inputs)?,
        intervals,
    })
}

type SourceSelection = (
    Vec<usize>,
    Option<TemporalBracket>,
    Option<usize>,
    Option<IntervalQuantity>,
);

impl CanonicalGpuField<'_> {
    fn select_sources(
        &self,
        selection: MeteorologyTimeSelection,
    ) -> Result<SourceSelection, MeteorologyCompositionError> {
        let kind = self.members[0].field.time.kind;
        match (kind, selection) {
            (TemporalKind::Instantaneous, MeteorologyTimeSelection::Instantaneous(request)) => {
                let snapshots: Vec<_> = self.members.iter().map(|m| &m.snapshot).collect();
                let bracket = resolve_temporal_bracket(self.field_id, &snapshots, request)?;
                Ok((
                    vec![bracket.lower_index, bracket.upper_index],
                    Some(bracket),
                    None,
                    None,
                ))
            }
            (TemporalKind::Static, MeteorologyTimeSelection::Static) => {
                Ok((vec![0], None, None, None))
            }
            (
                TemporalKind::AccumulatedSinceReset,
                MeteorologyTimeSelection::AccumulatedInterval {
                    start_epoch_seconds,
                    end_epoch_seconds,
                    quantity,
                },
            ) => {
                let source = self
                    .accumulation
                    .as_ref()
                    .ok_or(MeteorologyCompositionError::Missing("interval resources"))?;
                let index = source
                    .intervals
                    .iter()
                    .position(|i| {
                        i.interval_start_epoch_seconds == start_epoch_seconds
                            && i.interval_end_epoch_seconds == end_epoch_seconds
                    })
                    .ok_or(MeteorologyCompositionError::Missing(
                        "exact accumulated interval coverage",
                    ))?;
                Ok((vec![index], None, Some(index), Some(quantity)))
            }
            (
                TemporalKind::IntervalMean,
                MeteorologyTimeSelection::IntervalMean {
                    start_epoch_seconds,
                    end_epoch_seconds,
                },
            )
            | (
                TemporalKind::IntervalTotal,
                MeteorologyTimeSelection::IntervalTotal {
                    start_epoch_seconds,
                    end_epoch_seconds,
                },
            ) => {
                let matches: Vec<_> = self
                    .members
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| {
                        m.field.time.interval_start_epoch_seconds == Some(start_epoch_seconds)
                            && m.field.time.interval_end_epoch_seconds == Some(end_epoch_seconds)
                    })
                    .map(|(i, _)| i)
                    .collect();
                if matches.len() != 1 {
                    return Err(MeteorologyCompositionError::Incompatible(
                        "interval coverage is absent or ambiguous",
                    ));
                }
                Ok((matches, None, None, None))
            }
            _ => Err(MeteorologyCompositionError::Unsupported(
                "time selection does not match the field class",
            )),
        }
    }
}

struct HorizontalResources {
    queries: HorizontalQueryBuffers,
    output: HorizontalSampleOutput,
    uniforms: HorizontalUniforms,
}

struct SpatialResources {
    query_agl_m: Option<f32>,
    horizontal: Vec<HorizontalResources>,
    // Accumulated products need a device-copy destination before #87 sampling.
    interval_field: Option<HorizontalFieldBuffers>,
    vertical_grid: Option<VerticalGridBuffers>,
    vertical_query: Option<VerticalQueryBuffers>,
    vertical_output: Option<VerticalSampleOutput>,
    output: TemporalBlendOutput,
}

impl SpatialResources {
    fn value_count(&self) -> usize {
        self.output.element_count()
    }
}

fn prepare_spatial(
    ctx: &GpuContext,
    member: &SourceMember,
    request: MeteorologySampleRequest,
    quantity: Option<IntervalQuantity>,
) -> Result<SpatialResources, MeteorologyCompositionError> {
    let grid = &member.snapshot.horizontal_grid;
    let scratch = vec![0.0; grid.nx * grid.ny];
    let stencil = horizontal_sample_geometry(
        grid,
        member.plane_values.first().unwrap_or(&scratch),
        member.field.horizontal_staggering,
        request.xt,
        request.yt,
    )?;
    let mut horizontal = Vec::new();
    let interval_field = if quantity.is_some() {
        // Finite source/reset validation already ran in #90. The full plane is
        // overwritten by a device copy before #87 can consume this private resource.
        Some(HorizontalFieldBuffers::from_grid_and_values(
            ctx,
            grid,
            &scratch,
            member.field.horizontal_staggering,
        )?)
    } else {
        None
    };
    let horizontal_planes: Vec<&[f32]> = if quantity.is_some() {
        vec![&scratch]
    } else {
        member.plane_values.iter().map(Vec::as_slice).collect()
    };
    for values in horizontal_planes {
        horizontal.push(HorizontalResources {
            queries: create_horizontal_query_buffers(
                ctx,
                grid,
                values,
                member.field.horizontal_staggering,
                &[(request.xt, request.yt)],
            )?
            .0,
            output: create_horizontal_output_buffer(ctx, 1)?,
            uniforms: create_horizontal_uniform_buffer(ctx, grid, 1)?,
        });
    }
    let (vertical_grid, vertical_query, vertical_output, query_agl_m) =
        if let Some(runtime) = &member.runtime {
            let height = request.height.ok_or(MeteorologyCompositionError::Missing(
                "model field height query",
            ))?;
            let column = compatible_column(runtime, grid, &stencil, height.reference)?;
            let query_agl = resolve_query_heights_agl(
                runtime.view,
                column % grid.nx,
                column / grid.nx,
                &[height.meters],
                height.reference,
            )?[0];
            let heights = &runtime.heights[column];
            let vertical_grid = if let Some(shared) = &member.shared_heights {
                create_vertical_shared_grid(ctx, shared)?
            } else {
                create_vertical_grid_buffers(ctx, heights, &vec![0.0; heights.len()])?
            };
            (
                Some(vertical_grid),
                Some(create_vertical_query_buffers(ctx, &[query_agl])?),
                Some(create_vertical_output_buffer(ctx, 1)?),
                Some(query_agl),
            )
        } else {
            if request.height.is_some() {
                return Err(MeteorologyCompositionError::Unsupported(
                    "height query for surface/class field",
                ));
            }
            (None, None, None, None)
        };
    let count = if vertical_output.is_some() {
        1
    } else {
        horizontal.len()
    };
    Ok(SpatialResources {
        query_agl_m,
        horizontal,
        interval_field,
        vertical_grid,
        vertical_query,
        vertical_output,
        output: create_temporal_output_buffer(ctx, count)?,
    })
}

#[allow(clippy::float_cmp)] // Exact geometry identity, not an algorithm-specific tolerance.
fn compatible_column(
    runtime: &RuntimeColumns,
    grid: &HorizontalGrid,
    stencil: &HorizontalSampleGeometry,
    reference: VerticalReference,
) -> Result<usize, MeteorologyCompositionError> {
    let corners = [
        (stencil.ix, stencil.jy),
        (stencil.ixp % grid.nx, stencil.jy),
        (stencil.ix, stencil.jyp),
        (stencil.ixp % grid.nx, stencil.jyp),
    ];
    let column = corners[0].0 + grid.nx * corners[0].1;
    for ((x, y), weight) in corners.into_iter().zip(stencil.weights) {
        if weight == 0.0 {
            continue;
        }
        let other = x + grid.nx * y;
        if runtime.heights[other] != runtime.heights[column]
            || (reference == VerticalReference::AboveMeanSeaLevel
                && runtime.terrain[other] != runtime.terrain[column])
        {
            return Err(MeteorologyCompositionError::Unsupported("active stencil has differing #30 geometry; no horizontal geometry rule is owned by #88"));
        }
    }
    Ok(column)
}

struct TemporalResources {
    buffers: TemporalBracketBuffers,
    uniforms: TemporalBlendUniforms,
    output: TemporalBlendOutput,
    bracket: TemporalBracket,
}

/// Prepared query owner; intermediate GPU resources stay alive with the source borrow.
pub struct PreparedMeteorologySample<'a, 'ctx> {
    source: &'a CanonicalGpuField<'ctx>,
    indices: Vec<usize>,
    spatial: Vec<SpatialResources>,
    temporal: Option<TemporalResources>,
    interval_index: Option<usize>,
    quantity: Option<IntervalQuantity>,
    metadata: MeteorologySampleMetadata,
    stages: Vec<MeteorologyStageRecord>,
    device_copies: usize,
}

/// Borrowed device-side handoff for #77 consumers. Encoding is not an execution verdict.
///
/// Values are `STORAGE/COPY_SRC` f32 lanes in canonical class-index order, or one
/// scalar. The plan and source owner must outlive the last submitted GPU consumer.
pub struct EncodedMeteorologySample<'a> {
    /// Bind directly to the next GPU consumer; download only at an explicit host boundary.
    pub values: &'a wgpu::Buffer,
    /// Preserved canonical quantity/query/provenance identity.
    pub metadata: &'a MeteorologySampleMetadata,
    /// Actual successful encode calls in order; execution still needs caller submission.
    pub stages: &'a [MeteorologyStageRecord],
    /// Number of encoder-local device copies between resources; no host transfers.
    pub device_copies: usize,
}

impl PreparedMeteorologySample<'_, '_> {
    /// Encode the applicable existing stages into the caller's encoder and expose their output.
    ///
    /// Does not submit, poll, wait, map, download or initialize pipelines. All
    /// intermediate transfers are encoder-local device copies. The caller owns
    /// scoped device errors and submission/completion at the real consumer boundary.
    /// On any error discard the encoder; no handoff is returned for partial work.
    ///
    /// # Errors
    /// Returns the explicit composition error on device mismatch or stage encode failure.
    pub fn encode<'a>(
        &'a mut self,
        ctx: &GpuContext,
        kernels: &MeteorologyCompositionKernels<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<EncodedMeteorologySample<'a>, MeteorologyCompositionError> {
        if !std::ptr::eq(self.source.context, ctx) || !std::ptr::eq(kernels.context, ctx) {
            return Err(MeteorologyCompositionError::Incompatible(
                "source/plan/kernels belong to another GPU device",
            ));
        }
        self.stages.clear();
        self.device_copies = 0;
        if let Some(source) = &self.source.accumulation {
            encode_accumulated_intervals_gpu_with_kernel(
                ctx,
                &source.inputs,
                &source.outputs,
                &kernels.accumulation,
                encoder,
            )?;
            self.stages.push(MeteorologyStageRecord {
                stage: MeteorologyStage::AccumulatedTransform,
                source_index: None,
                plane_index: None,
            });
        }
        for (spatial, source_index) in self.spatial.iter().zip(&self.indices) {
            let member = &self.source.members[*source_index];
            if let Some(field) = &spatial.interval_field {
                let source = self
                    .source
                    .accumulation
                    .as_ref()
                    .ok_or(MeteorologyCompositionError::Missing("interval resources"))?;
                let buffer = match self
                    .quantity
                    .ok_or(MeteorologyCompositionError::Missing("interval quantity"))?
                {
                    IntervalQuantity::Amount => &source.outputs.amount_kg_per_square_meter,
                    IntervalQuantity::RateSi => &source.outputs.rate_si,
                    IntervalQuantity::RateMillimeterPerHour => {
                        &source.outputs.rate_millimeter_per_hour
                    }
                };
                let bytes = field.element_count() as u64 * VALUE_BYTES;
                encoder.copy_buffer_to_buffer(
                    buffer,
                    self.interval_index
                        .ok_or(MeteorologyCompositionError::Missing("interval index"))?
                        as u64
                        * bytes,
                    &field.buffer,
                    0,
                    bytes,
                );
                self.device_copies += 1;
            }
            let (stages, copies) = spatial.encode(ctx, kernels, member, *source_index, encoder)?;
            self.stages.extend(stages);
            self.device_copies += copies;
        }
        let values = if let Some(temporal) = &self.temporal {
            let bytes = temporal.output.element_count() as u64 * VALUE_BYTES;
            encoder.copy_buffer_to_buffer(
                &self.spatial[0].output.buffer,
                0,
                &temporal.buffers.lower_buffer,
                0,
                bytes,
            );
            encoder.copy_buffer_to_buffer(
                &self.spatial[1].output.buffer,
                0,
                &temporal.buffers.upper_buffer,
                0,
                bytes,
            );
            self.device_copies += 2;
            encode_temporal_blend(
                ctx,
                &temporal.buffers,
                &temporal.output,
                &temporal.uniforms,
                &temporal.bracket,
                &kernels.temporal,
                encoder,
            )?;
            self.stages.push(MeteorologyStageRecord {
                stage: MeteorologyStage::Temporal,
                source_index: None,
                plane_index: None,
            });
            &temporal.output.buffer
        } else {
            &self.spatial[0].output.buffer
        };
        Ok(EncodedMeteorologySample {
            values,
            metadata: &self.metadata,
            stages: &self.stages,
            device_copies: self.device_copies,
        })
    }
}

impl SpatialResources {
    fn encode(
        &self,
        ctx: &GpuContext,
        kernels: &MeteorologyCompositionKernels<'_>,
        member: &SourceMember<'_>,
        source_index: usize,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(Vec<MeteorologyStageRecord>, usize), MeteorologyCompositionError> {
        let mut stages = Vec::new();
        let mut device_copies = 0;
        if let Some(inputs) = &member.w_inputs {
            encode_vertical_remap_w_with_kernel(
                ctx,
                inputs,
                self.vertical_grid
                    .as_ref()
                    .ok_or(MeteorologyCompositionError::Missing("W remap shared grid"))?,
                &kernels.w_remap,
                encoder,
            )?;
            stages.push(MeteorologyStageRecord {
                stage: MeteorologyStage::WRemap,
                source_index: Some(source_index),
                plane_index: None,
            });
        }
        for (plane_index, horizontal) in self.horizontal.iter().enumerate() {
            let field = self
                .interval_field
                .as_ref()
                .unwrap_or_else(|| &member.planes[plane_index]);
            encode_horizontal_samples(
                ctx,
                field,
                &horizontal.queries,
                &horizontal.output,
                &horizontal.uniforms,
                &kernels.horizontal,
                encoder,
            )?;
            stages.push(MeteorologyStageRecord {
                stage: MeteorologyStage::Horizontal,
                source_index: Some(source_index),
                plane_index: Some(plane_index),
            });
            let destination = self
                .vertical_grid
                .as_ref()
                .map_or(&self.output.buffer, |grid| &grid.values);
            encoder.copy_buffer_to_buffer(
                &horizontal.output.buffer,
                0,
                destination,
                plane_index as u64 * VALUE_BYTES,
                4,
            );
            device_copies += 1;
        }
        if let (Some(grid), Some(query), Some(output)) = (
            &self.vertical_grid,
            &self.vertical_query,
            &self.vertical_output,
        ) {
            encode_vertical_sample_with_kernel(
                ctx,
                grid,
                query,
                output,
                &kernels.vertical,
                encoder,
            )?;
            stages.push(MeteorologyStageRecord {
                stage: MeteorologyStage::Vertical,
                source_index: Some(source_index),
                plane_index: None,
            });
            encoder.copy_buffer_to_buffer(&output.buffer, 0, &self.output.buffer, 0, VALUE_BYTES);
            device_copies += 1;
        }
        Ok((stages, device_copies))
    }
}
