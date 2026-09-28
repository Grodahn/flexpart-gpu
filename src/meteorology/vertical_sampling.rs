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
//! geometry. The #80-supported pressure-velocity/interface path first remaps
//! the runtime-owned W values onto FLEXPART's shared `[ground, model levels]`
//! height grid and only then samples at particle height. Direct interpolation
//! on W/interface heights is deliberately not production behavior.
//!
//! Horizontal interpolation (#72), temporal interpolation (#74),
//! accumulated-field handling (#75), 4D composition (#76), consumer migration
//! (#77) and provider decoding (#32) are out of scope.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    vertical::{
        NativeVerticalMotionKind, NativeVerticalMotionSign, NativeVerticalMotionUnit,
        NormalizedVerticalMotion, VerticalRuntimeView, VerticalTransformError,
    },
    FieldId, VerticalOrdering, VerticalReference, VerticalStaggering,
};

const PRESSURE_ALGORITHM_ID: &str = "hybrid_interface_ab_local_ps_fulllevel_adjacent_mean_v1";
const HEIGHT_ALGORITHM_ID: &str = "flexpart11_verttransform_ecmwf_heights_v1";
const W_HEIGHT_ALGORITHM_ID: &str = "flexpart11_wzlev_from_uvzlev_v1";
const INTERFACE_OMEGA_ALGORITHM_ID: &str = "omega_interface_flexpart11_pinmconv_v1";

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
    /// Physical bottom-to-top lower index (0-based) on the sampled grid.
    pub lower_physical_index: usize,
    /// Physical bottom-to-top upper index (0-based, always `lower + 1`).
    pub upper_physical_index: usize,
    /// Canonical storage index of the lower sampled-grid level (depends on ordering).
    pub lower_canonical_index: usize,
    /// Canonical storage index of the upper sampled-grid level (depends on ordering).
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
    /// Interface motion is outside the exact pressure-omega contract frozen by #80.
    #[error(
        "unsupported interface vertical-motion semantics: kind={kind:?}, unit={unit:?}, sign={sign:?}, source_staggering={source_staggering:?}, output_staggering={output_staggering:?}"
    )]
    UnsupportedInterfaceVerticalMotion {
        kind: NativeVerticalMotionKind,
        unit: NativeVerticalMotionUnit,
        sign: NativeVerticalMotionSign,
        source_staggering: VerticalStaggering,
        output_staggering: VerticalStaggering,
    },
    /// Runtime provenance does not identify the #30/#80 geometry or conversion.
    #[error("unsupported interface vertical-motion runtime contract: {reason}")]
    UnsupportedInterfaceRuntime { reason: &'static str },
    /// Canonical vertical velocity must come from the same #30 runtime transform.
    #[error("vertical velocity is absent from the #30 runtime transform")]
    MissingRuntimeVerticalMotion,
    /// Callers must not recombine external vertical-motion values with runtime geometry.
    #[error("vertical velocity values must come from the #30 runtime transform")]
    ExternalVerticalMotionValues,
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
    /// The frozen #80 remapping contract requires at least two model levels.
    #[error("insufficient model levels for interface vertical-motion remapping: nz={nz}")]
    InsufficientInterfaceLevels { nz: usize },
    /// Wrapped #30 runtime access failure (out-of-bounds column or corrupt shape).
    #[error(transparent)]
    Runtime(#[from] VerticalTransformError),
}

/// Sample one vertical column on the authoritative #30 runtime geometry.
///
/// For ordinary model-level fields, `values` holds the complete canonical
/// volume in X-fastest `[x,y,z]` order with `z` following the Snapshot's
/// declared storage ordering and length `nx*ny*nz`. For `VerticalVelocity`,
/// `values` must be empty: values and retained staggering are read from the
/// same #30 runtime transform as the geometry, preventing callers from
/// recombining independently derived motion and heights.
/// Interface-staggered motion is currently limited to #80's single-column
/// pressure-omega contract, which has no horizontal eta-surface slope
/// correction. Multi-column interface motion fails closed until a normative
/// horizontal remapping contract supplies the required U/V context.
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

    let values = if field == FieldId::VerticalVelocity {
        if !values.is_empty() {
            return Err(VerticalSamplingError::ExternalVerticalMotionValues);
        }
        let motion = runtime
            .vertical_velocity()
            .ok_or(VerticalSamplingError::MissingRuntimeVerticalMotion)?;
        let retained = motion.vertical_staggering();
        if retained != staggering {
            return Err(VerticalSamplingError::WrongStaggering {
                field,
                requested: staggering,
            });
        }
        if retained == VerticalStaggering::LevelInterface {
            return sample_interface_vertical_motion(runtime, motion, x, y, resolved_agl_m);
        }
        debug_assert_eq!(retained, VerticalStaggering::LevelCenter);
        motion.values_ms()
    } else {
        values
    };

    let (nx, ny, nz) = runtime.dimensions();
    let horizontal = nx
        .checked_mul(ny)
        .ok_or(VerticalSamplingError::ShapeMismatch {
            field,
            expected: usize::MAX,
            actual: values.len(),
        })?;
    let expected = horizontal
        .checked_mul(nz)
        .ok_or(VerticalSamplingError::ShapeMismatch {
            field,
            expected: usize::MAX,
            actual: values.len(),
        })?;
    if values.len() != expected {
        return Err(VerticalSamplingError::ShapeMismatch {
            field,
            expected,
            actual: values.len(),
        });
    }

    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientLevels { nz });
    }

    let ordering = runtime.provenance().source_vertical_ordering;
    let column = collect_physical_column(runtime, field, values, x, y, ordering)?;
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
    let supported_staggering = if field == FieldId::VerticalVelocity {
        matches!(
            staggering,
            VerticalStaggering::LevelCenter | VerticalStaggering::LevelInterface
        )
    } else {
        staggering == VerticalStaggering::LevelCenter
    };
    if !supported_staggering {
        return Err(VerticalSamplingError::WrongStaggering {
            field,
            requested: staggering,
        });
    }
    Ok(())
}

/// Reproduce FLEXPART 11.1's `eta=no` two-stage interface-W production path.
///
/// Ported from `verttransform_mod.f90:544-545,607-622` and
/// `interpol_mod.f90:1022-1027,1651-1703` at pinned revision
/// `c70586c2b7f5258850705325881c61f557ea9bd8`. The #30 runtime already owns
/// the `omega * pinmconv` conversion and W/interface heights. This function
/// remaps those values to FLEXPART's shared `[ground, model levels]` height
/// grid before applying the ordinary meter-coordinate sampling primitive.
fn sample_interface_vertical_motion(
    runtime: VerticalRuntimeView<'_>,
    motion: &NormalizedVerticalMotion,
    x: usize,
    y: usize,
    height_agl_m: f32,
) -> Result<VerticalSample, VerticalSamplingError> {
    validate_interface_runtime_contract(runtime, motion)?;

    let (nx, ny, nz) = runtime.dimensions();
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels { nz });
    }
    let horizontal = nx
        .checked_mul(ny)
        .ok_or(VerticalSamplingError::ShapeMismatch {
            field: FieldId::VerticalVelocity,
            expected: usize::MAX,
            actual: motion.values_ms().len(),
        })?;
    let expected = horizontal
        .checked_mul(nz + 1)
        .ok_or(VerticalSamplingError::ShapeMismatch {
            field: FieldId::VerticalVelocity,
            expected: usize::MAX,
            actual: motion.values_ms().len(),
        })?;
    if motion.values_ms().len() != expected {
        return Err(VerticalSamplingError::ShapeMismatch {
            field: FieldId::VerticalVelocity,
            expected,
            actual: motion.values_ms().len(),
        });
    }

    let ordering = runtime.provenance().source_vertical_ordering;
    let interfaces = collect_physical_interface_column(runtime, motion, x, y, ordering)?;
    validate_physical_column(x, y, &interfaces)?;
    let model_grid = remap_interface_motion_to_model_grid(runtime, x, y, ordering, &interfaces)?;
    validate_physical_column(x, y, &model_grid)?;

    Ok(interpolate_flexpart_meter_mode(&model_grid, height_agl_m))
}

fn validate_interface_runtime_contract(
    runtime: VerticalRuntimeView<'_>,
    motion: &NormalizedVerticalMotion,
) -> Result<(), VerticalSamplingError> {
    let provenance = motion.provenance();
    let supported_motion = provenance.source_kind
        == NativeVerticalMotionKind::PressureVelocityOmega
        && provenance.source_unit == NativeVerticalMotionUnit::PascalPerSecond
        && provenance.source_sign == NativeVerticalMotionSign::PositivePressureIncreasing
        && provenance.source_vertical_staggering == VerticalStaggering::LevelInterface
        && provenance.output_vertical_staggering == VerticalStaggering::LevelInterface
        && provenance.algorithm_id == INTERFACE_OMEGA_ALGORITHM_ID;
    if !supported_motion {
        return Err(VerticalSamplingError::UnsupportedInterfaceVerticalMotion {
            kind: provenance.source_kind,
            unit: provenance.source_unit,
            sign: provenance.source_sign,
            source_staggering: provenance.source_vertical_staggering,
            output_staggering: provenance.output_vertical_staggering,
        });
    }

    let runtime_provenance = runtime.provenance();
    let (nx, ny, nz) = runtime.dimensions();
    if nx != 1 || ny != 1 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "#80 freezes a single vertical column without horizontal slope correction",
        });
    }
    if runtime_provenance.source_level_count != nz {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "runtime level count differs from the #30 source provenance",
        });
    }
    if runtime_provenance.pressure_algorithm_id != PRESSURE_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "pressure reconstruction is not the #30 hybrid-interface contract",
        });
    }
    if runtime_provenance.height_algorithm_id != HEIGHT_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "model heights are not the #30 FLEXPART height contract",
        });
    }
    if runtime_provenance.w_height_algorithm_id != W_HEIGHT_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "W/interface heights are not the #30 FLEXPART wzlev contract",
        });
    }
    if runtime_provenance.terrain_reference != VerticalReference::AboveMeanSeaLevel {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "runtime terrain reference is not explicit ASL",
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
    values: &[f32],
    x: usize,
    y: usize,
    ordering: VerticalOrdering,
) -> Result<Vec<PhysicalEntry>, VerticalSamplingError> {
    let (nx, ny, nz) = runtime.dimensions();
    let mut column = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = match ordering {
            VerticalOrdering::Increasing => nz - 1 - physical,
            VerticalOrdering::Decreasing => physical,
        };
        let height_agl_m = runtime.level(x, y, canonical)?.height_agl_m;
        let flat = volume_offset(x, y, canonical, nx, ny);
        let value = values
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

fn collect_physical_interface_column(
    runtime: VerticalRuntimeView<'_>,
    motion: &NormalizedVerticalMotion,
    x: usize,
    y: usize,
    ordering: VerticalOrdering,
) -> Result<Vec<PhysicalEntry>, VerticalSamplingError> {
    let (nx, ny, nz) = runtime.dimensions();
    let interface_count = nz + 1;
    let mut column = Vec::with_capacity(interface_count);
    for physical in 0..interface_count {
        let canonical = canonical_index(ordering, interface_count, physical);
        let height_agl_m = runtime.interface(x, y, canonical)?.height_agl_m;
        let flat = volume_offset(x, y, canonical, nx, ny);
        let value =
            motion
                .values_ms()
                .get(flat)
                .copied()
                .ok_or(VerticalSamplingError::ShapeMismatch {
                    field: FieldId::VerticalVelocity,
                    expected: nx * ny * interface_count,
                    actual: motion.values_ms().len(),
                })?;
        if !value.is_finite() {
            return Err(VerticalSamplingError::NonFiniteFieldValue {
                field: FieldId::VerticalVelocity,
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

fn remap_interface_motion_to_model_grid(
    runtime: VerticalRuntimeView<'_>,
    x: usize,
    y: usize,
    ordering: VerticalOrdering,
    interfaces: &[PhysicalEntry],
) -> Result<Vec<PhysicalEntry>, VerticalSamplingError> {
    let (_, _, nz) = runtime.dimensions();
    debug_assert_eq!(interfaces.len(), nz + 1);

    if interfaces[0].height_agl_m != 0.0 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the #80 shared height grid requires a zero-metre AGL ground boundary",
        });
    }

    let grid_count = nz + 1;
    let mut model_grid = Vec::with_capacity(grid_count);
    model_grid.push(PhysicalEntry {
        height_agl_m: interfaces[0].height_agl_m,
        value: interfaces[0].value,
        canonical_index: canonical_index(ordering, grid_count, 0),
    });

    // FLEXPART assigns the native interface boundary values directly at the
    // first and last shared height levels. Only the strict interior shared
    // model heights pass through the interface-height remapping.
    for model_physical in 1..nz {
        let source_physical = model_physical - 1;
        let source_canonical = canonical_index(ordering, nz, source_physical);
        let height_agl_m = runtime.level(x, y, source_canonical)?.height_agl_m;
        if height_agl_m <= interfaces[0].height_agl_m
            || height_agl_m >= interfaces[interfaces.len() - 1].height_agl_m
        {
            return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
                reason: "an interior shared model height is outside the W/interface domain",
            });
        }
        let value = interpolate_flexpart_meter_mode(interfaces, height_agl_m).value;
        model_grid.push(PhysicalEntry {
            height_agl_m,
            value,
            canonical_index: canonical_index(ordering, grid_count, model_physical),
        });
    }

    let top_model_canonical = canonical_index(ordering, nz, nz - 1);
    let top_model_height_agl_m = runtime.level(x, y, top_model_canonical)?.height_agl_m;
    if top_model_height_agl_m <= model_grid[model_grid.len() - 1].height_agl_m
        || top_model_height_agl_m > interfaces[interfaces.len() - 1].height_agl_m
    {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the top shared model height is incompatible with the W/interface domain",
        });
    }
    model_grid.push(PhysicalEntry {
        height_agl_m: top_model_height_agl_m,
        value: interfaces[interfaces.len() - 1].value,
        canonical_index: canonical_index(ordering, grid_count, nz),
    });

    Ok(model_grid)
}

#[inline]
const fn canonical_index(ordering: VerticalOrdering, count: usize, physical_index: usize) -> usize {
    match ordering {
        VerticalOrdering::Increasing => count - 1 - physical_index,
        VerticalOrdering::Decreasing => physical_index,
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
            NativeVerticalMotion, NativeVerticalMotionKind, NativeVerticalMotionProvenance,
            NativeVerticalMotionSign, NativeVerticalMotionUnit,
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
    fn interface_vertical_motion_matches_frozen_issue_80_production_oracle() {
        let snapshot = synthetic_snapshot();
        let omega: NativeVerticalMotion = serde_json::from_str(include_str!(
            "../../fixtures/vertical/synthetic-omega-interface-nonlinear-v1.json"
        ))
        .expect("omega fixture must parse");
        let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &omega)
            .expect("geometry with motion");
        let runtime = geometry.runtime_view().expect("runtime view");
        let report: serde_json::Value = serde_json::from_str(include_str!(
            "../../fixtures/interpolation/w-production-oracle-v1.json"
        ))
        .expect("#80 report must parse");
        let absolute = report["tolerances"]["absolute_m_s"]
            .as_f64()
            .expect("absolute tolerance");
        let relative = report["tolerances"]["relative"]
            .as_f64()
            .expect("relative tolerance");
        let queries = report["synthetic_case"]["queries"]
            .as_array()
            .expect("oracle queries");
        let comparisons = report["synthetic_case"]["comparisons"]
            .as_array()
            .expect("direct-interface comparisons");
        assert_eq!(queries.len(), comparisons.len());

        for (query, comparison) in queries.iter().zip(comparisons) {
            let query_number = query["query"].as_u64().expect("query number");
            let height = checked_f64_to_f32(
                query["particle_height_m_agl"]
                    .as_f64()
                    .expect("particle height"),
                "#80 particle height",
            );
            let expected = query["pristine_w_m_s"].as_f64().expect("pristine W");
            let sample = sample_vertical(
                runtime,
                FieldId::VerticalVelocity,
                VerticalStaggering::LevelInterface,
                &[],
                0,
                0,
                height,
                VerticalReference::AboveGroundLevel,
            )
            .expect("supported interface W sample");
            let actual = f64::from(sample.value);
            let tolerance = absolute + relative * actual.abs().max(expected.abs());
            assert!(
                (actual - expected).abs() <= tolerance,
                "#80 query {query_number}: candidate {actual} != pristine {expected} (tolerance {tolerance})"
            );

            let direct = comparison["direct_interface_w_m_s"]
                .as_f64()
                .expect("direct-interface W");
            if !comparison["equivalent"]
                .as_bool()
                .expect("equivalence verdict")
            {
                assert!(
                    (actual - direct).abs() > tolerance,
                    "#80 query {query_number}: candidate followed the rejected direct-interface value"
                );
            }
        }
    }

    #[test]
    fn unsupported_geometric_interface_motion_fails_closed() {
        let snapshot = synthetic_snapshot();
        let motion = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::GeometricVelocity,
            unit: NativeVerticalMotionUnit::MeterPerSecond,
            sign: NativeVerticalMotionSign::PositiveUpward,
            vertical_staggering: VerticalStaggering::LevelInterface,
            values: vec![0.3, 0.2, 0.1, 0.0],
            provenance: NativeVerticalMotionProvenance {
                source_id: "unsupported-geometric-interface-test".to_string(),
            },
        };
        let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &motion)
            .expect("geometry with geometric interface motion");
        let runtime = geometry.runtime_view().expect("runtime view");
        assert!(matches!(
            sample_vertical(
                runtime,
                FieldId::VerticalVelocity,
                VerticalStaggering::LevelInterface,
                &[],
                0,
                0,
                100.0,
                VerticalReference::AboveGroundLevel,
            ),
            Err(VerticalSamplingError::UnsupportedInterfaceVerticalMotion { .. })
        ));
    }

    #[test]
    fn center_vertical_motion_is_sourced_from_runtime_transform() {
        let snapshot = synthetic_snapshot();
        let motion = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::GeometricVelocity,
            unit: NativeVerticalMotionUnit::MeterPerSecond,
            sign: NativeVerticalMotionSign::PositiveUpward,
            vertical_staggering: VerticalStaggering::LevelCenter,
            values: vec![0.3, 0.2, 0.1],
            provenance: NativeVerticalMotionProvenance {
                source_id: "vertical-sampling-center-test".to_string(),
            },
        };
        let geometry = reconstruct_vertical_geometry_with_motion(&snapshot, &motion)
            .expect("geometry with center motion");
        let runtime = geometry.runtime_view().expect("runtime view");
        let (_, _, nz) = runtime.dimensions();
        let lower_height = runtime
            .level(0, 0, nz - 1)
            .expect("lowest level")
            .height_agl_m;
        let upper_height = runtime
            .level(0, 0, nz - 2)
            .expect("next level")
            .height_agl_m;
        let sample = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelCenter,
            &[],
            0,
            0,
            lower_height.midpoint(upper_height),
            VerticalReference::AboveGroundLevel,
        )
        .expect("runtime-owned center motion sample");
        assert!((sample.value - 0.15).abs() <= 1.0e-6);

        let external_values = sample_vertical(
            runtime,
            FieldId::VerticalVelocity,
            VerticalStaggering::LevelCenter,
            &[9.0, 9.0, 9.0],
            0,
            0,
            lower_height,
            VerticalReference::AboveGroundLevel,
        );
        assert_eq!(
            external_values,
            Err(VerticalSamplingError::ExternalVerticalMotionValues)
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

    fn checked_f64_to_f32(value: f64, what: &str) -> f32 {
        assert!(value.is_finite(), "{what}: non-finite f64 input");
        assert!(
            value >= f64::from(f32::MIN) && value <= f64::from(f32::MAX),
            "{what}: f64 input is outside the supported f32 range"
        );
        // The range check makes this the explicit oracle-f64 -> runtime-f32
        // rounding boundary. Production inputs are already canonical f32.
        let rounded = value as f32;
        assert!(rounded.is_finite(), "{what}: f32 rounding was non-finite");
        rounded
    }

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
                height_agl_m: checked_f64_to_f32(*height, "oracle height"),
                value: checked_f64_to_f32(*value, "oracle value"),
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
            let sample = interpolate_flexpart_meter_mode(
                &column,
                checked_f64_to_f32(*zt, "oracle query height"),
            );
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
