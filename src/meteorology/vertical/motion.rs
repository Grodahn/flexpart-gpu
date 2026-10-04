//! Explicit native motion semantics and same-Snapshot geometric normalization.

use super::super::{Snapshot, VerticalStaggering};
use super::interfaces::pressure_velocity_interfaces_to_geometric;
use super::provenance::sha256_serialized;
use super::{reconstruct_vertical_geometry, VerticalTransformError, VerticalTransformResult};
use serde::{Deserialize, Serialize};

/// Provider-independent representation of native vertical air motion before
/// normalization to canonical upward-positive geometric velocity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeVerticalMotionKind {
    /// Already-geometric dz/dt.
    GeometricVelocity,
    /// Pressure velocity omega = dp/dt.
    PressureVelocityOmega,
    /// Native hybrid-coordinate tendency d(eta)/dt.
    ///
    /// Recognized at the boundary. Preprocessing is oracle-validated only for
    /// the positive-eta-increasing convention; production normalization stays
    /// fail-closed until it is explicitly enabled after #70.
    EtaCoordinateVelocity,
}

/// Explicit units prevent native tendencies from being treated as geometric m/s.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeVerticalMotionUnit {
    MeterPerSecond,
    PascalPerSecond,
    PerSecond,
}

/// Explicit positive direction disambiguates upward, pressure and eta tendencies.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeVerticalMotionSign {
    PositiveUpward,
    PositivePressureIncreasing,
    PositiveEtaIncreasing,
    PositiveEtaDecreasing,
}

/// Native vertical-motion values plus the semantics needed to normalize them.
///
/// Provider-specific variable names and parameter identifiers do not belong in
/// this type; adapters in #32 map those encodings onto this physical contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NativeVerticalMotion {
    pub kind: NativeVerticalMotionKind,
    pub unit: NativeVerticalMotionUnit,
    pub sign: NativeVerticalMotionSign,
    pub vertical_staggering: VerticalStaggering,
    pub values: Vec<f32>,
    pub provenance: NativeVerticalMotionProvenance,
}

/// Minimal machine-readable lineage for a native vertical-motion field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeVerticalMotionProvenance {
    /// Stable identifier supplied by the upstream normalization stage.
    pub source_id: String,
}

/// Physics-ready vertical air motion. Values are always geometric m/s with
/// positive-upward sign. Staggering is retained so #31 owns interpolation.
///
/// ```compile_fail
/// use flexpart_gpu::meteorology::vertical::NormalizedVerticalMotion;
/// fn mutate_motion(motion: &mut NormalizedVerticalMotion) {
///     motion.values_ms.clear();
/// }
/// ```
///
/// ```compile_fail
/// use flexpart_gpu::meteorology::vertical::NormalizedVerticalMotion;
/// let _ = serde_json::from_str::<NormalizedVerticalMotion>("{}");
/// ```
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NormalizedVerticalMotion {
    pub(super) vertical_staggering: VerticalStaggering,
    pub(super) values_ms: Vec<f32>,
    pub(super) provenance: NormalizedVerticalMotionProvenance,
}

impl NormalizedVerticalMotion {
    /// Retain the source grid identity for downstream sampling.
    #[must_use]
    pub const fn vertical_staggering(&self) -> VerticalStaggering {
        self.vertical_staggering
    }

    /// Borrow geometric m/s values without permitting source recombination.
    #[must_use]
    pub fn values_ms(&self) -> &[f32] {
        &self.values_ms
    }

    /// Borrow the identity of the native-to-geometric conversion.
    #[must_use]
    pub const fn provenance(&self) -> &NormalizedVerticalMotionProvenance {
        &self.provenance
    }
}

/// Lineage preserves the physical semantics and exact native source of m/s values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedVerticalMotionProvenance {
    pub source_id: String,
    /// SHA-256 of the compact serde representation of the complete native
    /// motion contract supplied to this transform.
    pub source_native_motion_sha256: String,
    pub source_kind: NativeVerticalMotionKind,
    pub source_unit: NativeVerticalMotionUnit,
    pub source_sign: NativeVerticalMotionSign,
    pub source_vertical_staggering: VerticalStaggering,
    pub output_vertical_staggering: VerticalStaggering,
    /// Stable machine-readable conversion identity. Numeric inputs such as
    /// hybrid A/B and reference surface pressure remain in the canonical
    /// Snapshot and are therefore reconstructible from the run inputs.
    pub algorithm_id: String,
    /// Human-readable summary; not the machine identity of the transform.
    pub conversion: String,
}

/// Reconstruct vertical geometry and normalize native vertical motion without
/// performing vertical interpolation (#31 owns that).
///
/// This is the public normalization boundary: geometry and motion conversion
/// are intentionally derived from the same Snapshot in one operation so callers
/// cannot combine pressure/height geometry from one meteorological state with
/// surface pressure or ordering metadata from another.
pub fn reconstruct_vertical_geometry_with_motion(
    snapshot: &Snapshot,
    native_motion: &NativeVerticalMotion,
) -> Result<VerticalTransformResult, VerticalTransformError> {
    let mut result = reconstruct_vertical_geometry(snapshot)?;
    result.vertical_velocity = Some(normalize_vertical_motion(snapshot, &result, native_motion)?);
    Ok(result)
}

/// Normalize native vertical motion to geometric m/s, positive upward.
///
/// Pressure velocity follows FLEXPART's pinmconv concept: multiply omega
/// [Pa/s] by dz/dp [m/Pa]. Raw eta-dot preprocessing has an independently
/// validated path, but production normalization remains deliberately
/// fail-closed until it is explicitly enabled after #70.
pub(super) fn normalize_vertical_motion(
    snapshot: &Snapshot,
    geometry: &VerticalTransformResult,
    native_motion: &NativeVerticalMotion,
) -> Result<NormalizedVerticalMotion, VerticalTransformError> {
    validate_geometry_identity(snapshot, geometry)?;
    validate_native_motion_semantics(native_motion)?;

    let nx = geometry.nx;
    let ny = geometry.ny;
    let nz = geometry.nz;
    let horizontal = nx * ny;
    let center_count = horizontal * nz;
    let interface_count = horizontal * (nz + 1);

    let expected = match native_motion.vertical_staggering {
        VerticalStaggering::LevelCenter => center_count,
        VerticalStaggering::LevelInterface => interface_count,
        VerticalStaggering::NotApplicable => {
            return Err(VerticalTransformError::InvalidNativeVerticalMotion {
                reason: "vertical motion cannot use not_applicable staggering",
            });
        }
    };
    if native_motion.values.len() != expected {
        return Err(VerticalTransformError::ShapeMismatch {
            field: "native_vertical_motion",
            expected,
            actual: native_motion.values.len(),
        });
    }
    for (index, value) in native_motion.values.iter().copied().enumerate() {
        if !value.is_finite() {
            return Err(VerticalTransformError::InvalidNativeVerticalMotionValue { index, value });
        }
    }

    let (vertical_staggering, values_ms, algorithm_id, conversion) = match native_motion.kind {
        NativeVerticalMotionKind::GeometricVelocity => (
            native_motion.vertical_staggering,
            native_motion.values.clone(),
            "geometric_identity_v1".to_string(),
            "identity: already geometric m/s positive upward".to_string(),
        ),
        NativeVerticalMotionKind::PressureVelocityOmega => match native_motion.vertical_staggering {
            VerticalStaggering::LevelCenter => {
                return Err(VerticalTransformError::InvalidNativeVerticalMotion {
                    reason: "center-staggered pressure velocity is not enabled without a pinned FLEXPART 11.1 oracle; supply interface/W-staggered omega instead",
                });
            }
            VerticalStaggering::LevelInterface => (
                VerticalStaggering::LevelInterface,
                pressure_velocity_interfaces_to_geometric(
                    snapshot,
                    geometry,
                    &native_motion.values,
                )?,
                "omega_interface_flexpart11_pinmconv_v1".to_string(),
                "omega[Pa/s] * FLEXPART-style pinmconv dz/dp on W/interfaces -> geometric m/s positive upward"
                    .to_string(),
            ),
            VerticalStaggering::NotApplicable => unreachable!(),
        },
        NativeVerticalMotionKind::EtaCoordinateVelocity => {
            return Err(VerticalTransformError::InvalidNativeVerticalMotion {
                reason: "eta-dot preprocessing is validation-only; production normalization remains disabled until explicitly enabled after #70",
            });
        }
    };

    Ok(NormalizedVerticalMotion {
        vertical_staggering,
        values_ms,
        provenance: NormalizedVerticalMotionProvenance {
            source_id: native_motion.provenance.source_id.clone(),
            source_native_motion_sha256: sha256_serialized(
                native_motion,
                "native_vertical_motion",
            )?,
            source_kind: native_motion.kind,
            source_unit: native_motion.unit,
            source_sign: native_motion.sign,
            source_vertical_staggering: native_motion.vertical_staggering,
            output_vertical_staggering: vertical_staggering,
            algorithm_id,
            conversion,
        },
    })
}

/// Reject kind/unit/sign/staggering combinations outside the pinned native contract.
pub(super) fn validate_native_motion_semantics(
    motion: &NativeVerticalMotion,
) -> Result<(), VerticalTransformError> {
    let valid = match motion.kind {
        NativeVerticalMotionKind::GeometricVelocity => {
            motion.unit == NativeVerticalMotionUnit::MeterPerSecond
                && motion.sign == NativeVerticalMotionSign::PositiveUpward
        }
        NativeVerticalMotionKind::PressureVelocityOmega => {
            motion.unit == NativeVerticalMotionUnit::PascalPerSecond
                && motion.sign == NativeVerticalMotionSign::PositivePressureIncreasing
                && motion.vertical_staggering == VerticalStaggering::LevelInterface
        }
        NativeVerticalMotionKind::EtaCoordinateVelocity => {
            motion.unit == NativeVerticalMotionUnit::PerSecond
                && motion.sign == NativeVerticalMotionSign::PositiveEtaIncreasing
        }
    };
    if !valid {
        return Err(VerticalTransformError::InvalidNativeVerticalMotion {
            reason: "kind/unit/sign combination is not canonical for the declared representation",
        });
    }
    Ok(())
}

fn validate_geometry_identity(
    snapshot: &Snapshot,
    geometry: &VerticalTransformResult,
) -> Result<(), VerticalTransformError> {
    let nx = snapshot.horizontal_grid.nx;
    let ny = snapshot.horizontal_grid.ny;
    let nz = snapshot.vertical_coordinate.level_values.len();
    if geometry.nx != nx || geometry.ny != ny || geometry.nz != nz {
        return Err(VerticalTransformError::InvalidNativeVerticalMotion {
            reason: "vertical geometry does not match the canonical snapshot",
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/motion.rs"]
mod tests;
