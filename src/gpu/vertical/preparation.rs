//! Validate and extract canonical runtime columns for GPU upload without sampling.
use super::error::GpuVerticalError;
use crate::meteorology::{
    vertical::{NormalizedVerticalMotion, VerticalRuntimeView},
    vertical_sampling::VerticalSamplingError,
    FieldId, VerticalOrdering, VerticalReference, VerticalStaggering,
};

/// Expected #30 pressure reconstruction algorithm.
const EXPECTED_PRESSURE_ALGORITHM_ID: &str =
    "hybrid_interface_ab_local_ps_fulllevel_adjacent_mean_v1";

/// Expected #30 height reconstruction algorithm.
const EXPECTED_HEIGHT_ALGORITHM_ID: &str = "flexpart11_verttransform_ecmwf_heights_v1";

/// Expected #30 W height reconstruction algorithm.
const EXPECTED_W_HEIGHT_ALGORITHM_ID: &str = "flexpart11_wzlev_from_uvzlev_v1";

/// Expected #80 interface omega conversion algorithm.
const EXPECTED_INTERFACE_OMEGA_ALGORITHM_ID: &str = "omega_interface_flexpart11_pinmconv_v1";

#[inline]
const fn canonical_index(ordering: VerticalOrdering, count: usize, physical_index: usize) -> usize {
    match ordering {
        VerticalOrdering::Increasing => count - 1 - physical_index,
        VerticalOrdering::Decreasing => physical_index,
    }
}

#[inline]
const fn volume_offset(x: usize, y: usize, z: usize, nx: usize, ny: usize) -> usize {
    x + nx * (y + ny * z)
}

/// Extract physical bottom-to-top model-level columns from the #30 runtime.
///
/// Mirrors the CPU `collect_physical_column` ordering, finiteness, shape, and
/// monotonicity checks without computing a sampled value. Both storage
/// orderings are supported; the returned lanes are always physical
/// bottom-to-top for device upload.
///
/// # Errors
/// Returns [`GpuVerticalError`] for any #73 fail-closed condition.
pub fn physical_model_column_from_runtime(
    runtime: VerticalRuntimeView<'_>,
    field: FieldId,
    values: &[f32],
    x: usize,
    y: usize,
) -> Result<(Vec<f32>, Vec<f32>), GpuVerticalError> {
    if !is_vertically_sampled(field) {
        return Err(VerticalSamplingError::UnsupportedField { field }.into());
    }
    if field == FieldId::VerticalVelocity {
        return Err(VerticalSamplingError::WrongStaggering {
            field,
            requested: VerticalStaggering::LevelCenter,
        }
        .into());
    }
    let (nx, ny, nz) = runtime.dimensions();
    let horizontal = nx.checked_mul(ny).ok_or(GpuVerticalError::SizeOverflow {
        field: "vertical_horizontal",
    })?;
    let expected = horizontal
        .checked_mul(nz)
        .ok_or(GpuVerticalError::SizeOverflow {
            field: "vertical_volume",
        })?;
    if values.len() != expected {
        return Err(VerticalSamplingError::ShapeMismatch {
            field,
            expected,
            actual: values.len(),
        }
        .into());
    }
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientLevels { nz }.into());
    }
    let ordering = runtime.provenance().source_vertical_ordering;
    let mut heights = Vec::with_capacity(nz);
    let mut lane = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = canonical_index(ordering, nz, physical);
        let point = runtime.level(x, y, canonical)?;
        if !point.height_agl_m.is_finite() {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index: physical,
                lower_m: point.height_agl_m,
                upper_m: point.height_agl_m,
            }
            .into());
        }
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
            }
            .into());
        }
        heights.push(point.height_agl_m);
        lane.push(value);
    }
    for (index, pair) in heights.windows(2).enumerate() {
        if !(pair[1] > pair[0]) {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index,
                lower_m: pair[0],
                upper_m: pair[1],
            }
            .into());
        }
    }
    Ok((heights, lane))
}

/// Extract a physical bottom-to-top center-staggered vertical-motion column.
///
/// This is the GPU counterpart of the CPU center-motion branch in
/// [`crate::meteorology::vertical_sampling::sample_vertical`]: values and
/// retained staggering come from the same #30 runtime transform as the
/// geometry, so callers cannot recombine independently derived motion with
/// runtime heights. Only [`VerticalStaggering::LevelCenter`] motion is
/// accepted here; interface-staggered motion follows the two-stage
/// [`physical_w_columns_from_runtime`] production path instead.
///
/// # Errors
///
/// Returns [`GpuVerticalError`] for missing motion, wrong staggering,
/// non-finite values, or malformed geometry, mirroring the CPU
/// fail-closed conditions.
pub fn physical_center_w_column_from_runtime(
    runtime: VerticalRuntimeView<'_>,
    x: usize,
    y: usize,
) -> Result<(Vec<f32>, Vec<f32>), GpuVerticalError> {
    let motion = runtime
        .vertical_velocity()
        .ok_or(VerticalSamplingError::MissingRuntimeVerticalMotion)?;
    if motion.vertical_staggering() != VerticalStaggering::LevelCenter {
        return Err(VerticalSamplingError::WrongStaggering {
            field: FieldId::VerticalVelocity,
            requested: VerticalStaggering::LevelCenter,
        }
        .into());
    }
    let (nx, ny, nz) = runtime.dimensions();
    let ordering = runtime.provenance().source_vertical_ordering;
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientLevels { nz }.into());
    }
    let mut heights = Vec::with_capacity(nz);
    let mut lane = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = canonical_index(ordering, nz, physical);
        let point = runtime.level(x, y, canonical)?;
        if !point.height_agl_m.is_finite() {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index: physical,
                lower_m: point.height_agl_m,
                upper_m: point.height_agl_m,
            }
            .into());
        }
        let flat = volume_offset(x, y, canonical, nx, ny);
        let value =
            motion
                .values_ms()
                .get(flat)
                .copied()
                .ok_or(VerticalSamplingError::ShapeMismatch {
                    field: FieldId::VerticalVelocity,
                    expected: nx * ny * nz,
                    actual: motion.values_ms().len(),
                })?;
        if !value.is_finite() {
            return Err(VerticalSamplingError::NonFiniteFieldValue {
                field: FieldId::VerticalVelocity,
                x,
                y,
                index: canonical,
            }
            .into());
        }
        heights.push(point.height_agl_m);
        lane.push(value);
    }
    for (index, pair) in heights.windows(2).enumerate() {
        if !(pair[1] > pair[0]) {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index,
                lower_m: pair[0],
                upper_m: pair[1],
            }
            .into());
        }
    }
    Ok((heights, lane))
}

/// Extract physical W/interface columns from the #30 runtime with the #80 contract.
///
/// Returns `(interface_heights, interface_values, level_heights,
/// shared_heights)` in physical bottom-to-top order. `shared_heights` is
/// `[0.0, level_heights...]`. Enforces the single-column, provenance,
/// algorithm, ground, domain, and top-compatibility checks owned by #80.
///
/// # Errors
/// Returns [`GpuVerticalError`] for any #73/#80 fail-closed condition.
#[allow(clippy::type_complexity)]
pub fn physical_w_columns_from_runtime(
    runtime: VerticalRuntimeView<'_>,
    x: usize,
    y: usize,
) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>), GpuVerticalError> {
    let motion = runtime
        .vertical_velocity()
        .ok_or(VerticalSamplingError::MissingRuntimeVerticalMotion)?;
    if motion.vertical_staggering() != VerticalStaggering::LevelInterface {
        return Err(VerticalSamplingError::WrongStaggering {
            field: FieldId::VerticalVelocity,
            requested: VerticalStaggering::LevelCenter,
        }
        .into());
    }
    validate_interface_runtime_contract(runtime, motion)?;

    let (nx, ny, nz) = runtime.dimensions();
    if nz < 2 {
        return Err(VerticalSamplingError::InsufficientInterfaceLevels { nz }.into());
    }
    let ordering = runtime.provenance().source_vertical_ordering;
    let interface_count = nz + 1;
    let mut interface_heights = Vec::with_capacity(interface_count);
    let mut interface_values = Vec::with_capacity(interface_count);
    for physical in 0..interface_count {
        let canonical = canonical_index(ordering, interface_count, physical);
        let point = runtime.interface(x, y, canonical)?;
        if !point.height_agl_m.is_finite() {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index: physical,
                lower_m: point.height_agl_m,
                upper_m: point.height_agl_m,
            }
            .into());
        }
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
            }
            .into());
        }
        interface_heights.push(point.height_agl_m);
        interface_values.push(value);
    }
    let mut level_heights = Vec::with_capacity(nz);
    for physical in 0..nz {
        let canonical = canonical_index(ordering, nz, physical);
        let point = runtime.level(x, y, canonical)?;
        if !point.height_agl_m.is_finite() {
            return Err(VerticalSamplingError::MalformedGeometry {
                x,
                y,
                index: physical,
                lower_m: point.height_agl_m,
                upper_m: point.height_agl_m,
            }
            .into());
        }
        level_heights.push(point.height_agl_m);
    }
    for lane in [&interface_heights, &level_heights] {
        for (index, pair) in lane.windows(2).enumerate() {
            if !(pair[1] > pair[0]) {
                return Err(VerticalSamplingError::MalformedGeometry {
                    x,
                    y,
                    index,
                    lower_m: pair[0],
                    upper_m: pair[1],
                }
                .into());
            }
        }
    }
    if interface_heights[0] != 0.0 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the #80 shared height grid requires a zero-metre AGL ground boundary",
        }
        .into());
    }
    let interface_top = interface_heights[interface_count - 1];
    for height in level_heights.iter().take(nz - 1) {
        if *height <= interface_heights[0] || *height >= interface_top {
            return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
                reason: "an interior shared model height is outside the W/interface domain",
            }
            .into());
        }
    }
    let top_level = level_heights[nz - 1];
    let previous_shared = if nz >= 2 { level_heights[nz - 2] } else { 0.0 };
    if top_level <= previous_shared || top_level > interface_top {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "the top shared model height is incompatible with the W/interface domain",
        }
        .into());
    }

    let mut shared_heights = Vec::with_capacity(nz + 1);
    shared_heights.push(interface_heights[0]);
    shared_heights.extend_from_slice(&level_heights);
    debug_assert_eq!(shared_heights.len(), nz + 1);
    Ok((
        interface_heights,
        interface_values,
        level_heights,
        shared_heights,
    ))
}

fn validate_interface_runtime_contract(
    runtime: VerticalRuntimeView<'_>,
    motion: &NormalizedVerticalMotion,
) -> Result<(), GpuVerticalError> {
    use crate::meteorology::vertical::{
        NativeVerticalMotionKind, NativeVerticalMotionSign, NativeVerticalMotionUnit,
    };
    let provenance = motion.provenance();
    let supported_motion = provenance.source_kind
        == NativeVerticalMotionKind::PressureVelocityOmega
        && provenance.source_unit == NativeVerticalMotionUnit::PascalPerSecond
        && provenance.source_sign == NativeVerticalMotionSign::PositivePressureIncreasing
        && provenance.source_vertical_staggering == VerticalStaggering::LevelInterface
        && provenance.output_vertical_staggering == VerticalStaggering::LevelInterface
        && provenance.algorithm_id == EXPECTED_INTERFACE_OMEGA_ALGORITHM_ID;
    if !supported_motion {
        return Err(VerticalSamplingError::UnsupportedInterfaceVerticalMotion {
            kind: provenance.source_kind,
            unit: provenance.source_unit,
            sign: provenance.source_sign,
            source_staggering: provenance.source_vertical_staggering,
            output_staggering: provenance.output_vertical_staggering,
        }
        .into());
    }
    let runtime_provenance = runtime.provenance();
    let (nx, ny, nz) = runtime.dimensions();
    if nx != 1 || ny != 1 {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "#80 freezes a single vertical column without horizontal slope correction",
        }
        .into());
    }
    if runtime_provenance.source_level_count != nz {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "runtime level count differs from the #30 source provenance",
        }
        .into());
    }
    if runtime_provenance.pressure_algorithm_id != EXPECTED_PRESSURE_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "pressure reconstruction is not the #30 hybrid-interface contract",
        }
        .into());
    }
    if runtime_provenance.height_algorithm_id != EXPECTED_HEIGHT_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "model heights are not the #30 FLEXPART height contract",
        }
        .into());
    }
    if runtime_provenance.w_height_algorithm_id != EXPECTED_W_HEIGHT_ALGORITHM_ID {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "W/interface heights are not the #30 FLEXPART wzlev contract",
        }
        .into());
    }
    if runtime_provenance.terrain_reference != VerticalReference::AboveMeanSeaLevel {
        return Err(VerticalSamplingError::UnsupportedInterfaceRuntime {
            reason: "runtime terrain reference is not explicit ASL",
        }
        .into());
    }
    Ok(())
}

pub(super) fn is_vertically_sampled(field: FieldId) -> bool {
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

/// Resolve query heights to AGL through the column's local #30 terrain.
///
/// `AboveGroundLevel` passes through; `AboveMeanSeaLevel` subtracts terrain;
/// `ModelNative` fails closed as ambiguous.
///
/// # Errors
/// Returns [`GpuVerticalError`] for ambiguous references or non-finite heights.
pub fn resolve_query_heights_agl(
    runtime: VerticalRuntimeView<'_>,
    x: usize,
    y: usize,
    heights_m: &[f32],
    reference: VerticalReference,
) -> Result<Vec<f32>, GpuVerticalError> {
    if heights_m.is_empty() {
        return Err(GpuVerticalError::EmptyQueries);
    }
    for height in heights_m {
        if !height.is_finite() {
            return Err(VerticalSamplingError::NonFiniteHeight { height_m: *height }.into());
        }
    }
    match reference {
        VerticalReference::AboveGroundLevel => Ok(heights_m.to_vec()),
        VerticalReference::AboveMeanSeaLevel => {
            let terrain = runtime.terrain_asl_m(x, y)?;
            let mut resolved = Vec::with_capacity(heights_m.len());
            for height in heights_m {
                let agl = *height - terrain;
                if !agl.is_finite() {
                    return Err(VerticalSamplingError::NonFiniteHeight { height_m: agl }.into());
                }
                resolved.push(agl);
            }
            Ok(resolved)
        }
        VerticalReference::ModelNative => {
            Err(VerticalSamplingError::AmbiguousReference { reference }.into())
        }
    }
}
