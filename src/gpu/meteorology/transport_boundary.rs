//! #198 lends the original run-owned AGL endpoint to production Petterssen stages.

use super::{CanonicalGpuField, FieldId, GpuContext, MeteorologyCompositionError};
use crate::gpu::shared_height::SharedHeightTarget;
use crate::meteorology::HorizontalGrid;
use std::sync::Arc;

/// Retains the device target and its exact initializer/source/context lifetime.
/// `nxmax` is the validated regional mother-grid `nxfield`, never legacy wind geometry.
#[derive(Clone)]
pub(crate) struct SharedHeightTransportBoundary {
    pub(crate) target: SharedHeightTarget,
    pub(crate) levels: u32,
}

impl SharedHeightTransportBoundary {
    /// Validate the complete immutable vector; completion is checked on-device.
    pub(crate) fn new(
        ctx: &GpuContext,
        fields: &[CanonicalGpuField<'_>; 3],
        grid: &HorizontalGrid,
    ) -> Result<Self, MeteorologyCompositionError> {
        let invalid = || {
            MeteorologyCompositionError::Incompatible(
                "transport boundary vector/source/target/context identity",
            )
        };
        let first = fields[0].members.first().ok_or_else(invalid)?;
        let initial = first.prepared_uv.as_ref().ok_or_else(invalid)?;
        let target = initial.target();
        let identity = target.identity();
        let levels = identity.initial_source.levels;
        if levels < 3
            || identity.initial_source.grid != *grid
            || identity.height_m_agl.len() != levels as usize
        {
            return Err(invalid());
        }
        for (field, id) in
            fields
                .iter()
                .zip([FieldId::WindU, FieldId::WindV, FieldId::VerticalVelocity])
        {
            if !field.belongs_to(ctx) || field.field_id != id || field.members.len() != 2 {
                return Err(invalid());
            }
            for (member, source) in field.members.iter().zip(&fields[0].members) {
                let uv = member.prepared_uv.as_ref().ok_or_else(invalid)?;
                let w = member.prepared_w.as_ref().ok_or_else(invalid)?;
                let source_uv = source.prepared_uv.as_ref().ok_or_else(invalid)?;
                let source_w = source.prepared_w.as_ref().ok_or_else(invalid)?;
                if !uv.belongs_to(ctx)
                    || !uv.shares_target(initial)
                    || !Arc::ptr_eq(uv, source_uv)
                    || !Arc::ptr_eq(w, source_w)
                    || !std::ptr::eq(uv.as_ref(), w.source().uv())
                    || uv.source_identity().grid != *grid
                    || uv.source_identity().levels != levels
                    || member.snapshot_sha256 != source.snapshot_sha256
                    || member.field.time != source.field.time
                {
                    return Err(invalid());
                }
                w.require_reuse(ctx, w.source())?;
            }
        }
        Ok(Self {
            target: target.clone(),
            levels,
        })
    }
}
