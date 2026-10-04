//! Pinned FLEXPART thermodynamics and model-level height reconstruction.

use super::super::{FieldId, Requirements, Snapshot, VerticalOrdering};
use super::interfaces::reconstruct_flexpart_w_heights;
use super::layout::{field_values, model_level_index_from_surface, surface_offset, volume_offset};
use super::{
    reconstruct_hybrid_pressure, VerticalTransformError, VerticalTransformProvenance,
    VerticalTransformResult,
};
use crate::constants::{GA, R_AIR};

fn vertical_transform_requirements() -> Requirements {
    Requirements {
        required_fields: [
            FieldId::SurfacePressure,
            FieldId::Temperature,
            FieldId::SpecificHumidity,
            FieldId::Orography,
            FieldId::Temperature2m,
            FieldId::Dewpoint2m,
        ]
        .into_iter()
        .collect(),
    }
}

/// Reconstruct FLEXPART-11.1-compatible model-level heights for every column.
///
/// FLEXPART \`verttransform_ecmwf_heights\` starts at the local surface with
/// AGL=0, surface pressure, and virtual temperature derived from 2-m
/// temperature/dew point. It then integrates upward through the model levels
/// using model-level temperature and specific humidity. The canonical snapshot
/// contains only the real model levels (the FLEXPART surface pseudo-level is
/// not serialized), so this routine visits the surface-most real level first
/// and preserves the snapshot's declared level ordering in its outputs.
///
/// Orography is canonical metres ASL. The hypsometric integral therefore
/// produces AGL directly; ASL is obtained by adding local orography.
/// Ported from `verttransform_mod.f90:1997-2022` at the pinned 11.1 revision.
pub fn reconstruct_vertical_geometry(
    snapshot: &Snapshot,
) -> Result<VerticalTransformResult, VerticalTransformError> {
    snapshot.validate(&vertical_transform_requirements())?;
    let pressure = reconstruct_hybrid_pressure(snapshot)?;

    let surface_pressure = field_values(snapshot, FieldId::SurfacePressure)?;
    let temperature = field_values(snapshot, FieldId::Temperature)?;
    let humidity = field_values(snapshot, FieldId::SpecificHumidity)?;
    let terrain = field_values(snapshot, FieldId::Orography)?;
    let temperature_2m = field_values(snapshot, FieldId::Temperature2m)?;
    let dewpoint_2m = field_values(snapshot, FieldId::Dewpoint2m)?;

    let nx = pressure.nx;
    let ny = pressure.ny;
    let nz = pressure.nz;
    let horizontal = nx * ny;
    let volume = horizontal * nz;

    for (name, values, expected) in [
        ("surface_pressure", surface_pressure, horizontal),
        ("orography", terrain, horizontal),
        ("temperature_2m", temperature_2m, horizontal),
        ("dewpoint_2m", dewpoint_2m, horizontal),
        ("temperature", temperature, volume),
        ("specific_humidity", humidity, volume),
    ] {
        if values.len() != expected {
            return Err(VerticalTransformError::ShapeMismatch {
                field: name,
                expected,
                actual: values.len(),
            });
        }
    }

    let mut height_agl_m = vec![0.0_f32; volume];
    let mut height_asl_m = vec![0.0_f32; volume];

    for y in 0..ny {
        for x in 0..nx {
            let horizontal_index = surface_offset(x, y, nx);
            let ps = surface_pressure[horizontal_index];
            let terrain_m = terrain[horizontal_index];
            let mut previous_virtual_temperature_k = surface_virtual_temperature_k(
                temperature_2m[horizontal_index],
                dewpoint_2m[horizontal_index],
                ps,
            )
            .ok_or(VerticalTransformError::InvalidSurfaceThermodynamics {
                x,
                y,
                temperature_k: temperature_2m[horizontal_index],
                dewpoint_k: dewpoint_2m[horizontal_index],
                pressure_pa: ps,
            })?;
            if !terrain_m.is_finite() {
                return Err(VerticalTransformError::InvalidHeightColumn {
                    x,
                    y,
                    z: surface_level_index(snapshot.vertical_coordinate.ordering, nz),
                    height_agl_m: terrain_m,
                });
            }

            let mut previous_pressure_pa = ps;
            let mut previous_height_agl_m = 0.0_f32;

            for step in 0..nz {
                let z =
                    model_level_index_from_surface(snapshot.vertical_coordinate.ordering, nz, step);
                let index = volume_offset(x, y, z, nx, ny);
                let current_pressure_pa = pressure.level_pressure_pa[index];
                if !current_pressure_pa.is_finite()
                    || current_pressure_pa <= 0.0
                    || current_pressure_pa >= previous_pressure_pa
                {
                    return Err(VerticalTransformError::InvalidPressureColumn { x, y, index: z });
                }

                let current_virtual_temperature_k =
                    model_virtual_temperature_k(temperature[index], humidity[index]).ok_or(
                        VerticalTransformError::InvalidThermodynamics {
                            x,
                            y,
                            z,
                            temperature_k: temperature[index],
                            specific_humidity: humidity[index],
                        },
                    )?;

                let layer_thickness_m = flexpart_hypsometric_layer_thickness_m(
                    previous_pressure_pa,
                    current_pressure_pa,
                    previous_virtual_temperature_k,
                    current_virtual_temperature_k,
                );
                let current_height_agl_m = previous_height_agl_m + layer_thickness_m;
                if !current_height_agl_m.is_finite()
                    || current_height_agl_m <= previous_height_agl_m
                {
                    return Err(VerticalTransformError::InvalidHeightColumn {
                        x,
                        y,
                        z,
                        height_agl_m: current_height_agl_m,
                    });
                }

                let current_height_asl_m = current_height_agl_m + terrain_m;
                if !current_height_asl_m.is_finite() {
                    return Err(VerticalTransformError::InvalidAbsoluteHeight {
                        x,
                        y,
                        z,
                        height_asl_m: current_height_asl_m,
                        terrain_asl_m: terrain_m,
                    });
                }
                height_agl_m[index] = current_height_agl_m;
                height_asl_m[index] = current_height_asl_m;

                previous_pressure_pa = current_pressure_pa;
                previous_virtual_temperature_k = current_virtual_temperature_k;
                previous_height_agl_m = current_height_agl_m;
            }
        }
    }

    let (interface_height_agl_m, interface_height_asl_m) = reconstruct_flexpart_w_heights(
        snapshot.vertical_coordinate.ordering,
        nx,
        ny,
        nz,
        &height_agl_m,
        terrain,
    )?;

    Ok(VerticalTransformResult {
        nx,
        ny,
        nz,
        interface_pressure_pa: pressure.interface_pressure_pa,
        interface_height_asl_m,
        interface_height_agl_m,
        level_pressure_pa: pressure.level_pressure_pa,
        terrain_asl_m: terrain.to_vec(),
        height_asl_m,
        height_agl_m,
        vertical_velocity: None,
        provenance: VerticalTransformProvenance::from_snapshot(snapshot)?,
    })
}

fn surface_virtual_temperature_k(
    temperature_2m_k: f32,
    dewpoint_2m_k: f32,
    surface_pressure_pa: f32,
) -> Option<f32> {
    if !temperature_2m_k.is_finite()
        || temperature_2m_k <= 0.0
        || !dewpoint_2m_k.is_finite()
        || dewpoint_2m_k <= 0.0
        || !surface_pressure_pa.is_finite()
        || surface_pressure_pa <= 0.0
    {
        return None;
    }

    let vapor_pressure_pa = flexpart_ew_pa(dewpoint_2m_k)?;
    let virtual_temperature_k =
        temperature_2m_k * (1.0 + 0.378 * vapor_pressure_pa / surface_pressure_pa);
    (virtual_temperature_k.is_finite() && virtual_temperature_k > 0.0)
        .then_some(virtual_temperature_k)
}

fn model_virtual_temperature_k(temperature_k: f32, specific_humidity: f32) -> Option<f32> {
    if !temperature_k.is_finite()
        || temperature_k <= 0.0
        || !specific_humidity.is_finite()
        || !(0.0..=1.0).contains(&specific_humidity)
    {
        return None;
    }
    let virtual_temperature_k = temperature_k * (1.0 + 0.608 * specific_humidity);
    (virtual_temperature_k.is_finite() && virtual_temperature_k > 0.0)
        .then_some(virtual_temperature_k)
}

/// FLEXPART \`qvsat_mod::ew\`: Goff-Gratch saturation vapor pressure over water.
///
/// The pressure argument in the Fortran function is unused; this helper
/// therefore takes only temperature in kelvin and returns pascals.
fn flexpart_ew_pa(temperature_k: f32) -> Option<f32> {
    if !temperature_k.is_finite() || temperature_k <= 0.0 {
        return None;
    }

    let y = 373.16 / temperature_k;
    if !y.is_finite() || y <= 0.0 {
        return None;
    }
    let mut exponent = -7.90298 * (y - 1.0);
    exponent += 5.02808 * 0.43429 * y.ln();

    let c_power = (1.0 - 1.0 / y) * 11.344;
    let c = -1.3816 * (10.0_f32.powf(c_power) - 1.0) / 10.0_f32.powi(7);
    let d_power = (1.0 - y) * 3.49149;
    let d = 8.1328 * (10.0_f32.powf(d_power) - 1.0) / 10.0_f32.powi(3);
    exponent += c + d;

    let vapor_pressure_pa = 101_324.6 * 10.0_f32.powf(exponent);
    (vapor_pressure_pa.is_finite() && vapor_pressure_pa >= 0.0).then_some(vapor_pressure_pa)
}

/// FLEXPART-11.1 hypsometric layer integration.
///
/// The >0.2 K virtual-temperature branch is preserved exactly from
/// \`verttransform_ecmwf_heights\`; the near-isothermal branch avoids the
/// logarithmic-temperature quotient.
fn flexpart_hypsometric_layer_thickness_m(
    previous_pressure_pa: f32,
    current_pressure_pa: f32,
    previous_virtual_temperature_k: f32,
    current_virtual_temperature_k: f32,
) -> f32 {
    let scale = R_AIR / GA;
    let pressure_log = (previous_pressure_pa / current_pressure_pa).ln();
    let delta_virtual_temperature = current_virtual_temperature_k - previous_virtual_temperature_k;

    if delta_virtual_temperature.abs() > 0.2 {
        scale * pressure_log * delta_virtual_temperature
            / (current_virtual_temperature_k / previous_virtual_temperature_k).ln()
    } else {
        scale * pressure_log * current_virtual_temperature_k
    }
}

#[inline]
const fn surface_level_index(ordering: VerticalOrdering, nz: usize) -> usize {
    match ordering {
        VerticalOrdering::Increasing => nz - 1,
        VerticalOrdering::Decreasing => 0,
    }
}

#[cfg(test)]
#[path = "tests/model_levels.rs"]
mod tests;
