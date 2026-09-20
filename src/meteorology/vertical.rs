//! Provider-independent vertical transformation support for canonical meteorology.
//!
//! This module is the #30 production boundary between the immutable canonical
//! \`meteorology::Snapshot\` and derived three-dimensional runtime geometry.
//! It deliberately does not depend on the legacy \`WindFieldGrid\` or
//! \`VerticalCoordinates\` types.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::constants::{GA, R_AIR};

use super::{
    ContractError, FieldId, Requirements, Snapshot, VerticalCoordinateKind, VerticalOrdering,
    VerticalStaggering, SCHEMA_ID, SCHEMA_VERSION,
};

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
    EtaCoordinateVelocity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeVerticalMotionUnit {
    MeterPerSecond,
    PascalPerSecond,
    PerSecond,
}

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalizedVerticalMotion {
    pub vertical_staggering: VerticalStaggering,
    pub values_ms: Vec<f32>,
    pub provenance: NormalizedVerticalMotionProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedVerticalMotionProvenance {
    pub source_id: String,
    pub source_kind: NativeVerticalMotionKind,
    pub source_unit: NativeVerticalMotionUnit,
    pub source_sign: NativeVerticalMotionSign,
    pub conversion: String,
}

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

/// Final derived vertical runtime state consumed by later sampling/interpolation.
///
/// Heights are column-wise 3-D fields; they must never be collapsed to one
/// horizontally averaged vertical profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerticalTransformResult {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub interface_pressure_pa: Vec<f32>,
    pub level_pressure_pa: Vec<f32>,
    pub terrain_asl_m: Vec<f32>,
    pub height_asl_m: Vec<f32>,
    pub height_agl_m: Vec<f32>,
    pub vertical_velocity: Option<NormalizedVerticalMotion>,
    pub provenance: VerticalTransformProvenance,
}

/// Machine-readable identity of the canonical vertical transformation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerticalTransformProvenance {
    pub source_schema_id: String,
    pub source_schema_version: u32,
    pub pressure_reconstruction: String,
    pub height_reconstruction: String,
    pub height_reference: String,
}

impl Default for VerticalTransformProvenance {
    fn default() -> Self {
        Self {
            source_schema_id: SCHEMA_ID.to_string(),
            source_schema_version: SCHEMA_VERSION,
            pressure_reconstruction: "p_interface=a_interface+b_interface*local_surface_pressure; p_level=mean(adjacent_interfaces)".to_string(),
            height_reconstruction: "FLEXPART-11.1 verttransform_ecmwf_heights hypsometric integration from surface virtual temperature (T2m/dewpoint) through model-level T/q".to_string(),
            height_reference: "agl_integrated_from_local_surface; asl=agl+orography_asl".to_string(),
        }
    }
}

/// Errors returned by canonical vertical transformations.
#[derive(Debug, Error, PartialEq)]
pub enum VerticalTransformError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("vertical transform requires hybrid sigma-pressure coordinates")]
    UnsupportedVerticalCoordinate,
    #[error("missing canonical field {0:?}")]
    MissingField(FieldId),
    #[error("invalid local surface pressure at (x={x}, y={y}): {pressure_pa} Pa")]
    InvalidSurfacePressure {
        x: usize,
        y: usize,
        pressure_pa: f32,
    },
    #[error(
        "reconstructed pressure is invalid/non-monotonic at (x={x}, y={y}, index={index})"
    )]
    InvalidPressureColumn {
        x: usize,
        y: usize,
        index: usize,
    },
    #[error("invalid surface thermodynamic state at (x={x}, y={y}): T2m={temperature_k} K, Td2m={dewpoint_k} K, ps={pressure_pa} Pa")]
    InvalidSurfaceThermodynamics {
        x: usize,
        y: usize,
        temperature_k: f32,
        dewpoint_k: f32,
        pressure_pa: f32,
    },
    #[error("invalid model-level thermodynamic state at (x={x}, y={y}, z={z}): T={temperature_k} K, q={specific_humidity}")]
    InvalidThermodynamics {
        x: usize,
        y: usize,
        z: usize,
        temperature_k: f32,
        specific_humidity: f32,
    },
    #[error("invalid reconstructed height at (x={x}, y={y}, z={z}): {height_agl_m} m AGL")]
    InvalidHeightColumn {
        x: usize,
        y: usize,
        z: usize,
        height_agl_m: f32,
    },
    #[error("shape mismatch for {field}: expected {expected}, got {actual}")]
    ShapeMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("unsupported or ambiguous native vertical-motion semantics: {reason}")]
    InvalidNativeVerticalMotion { reason: &'static str },
    #[error("invalid native vertical-motion value at index {index}: {value}")]
    InvalidNativeVerticalMotionValue { index: usize, value: f32 },
    #[error("vertical-motion conversion requires at least two model levels")]
    InsufficientVerticalLevels,
    #[error("invalid dz/dp conversion at (x={x}, y={y}, z={z})")]
    InvalidPressureToHeightDerivative { x: usize, y: usize, z: usize },
    #[error("height at (x={x}, y={y}, z={z}) lies below terrain: ASL={height_asl_m} m, terrain={terrain_asl_m} m")]
    HeightBelowTerrain {
        x: usize,
        y: usize,
        z: usize,
        height_asl_m: f32,
        terrain_asl_m: f32,
    },
}

fn validation_requirements() -> Requirements {
    Requirements {
        required_fields: BTreeSet::new(),
    }
}

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
        .ok_or(VerticalTransformError::MissingField(FieldId::SurfacePressure))?;

    let nx = snapshot.horizontal_grid.nx;
    let ny = snapshot.horizontal_grid.ny;
    let nz = vertical.level_values.len();
    let interface_count = nz + 1;
    let expected_surface_count = nx
        .checked_mul(ny)
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
                    return Err(VerticalTransformError::InvalidPressureColumn {
                        x,
                        y,
                        index: k,
                    });
                }
                interfaces[volume_offset(x, y, k, nx, ny)] = pressure;
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
                    return Err(VerticalTransformError::InvalidPressureColumn {
                        x,
                        y,
                        index: k,
                    });
                }
                levels[volume_offset(x, y, k, nx, ny)] = pressure;
            }

            validate_column_ordering(
                &levels,
                x,
                y,
                nz,
                nx,
                ny,
                vertical.ordering,
                false,
            )?;
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
                let z = model_level_index_from_surface(
                    snapshot.vertical_coordinate.ordering,
                    nz,
                    step,
                );
                let index = volume_offset(x, y, z, nx, ny);
                let current_pressure_pa = pressure.level_pressure_pa[index];
                if !current_pressure_pa.is_finite()
                    || current_pressure_pa <= 0.0
                    || current_pressure_pa >= previous_pressure_pa
                {
                    return Err(VerticalTransformError::InvalidPressureColumn {
                        x,
                        y,
                        index: z,
                    });
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

                height_agl_m[index] = current_height_agl_m;
                height_asl_m[index] = current_height_agl_m + terrain_m;

                previous_pressure_pa = current_pressure_pa;
                previous_virtual_temperature_k = current_virtual_temperature_k;
                previous_height_agl_m = current_height_agl_m;
            }
        }
    }

    Ok(VerticalTransformResult {
        nx,
        ny,
        nz,
        interface_pressure_pa: pressure.interface_pressure_pa,
        level_pressure_pa: pressure.level_pressure_pa,
        terrain_asl_m: terrain.to_vec(),
        height_asl_m,
        height_agl_m,
        vertical_velocity: None,
        provenance: VerticalTransformProvenance::default(),
    })
}

/// Reconstruct vertical geometry and normalize an optional native vertical
/// motion field without performing vertical interpolation (#31 owns that).
pub fn reconstruct_vertical_geometry_with_motion(
    snapshot: &Snapshot,
    native_motion: &NativeVerticalMotion,
) -> Result<VerticalTransformResult, VerticalTransformError> {
    let mut result = reconstruct_vertical_geometry(snapshot)?;
    result.vertical_velocity = Some(normalize_vertical_motion(
        snapshot,
        &result,
        native_motion,
    )?);
    Ok(result)
}

/// Normalize native vertical motion to geometric m/s, positive upward.
///
/// Pressure velocity follows FLEXPART's pinmconv concept: multiply omega
/// [Pa/s] by dz/dp [m/Pa]. Eta-dot first becomes the FLEXPART-ready pressure
/// vertical velocity using the centered half-level reconstruction used by
/// FLEXPART preprocessing, then follows the same pressure-to-height step.
pub fn normalize_vertical_motion(
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
            return Err(VerticalTransformError::InvalidNativeVerticalMotionValue {
                index,
                value,
            });
        }
    }

    let (vertical_staggering, values_ms, conversion) = match native_motion.kind {
        NativeVerticalMotionKind::GeometricVelocity => (
            native_motion.vertical_staggering,
            native_motion.values.clone(),
            "identity: already geometric m/s positive upward".to_string(),
        ),
        NativeVerticalMotionKind::PressureVelocityOmega => match native_motion.vertical_staggering {
            VerticalStaggering::LevelCenter => (
                VerticalStaggering::LevelCenter,
                pressure_velocity_centers_to_geometric(snapshot, geometry, &native_motion.values)?,
                "omega[Pa/s] * dz/dp on model centers -> geometric m/s positive upward"
                    .to_string(),
            ),
            VerticalStaggering::LevelInterface => (
                VerticalStaggering::LevelInterface,
                pressure_velocity_interfaces_to_geometric(
                    snapshot,
                    geometry,
                    &native_motion.values,
                )?,
                "omega[Pa/s] * FLEXPART-style pinmconv dz/dp on W/interfaces -> geometric m/s positive upward"
                    .to_string(),
            ),
            VerticalStaggering::NotApplicable => unreachable!(),
        },
        NativeVerticalMotionKind::EtaCoordinateVelocity => {
            if native_motion.vertical_staggering != VerticalStaggering::LevelCenter {
                return Err(VerticalTransformError::InvalidNativeVerticalMotion {
                    reason: "eta-dot must be supplied on model-level centers",
                });
            }
            let omega_interfaces =
                eta_dot_to_pressure_velocity_interfaces(snapshot, &native_motion.values, native_motion.sign)?;
            let values_ms =
                pressure_velocity_interfaces_to_geometric(snapshot, geometry, &omega_interfaces)?;
            (
                VerticalStaggering::LevelInterface,
                values_ms,
                "eta-dot[1/s] -> centered FLEXPART-ready pressure velocity on interfaces; omega[Pa/s] * FLEXPART-style pinmconv dz/dp -> geometric m/s positive upward"
                    .to_string(),
            )
        }
    };

    Ok(NormalizedVerticalMotion {
        vertical_staggering,
        values_ms,
        provenance: NormalizedVerticalMotionProvenance {
            source_id: native_motion.provenance.source_id.clone(),
            source_kind: native_motion.kind,
            source_unit: native_motion.unit,
            source_sign: native_motion.sign,
            conversion,
        },
    })
}

fn validate_native_motion_semantics(
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
        }
        NativeVerticalMotionKind::EtaCoordinateVelocity => {
            motion.unit == NativeVerticalMotionUnit::PerSecond
                && matches!(
                    motion.sign,
                    NativeVerticalMotionSign::PositiveEtaIncreasing
                        | NativeVerticalMotionSign::PositiveEtaDecreasing
                )
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

fn pressure_velocity_centers_to_geometric(
    snapshot: &Snapshot,
    geometry: &VerticalTransformResult,
    omega_pa_s: &[f32],
) -> Result<Vec<f32>, VerticalTransformError> {
    let nx = geometry.nx;
    let ny = geometry.ny;
    let nz = geometry.nz;
    if nz < 2 {
        return Err(VerticalTransformError::InsufficientVerticalLevels);
    }
    let mut result = vec![0.0_f32; omega_pa_s.len()];

    for y in 0..ny {
        for x in 0..nx {
            for z in 0..nz {
                let (za, zb) = derivative_pair(z, nz);
                let ia = volume_offset(x, y, za, nx, ny);
                let ib = volume_offset(x, y, zb, nx, ny);
                let dz = geometry.height_agl_m[ib] - geometry.height_agl_m[ia];
                let dp = geometry.level_pressure_pa[ib] - geometry.level_pressure_pa[ia];
                let dzdp = dz / dp;
                if !dzdp.is_finite() || dzdp >= 0.0 {
                    return Err(VerticalTransformError::InvalidPressureToHeightDerivative {
                        x,
                        y,
                        z,
                    });
                }
                let index = volume_offset(x, y, z, nx, ny);
                result[index] = omega_pa_s[index] * dzdp;
            }
        }
    }

    // Ordering is already encoded in signed dz/dp. This explicit read prevents
    // accidental future code from treating one array direction as normative.
    let _ordering = snapshot.vertical_coordinate.ordering;
    Ok(result)
}

fn pressure_velocity_interfaces_to_geometric(
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
                let z = model_level_index_from_surface(
                    snapshot.vertical_coordinate.ordering,
                    nz,
                    step,
                );
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

fn eta_dot_to_pressure_velocity_interfaces(
    snapshot: &Snapshot,
    eta_dot_s: &[f32],
    sign: NativeVerticalMotionSign,
) -> Result<Vec<f32>, VerticalTransformError> {
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
    let pref = vertical
        .reference_surface_pressure_pa
        .ok_or(VerticalTransformError::InvalidNativeVerticalMotion {
            reason: "eta-dot conversion requires reference_surface_pressure_pa",
        })?;
    if !pref.is_finite() || pref <= 0.0 {
        return Err(VerticalTransformError::InvalidNativeVerticalMotion {
            reason: "eta-dot conversion requires positive reference_surface_pressure_pa",
        });
    }

    let surface_pressure = field_values(snapshot, FieldId::SurfacePressure)?;
    let nx = snapshot.horizontal_grid.nx;
    let ny = snapshot.horizontal_grid.ny;
    let nz = vertical.level_values.len();
    let mut omega = vec![0.0_f32; nx * ny * (nz + 1)];

    for y in 0..ny {
        for x in 0..nx {
            let ps = surface_pressure[surface_offset(x, y, nx)];
            let mut previous_omega = 0.0_f32; // model-top boundary

            for step in 0..nz {
                let (level, upper_interface, lower_interface) =
                    top_down_layer_indices(vertical.ordering, nz, step);
                let d_a = a[lower_interface] - a[upper_interface];
                let d_b = b[lower_interface] - b[upper_interface];
                let reference_thickness = d_a / pref + d_b;
                if !reference_thickness.is_finite()
                    || reference_thickness.abs() <= f32::EPSILON
                {
                    return Err(VerticalTransformError::InvalidNativeVerticalMotion {
                        reason: "eta-dot conversion encountered zero/invalid reference layer thickness",
                    });
                }

                let local_scale =
                    ps * (d_a / ps + d_b) / reference_thickness;
                if !local_scale.is_finite() {
                    return Err(VerticalTransformError::InvalidNativeVerticalMotion {
                        reason: "eta-dot conversion produced invalid dp/deta scale",
                    });
                }

                let eta_index = volume_offset(x, y, level, nx, ny);
                let eta_increasing = match sign {
                    NativeVerticalMotionSign::PositiveEtaIncreasing => eta_dot_s[eta_index],
                    NativeVerticalMotionSign::PositiveEtaDecreasing => -eta_dot_s[eta_index],
                    _ => unreachable!(),
                };
                // FLEXPART preprocessing maps the full-level eta tendency to
                // half-level pressure velocity via the centered recurrence
                // omega_lower + omega_upper = 2 * F(level).
                let f_level = eta_increasing * local_scale;
                let current_omega = 2.0 * f_level - previous_omega;
                if !current_omega.is_finite() {
                    return Err(VerticalTransformError::InvalidNativeVerticalMotionValue {
                        index: eta_index,
                        value: current_omega,
                    });
                }
                let out_index =
                    interface_offset(x, y, lower_interface, nx, ny);
                omega[out_index] = current_omega;
                previous_omega = current_omega;
            }
        }
    }
    Ok(omega)
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

#[inline]
const fn top_down_layer_indices(
    ordering: VerticalOrdering,
    nz: usize,
    step: usize,
) -> (usize, usize, usize) {
    match ordering {
        VerticalOrdering::Increasing => (step, step, step + 1),
        VerticalOrdering::Decreasing => {
            let level = nz - 1 - step;
            (level, level + 1, level)
        }
    }
}

fn field_values(
    snapshot: &Snapshot,
    id: FieldId,
) -> Result<&[f32], VerticalTransformError> {
    snapshot
        .fields
        .iter()
        .find(|field| field.id == id)
        .map(|field| field.values.as_slice())
        .ok_or(VerticalTransformError::MissingField(id))
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
    let delta_virtual_temperature =
        current_virtual_temperature_k - previous_virtual_temperature_k;

    if delta_virtual_temperature.abs() > 0.2 {
        scale
            * pressure_log
            * delta_virtual_temperature
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

#[inline]
const fn model_level_index_from_surface(
    ordering: VerticalOrdering,
    nz: usize,
    step: usize,
) -> usize {
    match ordering {
        VerticalOrdering::Increasing => nz - 1 - step,
        VerticalOrdering::Decreasing => step,
    }
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
            return Err(VerticalTransformError::InvalidPressureColumn {
                x,
                y,
                index: k,
            });
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
            return Err(VerticalTransformError::InvalidPressureColumn {
                x,
                y,
                index: k,
            });
        }
    }
    Ok(())
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
                if !height.is_finite() || !terrain.is_finite() || height < terrain {
                    return Err(VerticalTransformError::HeightBelowTerrain {
                        x,
                        y,
                        z,
                        height_asl_m: height,
                        terrain_asl_m: terrain,
                    });
                }
                result.push(height - terrain);
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
                if !height.is_finite() || !terrain.is_finite() || height < 0.0 {
                    return Err(VerticalTransformError::HeightBelowTerrain {
                        x,
                        y,
                        z,
                        height_asl_m: height + terrain,
                        terrain_asl_m: terrain,
                    });
                }
                result.push(height + terrain);
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

#[inline]
const fn surface_offset(x: usize, y: usize, nx: usize) -> usize {
    x + nx * y
}

#[inline]
const fn volume_offset(x: usize, y: usize, z: usize, nx: usize, ny: usize) -> usize {
    x + nx * (y + ny * z)
}

#[inline]
const fn interface_offset(x: usize, y: usize, z: usize, nx: usize, ny: usize) -> usize {
    x + nx * (y + ny * z)
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;
    use crate::meteorology::{
        Axis, Calendar, Field, FieldTime, HorizontalGrid, HorizontalStaggering, LongitudeDomain,
        SchemaIdentity, SignConvention, StorageOrder, TemporalKind, Unit, VerticalCoordinate,
        VerticalReference,
    };

    fn field_time(kind: TemporalKind) -> FieldTime {
        FieldTime {
            calendar: Calendar::Gregorian,
            kind,
            valid_time_epoch_seconds: 1_700_000_000,
            interval_start_epoch_seconds: None,
            interval_end_epoch_seconds: None,
            accumulation: None,
        }
    }

    fn hybrid_snapshot(ordering: VerticalOrdering) -> Snapshot {
        let (interfaces, levels) = match ordering {
            VerticalOrdering::Increasing => (
                vec![50_000.0, 75_000.0, 100_000.0],
                vec![62_500.0, 87_500.0],
            ),
            VerticalOrdering::Decreasing => (
                vec![100_000.0, 75_000.0, 50_000.0],
                vec![87_500.0, 62_500.0],
            ),
        };
        let (a, b) = match ordering {
            VerticalOrdering::Increasing => (vec![0.0, 0.0, 0.0], vec![0.5, 0.75, 1.0]),
            VerticalOrdering::Decreasing => (vec![0.0, 0.0, 0.0], vec![1.0, 0.75, 0.5]),
        };

        Snapshot {
            schema: SchemaIdentity::default(),
            horizontal_grid: HorizontalGrid {
                nx: 2,
                ny: 1,
                xlon0_deg: 0.0,
                ylat0_deg: 0.0,
                dx_deg: 1.0,
                dy_deg: 1.0,
                longitude_domain: LongitudeDomain::Minus180To180,
            },
            vertical_coordinate: VerticalCoordinate {
                kind: VerticalCoordinateKind::HybridSigmaPressure,
                reference: VerticalReference::ModelNative,
                ordering,
                level_values: levels,
                interface_values: Some(interfaces),
                hybrid_a_interface_pa: Some(a),
                hybrid_b_interface: Some(b),
                reference_surface_pressure_pa: Some(100_000.0),
                surface_pressure_dependency: Some(FieldId::SurfacePressure),
            },
            fields: vec![
                Field {
                    id: FieldId::SurfacePressure,
                    shape: vec![2, 1],
                    axis_order: vec![Axis::X, Axis::Y],
                    storage_order: StorageOrder::XFastest,
                    unit: Unit::Pascal,
                    sign: SignConvention::NonNegative,
                    horizontal_staggering: HorizontalStaggering::CellCenter,
                    vertical_staggering: VerticalStaggering::NotApplicable,
                    time: field_time(TemporalKind::Instantaneous),
                    values: vec![100_000.0, 90_000.0],
                },
                Field {
                    id: FieldId::Orography,
                    shape: vec![2, 1],
                    axis_order: vec![Axis::X, Axis::Y],
                    storage_order: StorageOrder::XFastest,
                    unit: Unit::Meter,
                    sign: SignConvention::SignedScalar,
                    horizontal_staggering: HorizontalStaggering::CellCenter,
                    vertical_staggering: VerticalStaggering::NotApplicable,
                    time: field_time(TemporalKind::Static),
                    values: vec![100.0, -20.0],
                },
            ],
        }
    }

    fn geometry_snapshot(ordering: VerticalOrdering) -> Snapshot {
        let mut snapshot = hybrid_snapshot(ordering);
        let (temperature, specific_humidity) = match ordering {
            // X-fastest storage: both X cells for the upper level, then both
            // X cells for the surface-most level.
            VerticalOrdering::Increasing => (
                vec![280.0, 281.0, 290.0, 291.0],
                vec![0.005, 0.006, 0.010, 0.011],
            ),
            VerticalOrdering::Decreasing => (
                vec![290.0, 291.0, 280.0, 281.0],
                vec![0.010, 0.011, 0.005, 0.006],
            ),
        };

        snapshot.fields.extend([
            Field {
                id: FieldId::Temperature,
                shape: vec![2, 1, 2],
                axis_order: vec![Axis::X, Axis::Y, Axis::Z],
                storage_order: StorageOrder::XFastest,
                unit: Unit::Kelvin,
                sign: SignConvention::SignedScalar,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::LevelCenter,
                time: field_time(TemporalKind::Instantaneous),
                values: temperature,
            },
            Field {
                id: FieldId::SpecificHumidity,
                shape: vec![2, 1, 2],
                axis_order: vec![Axis::X, Axis::Y, Axis::Z],
                storage_order: StorageOrder::XFastest,
                unit: Unit::KilogramPerKilogram,
                sign: SignConvention::NonNegative,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::LevelCenter,
                time: field_time(TemporalKind::Instantaneous),
                values: specific_humidity,
            },
            Field {
                id: FieldId::Temperature2m,
                shape: vec![2, 1],
                axis_order: vec![Axis::X, Axis::Y],
                storage_order: StorageOrder::XFastest,
                unit: Unit::Kelvin,
                sign: SignConvention::SignedScalar,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::NotApplicable,
                time: field_time(TemporalKind::Instantaneous),
                values: vec![292.0, 294.0],
            },
            Field {
                id: FieldId::Dewpoint2m,
                shape: vec![2, 1],
                axis_order: vec![Axis::X, Axis::Y],
                storage_order: StorageOrder::XFastest,
                unit: Unit::Kelvin,
                sign: SignConvention::SignedScalar,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::NotApplicable,
                time: field_time(TemporalKind::Instantaneous),
                values: vec![285.0, 286.0],
            },
        ]);
        snapshot
    }

    #[test]
    fn hybrid_pressure_uses_local_surface_pressure_and_declared_ordering() {
        let increasing = reconstruct_hybrid_pressure(&hybrid_snapshot(VerticalOrdering::Increasing))
            .expect("increasing pressure coordinate must reconstruct");
        assert_eq!(
            increasing.interface_pressure_pa,
            vec![50_000.0, 45_000.0, 75_000.0, 67_500.0, 100_000.0, 90_000.0]
        );
        assert_eq!(
            increasing.level_pressure_pa,
            vec![62_500.0, 56_250.0, 87_500.0, 78_750.0]
        );

        let decreasing = reconstruct_hybrid_pressure(&hybrid_snapshot(VerticalOrdering::Decreasing))
            .expect("decreasing pressure coordinate must reconstruct");
        assert_eq!(
            decreasing.interface_pressure_pa,
            vec![100_000.0, 90_000.0, 75_000.0, 67_500.0, 50_000.0, 45_000.0]
        );
    }

    #[test]
    fn flexpart_hypsometric_near_isothermal_branch_matches_analytic_solution() {
        let actual =
            flexpart_hypsometric_layer_thickness_m(100_000.0, 90_000.0, 300.0, 300.0);
        let expected = (R_AIR / GA) * (100_000.0_f32 / 90_000.0).ln() * 300.0;
        assert_relative_eq!(actual, expected, max_relative = 1.0e-6);
    }

    #[test]
    fn height_reconstruction_is_independent_of_storage_direction() {
        let increasing =
            reconstruct_vertical_geometry(&geometry_snapshot(VerticalOrdering::Increasing))
                .expect("top-to-bottom storage must reconstruct");
        let decreasing =
            reconstruct_vertical_geometry(&geometry_snapshot(VerticalOrdering::Decreasing))
                .expect("bottom-to-top storage must reconstruct");

        for x in 0..2 {
            let inc_top = increasing.height_agl_m[volume_offset(x, 0, 0, 2, 1)];
            let inc_bottom = increasing.height_agl_m[volume_offset(x, 0, 1, 2, 1)];
            let dec_bottom = decreasing.height_agl_m[volume_offset(x, 0, 0, 2, 1)];
            let dec_top = decreasing.height_agl_m[volume_offset(x, 0, 1, 2, 1)];

            assert!(inc_top > inc_bottom);
            assert!(dec_top > dec_bottom);
            assert!(inc_bottom > 0.0);
            assert_relative_eq!(inc_bottom, dec_bottom, max_relative = 1.0e-6);
            assert_relative_eq!(inc_top, dec_top, max_relative = 1.0e-6);

            let terrain = increasing.terrain_asl_m[x];
            assert_relative_eq!(
                increasing.height_asl_m[volume_offset(x, 0, 0, 2, 1)],
                inc_top + terrain,
                epsilon = 1.0e-4
            );
            assert_relative_eq!(
                increasing.height_asl_m[volume_offset(x, 0, 1, 2, 1)],
                inc_bottom + terrain,
                epsilon = 1.0e-4
            );
        }
    }

    #[test]
    fn exact_height_reconstruction_requires_flexpart_surface_thermodynamics() {
        let mut snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        snapshot
            .fields
            .retain(|field| field.id != FieldId::Dewpoint2m);
        let error = reconstruct_vertical_geometry(&snapshot)
            .expect_err("missing 2-m dewpoint must fail instead of using a fallback");
        assert_eq!(
            error,
            VerticalTransformError::Contract(ContractError::MissingRequiredField(
                FieldId::Dewpoint2m
            ))
        );
    }

    #[test]
    fn agl_asl_conversion_is_column_local_and_handles_below_sea_level_terrain() {
        let terrain = vec![100.0, -20.0];
        let agl = vec![0.0, 0.0, 500.0, 500.0];
        let asl = height_agl_to_asl(2, 1, 2, &agl, &terrain).expect("AGL to ASL");
        assert_eq!(asl, vec![100.0, -20.0, 600.0, 480.0]);
        let roundtrip = height_asl_to_agl(2, 1, 2, &asl, &terrain).expect("ASL to AGL");
        assert_eq!(roundtrip, agl);
    }

    #[test]
    fn asl_below_local_terrain_fails_closed() {
        let error = height_asl_to_agl(1, 1, 1, &[99.0], &[100.0])
            .expect_err("below-terrain ASL height must fail");
        assert!(matches!(
            error,
            VerticalTransformError::HeightBelowTerrain { .. }
        ));
    }
}
