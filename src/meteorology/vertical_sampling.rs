//! Canonical vertical meteorology sampling for issue #73.
//!
//! This module owns vertical interpolation only. It samples a canonical field
//! column on the derived runtime geometry supplied by #30
//! (`VerticalTransformResult::runtime_view()` / `VerticalRuntimeView`) without
//! deriving a second vertical coordinate, without using legacy
//! `WindFieldGrid::heights_m`, and without synthesizing another W/interface
//! height coordinate.
//!
//! Ported from `interpol_mod.f90:215-242` (`find_z_level_meters`, METRE mode),
//! `interpol_mod.f90:406-430` (`find_vert_vars_lin`, `log_interpol=.false.` in
//! the pinned build) and `interpol_mod.f90:539-547` (`vert_interpol`).
//!
//! Model-level (`LevelCenter`) fields interpolate on the #30 model-level AGL
//! geometry. Interface-staggered (`LevelInterface`) vertical motion
//! interpolates on the #30 FLEXPART-`wzlev`-compatible W/interface AGL geometry.
//! The interface path reproduces the pinned primitive
//! `find_z_level_meters -> find_vert_vars -> vert_interpol` on the #30 handoff
//! frozen by #71 (`vertical-interface-wzlev`). It is explicitly **not**
//! end-to-end evidence for pristine FLEXPART's `eta=no` two-stage W production
//! path (`verttransform_ecmwf_windfields -> interpol_wind ->
//! interpol_wind_meter`), which remains owned by blocking oracle issue #80.
//!
//! Horizontal interpolation (#72), temporal interpolation (#74),
//! accumulated-field handling (#75), 4D composition (#76), consumer migration
//! (#77) and provider decoding (#32) are out of scope.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    vertical::{VerticalRuntimeView, VerticalTransformError},
    FieldId, VerticalOrdering, VerticalReference, VerticalStaggering,
};

/// Vertical sample with full oracle-traceable evidence.
///
/// FLEXPART weights follow `find_vert_vars_lin` / `vert_interpol`:
/// `output = lower_value * weight_lower + upper_value * weight_upper`, where
/// `weight_upper` is `dz1` and `weight_lower` is `dz2` in the Fortran. The
/// lower level is closer to the ground in physical bottom-to-top order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VerticalSample {
    /// Interpolated field value.
    pub value: f32,
    /// Requested height resolved to AGL in metres.
    pub height_agl_m: f32,
    /// Physical bottom-to-top lower index (0-based).
    pub lower_physical_index: usize,
    /// Physical bottom-to-top upper index (0-based, always `lower + 1`).
    pub upper_physical_index: usize,
    /// Canonical storage index of the lower level (depends on ordering).
    pub lower_canonical_index: usize,
    /// Canonical storage index of the upper level (depends on ordering).
    pub upper_canonical_index: usize,
    /// Weight applied to the lower (closer-to-ground) value (`dz2`).
    pub weight_lower: f32,
    /// Weight applied to the upper value (`dz1`).
    pub weight_upper: f32,
    /// True when the request was at or below the lowest height (FLEXPART `lbounds(1)`).
    pub clamped_to_lower: bool,
    /// True when the request was at or above the highest height (FLEXPART `lbounds(2)`).
    pub clamped_to_upper: bool,
}

/// Fail-closed errors for canonical vertical sampling.
#[derive(Debug, Error, PartialEq)]
pub enum VerticalSamplingError {
    /// Requested height is non-finite.
    #[error("non-finite requested vertical height: {height_m}")]
    NonFiniteHeight { height_m: f32 },
    /// Only explicit AGL/ASL requests are supported; `ModelNative` is ambiguous.
    #[error("ambiguous vertical reference {reference:?}; request explicit AGL or ASL")]
    AmbiguousReference { reference: VerticalReference },
    /// Field is not a 3-D vertically sampled field; 2-D/static fields belong elsewhere.
    #[error("unsupported field for vertical sampling: {field:?}")]
    UnsupportedField { field: FieldId },
    /// Staggering does not match the field's canonical contract or the requested geometry.
    #[error("wrong vertical staggering for {field:?}: requested {requested:?}")]
    WrongStaggering {
        field: FieldId,
        requested: VerticalStaggering,
    },
    /// Value count does not match the runtime column geometry for the requested staggering.
    #[error(
        "vertical field shape mismatch for {field:?}: expected {expected} values, got {actual}"
    )]
    ShapeMismatch {
        field: FieldId,
        expected: usize,
        actual: usize,
    },
    /// Column heights are non-finite or not strictly increasing in physical order.
    #[error("malformed vertical geometry at (x={x}, y={y}, physical={index}): heights {lower_m} -> {upper_m}")]
    MalformedGeometry {
        x: usize,
        y: usize,
        index: usize,
        lower_m: f32,
        upper_m: f32,
    },
    /// Column field values are non-finite.
    #[error("non-finite vertical field value for {field:?} at (x={x}, y={y}, canonical={index})")]
    NonFiniteFieldValue {
        field: FieldId,
        x: usize,
        y: usize,
        index: usize,
    },
    /// Center-staggered sampling needs at least two model levels.
    #[error("insufficient model levels for center-staggered sampling: nz={nz}")]
    InsufficientLevels { nz: usize },
    /// Wrapped #30 runtime access failure (out-of-bounds column or corrupt shape).
    #[error(transparent)]
    Runtime(#[from] VerticalTransformError),
}

/// Sample one vertical column on the authoritative #30 runtime geometry.
///
/// `values` holds the complete canonical volume in X-fastest `[x,y,z]` order
/// with `z` following the Snapshot's declared storage ordering: length
/// `nx*ny*nz` for `LevelCenter`, `nx*ny*(nz+1)` for `LevelInterface`. Only the
/// requested `(x, y)` column is read. `field` identifies the canonical field
/// for staggering validation and comparison evidence; `staggering` selects the
/// geometry (model-level heights for centers, W/interface heights for
/// interfaces) and must agree with `field`.
///
/// `reference` must be `AboveGroundLevel` or `AboveMeanSeaLevel`.
/// ASL requests resolve through the column's local #30 terrain
/// (`agl = asl - terrain_asl`); `ModelNative` fails closed as ambiguous.
///
/// Out-of-range heights clamp to the nearest boundary value exactly like the
/// pinned FLEXPART primitive (`lbounds(1)` below the lowest height,
/// `lbounds(2)` above the highest height). Malformed geometry, shape mismatch,
/// wrong center/interface association, non-finite inputs and ambiguous
/// references fail closed instead of producing plausible values.
///
/// # Errors
///
/// Returns `VerticalSamplingError` for any unsupported or inconsistent input.
#[allow(clippy::too_many_arguments)]
pub fn sample_vertical(
    runtime: VerticalRuntimeView<'_>,
    field: FieldId,
    staggering: VerticalStaggering,
    values: &[f32],
    x: usize,
    y: usize,
    height_m: f32,
    reference: VerticalReference,
) -> Result<VerticalSample, VerticalSamplingError> {
    if !height_m.is_finite() {
        return Err(VerticalSamplingError::NonFiniteHeight { height_m });
    }
    let resolved_agl_m = match reference {
        VerticalReference::AboveGroundLevel => height_m,
        VerticalReference::AboveMeanSeaLevel => {
            let terrain_asl_m = runtime.terrain_asl_m(x, y)?;
            let agl = height_m - terrain_asl_m;
            if !agl.is_finite() {
                return Err(VerticalSamplingError::NonFiniteHeight { height_m: agl });
            }
            agl
        }
        VerticalReference::ModelNative => {
            return Err(VerticalSamplingError::AmbiguousReference { reference });
        }
    };

    validate_field_and_staggering(field, staggering)?;

    let (nx, ny, nz) = runtime.dimensions();
    let horizontal = nx
        .checked_mul(ny)
        .ok_or(VerticalSamplingError::ShapeMismatch {
            field,
            expected: usize::MAX,
            actual: values.len(),
        })?;
    let expected = match staggering {
        VerticalStaggering::LevelCenter => {
            horizontal
                .checked_mul(nz)
                .ok_or(VerticalSamplingError::ShapeMismatch {
                    field,
                    expected: usize::MAX,
                    actual: values.len(),
                })?
        }
        VerticalStaggering::LevelInterface => {
            horizontal
                .checked_mul(nz + 1)
                .ok_or(VerticalSamplingError::ShapeMismatch {
                    field,
                    expected: usize::MAX,
                    actual: values.len(),
                })?
        }
        VerticalStaggering::NotApplicable => {
            return Err(VerticalSamplingError::WrongStaggering {
                field,
                requested: staggering,
            });
        }
    };
    if values.len() != expected {
        return Err(VerticalSamplingError::ShapeMismatch {
            field,
            expected,
            actual: values.len(),
        });
    }

    if staggering == VerticalStaggering::LevelCenter && nz < 2 {
        return Err(VerticalSamplingError::InsufficientLevels { nz });
    }

    let ordering = runtime.provenance().source_vertical_ordering;
    let column = collect_physical_column(runtime, field, staggering, values, x, y, ordering)?;
    validate_physical_column(x, y, &column)?;

    Ok(interpolate_flexpart_meter_mode(&column, resolved_agl_m))
}

/// Physical bottom-to-top column entry: height plus value plus canonical index.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PhysicalEntry {
    height_agl_m: f32,
    value: f32,
    canonical_index: usize,
}

fn validate_field_and_staggering(
    field: FieldId,
    staggering: VerticalStaggering,
) -> Result<(), VerticalSamplingError> {
    if !is_vertically_sampled(field) {
        return Err(VerticalSamplingError::UnsupportedField { field });
    }
    let supported = match field {
        FieldId::VerticalVelocity => matches!(
            staggering,
            VerticalStaggering::LevelCenter | VerticalStaggering::LevelInterface
        ),
        _ => staggering == VerticalStaggering::LevelCenter,
    };
    if !supported {
        return Err(VerticalSamplingError::WrongStaggering {
            field,
            requested: staggering,
        });
    }
    Ok(())
}

/// 3-D fields owned by vertical sampling. Mirrors the #29 `is_3d` contract
/// without reopening it: only these ids may be sampled vertically here.
fn is_vertically_sampled(field: FieldId) -> bool {
    matches!(
        field,
        FieldId::WindU
            | FieldId::WindV
            | FieldId::VerticalVelocity
            | FieldId::Temperature
            | FieldId::SpecificHumidity
            | FieldId::Pressure
            | FieldId::AirDensity
            | FieldId::DensityGradient
            | FieldId::CloudTotalWater
    )
}

fn collect_physical_column(
    runtime: VerticalRuntimeView<'_>,
    field: FieldId,
    staggering: VerticalStaggering,
    values: &[f32],
    x: usize,
    y: usize,
    ordering: VerticalOrdering,
) -> Result<Vec<PhysicalEntry>, VerticalSamplingError> {
    let (nx, ny, nz) = runtime.dimensions();
    match staggering {
        VerticalStaggering::LevelCenter => {
            let mut column = Vec::with_capacity(nz);
            for physical in 0..nz {
                let canonical = match ordering {
                    VerticalOrdering::Increasing => nz - 1 - physical,
                    VerticalOrdering::Decreasing => physical,
                };
                let height_agl_m = runtime.level(x, y, canonical)?.height_agl_m;
                let flat = volume_offset(x, y, canonical, nx, ny);
                let value =
                    values
                        .get(flat)
                        .copied()
                        .ok_or(VerticalSamplingError::ShapeMismatch {
                            field,
                            expected: nx * ny * nz,
                            actual: values.len(),
                        })?;
                if !value.is_finite() {
                    return Err(VerticalSamplingError::NonFiniteFieldValue {
                        field,
                        x,
                        y,
                        index: canonical,
                    });
                }
                column.push(PhysicalEntry {
                    height_agl_m,
                    value,
                    canonical_index: canonical,
                });
            }
            Ok(column)
        }
        VerticalStaggering::LevelInterface => {
            let mut column = Vec::with_capacity(nz + 1);
            for physical in 0..=nz {
                let canonical = match ordering {
                    VerticalOrdering::Increasing => nz - physical,
                    VerticalOrdering::Decreasing => physical,
                };
                let height_agl_m = runtime.interface(x, y, canonical)?.height_agl_m;
                let flat = volume_offset(x, y, canonical, nx, ny);
                let value =
                    values
                        .get(flat)
                        .copied()
                        .ok_or(VerticalSamplingError::ShapeMismatch {
                            field,
                            expected: nx * ny * (nz + 1),
                            actual: values.len(),
                        })?;
                if !value.is_finite() {
                    return Err(VerticalSamplingError::NonFiniteFieldValue {
                        field,
                        x,
                        y,
                        index: canonical,
                    });
                }
                column.push(PhysicalEntry {
                    height_agl_m,
                    value,
                    canonical_index: canonical,
                });
            }
            Ok(column)
        }
        VerticalStaggering::NotApplicable => Err(VerticalSamplingError::WrongStaggering {
            field,
            requested: staggering,
        }),
    }
}

fn validate_physical_column(
    x: usize,
    y: usize,
    column: &[PhysicalEntry],
) -> Result<(), VerticalSamplingError> {
    for (index, entry) in column.iter().enumerate() {
        if !entry.height_agl_m.is_finite() {
            let upper = column
                .get(index + 1)
                .map_or(entry.height_agl_m, |next| next.height_agl_m);
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index,
                lower_m: entry.height_agl_m,
                upper_m: upper,
            });
        }
        if index == 0 {
            continue;
        }
        let previous = column[index - 1].height_agl_m;
        // Heights are validated finite before this comparison, so the direct
        // ordering check matches the FLEXPART strict-increase requirement.
        if entry.height_agl_m <= previous {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index,
                lower_m: previous,
                upper_m: entry.height_agl_m,
            });
        }
    }
    Ok(())
}

/// FLEXPART METRE-mode linear primitive on a validated physical column.
///
/// Heights must already be strictly increasing bottom-to-top. Requests at or
/// below the lowest height return the lowest value (`lbounds(1)`); requests at
/// or above the highest height return the highest value (`lbounds(2)`);
/// interior requests blend linearly (`find_vert_vars_lin` + `vert_interpol`).
fn interpolate_flexpart_meter_mode(column: &[PhysicalEntry], height_agl_m: f32) -> VerticalSample {
    debug_assert!(column.len() >= 2);
    let lowest = column[0];
    let highest = column[column.len() - 1];
    if height_agl_m <= lowest.height_agl_m {
        return VerticalSample {
            value: lowest.value,
            height_agl_m,
            lower_physical_index: 0,
            upper_physical_index: 1,
            lower_canonical_index: lowest.canonical_index,
            upper_canonical_index: column[1].canonical_index,
            weight_lower: 1.0,
            weight_upper: 0.0,
            clamped_to_lower: true,
            clamped_to_upper: false,
        };
    }
    if height_agl_m >= highest.height_agl_m {
        let lower = column.len() - 2;
        let upper = column.len() - 1;
        return VerticalSample {
            value: highest.value,
            height_agl_m,
            lower_physical_index: lower,
            upper_physical_index: upper,
            lower_canonical_index: column[lower].canonical_index,
            upper_canonical_index: column[upper].canonical_index,
            weight_lower: 0.0,
            weight_upper: 1.0,
            clamped_to_lower: false,
            clamped_to_upper: true,
        };
    }
    let mut upper = 1;
    // Heights are validated finite and strictly increasing, so a direct
    // comparison reproduces FLEXPART's first-height-above-zt search.
    while upper < column.len() && column[upper].height_agl_m <= height_agl_m {
        upper += 1;
    }
    // The range check above guarantees `upper` lands strictly inside the column.
    let lower = upper - 1;
    let lower_height = column[lower].height_agl_m;
    let upper_height = column[upper].height_agl_m;
    let denominator = upper_height - lower_height;
    // Validated strictly increasing geometry keeps the denominator positive.
    let weight_upper = (height_agl_m - lower_height) / denominator;
    let weight_lower = (upper_height - height_agl_m) / denominator;
    let value = column[lower].value * weight_lower + column[upper].value * weight_upper;
    VerticalSample {
        value,
        height_agl_m,
        lower_physical_index: lower,
        upper_physical_index: upper,
        lower_canonical_index: column[lower].canonical_index,
        upper_canonical_index: column[upper].canonical_index,
        weight_lower,
        weight_upper,
        clamped_to_lower: false,
        clamped_to_upper: false,
    }
}

#[inline]
const fn volume_offset(x: usize, y: usize, z: usize, nx: usize, ny: usize) -> usize {
    x + nx * (y + ny * z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meteorology::{
        vertical::{
            reconstruct_vertical_geometry, reconstruct_vertical_geometry_with_motion,
            NativeVerticalMotion,
        },
        Snapshot,
    };

    fn synthetic_snapshot() -> Snapshot {
        serde_json::from_str(include_str!(
            "../../fixtures/vertical/synthetic-column-v1.json"
        ))
        .expect("synthetic #30 vertical fixture must parse")
    }

    #[test]
    fn center_linear_profile_reproduces_hand_computed_values() {
        let snapshot = synthetic_snapshot();
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let runtime = geometry.runtime_view().expect("runtime view");
        let (nx, ny, nz) = runtime.dimensions();
        assert_eq!((nx, ny, nz), (1, 1, 3));

        let mut heights = Vec::with_capacity(nz);
        for physical in 0..nz {
            let canonical = nz - 1 - physical;
            heights.push(runtime.level(0, 0, canonical).expect("level").height_agl_m);
        }
        assert!(heights.windows(2).all(|pair| pair[1] > pair[0]));

        let slope = 0.01_f32;
        let intercept = 5.0_f32;
        let mut values = vec![0.0_f32; nx * ny * nz];
        for physical in 0..nz {
            let canonical = nz - 1 - physical;
            values[volume_offset(0, 0, canonical, nx, ny)] = intercept + slope * heights[physical];
        }

        let mid = 0.5 * (heights[0] + heights[1]);
        let sample = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            mid,
            VerticalReference::AboveGroundLevel,
        )
        .expect("interior linear sample");
        let expected = intercept + slope * mid;
        let tolerance = 1.0e-5_f32.max(expected.abs() * 1.0e-5);
        assert!(
            (sample.value - expected).abs() <= tolerance,
            "linear profile must reproduce hand value: got {}, want {expected}",
            sample.value
        );
        assert!(!sample.clamped_to_lower && !sample.clamped_to_upper);

        let exact = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            heights[1],
            VerticalReference::AboveGroundLevel,
        )
        .expect("exact level sample");
        let exact_expected = intercept + slope * heights[1];
        assert!(
            (exact.value - exact_expected).abs() <= tolerance,
            "exact level must return level value"
        );
    }

    #[test]
    fn interface_linear_profile_uses_w_geometry() {
        let snapshot = synthetic_snapshot();
        let omega: NativeVerticalMotion = serde_json::from_str(include_str!(
            "../../fixtures/vertical/synthetic-omega-interface-v1.json"
        ))
        .expect("omega fixture must parse");
        let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &omega)
            .expect("geometry with motion");
        let runtime = geometry.runtime_view().expect("runtime view");
        let (nx, _, nz) = runtime.dimensions();

        let mut heights = Vec::with_capacity(nz + 1);
        for physical in 0..=nz {
            let canonical = nz - physical;
            heights.push(
                runtime
                    .interface(0, 0, canonical)
                    .expect("interface")
                    .height_agl_m,
            );
        }
        assert_eq!(heights[0], 0.0);
        assert!(heights.windows(2).all(|pair| pair[1] > pair[0]));

        let slope = 2.5e-5_f32;
        let intercept = 0.02_f32;
        let mut values = vec![0.0_f32; nx * (nz + 1)];
        for physical in 0..=nz {
            let canonical = nz - physical;
            values[volume_offset(0, 0, canonical, nx, 1)] = intercept + slope * heights[physical];
        }

        let query = 0.5 * (heights[1] + heights[2]);
        let sample = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelInterface,
            &values,
            0,
            0,
            query,
            VerticalReference::AboveGroundLevel,
        )
        .expect("interface linear sample");
        let expected = intercept + slope * query;
        let tolerance = 2.0e-5_f32.max(expected.abs() * 1.0e-5);
        assert!(
            (sample.value - expected).abs() <= tolerance,
            "interface linear profile must use W geometry"
        );

        let mismatched = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelInterface,
            &values,
            0,
            0,
            query,
            VerticalReference::AboveGroundLevel,
        );
        assert_eq!(
            mismatched,
            Err(VerticalSamplingError::WrongStaggering {
                field: FieldId::Temperature,
                requested: VerticalStaggering::LevelInterface
            })
        );
    }

    #[test]
    fn ambiguous_reference_and_bad_shapes_fail_closed() {
        let snapshot = synthetic_snapshot();
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let runtime = geometry.runtime_view().expect("runtime view");
        let (nx, _, nz) = runtime.dimensions();
        let values = vec![280.0_f32; nx * nz];

        assert_eq!(
            sample_vertical(
                runtime,
                FieldId::Temperature,
                VerticalStaggering::LevelCenter,
                &values,
                0,
                0,
                100.0,
                VerticalReference::ModelNative
            ),
            Err(VerticalSamplingError::AmbiguousReference {
                reference: VerticalReference::ModelNative
            })
        );
        assert!(matches!(
            sample_vertical(
                runtime,
                FieldId::Temperature,
                VerticalStaggering::LevelCenter,
                &values,
                0,
                0,
                f32::NAN,
                VerticalReference::AboveGroundLevel
            ),
            Err(VerticalSamplingError::NonFiniteHeight { .. })
        ));
        assert!(matches!(
            sample_vertical(
                runtime,
                FieldId::Temperature,
                VerticalStaggering::LevelCenter,
                &values[..values.len() - 1],
                0,
                0,
                100.0,
                VerticalReference::AboveGroundLevel
            ),
            Err(VerticalSamplingError::ShapeMismatch { .. })
        ));
        assert!(matches!(
            sample_vertical(
                runtime,
                FieldId::SurfacePressure,
                VerticalStaggering::NotApplicable,
                &[],
                0,
                0,
                100.0,
                VerticalReference::AboveGroundLevel
            ),
            Err(VerticalSamplingError::UnsupportedField { .. })
        ));
    }

    #[test]
    fn asl_requests_resolve_through_local_terrain() {
        let snapshot = synthetic_snapshot();
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let runtime = geometry.runtime_view().expect("runtime view");
        let terrain = runtime.terrain_asl_m(0, 0).expect("terrain");
        assert_eq!(terrain, 250.0);
        let values = vec![1.0_f32, 2.0, 3.0];

        let agl_query = 100.0_f32;
        let from_agl = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            agl_query,
            VerticalReference::AboveGroundLevel,
        )
        .expect("AGL sample");
        let from_asl = sample_vertical(
            runtime,
            FieldId::Temperature,
            VerticalStaggering::LevelCenter,
            &values,
            0,
            0,
            agl_query + terrain,
            VerticalReference::AboveMeanSeaLevel,
        )
        .expect("ASL sample");
        assert_eq!(from_agl.value, from_asl.value);
        assert!((from_asl.height_agl_m - agl_query).abs() <= 1.0e-6);
    }

    /// Predeclared oracle tolerances for #71 vertical fixtures.
    const ORACLE_REL_TOL: f64 = 1.0e-4;
    const ORACLE_ABS_TOL: f64 = 1.0e-6;

    fn oracle_close(actual: f64, expected: f64, what: &str) {
        let diff = (actual - expected).abs();
        let tolerance = ORACLE_ABS_TOL + ORACLE_REL_TOL * expected.abs();
        assert!(
            diff <= tolerance,
            "{what}: candidate {actual} != oracle {expected} (diff {diff}, tolerance {tolerance})"
        );
    }

    fn oracle_column(heights: &[f64], values: &[f64]) -> Vec<PhysicalEntry> {
        assert_eq!(heights.len(), values.len());
        assert!(heights.windows(2).all(|pair| pair[1] > pair[0]));
        heights
            .iter()
            .zip(values.iter())
            .enumerate()
            .map(|(index, (height, value))| PhysicalEntry {
                height_agl_m: *height as f32,
                value: *value as f32,
                canonical_index: index,
            })
            .collect()
    }

    fn vertical_case(id: &str) -> serde_json::Value {
        let source = include_str!("../../fixtures/interpolation/contract-v1.json");
        let contract: serde_json::Value =
            serde_json::from_str(source).expect("parse interpolation contract");
        contract["cases"]
            .as_array()
            .expect("cases array")
            .iter()
            .find(|case| case["id"] == id)
            .unwrap_or_else(|| panic!("oracle case {id} must exist"))
            .clone()
    }

    fn parse_oracle_profile(case: &serde_json::Value) -> (Vec<f64>, Vec<f64>, Vec<(u32, f64)>) {
        let input = case["input"]
            .as_array()
            .expect("input lines")
            .iter()
            .map(|line| line.as_str().expect("input line").to_string())
            .collect::<Vec<_>>();
        let mut cursor = 1;
        let nlevel: usize = input[cursor].trim().parse().expect("nlevel");
        cursor += 1;
        let mut heights = Vec::with_capacity(nlevel);
        for _ in 0..nlevel {
            heights.push(input[cursor].trim().parse::<f64>().expect("height"));
            cursor += 1;
        }
        let nvalues: usize = input[cursor].trim().parse().expect("nvalues");
        assert_eq!(nvalues, nlevel);
        cursor += 1;
        let mut values = Vec::with_capacity(nvalues);
        for _ in 0..nvalues {
            values.push(input[cursor].trim().parse::<f64>().expect("value"));
            cursor += 1;
        }
        let nquery: usize = input[cursor].trim().parse().expect("nquery");
        cursor += 1;
        let mut queries = Vec::with_capacity(nquery);
        for _ in 0..nquery {
            let tokens: Vec<&str> = input[cursor].split_whitespace().collect();
            assert_eq!(tokens.len(), 2, "vertical query is `coordinate zt`");
            queries.push((
                tokens[0].parse::<u32>().expect("coordinate id"),
                tokens[1].parse::<f64>().expect("zt"),
            ));
            cursor += 1;
        }
        (heights, values, queries)
    }

    fn check_vertical_oracle_case(id: &str, expected_coordinate: u32) {
        let case = vertical_case(id);
        let (heights, values, queries) = parse_oracle_profile(&case);
        let column = oracle_column(&heights, &values);
        let goldens = case["golden"]["queries"]
            .as_array()
            .expect("golden queries");
        assert_eq!(goldens.len(), queries.len(), "{id}: query count");
        for (index, ((coordinate, zt), golden)) in queries.iter().zip(goldens.iter()).enumerate() {
            assert_eq!(
                *coordinate, expected_coordinate,
                "{id} query {index}: coordinate id must match declared staggering"
            );
            let sample = interpolate_flexpart_meter_mode(&column, *zt as f32);
            let oracle_value = golden["VALUE"][0].as_f64().expect("oracle value");
            oracle_close(
                f64::from(sample.value),
                oracle_value,
                &format!("{id} query {index} value (zt={zt})"),
            );
            let indz = golden["LEVELS"][0].as_u64().expect("indz") as usize;
            let indzp = golden["LEVELS"][1].as_u64().expect("indzp") as usize;
            assert_eq!(
                sample.lower_physical_index + 1,
                indz,
                "{id} query {index}: lower level"
            );
            assert_eq!(
                sample.upper_physical_index + 1,
                indzp,
                "{id} query {index}: upper level"
            );
            let dz1 = golden["DZ"][0].as_f64().expect("dz1");
            let dz2 = golden["DZ"][1].as_f64().expect("dz2");
            oracle_close(
                f64::from(sample.weight_upper),
                dz1,
                &format!("{id} query {index} dz1"),
            );
            oracle_close(
                f64::from(sample.weight_lower),
                dz2,
                &format!("{id} query {index} dz2"),
            );
            let bounds = golden["BOUNDS"].as_array().expect("bounds");
            assert_eq!(
                sample.clamped_to_lower,
                bounds[0] == "T",
                "{id} query {index}: lower bound flag"
            );
            assert_eq!(
                sample.clamped_to_upper,
                bounds[1] == "T",
                "{id} query {index}: upper bound flag"
            );
        }
    }

    #[test]
    fn model_level_primitive_matches_flexpart_oracle() {
        check_vertical_oracle_case("vertical-model-levels", 0);
    }

    #[test]
    fn interface_primitive_matches_flexpart_oracle_on_wzlev_handoff() {
        check_vertical_oracle_case("vertical-interface-wzlev", 1);
    }

    #[test]
    fn real_era5_temperature_column_matches_flexpart_oracle() {
        check_vertical_oracle_case("real-era5-etex-temperature-column", 0);
    }

    #[test]
    fn malformed_physical_column_fails_closed() {
        fn entry(height: f32, value: f32, index: usize) -> PhysicalEntry {
            PhysicalEntry {
                height_agl_m: height,
                value,
                canonical_index: index,
            }
        }
        let good = oracle_column(&[10.0, 100.0, 1000.0], &[1.0, 2.0, 3.0]);
        assert!(validate_physical_column(0, 0, &good).is_ok());
        let flat = vec![
            entry(10.0, 1.0, 0),
            entry(10.0, 2.0, 1),
            entry(1000.0, 3.0, 2),
        ];
        assert!(matches!(
            validate_physical_column(0, 0, &flat),
            Err(VerticalSamplingError::MalformedGeometry { index: 1, .. })
        ));
        let decreasing = vec![
            entry(10.0, 1.0, 0),
            entry(5.0, 2.0, 1),
            entry(1000.0, 3.0, 2),
        ];
        assert!(matches!(
            validate_physical_column(0, 0, &decreasing),
            Err(VerticalSamplingError::MalformedGeometry { .. })
        ));
        let non_finite = vec![entry(f32::NAN, 1.0, 0), entry(100.0, 2.0, 1)];
        assert!(matches!(
            validate_physical_column(0, 0, &non_finite),
            Err(VerticalSamplingError::MalformedGeometry { .. })
        ));
    }
}
