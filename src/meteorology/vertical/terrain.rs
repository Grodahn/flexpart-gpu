//! Explicit local-terrain AGL/ASL and release-height reference resolution.

use super::super::VerticalReference;
use super::layout::{surface_offset, volume_offset};
use super::{VerticalRuntimeView, VerticalTransformError};
use serde::{Deserialize, Serialize};

/// Explicit terrain-dependent release-height resolution for #28.
///
/// This type preserves both references so downstream release/injection code
/// never has to infer whether a source height was AGL or ASL.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ResolvedReleaseHeight {
    pub input_m: f32,
    pub input_reference: VerticalReference,
    pub terrain_asl_m: f32,
    pub height_agl_m: f32,
    pub height_asl_m: f32,
}

/// Ordered release bounds retain both metre-based references over local terrain.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ResolvedReleaseHeightRange {
    pub lower: ResolvedReleaseHeight,
    pub upper: ResolvedReleaseHeight,
}

/// Resolve one explicitly referenced release height against local terrain.
///
/// AGL must be non-negative. ASL below local terrain is impossible for a
/// release point and fails closed. ModelNative is never accepted here.
pub fn resolve_release_height(
    height_m: f32,
    reference: VerticalReference,
    terrain_asl_m: f32,
) -> Result<ResolvedReleaseHeight, VerticalTransformError> {
    if !height_m.is_finite() || !terrain_asl_m.is_finite() {
        return Err(VerticalTransformError::InvalidReleaseHeight {
            height_m,
            reference,
            terrain_asl_m,
        });
    }

    let (height_agl_m, height_asl_m) = match reference {
        VerticalReference::AboveGroundLevel => {
            if height_m < 0.0 {
                return Err(VerticalTransformError::InvalidReleaseHeight {
                    height_m,
                    reference,
                    terrain_asl_m,
                });
            }
            (height_m, height_m + terrain_asl_m)
        }
        VerticalReference::AboveMeanSeaLevel => {
            let agl = height_m - terrain_asl_m;
            if agl < 0.0 {
                return Err(VerticalTransformError::InvalidReleaseHeight {
                    height_m,
                    reference,
                    terrain_asl_m,
                });
            }
            (agl, height_m)
        }
        VerticalReference::ModelNative => {
            return Err(VerticalTransformError::UnsupportedReleaseHeightReference { reference });
        }
    };

    if !height_agl_m.is_finite() || !height_asl_m.is_finite() {
        return Err(VerticalTransformError::InvalidReleaseHeight {
            height_m,
            reference,
            terrain_asl_m,
        });
    }

    Ok(ResolvedReleaseHeight {
        input_m: height_m,
        input_reference: reference,
        terrain_asl_m,
        height_agl_m,
        height_asl_m,
    })
}

/// Resolve ordered release-height bounds with one explicit reference.
pub fn resolve_release_height_range(
    lower_m: f32,
    upper_m: f32,
    reference: VerticalReference,
    terrain_asl_m: f32,
) -> Result<ResolvedReleaseHeightRange, VerticalTransformError> {
    if lower_m > upper_m {
        return Err(VerticalTransformError::InvalidReleaseHeightRange {
            lower_m,
            upper_m,
            reference,
        });
    }
    let lower = resolve_release_height(lower_m, reference, terrain_asl_m)?;
    let upper = resolve_release_height(upper_m, reference, terrain_asl_m)?;
    Ok(ResolvedReleaseHeightRange { lower, upper })
}

/// Resolve ordered release bounds using terrain from a validated runtime column.
pub fn resolve_release_height_range_at_column(
    runtime: VerticalRuntimeView<'_>,
    x: usize,
    y: usize,
    lower_m: f32,
    upper_m: f32,
    reference: VerticalReference,
) -> Result<ResolvedReleaseHeightRange, VerticalTransformError> {
    resolve_release_height_range(lower_m, upper_m, reference, runtime.terrain_asl_m(x, y)?)
}

/// Resolve a release height using terrain from a validated #30 runtime column.
pub fn resolve_release_height_at_column(
    runtime: VerticalRuntimeView<'_>,
    x: usize,
    y: usize,
    height_m: f32,
    reference: VerticalReference,
) -> Result<ResolvedReleaseHeight, VerticalTransformError> {
    resolve_release_height(height_m, reference, runtime.terrain_asl_m(x, y)?)
}

/// Convert a column-wise ASL height field to AGL using local terrain.
///
/// Heights meaningfully below terrain fail closed instead of becoming a
/// silently negative AGL coordinate.
pub fn height_asl_to_agl(
    nx: usize,
    ny: usize,
    nz: usize,
    height_asl_m: &[f32],
    terrain_asl_m: &[f32],
) -> Result<Vec<f32>, VerticalTransformError> {
    validate_height_shapes(nx, ny, nz, height_asl_m, terrain_asl_m)?;
    let mut result = Vec::with_capacity(height_asl_m.len());

    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let index = volume_offset(x, y, z, nx, ny);
                let terrain = terrain_asl_m[surface_offset(x, y, nx)];
                let height = height_asl_m[index];
                let agl = height - terrain;
                if !height.is_finite() || !terrain.is_finite() || !agl.is_finite() {
                    return Err(VerticalTransformError::InvalidAbsoluteHeight {
                        x,
                        y,
                        z,
                        height_asl_m: height,
                        terrain_asl_m: terrain,
                    });
                }
                if height < terrain {
                    return Err(VerticalTransformError::HeightBelowTerrain {
                        x,
                        y,
                        z,
                        height_asl_m: height,
                        terrain_asl_m: terrain,
                    });
                }
                result.push(agl);
            }
        }
    }
    Ok(result)
}

/// Convert a column-wise AGL height field to ASL using local terrain.
pub fn height_agl_to_asl(
    nx: usize,
    ny: usize,
    nz: usize,
    height_agl_m: &[f32],
    terrain_asl_m: &[f32],
) -> Result<Vec<f32>, VerticalTransformError> {
    validate_height_shapes(nx, ny, nz, height_agl_m, terrain_asl_m)?;
    let mut result = Vec::with_capacity(height_agl_m.len());

    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let index = volume_offset(x, y, z, nx, ny);
                let terrain = terrain_asl_m[surface_offset(x, y, nx)];
                let height = height_agl_m[index];
                let asl = height + terrain;
                if !height.is_finite() || !terrain.is_finite() || !asl.is_finite() {
                    return Err(VerticalTransformError::InvalidAbsoluteHeight {
                        x,
                        y,
                        z,
                        height_asl_m: asl,
                        terrain_asl_m: terrain,
                    });
                }
                if height < 0.0 {
                    return Err(VerticalTransformError::HeightBelowTerrain {
                        x,
                        y,
                        z,
                        height_asl_m: asl,
                        terrain_asl_m: terrain,
                    });
                }
                result.push(asl);
            }
        }
    }
    Ok(result)
}

fn validate_height_shapes(
    nx: usize,
    ny: usize,
    nz: usize,
    heights: &[f32],
    terrain: &[f32],
) -> Result<(), VerticalTransformError> {
    let horizontal = nx
        .checked_mul(ny)
        .ok_or(VerticalTransformError::ShapeMismatch {
            field: "terrain_asl_m",
            expected: usize::MAX,
            actual: terrain.len(),
        })?;
    let volume = horizontal
        .checked_mul(nz)
        .ok_or(VerticalTransformError::ShapeMismatch {
            field: "height",
            expected: usize::MAX,
            actual: heights.len(),
        })?;
    if terrain.len() != horizontal {
        return Err(VerticalTransformError::ShapeMismatch {
            field: "terrain_asl_m",
            expected: horizontal,
            actual: terrain.len(),
        });
    }
    if heights.len() != volume {
        return Err(VerticalTransformError::ShapeMismatch {
            field: "height",
            expected: volume,
            actual: heights.len(),
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/terrain.rs"]
mod tests;
