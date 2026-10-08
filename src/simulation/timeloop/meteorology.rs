//! Explicit legacy and validated canonical meteorology preparation handoffs.

use crate::gpu::meteorology::{
    snapshot_hash, validate_runtime_binding, CanonicalGpuField, MeteorologyCompositionError,
    MeteorologyTimeSelection,
};
use crate::gpu::{vertical_geometry_identity, GpuContext};
use crate::meteorology::{
    temporal::{resolve_temporal_bracket, RequestedSampleTime},
    vertical::{
        NormalizedVerticalMotionProvenance, VerticalRuntimeView, VerticalTransformProvenance,
    },
    FieldId, HorizontalGrid, HorizontalStaggering, Requirements, Snapshot, VerticalStaggering,
};
use crate::wind::{SurfaceFields, WindField3D};
use serde::Serialize;
use std::sync::Arc;

/// The exact scalar components prepared for the subsequent #112 migration.
pub const CANONICAL_WIND_FIELDS: [FieldId; 3] =
    [FieldId::WindU, FieldId::WindV, FieldId::VerticalVelocity];

/// Per-step meteorological bracket used for temporal interpolation (IO-04).
pub struct MetTimeBracket<'a> {
    /// Validated #173 U/V/normalized-W owners; required by production advection.
    pub canonical: Option<&'a CanonicalMeteorologyResources<'a>>,
    /// 3-D met field at lower timestamp bound.
    pub wind_t0: &'a WindField3D,
    /// 3-D met field at upper timestamp bound.
    pub wind_t1: &'a WindField3D,
    /// 2-D surface field at lower timestamp bound.
    pub surface_t0: &'a SurfaceFields,
    /// 2-D surface field at upper timestamp bound.
    pub surface_t1: &'a SurfaceFields,
    /// Lower timestamp bound [s since epoch].
    pub time_t0_seconds: i64,
    /// Upper timestamp bound [s since epoch].
    pub time_t1_seconds: i64,
}

/// Immutable source identity shared by all three canonical field owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalMeteorologyIdentity {
    /// Exact U/V/W field identities, in owner order.
    pub field_ids: [FieldId; 3],
    /// Complete canonical source hashes in ascending validity-time order.
    pub source_snapshot_sha256: [String; 2],
    /// Source validity times, including the canonical calendar.
    pub source_times: [RequestedSampleTime; 2],
    /// Exact #30 runtime identities in the same source order.
    pub geometry_identity: [String; 2],
    /// Complete geometry lineage; no consumer needs to infer height semantics.
    pub geometry_provenance: [VerticalTransformProvenance; 2],
    /// Native-to-geometric W lineage, including native input hashes.
    pub motion_provenance: [NormalizedVerticalMotionProvenance; 2],
}

/// Caller-owned, canonical-valid source pair and its exact derived #30 geometry.
///
/// Construction validates all three components before any device allocation or
/// physics submission. The borrows prevent source mutation during resource use.
/// Legacy wind fields are never accepted or converted at this boundary.
/// A fixed pair makes missing runtime slots or insufficient member counts
/// unrepresentable; duplicate/reversed validity times still fail explicitly.
#[derive(Clone)]
pub struct CanonicalMeteorologyBracket<'source> {
    snapshots: [&'source Snapshot; 2],
    runtimes: [VerticalRuntimeView<'source>; 2],
    identity: CanonicalMeteorologyIdentity,
}

impl<'source> CanonicalMeteorologyBracket<'source> {
    /// Validate provider-independent canonical inputs without uploading or sampling.
    ///
    /// # Errors
    /// Reuses #29/#30/#76/#89 errors for invalid sources, chronology or geometry.
    /// Interface-W is explicitly unsupported by this advection-ready v1 handoff.
    pub fn new(
        snapshots: [&'source Snapshot; 2],
        runtimes: [VerticalRuntimeView<'source>; 2],
    ) -> Result<Self, MeteorologyCompositionError> {
        for snapshot in snapshots {
            snapshot.validate(&Requirements::advection())?;
            for field in snapshot
                .fields
                .iter()
                .filter(|f| CANONICAL_WIND_FIELDS.contains(&f.id))
            {
                if field.horizontal_staggering != HorizontalStaggering::CellCenter
                    || field.vertical_staggering != VerticalStaggering::LevelCenter
                {
                    return Err(MeteorologyCompositionError::Unsupported(
                        "canonical driver v1 requires cell-center U/V and normalized model-center W; interface-W is unsupported",
                    ));
                }
            }
        }
        if snapshots[0].horizontal_grid != snapshots[1].horizontal_grid
            || snapshots[0].vertical_coordinate != snapshots[1].vertical_coordinate
        {
            return Err(MeteorologyCompositionError::Incompatible(
                "canonical bracket grid or vertical coordinate changes",
            ));
        }
        let hashes = [snapshot_hash(snapshots[0])?, snapshot_hash(snapshots[1])?];
        let mut times = Vec::with_capacity(2);
        let mut motions = Vec::with_capacity(2);
        for ((snapshot, runtime), hash) in snapshots.iter().zip(runtimes).zip(&hashes) {
            for id in CANONICAL_WIND_FIELDS {
                let field = snapshot.fields.iter().find(|f| f.id == id).ok_or(
                    MeteorologyCompositionError::Missing("canonical wind component"),
                )?;
                validate_runtime_binding(snapshot, field, runtime, hash)?;
                if id == FieldId::WindU {
                    times.push(RequestedSampleTime::new(
                        field.time.calendar,
                        field.time.valid_time_epoch_seconds,
                    ));
                }
            }
            if runtime.dimensions().2 < 2 {
                return Err(MeteorologyCompositionError::Unsupported(
                    "#171 resident model fields require at least two levels",
                ));
            }
            let motion =
                runtime
                    .vertical_velocity()
                    .ok_or(MeteorologyCompositionError::Missing(
                        "normalized #30 center-W",
                    ))?;
            if motion.provenance().source_id.is_empty() {
                return Err(MeteorologyCompositionError::Missing(
                    "#30 native motion source identity",
                ));
            }
            motions.push(motion.provenance().clone());
        }
        for id in CANONICAL_WIND_FIELDS {
            resolve_temporal_bracket(id, &snapshots, times[0])?;
        }
        Ok(Self {
            snapshots,
            runtimes,
            identity: CanonicalMeteorologyIdentity {
                field_ids: CANONICAL_WIND_FIELDS,
                source_snapshot_sha256: hashes,
                source_times: [times[0], times[1]],
                geometry_identity: runtimes.map(vertical_geometry_identity),
                geometry_provenance: runtimes.map(|r| r.provenance().clone()),
                motion_provenance: [motions[0].clone(), motions[1].clone()],
            },
        })
    }

    /// Borrow the validated source/grid/geometry identity for downstream auditing.
    #[must_use]
    pub fn identity(&self) -> &CanonicalMeteorologyIdentity {
        &self.identity
    }

    /// Borrow the canonical horizontal grid without reconstructing it from legacy wind.
    #[must_use]
    pub fn horizontal_grid(&self) -> &HorizontalGrid {
        &self.snapshots[0].horizontal_grid
    }

    /// Resolve current and signed predicted-step coverage using the existing #89 policy.
    ///
    /// This resolves control metadata only, without querying particle positions.
    /// # Errors
    /// Rejects time overflow and either request outside the supplied source pair.
    pub fn resolve_step_times(
        &self,
        current_epoch_seconds: i64,
        signed_timestep_seconds: i64,
    ) -> Result<CanonicalMeteorologyStepTimes, MeteorologyCompositionError> {
        let predicted = current_epoch_seconds
            .checked_add(signed_timestep_seconds)
            .ok_or(MeteorologyCompositionError::Incompatible(
                "predicted canonical time overflows",
            ))?;
        let calendar = self.identity.source_times[0].calendar;
        let current = RequestedSampleTime::new(calendar, current_epoch_seconds);
        let predicted = RequestedSampleTime::new(calendar, predicted);
        for id in CANONICAL_WIND_FIELDS {
            resolve_temporal_bracket(id, &self.snapshots, current)?;
            resolve_temporal_bracket(id, &self.snapshots, predicted)?;
        }
        Ok(CanonicalMeteorologyStepTimes {
            current: MeteorologyTimeSelection::Instantaneous(current),
            predicted: MeteorologyTimeSelection::Instantaneous(predicted),
        })
    }
}

/// Distinct #89 selections for the current and predicted signed-step time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CanonicalMeteorologyStepTimes {
    /// Current source-time selection for all U/V/W components.
    pub current: MeteorologyTimeSelection,
    /// Predicted selection; forward adds dt and backward subtracts dt.
    pub predicted: MeteorologyTimeSelection,
}

/// Source-lifetime U/V/normalized-W owners for the subsequent #112 consumer.
///
/// The caller retains this owner through the last submitted consumer. The existing
/// context token prevents device mixing while runtime/source borrows prevent
/// geometry mutation. No driver/context self-reference or second field layout exists.
pub struct CanonicalMeteorologyResources<'source> {
    bracket: CanonicalMeteorologyBracket<'source>,
    fields: [CanonicalGpuField<'source>; 3],
}

impl<'source> CanonicalMeteorologyResources<'source> {
    /// Borrow all field owners in the exact U/V/W order declared by the identity.
    #[must_use]
    pub fn fields(&self) -> &[CanonicalGpuField<'source>; 3] {
        &self.fields
    }

    /// Borrow source/time/geometry metadata and resolve subsequent step selections.
    #[must_use]
    pub fn bracket(&self) -> &CanonicalMeteorologyBracket<'source> {
        &self.bracket
    }
}

/// One caller-held bracket slot, reused by either actual driver preparation boundary.
///
/// A transition replaces the slot only after all three uploads succeed. Outstanding
/// prepared owners retain earlier resources through their last in-flight consumer.
/// This is a wind-bracket owner, not a generic GPU cache or runtime.
#[derive(Default)]
pub struct CanonicalMeteorologySlot<'source> {
    resources: Option<Arc<CanonicalMeteorologyResources<'source>>>,
}

/// Scoped driver preparation result; it performs no advection or GPU submission.
pub struct PreparedCanonicalMeteorology<'step, 'source> {
    context: &'step GpuContext,
    resources: Arc<CanonicalMeteorologyResources<'source>>,
    /// Validated current and predicted time selections for #112.
    pub times: CanonicalMeteorologyStepTimes,
    /// True exactly when the existing bracket owners were reused without uploads.
    pub reused: bool,
}

impl<'step, 'source> PreparedCanonicalMeteorology<'step, 'source> {
    /// Borrow the exact driver context for #171 preparation and caller-owned encoding.
    #[must_use]
    pub fn context(&self) -> &'step GpuContext {
        self.context
    }

    /// Retain the three source owners independently of this timestep borrow.
    #[must_use]
    pub fn resources(&self) -> &Arc<CanonicalMeteorologyResources<'source>> {
        &self.resources
    }
}

impl<'source> CanonicalMeteorologySlot<'source> {
    pub(super) fn prepare<'step>(
        &mut self,
        context: &'step GpuContext,
        bracket: CanonicalMeteorologyBracket<'source>,
        current_epoch_seconds: i64,
        signed_timestep_seconds: i64,
    ) -> Result<PreparedCanonicalMeteorology<'step, 'source>, MeteorologyCompositionError> {
        let times = bracket.resolve_step_times(current_epoch_seconds, signed_timestep_seconds)?;
        if let Some(previous) = &self.resources {
            if !previous
                .fields
                .iter()
                .all(|field| field.belongs_to(context))
            {
                return Err(MeteorologyCompositionError::Incompatible(
                    "canonical bracket slot belongs to another driver context",
                ));
            }
            if previous.bracket.identity == bracket.identity {
                return Ok(PreparedCanonicalMeteorology {
                    context,
                    resources: Arc::clone(previous),
                    times,
                    reused: true,
                });
            }
        }
        let snapshots = &bracket.snapshots;
        let runtimes = &bracket.runtimes.map(Some);
        let resources = Arc::new(CanonicalMeteorologyResources {
            fields: [
                CanonicalGpuField::upload(context, FieldId::WindU, snapshots, runtimes)?,
                CanonicalGpuField::upload(context, FieldId::WindV, snapshots, runtimes)?,
                CanonicalGpuField::upload(context, FieldId::VerticalVelocity, snapshots, runtimes)?,
            ],
            bracket,
        });
        for field in &resources.fields {
            field.validate_resident_source()?;
        }
        self.resources = Some(Arc::clone(&resources));
        Ok(PreparedCanonicalMeteorology {
            context,
            resources,
            times,
            reused: false,
        })
    }
}
