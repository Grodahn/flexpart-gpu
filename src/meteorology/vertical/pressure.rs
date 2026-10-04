//! Column-local hybrid pressure reconstruction and physical surface anchoring.

use super::super::{FieldId, Requirements, Snapshot, VerticalCoordinateKind, VerticalOrdering};
use super::layout::{surface_offset, volume_offset};
use super::VerticalTransformError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Pressure reconstructed independently for every horizontal column.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HybridPressureGrid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// X-fastest \`[x,y,z_interface]\` storage.
    pub interface_pressure_pa: Vec<f32>,
    /// X-fastest \`[x,y,z_level]\` storage.
    pub level_pressure_pa: Vec<f32>,
}

fn validation_requirements() -> Requirements {
    Requirements {
        required_fields: BTreeSet::new(),
    }
}

/// Reconstruct hybrid interface and full-level pressure using the actual local
/// canonical surface pressure.
///
/// The canonical #29 interface A/B coefficients are authoritative. No fixed
/// level count or hard-coded index direction is assumed.
pub fn reconstruct_hybrid_pressure(
    snapshot: &Snapshot,
) -> Result<HybridPressureGrid, VerticalTransformError> {
    snapshot.validate(&validation_requirements())?;

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
    let surface_pressure = snapshot
        .fields
        .iter()
        .find(|field| field.id == FieldId::SurfacePressure)
        .ok_or(VerticalTransformError::MissingField(
            FieldId::SurfacePressure,
        ))?;

    let nx = snapshot.horizontal_grid.nx;
    let ny = snapshot.horizontal_grid.ny;
    let nz = vertical.level_values.len();
    let interface_count = nz + 1;
    let expected_surface_count =
        nx.checked_mul(ny)
            .ok_or(VerticalTransformError::ShapeMismatch {
                field: "surface_pressure",
                expected: usize::MAX,
                actual: surface_pressure.values.len(),
            })?;
    if surface_pressure.values.len() != expected_surface_count {
        return Err(VerticalTransformError::ShapeMismatch {
            field: "surface_pressure",
            expected: expected_surface_count,
            actual: surface_pressure.values.len(),
        });
    }

    let mut interfaces = vec![0.0_f32; expected_surface_count * interface_count];
    let mut levels = vec![0.0_f32; expected_surface_count * nz];

    for y in 0..ny {
        for x in 0..nx {
            let horizontal = surface_offset(x, y, nx);
            let ps = surface_pressure.values[horizontal];
            if !ps.is_finite() || ps <= 0.0 {
                return Err(VerticalTransformError::InvalidSurfacePressure {
                    x,
                    y,
                    pressure_pa: ps,
                });
            }

            for k in 0..interface_count {
                // Keep the canonical equation explicit: interface pressure is
                // reconstructed from native A/B coefficients and local surface
                // pressure for this column.
                let pressure = a[k] + b[k] * ps;
                if !pressure.is_finite() || pressure < 0.0 {
                    return Err(VerticalTransformError::InvalidPressureColumn { x, y, index: k });
                }
                interfaces[volume_offset(x, y, k, nx, ny)] = pressure;
            }

            // The FLEXPART W/interface geometry is anchored at the physical
            // surface (0 m AGL). The corresponding hybrid interface must
            // therefore be the actual local surface pressure, not merely a
            // coefficient set that happens to match the reference pressure.
            let surface_interface = match vertical.ordering {
                VerticalOrdering::Increasing => nz,
                VerticalOrdering::Decreasing => 0,
            };
            let surface_interface_pressure =
                interfaces[volume_offset(x, y, surface_interface, nx, ny)];
            if !pressure_close(surface_interface_pressure, ps) {
                return Err(VerticalTransformError::SurfaceInterfacePressureMismatch {
                    x,
                    y,
                    interface_pressure_pa: surface_interface_pressure,
                    surface_pressure_pa: ps,
                });
            }

            validate_column_ordering(
                &interfaces,
                x,
                y,
                interface_count,
                nx,
                ny,
                vertical.ordering,
                true,
            )?;

            for k in 0..nz {
                let lower = interfaces[volume_offset(x, y, k, nx, ny)];
                let upper = interfaces[volume_offset(x, y, k + 1, nx, ny)];
                let pressure = 0.5 * (lower + upper);
                if !pressure.is_finite() || pressure <= 0.0 {
                    return Err(VerticalTransformError::InvalidPressureColumn { x, y, index: k });
                }
                levels[volume_offset(x, y, k, nx, ny)] = pressure;
            }

            validate_column_ordering(&levels, x, y, nz, nx, ny, vertical.ordering, false)?;
        }
    }

    Ok(HybridPressureGrid {
        nx,
        ny,
        nz,
        interface_pressure_pa: interfaces,
        level_pressure_pa: levels,
    })
}

fn pressure_close(actual: f32, expected: f32) -> bool {
    let tolerance = 0.05_f32.max(expected.abs() * 1.0e-6);
    (actual - expected).abs() <= tolerance
}

fn validate_column_ordering(
    values: &[f32],
    x: usize,
    y: usize,
    count: usize,
    nx: usize,
    ny: usize,
    ordering: VerticalOrdering,
    allow_zero_endpoint: bool,
) -> Result<(), VerticalTransformError> {
    for k in 0..count {
        let value = values[volume_offset(x, y, k, nx, ny)];
        if !value.is_finite() || (!allow_zero_endpoint && value <= 0.0) || value < 0.0 {
            return Err(VerticalTransformError::InvalidPressureColumn { x, y, index: k });
        }
        if k == 0 {
            continue;
        }
        let previous = values[volume_offset(x, y, k - 1, nx, ny)];
        let ordered = match ordering {
            VerticalOrdering::Increasing => value > previous,
            VerticalOrdering::Decreasing => value < previous,
        };
        if !ordered {
            return Err(VerticalTransformError::InvalidPressureColumn { x, y, index: k });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/pressure.rs"]
mod tests;
