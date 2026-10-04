//! Validation-only pinned calc_etadot preprocessing; production remains fail-closed.

use super::super::{
    FieldId, Requirements, Snapshot, VerticalCoordinateKind, VerticalOrdering, VerticalStaggering,
};
use super::layout::{
    field_values, interface_offset, model_level_index_from_surface, surface_offset, volume_offset,
};
use super::motion::validate_native_motion_semantics;
use super::{NativeVerticalMotion, VerticalTransformError};
use serde::Serialize;

/// Reference pressure used by flex_extract's `calc_etadot` for the eta-dot
/// preprocessing factor (`P00` in `calc_etadot.f90`, v7.1.2 source line 541).
///
/// The numeric value is 101325.0 Pa. It is part of the pinned oracle contract
/// and must not be changed independently of `reference/flex-extract.json`.
pub const FLEX_EXTRACT_CALC_ETADOT_REFERENCE_PRESSURE_PA: f32 = 101_325.0;

fn eta_dot_preprocessing_requirements() -> Requirements {
    Requirements {
        required_fields: [FieldId::SurfacePressure].into_iter().collect(),
    }
}

/// Interface index in the canonical A/B arrays that belongs to the physical
/// hybrid interface `k` counting from the top of the atmosphere
/// (`k = 1` top boundary, `k = nz + 1` surface).
///
/// The canonical snapshot stores the interface coefficients in the direction
/// of its declared [`VerticalOrdering`]; this helper hides that storage detail
/// for the eta-dot preprocessing loop, which always walks top-to-bottom like
/// flex_extract `calc_etadot`.
#[inline]
const fn interface_index_from_top(ordering: VerticalOrdering, nz: usize, k: usize) -> usize {
    match ordering {
        VerticalOrdering::Increasing => k - 1,
        VerticalOrdering::Decreasing => nz + 1 - k,
    }
}

/// Result of the validated eta-coordinate velocity preprocessing.
///
/// `values_interface_pa_s` contains the pressure vertical velocity on the
/// hybrid *interfaces* (half levels), in the canonical interface order of the
/// source snapshot (index 0 follows the declared level ordering, e.g. model
/// top for `Increasing`). The top boundary value is canonical zero; the
/// remaining `nz` entries reproduce the `nz` interface values produced by
/// flex_extract `calc_etadot` for a full-level param-77 input field.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EtaDotPressureVelocity {
    /// Interface-staggered pressure velocity in Pa/s, canonical interface order.
    pub values_interface_pa_s: Vec<f32>,
    /// Stable machine-readable algorithm identity anchored to the pinned
    /// flex_extract/calc_etadot reference.
    pub algorithm_id: &'static str,
    /// Human-readable summary; not the machine identity of the transform.
    pub conversion: String,
}

/// Validate-only preprocessing of the ECMWF eta-coordinate vertical velocity
/// (GRIB parameter 77, deta/dt) into interface-staggered pressure velocity.
///
/// The transformation follows flex_extract `calc_etadot.f90` exactly for the
/// regular-grid `META=1, METADIFF=0, MOMEGA=0, MDPDETA=1` configuration:
///
/// ```text
/// P00 = FLEX_EXTRACT_CALC_ETADOT_REFERENCE_PRESSURE_PA      (Pa, source line 541)
/// for k = 1..nz          (full levels, top of atmosphere down):
///   DAK = A[k+1] - A[k]  (A in top-to-bottom interface order)
///   DBK = B[k+1] - B[k]
///   eta = 2*eta_raw*ps*(DAK/ps + DBK)/(DAK/P00 + DBK)       (source line 555)
///   if k > 1: eta = eta - eta_prev                          (source lines 545-546)
///   interface below level k <- eta
/// ```
///
/// The input raw field is a full-level (`LevelCenter`) field in `PerSecond`
/// using the `PositiveEtaIncreasing` convention validated by the pinned
/// calc_etadot oracle. The opposite eta sign convention remains fail-closed
/// until it has independent oracle evidence. The
/// result is deliberately **not** wired into `normalize_vertical_motion`:
/// production consumers keep rejecting `EtaCoordinateVelocity` until a
/// follow-up flips the switch after this validated preprocessing is adopted.
///
/// Source of truth: `reference/flex-extract.json` (pinned commit
/// `e0005c99ac81d12faa45a8ff799debbd592b0dc0`, tag 7.1.2;
/// `calc_etadot.f90` bytes hash to
/// sha256 160F267F8741F23D13FDBA2F7A88F110BB131AA84AD7894FA43605258E55B0D9;
/// git blob 741eba91eab049df23a560219d0f2656a6cc9881).
///
/// # Errors
///
/// Returns [`VerticalTransformError`] when the snapshot is not hybrid
/// sigma-pressure, the A/B coefficients or surface pressure are missing or
/// malformed, the native motion violates the eta-dot contract (unit, sign,
/// staggering, shape, finite values), or an operand of the pinned arithmetic
/// is not finite.
pub fn eta_dot_to_pressure_velocity(
    snapshot: &Snapshot,
    native_motion: &NativeVerticalMotion,
) -> Result<EtaDotPressureVelocity, VerticalTransformError> {
    snapshot.validate(&eta_dot_preprocessing_requirements())?;

    let vertical = &snapshot.vertical_coordinate;
    if vertical.kind != VerticalCoordinateKind::HybridSigmaPressure {
        return Err(VerticalTransformError::UnsupportedVerticalCoordinate);
    }
    let a = vertical
        .hybrid_a_interface_pa
        .as_deref()
        .ok_or(VerticalTransformError::UnsupportedVerticalCoordinate)?;
    let b = vertical
        .hybrid_b_interface
        .as_deref()
        .ok_or(VerticalTransformError::UnsupportedVerticalCoordinate)?;

    validate_native_motion_semantics(native_motion)?;
    if native_motion.vertical_staggering != VerticalStaggering::LevelCenter {
        return Err(VerticalTransformError::InvalidNativeVerticalMotion {
            reason:
                "raw eta-dot is a full-level field; interface staggering is rejected for the input",
        });
    }

    let nx = snapshot.horizontal_grid.nx;
    let ny = snapshot.horizontal_grid.ny;
    let nz = vertical.level_values.len();
    if nz < 1 {
        return Err(VerticalTransformError::InsufficientVerticalLevels);
    }
    let horizontal = nx
        .checked_mul(ny)
        .ok_or(VerticalTransformError::ShapeMismatch {
            field: "eta_dot_volume",
            expected: usize::MAX,
            actual: native_motion.values.len(),
        })?;
    let volume = horizontal
        .checked_mul(nz)
        .ok_or(VerticalTransformError::ShapeMismatch {
            field: "eta_dot_volume",
            expected: usize::MAX,
            actual: native_motion.values.len(),
        })?;
    if a.len() != nz + 1 || b.len() != nz + 1 {
        return Err(VerticalTransformError::ShapeMismatch {
            field: "hybrid_interface_coefficients",
            expected: nz + 1,
            actual: a.len().max(b.len()),
        });
    }
    if native_motion.values.len() != volume {
        return Err(VerticalTransformError::ShapeMismatch {
            field: "native_vertical_motion",
            expected: volume,
            actual: native_motion.values.len(),
        });
    }
    for (index, value) in native_motion.values.iter().copied().enumerate() {
        if !value.is_finite() {
            return Err(VerticalTransformError::InvalidNativeVerticalMotionValue { index, value });
        }
    }

    let surface_pressure = field_values(snapshot, FieldId::SurfacePressure)?;
    if surface_pressure.len() != horizontal {
        return Err(VerticalTransformError::ShapeMismatch {
            field: "surface_pressure",
            expected: horizontal,
            actual: surface_pressure.len(),
        });
    }
    for (index, value) in surface_pressure.iter().copied().enumerate() {
        if !value.is_finite() || value <= 0.0 {
            return Err(VerticalTransformError::InvalidSurfacePressure {
                x: index % nx,
                y: index / nx,
                pressure_pa: value,
            });
        }
    }

    let mut interface_pa_s = vec![0.0_f32; horizontal * (nz + 1)];
    for y in 0..ny {
        for x in 0..nx {
            let horizontal_index = surface_offset(x, y, nx);

            // The pinned calc_etadot oracle is compiled with
            // -fdefault-real-8. Preserve that arithmetic width throughout the
            // recursive ETAR(K)-ETAR(K-1) chain and round only once at the
            // canonical f32 output boundary. Re-rounding the previous value at
            // every model level accumulates visibly over a complete 137-level
            // ERA5 column.
            let ps = f64::from(surface_pressure[horizontal_index]);
            let p00 = f64::from(FLEX_EXTRACT_CALC_ETADOT_REFERENCE_PRESSURE_PA);
            let mut previous_output_pa_s = 0.0_f64;

            for k in 1..=nz {
                let above = interface_index_from_top(snapshot.vertical_coordinate.ordering, nz, k);
                let below =
                    interface_index_from_top(snapshot.vertical_coordinate.ordering, nz, k + 1);
                let dak_pa = f64::from(a[below]) - f64::from(a[above]);
                let dbk = f64::from(b[below]) - f64::from(b[above]);

                let level_index = model_level_index_from_surface(
                    snapshot.vertical_coordinate.ordering,
                    nz,
                    nz - k,
                );
                let deta_dt =
                    f64::from(native_motion.values[volume_offset(x, y, level_index, nx, ny)]);

                let scaled = 2.0_f64 * deta_dt * ps * (dak_pa / ps + dbk) / (dak_pa / p00 + dbk);
                if !scaled.is_finite() {
                    return Err(VerticalTransformError::InvalidEtaDotTransform { x, y, k });
                }
                let output = if k > 1 {
                    scaled - previous_output_pa_s
                } else {
                    scaled
                };
                if !output.is_finite() {
                    return Err(VerticalTransformError::InvalidEtaDotTransform { x, y, k });
                }
                previous_output_pa_s = output;

                let output_f32 = output as f32;
                if !output_f32.is_finite() {
                    return Err(VerticalTransformError::InvalidEtaDotTransform { x, y, k });
                }
                interface_pa_s[interface_offset(x, y, below, nx, ny)] = output_f32;
            }
        }
    }

    Ok(EtaDotPressureVelocity {
        values_interface_pa_s: interface_pa_s,
        algorithm_id: "flex_extract_7_1_2_calc_etadot_meta_mdpdeta_v1",
        conversion:
            "raw deta/dt [1/s] * 2*ps*(DAK/ps+DBK)/(DAK/P00+DBK), f64 recursive interface difference matching the pinned oracle, rounded once to canonical f32 interface-staggered Pa/s"
                .to_string(),
    })
}

#[cfg(test)]
#[path = "tests/eta_dot.rs"]
mod tests;
