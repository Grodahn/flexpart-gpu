//! Distinct FLEXPART W/interface heights and pressure-to-height derivatives.

use super::super::{FieldId, Snapshot, VerticalOrdering};
use super::layout::{
    field_values, interface_offset, model_level_index_from_surface, surface_offset, volume_offset,
};
use super::{VerticalTransformError, VerticalTransformResult};

/// Preserve FLEXPART `pinmconv` on W/interfaces, including the artificial ground level.
/// Ported from `verttransform_mod.f90:2031-2042` at the pinned 11.1 revision.
pub(super) fn pressure_velocity_interfaces_to_geometric(
    snapshot: &Snapshot,
    geometry: &VerticalTransformResult,
    omega_pa_s: &[f32],
) -> Result<Vec<f32>, VerticalTransformError> {
    let nx = geometry.nx;
    let ny = geometry.ny;
    let nz = geometry.nz;
    if nz < 1 {
        return Err(VerticalTransformError::InsufficientVerticalLevels);
    }
    let surface_pressure = field_values(snapshot, FieldId::SurfacePressure)?;
    let mut result = vec![0.0_f32; omega_pa_s.len()];

    for y in 0..ny {
        for x in 0..nx {
            let horizontal_index = surface_offset(x, y, nx);
            let ps = surface_pressure[horizontal_index];

            // FLEXPART physical indexing is bottom -> top and includes the
            // artificial ground UV level. Rebuild that exact center sequence.
            let mut center_height_bottom_up = Vec::with_capacity(nz + 1);
            let mut center_pressure_bottom_up = Vec::with_capacity(nz + 1);
            center_height_bottom_up.push(0.0);
            center_pressure_bottom_up.push(ps);
            for step in 0..nz {
                let z =
                    model_level_index_from_surface(snapshot.vertical_coordinate.ordering, nz, step);
                let index = volume_offset(x, y, z, nx, ny);
                center_height_bottom_up.push(geometry.height_agl_m[index]);
                center_pressure_bottom_up.push(geometry.level_pressure_pa[index]);
            }

            let mut pinmconv_bottom_up = vec![0.0_f32; nz + 1];
            for k in 0..=nz {
                let (a, b) = derivative_pair(k, nz + 1);
                let dz = center_height_bottom_up[b] - center_height_bottom_up[a];
                let dp = center_pressure_bottom_up[b] - center_pressure_bottom_up[a];
                let dzdp = dz / dp;
                if !dzdp.is_finite() || dzdp >= 0.0 {
                    return Err(VerticalTransformError::InvalidPressureToHeightDerivative {
                        x,
                        y,
                        z: k,
                    });
                }
                pinmconv_bottom_up[k] = dzdp;
            }

            for interface in 0..=nz {
                let physical_index = match snapshot.vertical_coordinate.ordering {
                    VerticalOrdering::Increasing => nz - interface,
                    VerticalOrdering::Decreasing => interface,
                };
                let canonical_index = interface_offset(x, y, interface, nx, ny);
                result[canonical_index] =
                    omega_pa_s[canonical_index] * pinmconv_bottom_up[physical_index];
            }
        }
    }
    Ok(result)
}

#[inline]
const fn derivative_pair(index: usize, count: usize) -> (usize, usize) {
    if index == 0 {
        (0, 1)
    } else if index + 1 == count {
        (count - 2, count - 1)
    } else {
        (index - 1, index + 1)
    }
}

/// Keep W heights distinct from model heights while anchoring the local surface at 0 m AGL.
/// Ported from `verttransform_mod.f90:2024-2029` at the pinned 11.1 revision.
pub(super) fn reconstruct_flexpart_w_heights(
    ordering: VerticalOrdering,
    nx: usize,
    ny: usize,
    nz: usize,
    level_height_agl_m: &[f32],
    terrain_asl_m: &[f32],
) -> Result<(Vec<f32>, Vec<f32>), VerticalTransformError> {
    if nz == 0 {
        return Err(VerticalTransformError::InsufficientVerticalLevels);
    }

    let mut interface_agl = vec![0.0_f32; nx * ny * (nz + 1)];
    let mut interface_asl = vec![0.0_f32; nx * ny * (nz + 1)];

    for y in 0..ny {
        for x in 0..nx {
            let terrain = terrain_asl_m[surface_offset(x, y, nx)];
            let mut uv_bottom_up = Vec::with_capacity(nz + 1);
            uv_bottom_up.push(0.0_f32);
            for step in 0..nz {
                let z = model_level_index_from_surface(ordering, nz, step);
                uv_bottom_up.push(level_height_agl_m[volume_offset(x, y, z, nx, ny)]);
            }

            let mut w_bottom_up = vec![0.0_f32; nz + 1];
            if nz == 1 {
                w_bottom_up[1] = uv_bottom_up[1];
            } else {
                for k in 1..nz {
                    w_bottom_up[k] = 0.5 * (uv_bottom_up[k] + uv_bottom_up[k + 1]);
                }
                w_bottom_up[nz] = w_bottom_up[nz - 1] + uv_bottom_up[nz] - uv_bottom_up[nz - 1];
            }

            for k in 1..=nz {
                if !w_bottom_up[k].is_finite() || w_bottom_up[k] <= w_bottom_up[k - 1] {
                    return Err(VerticalTransformError::InvalidHeightColumn {
                        x,
                        y,
                        z: k,
                        height_agl_m: w_bottom_up[k],
                    });
                }
            }

            for interface in 0..=nz {
                let physical_index = match ordering {
                    VerticalOrdering::Increasing => nz - interface,
                    VerticalOrdering::Decreasing => interface,
                };
                let index = interface_offset(x, y, interface, nx, ny);
                let height_asl_m = w_bottom_up[physical_index] + terrain;
                if !height_asl_m.is_finite() {
                    return Err(VerticalTransformError::InvalidAbsoluteHeight {
                        x,
                        y,
                        z: interface,
                        height_asl_m,
                        terrain_asl_m: terrain,
                    });
                }
                interface_agl[index] = w_bottom_up[physical_index];
                interface_asl[index] = height_asl_m;
            }
        }
    }

    Ok((interface_agl, interface_asl))
}

#[cfg(test)]
#[path = "tests/interfaces.rs"]
mod tests;
