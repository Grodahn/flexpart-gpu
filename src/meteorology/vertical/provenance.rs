//! Canonical transform identity and serialized input lineage.

use super::super::{Snapshot, VerticalOrdering, VerticalReference, SCHEMA_ID, SCHEMA_VERSION};
use super::VerticalTransformError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Machine-readable identity of the canonical vertical transformation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerticalTransformProvenance {
    pub source_schema_id: String,
    pub source_schema_version: u32,
    /// SHA-256 of the compact serde representation of the complete canonical
    /// Snapshot used to derive this runtime geometry.
    pub source_snapshot_sha256: String,
    pub source_vertical_ordering: VerticalOrdering,
    pub source_level_count: usize,
    pub pressure_algorithm_id: String,
    pub height_algorithm_id: String,
    pub w_height_algorithm_id: String,
    pub terrain_reference: VerticalReference,
    pub pressure_reconstruction: String,
    pub height_reconstruction: String,
    pub height_reference: String,
}

impl VerticalTransformProvenance {
    /// Bind derived geometry to its immutable Snapshot and existing algorithm identities.
    pub(super) fn from_snapshot(snapshot: &Snapshot) -> Result<Self, VerticalTransformError> {
        Ok(Self {
            source_schema_id: SCHEMA_ID.to_string(),
            source_schema_version: SCHEMA_VERSION,
            source_snapshot_sha256: sha256_serialized(snapshot, "canonical_snapshot")?,
            source_vertical_ordering: snapshot.vertical_coordinate.ordering,
            source_level_count: snapshot.vertical_coordinate.level_values.len(),
            pressure_algorithm_id: "hybrid_interface_ab_local_ps_fulllevel_adjacent_mean_v1".to_string(),
            height_algorithm_id: "flexpart11_verttransform_ecmwf_heights_v1".to_string(),
            w_height_algorithm_id: "flexpart11_wzlev_from_uvzlev_v1".to_string(),
            terrain_reference: VerticalReference::AboveMeanSeaLevel,
            pressure_reconstruction: "p_interface=a_interface+b_interface*local_surface_pressure; p_level=mean(adjacent_interfaces)".to_string(),
            height_reconstruction: "FLEXPART-11.1 verttransform_ecmwf_heights hypsometric integration from surface virtual temperature (T2m/dewpoint) through model-level T/q".to_string(),
            height_reference: "agl_integrated_from_local_surface; asl=agl+orography_asl".to_string(),
        })
    }
}

/// Hash the existing compact serde encoding to retain exact input lineage.
pub(super) fn sha256_serialized<T: Serialize>(
    value: &T,
    input: &'static str,
) -> Result<String, VerticalTransformError> {
    let encoded = serde_json::to_vec(value)
        .map_err(|_| VerticalTransformError::ProvenanceSerialization { input })?;
    let digest = Sha256::digest(encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
#[path = "tests/provenance.rs"]
mod tests;
