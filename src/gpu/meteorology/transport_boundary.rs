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
    /// Original `100.*eps` placement margin [m] for this mother grid's `nxmax`.
    pub(crate) top_margin_m: f32,
}

/// Largest `nxmax` whose integer-to-f32 conversion is exact.
const MAX_EXACT_F32_NXMAX: u32 = 1 << 24;

/// Original regional `eps = nxmax/3.e5` scaled by `100.` (`advance_mod.f90:879,893`).
///
/// Evaluated once in IEEE f32 on the host because WGSL division is not correctly
/// rounded and a device compiler may contract `top - 100*eps`; either could move the
/// pinned placement by one ulp. The device performs only the final subtraction from
/// its own run-owned endpoint.
pub(crate) fn original_top_margin_m(nxmax: usize) -> Option<f32> {
    const EPS_NX_DIVISOR: f32 = 3.0e5;
    const TOP_MARGIN_SCALE_M: f32 = 100.0;
    let nxmax = u32::try_from(nxmax)
        .ok()
        .filter(|n| (1..=MAX_EXACT_F32_NXMAX).contains(n))?;
    #[allow(clippy::cast_precision_loss)] // exact: bounded by MAX_EXACT_F32_NXMAX
    let eps = nxmax as f32 / EPS_NX_DIVISOR;
    Some(TOP_MARGIN_SCALE_M * eps)
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
        let top_margin_m = original_top_margin_m(grid.nx).ok_or_else(invalid)?;
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
            top_margin_m,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{original_top_margin_m, MAX_EXACT_F32_NXMAX};

    #[test]
    #[allow(clippy::excessive_precision)] // exact pinned decimal values, not rounded constants
    fn test_original_top_margin_reproduces_pinned_f32_placement() {
        let margin = original_top_margin_m(2).expect("regional nxmax");
        assert_eq!(
            margin.to_bits(),
            (100.0_f32 * (2.0_f32 / 3.0e5_f32)).to_bits()
        );
        assert_eq!(
            (9_521.843_75_f32 - margin).to_bits(),
            9_521.842_773_437_5_f32.to_bits()
        );
        assert!(original_top_margin_m(0).is_none());
        assert!(original_top_margin_m(MAX_EXACT_F32_NXMAX as usize + 1).is_none());
    }
}
