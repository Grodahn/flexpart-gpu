//! #190 adopts #188 corrected W; preparation and #87/#88/#89 retain all science.

use super::{
    CanonicalGpuField, FieldId, GpuContext, MeteorologyCompositionError, Snapshot,
    VerticalRuntimeView,
};
use crate::gpu::{shared_height::PreparedSharedHeightUv, shared_height_w::PreparedSharedHeightW};
use crate::meteorology::vertical::NormalizedVerticalMotionProvenance;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Source-interface, normalization and correction identity of one accepted vector member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedHeightWQueryIdentity {
    /// Exact #30 normalized interface motion lineage.
    pub normalization: NormalizedVerticalMotionProvenance,
    /// Physical bottom-to-top interface heights and normalized velocities.
    pub interface_input_sha256: String,
    /// Canonical regional horizontal geometry, including spacing and latitude.
    pub horizontal_sha256: String,
    /// Exact #184 source and target metadata consumed by W preparation.
    pub prepared_uv_sha256: String,
    /// Full physical correction; successful device status remains mandatory.
    pub correction_algorithm: String,
}

impl SharedHeightWQueryIdentity {
    pub(super) fn from_prepared(w: &PreparedSharedHeightW) -> Self {
        let identity = w.source().identity();
        Self {
            normalization: identity.normalization.clone(),
            interface_input_sha256: identity.interface_input_sha256.clone(),
            horizontal_sha256: identity.horizontal_sha256.clone(),
            prepared_uv_sha256: identity.prepared_uv_sha256.clone(),
            correction_algorithm: identity.correction_algorithm.into(),
        }
    }
}

impl CanonicalGpuField<'_> {
    /// Adopt one complete same-source U/V/W vector, in that order, on #184's AGL target.
    /// This validates immutable identities without reading device status. Each query
    /// requires completed target/U/V/W status on-device, allowing preparation and
    /// sampling to share the caller's encoder. Every component retains all W owners
    /// and their U/V, interface, target and context dependencies through consumption.
    /// A missing canonical W field is allowed: #30 normalized motion is authoritative.
    /// If present, canonical W must equal that exact normalized interface motion.
    /// # Errors
    /// Reject incomplete, foreign, mixed-source/target/time/geometry/motion vectors.
    pub fn from_shared_height_vector<'a>(
        ctx: &GpuContext,
        snapshots: &[&Snapshot],
        runtimes: &[VerticalRuntimeView<'a>],
        uv: &[Arc<PreparedSharedHeightUv>],
        w: &[Arc<PreparedSharedHeightW>],
    ) -> Result<[CanonicalGpuField<'a>; 3], MeteorologyCompositionError> {
        let invalid = || MeteorologyCompositionError::Incompatible("shared-height U/V/W identity");
        if w.is_empty()
            || w.len() != uv.len()
            || w.len() != snapshots.len()
            || w.len() != runtimes.len()
        {
            return Err(invalid());
        }
        for (((snapshot, runtime), uv), w) in snapshots.iter().zip(runtimes).zip(uv).zip(w) {
            w.require_reuse(ctx, w.source())?;
            let motion = runtime.vertical_velocity().ok_or_else(invalid)?;
            if !std::ptr::eq(uv.as_ref(), w.source().uv())
                || motion.provenance() != &w.source().identity().normalization
            {
                return Err(invalid());
            }
            if let Some(field) = snapshot
                .fields
                .iter()
                .find(|f| f.id == FieldId::VerticalVelocity)
            {
                super::validate_runtime_binding(
                    snapshot,
                    field,
                    *runtime,
                    &super::snapshot_hash(snapshot)?,
                )?;
            }
        }
        let mut fields = [
            Self::from_shared_height(ctx, FieldId::WindU, snapshots, runtimes, uv)?,
            Self::from_shared_height(ctx, FieldId::WindV, snapshots, runtimes, uv)?,
            Self::from_shared_height(ctx, FieldId::WindU, snapshots, runtimes, uv)?,
        ];
        for field in &mut fields {
            for (member, w) in field.members.iter_mut().zip(w) {
                member.prepared_w = Some(Arc::clone(w));
            }
        }
        let vertical = &mut fields[2];
        vertical.field_id = FieldId::VerticalVelocity;
        for member in &mut vertical.members {
            // The descriptor represents the prepared metric grid. Native interface
            // staggering and normalization remain explicit in corrected_w metadata.
            member.field.id = FieldId::VerticalVelocity;
            member.field.sign = crate::meteorology::SignConvention::PositiveUpward;
            *member.field.shape.first_mut().ok_or_else(invalid)? = member.planes.len();
            member.field.values.clear();
            member.runtime.as_mut().ok_or_else(invalid)?.motion = member
                .prepared_w
                .as_ref()
                .map(|w| w.source().identity().normalization.clone());
        }
        Ok(fields)
    }
}
