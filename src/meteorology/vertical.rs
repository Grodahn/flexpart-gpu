//! Provider-independent vertical transformation support for canonical meteorology.
//!
//! This module is the #30 production boundary between the immutable canonical
//! \`meteorology::Snapshot\` and derived three-dimensional runtime geometry.
//! It deliberately does not depend on the legacy \`WindFieldGrid\` or
//! \`VerticalCoordinates\` types.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::constants::{GA, R_AIR};

use super::{
    ContractError, FieldId, Requirements, Snapshot, VerticalCoordinateKind, VerticalOrdering,
    VerticalReference, VerticalStaggering, SCHEMA_ID, SCHEMA_VERSION,
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
    ///
    /// Recognized at the boundary. Preprocessing is oracle-validated only for
    /// the positive-eta-increasing convention; production normalization stays
    /// fail-closed until it is explicitly enabled after #70.
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

/// Reference pressure used by flex_extract's `calc_etadot` for the eta-dot
/// preprocessing factor (`P00` in `calc_etadot.f90`, v7.1.2 source line 541).
///
/// The numeric value is 101325.0 Pa. It is part of the pinned oracle contract
/// and must not be changed independently of `reference/flex-extract.json`.
pub const FLEX_EXTRACT_CALC_ETADOT_REFERENCE_PRESSURE_PA: f32 = 101_325.0;

/// Physics-ready vertical air motion. Values are always geometric m/s with
/// positive-upward sign. Staggering is retained so #31 owns interpolation.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NormalizedVerticalMotion {
    vertical_staggering: VerticalStaggering,
    values_ms: Vec<f32>,
    provenance: NormalizedVerticalMotionProvenance,
}

impl NormalizedVerticalMotion {
    #[must_use]
    pub const fn vertical_staggering(&self) -> VerticalStaggering {
        self.vertical_staggering
    }

    #[must_use]
    pub fn values_ms(&self) -> &[f32] {
        &self.values_ms
    }

    #[must_use]
    pub const fn provenance(&self) -> &NormalizedVerticalMotionProvenance {
        &self.provenance
    }
}

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
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VerticalTransformResult {
    nx: usize,
    ny: usize,
    nz: usize,
    interface_pressure_pa: Vec<f32>,
    interface_height_asl_m: Vec<f32>,
    interface_height_agl_m: Vec<f32>,
    level_pressure_pa: Vec<f32>,
    terrain_asl_m: Vec<f32>,
    height_asl_m: Vec<f32>,
    height_agl_m: Vec<f32>,
    vertical_velocity: Option<NormalizedVerticalMotion>,
    provenance: VerticalTransformProvenance,
}

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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ResolvedReleaseHeightRange {
    pub lower: ResolvedReleaseHeight,
    pub upper: ResolvedReleaseHeight,
}

/// One model-level point exposed through the immutable #30 runtime boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalLevelPoint {
    pub pressure_pa: f32,
    pub height_agl_m: f32,
    pub height_asl_m: f32,
}

/// One W/interface point exposed through the immutable #30 runtime boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalInterfacePoint {
    pub pressure_pa: f32,
    pub height_agl_m: f32,
    pub height_asl_m: f32,
}

/// Borrowed, validated provider-independent runtime view intended for #31.
///
/// It exposes vertical geometry and motion without performing horizontal,
/// vertical, or temporal interpolation.
#[derive(Debug, Clone, Copy)]
pub struct VerticalRuntimeView<'a> {
    result: &'a VerticalTransformResult,
}

impl VerticalTransformResult {
    /// Validate all derived array shapes before exposing this result to #31.
    pub fn runtime_view(&self) -> Result<VerticalRuntimeView<'_>, VerticalTransformError> {
        let horizontal = self
            .nx
            .checked_mul(self.ny)
            .ok_or(VerticalTransformError::RuntimeShapeMismatch {
                field: "horizontal",
                expected: usize::MAX,
                actual: 0,
            })?;
        let level_count = horizontal
            .checked_mul(self.nz)
            .ok_or(VerticalTransformError::RuntimeShapeMismatch {
                field: "level_geometry",
                expected: usize::MAX,
                actual: 0,
            },
        )?;
        let interface_count = horizontal.checked_mul(self.nz + 1).ok_or(
            VerticalTransformError::RuntimeShapeMismatch {
                field: "interface_pressure_pa",
                expected: usize::MAX,
                actual: 0,
            },
        )?;

        for (field, actual, expected) in [
            ("terrain_asl_m", self.terrain_asl_m.len(), horizontal),
            (
                "level_pressure_pa",
                self.level_pressure_pa.len(),
                level_count,
            ),
            ("height_asl_m", self.height_asl_m.len(), level_count),
            ("height_agl_m", self.height_agl_m.len(), level_count),
            (
                "interface_pressure_pa",
                self.interface_pressure_pa.len(),
                interface_count,
            ),
            (
                "interface_height_asl_m",
                self.interface_height_asl_m.len(),
                interface_count,
            ),
            (
                "interface_height_agl_m",
                self.interface_height_agl_m.len(),
                interface_count,
            ),
        ] {
            if actual != expected {
                return Err(VerticalTransformError::RuntimeShapeMismatch {
                    field,
                    expected,
                    actual,
                });
            }
        }

        if let Some(motion) = &self.vertical_velocity {
            let expected = match motion.vertical_staggering {
                VerticalStaggering::LevelCenter => level_count,
                VerticalStaggering::LevelInterface => interface_count,
                VerticalStaggering::NotApplicable => {
                    return Err(VerticalTransformError::InvalidNativeVerticalMotion {
                        reason: "normalized vertical motion cannot use not_applicable staggering",
                    });
                }
            };
            if motion.values_ms.len() != expected {
                return Err(VerticalTransformError::RuntimeShapeMismatch {
                    field: "vertical_velocity.values_ms",
                    expected,
                    actual: motion.values_ms.len(),
                });
            }
        }

        Ok(VerticalRuntimeView { result: self })
    }
}

impl VerticalRuntimeView<'_> {
    #[must_use]
    pub const fn dimensions(&self) -> (usize, usize, usize) {
        (self.result.nx, self.result.ny, self.result.nz)
    }

    pub fn terrain_asl_m(&self, x: usize, y: usize) -> Result<f32, VerticalTransformError> {
        validate_xy(x, y, self.result.nx, self.result.ny)?;
        Ok(self.result.terrain_asl_m[surface_offset(x, y, self.result.nx)])
    }

    pub fn level(
        &self,
        x: usize,
        y: usize,
        z: usize,
    ) -> Result<VerticalLevelPoint, VerticalTransformError> {
        validate_xyz(x, y, z, self.result.nx, self.result.ny, self.result.nz)?;
        let index = volume_offset(x, y, z, self.result.nx, self.result.ny);
        Ok(VerticalLevelPoint {
            pressure_pa: self.result.level_pressure_pa[index],
            height_agl_m: self.result.height_agl_m[index],
            height_asl_m: self.result.height_asl_m[index],
        })
    }

    pub fn interface(
        &self,
        x: usize,
        y: usize,
        interface: usize,
    ) -> Result<VerticalInterfacePoint, VerticalTransformError> {
        validate_xyz(
            x,
            y,
            interface,
            self.result.nx,
            self.result.ny,
            self.result.nz + 1,
        )?;
        let index = interface_offset(x, y, interface, self.result.nx, self.result.ny);
        Ok(VerticalInterfacePoint {
            pressure_pa: self.result.interface_pressure_pa[index],
            height_agl_m: self.result.interface_height_agl_m[index],
            height_asl_m: self.result.interface_height_asl_m[index],
        })
    }

    pub fn interface_pressure_pa(
        &self,
        x: usize,
        y: usize,
        interface: usize,
    ) -> Result<f32, VerticalTransformError> {
        Ok(self.interface(x, y, interface)?.pressure_pa)
    }

    #[must_use]
    pub fn vertical_velocity(&self) -> Option<&NormalizedVerticalMotion> {
        self.result.vertical_velocity.as_ref()
    }

    #[must_use]
    pub const fn provenance(&self) -> &VerticalTransformProvenance {
        &self.result.provenance
    }
}

/// Machine-readable identity of the canonical vertical transformation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerticalTransformProvenance {
    pub source_schema_id: String,
    pub source_schema_version: u32,
    /// SHA-256 of the compact serde representation of the complete canonical
    /// Snapshot used to derive this runtime geometry.
    pub source_snapshot_sha256: String,
    pub source_vertical_ordering: VerticalOrdering,
    pub source_level_count: usize,
    pub pressure_algorithm_id: String,
    pub height_algorithm_id: String,
    pub w_height_algorithm_id: String,
    pub terrain_reference: VerticalReference,
    pub pressure_reconstruction: String,
    pub height_reconstruction: String,
    pub height_reference: String,
}

impl VerticalTransformProvenance {
    fn from_snapshot(snapshot: &Snapshot) -> Result<Self, VerticalTransformError> {
        Ok(Self {
            source_schema_id: SCHEMA_ID.to_string(),
            source_schema_version: SCHEMA_VERSION,
            source_snapshot_sha256: sha256_serialized(snapshot, "canonical_snapshot")?,
            source_vertical_ordering: snapshot.vertical_coordinate.ordering,
            source_level_count: snapshot.vertical_coordinate.level_values.len(),
            pressure_algorithm_id: "hybrid_interface_ab_local_ps_fulllevel_adjacent_mean_v1".to_string(),
            height_algorithm_id: "flexpart11_verttransform_ecmwf_heights_v1".to_string(),
            w_height_algorithm_id: "flexpart11_wzlev_from_uvzlev_v1".to_string(),
            terrain_reference: VerticalReference::AboveMeanSeaLevel,
            pressure_reconstruction: "p_interface=a_interface+b_interface*local_surface_pressure; p_level=mean(adjacent_interfaces)".to_string(),
            height_reconstruction: "FLEXPART-11.1 verttransform_ecmwf_heights hypsometric integration from surface virtual temperature (T2m/dewpoint) through model-level T/q".to_string(),
            height_reference: "agl_integrated_from_local_surface; asl=agl+orography_asl".to_string(),
        })
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
    #[error("hybrid surface interface at (x={x}, y={y}) is {interface_pressure_pa} Pa but local surface pressure is {surface_pressure_pa} Pa")]
    SurfaceInterfacePressureMismatch {
        x: usize,
        y: usize,
        interface_pressure_pa: f32,
        surface_pressure_pa: f32,
    },
    #[error("reconstructed pressure is invalid/non-monotonic at (x={x}, y={y}, index={index})")]
    InvalidPressureColumn { x: usize, y: usize, index: usize },
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
    #[error("runtime vertical shape mismatch for {field}: expected {expected}, got {actual}")]
    RuntimeShapeMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("vertical runtime index out of bounds: x={x}, y={y}, z={z:?}, shape=({nx},{ny},{nz:?})")]
    RuntimeIndexOutOfBounds {
        x: usize,
        y: usize,
        z: Option<usize>,
        nx: usize,
        ny: usize,
        nz: Option<usize>,
    },
    #[error("release height must use explicit AGL or ASL reference, got {reference:?}")]
    UnsupportedReleaseHeightReference { reference: VerticalReference },
    #[error("invalid release height {height_m} m for {reference:?} over terrain {terrain_asl_m} m ASL")]
    InvalidReleaseHeight {
        height_m: f32,
        reference: VerticalReference,
        terrain_asl_m: f32,
    },
    #[error("invalid release height range {lower_m}..{upper_m} m for {reference:?}")]
    InvalidReleaseHeightRange {
        lower_m: f32,
        upper_m: f32,
        reference: VerticalReference,
    },
    #[error("unsupported or ambiguous native vertical-motion semantics: {reason}")]
    InvalidNativeVerticalMotion { reason: &'static str },
    #[error("invalid native vertical-motion value at index {index}: {value}")]
    InvalidNativeVerticalMotionValue { index: usize, value: f32 },
    #[error(
        "eta-dot preprocessing produced non-finite pressure velocity at (x={x}, y={y}, level={k})"
    )]
    InvalidEtaDotTransform { x: usize, y: usize, k: usize },
    #[error("failed to serialize {input} for provenance hashing")]
    ProvenanceSerialization { input: &'static str },
    #[error("vertical-motion conversion requires at least two model levels")]
    InsufficientVerticalLevels,
    #[error("invalid dz/dp conversion at (x={x}, y={y}, z={z})")]
    InvalidPressureToHeightDerivative { x: usize, y: usize, z: usize },
    #[error("invalid ASL height at (x={x}, y={y}, z={z}): ASL={height_asl_m} m, terrain={terrain_asl_m} m")]
    InvalidAbsoluteHeight {
        x: usize,
        y: usize,
        z: usize,
        height_asl_m: f32,
        terrain_asl_m: f32,
    },
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

fn eta_dot_preprocessing_requirements() -> Requirements {
    Requirements {
        required_fields: [FieldId::SurfacePressure].into_iter().collect(),
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

    let (interface_height_agl_m, interface_height_asl_m) =
        reconstruct_flexpart_w_heights(
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
fn normalize_vertical_motion(
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
                let deta_dt = f64::from(
                    native_motion.values[volume_offset(x, y, level_index, nx, ny)],
                );

                let scaled =
                    2.0_f64 * deta_dt * ps * (dak_pa / ps + dbk) / (dak_pa / p00 + dbk);
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

fn reconstruct_flexpart_w_heights(
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
                w_bottom_up[nz] =
                    w_bottom_up[nz - 1] + uv_bottom_up[nz] - uv_bottom_up[nz - 1];
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

fn sha256_serialized<T: Serialize>(
    value: &T,
    input: &'static str,
) -> Result<String, VerticalTransformError> {
    let encoded = serde_json::to_vec(value)
        .map_err(|_| VerticalTransformError::ProvenanceSerialization { input })?;
    let digest = Sha256::digest(encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn field_values(snapshot: &Snapshot, id: FieldId) -> Result<&[f32], VerticalTransformError> {
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

fn validate_xy(x: usize, y: usize, nx: usize, ny: usize) -> Result<(), VerticalTransformError> {
    if x >= nx || y >= ny {
        return Err(VerticalTransformError::RuntimeIndexOutOfBounds {
            x,
            y,
            z: None,
            nx,
            ny,
            nz: None,
        });
    }
    Ok(())
}

fn validate_xyz(
    x: usize,
    y: usize,
    z: usize,
    nx: usize,
    ny: usize,
    nz: usize,
) -> Result<(), VerticalTransformError> {
    if x >= nx || y >= ny || z >= nz {
        return Err(VerticalTransformError::RuntimeIndexOutOfBounds {
            x,
            y,
            z: Some(z),
            nx,
            ny,
            nz: Some(nz),
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
    fn center_staggered_omega_fails_closed_without_flexpart_oracle() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::PressureVelocityOmega,
            unit: NativeVerticalMotionUnit::PascalPerSecond,
            sign: NativeVerticalMotionSign::PositivePressureIncreasing,
            vertical_staggering: VerticalStaggering::LevelCenter,
            values: vec![-1.0, 0.0, -2.0, 0.0],
            provenance: NativeVerticalMotionProvenance {
                source_id: "unvalidated-center-omega".to_string(),
            },
        };

        let error = reconstruct_vertical_geometry_with_motion(&snapshot, &native)
            .expect_err("center-staggered omega must fail until independently validated");
        assert!(matches!(
            error,
            VerticalTransformError::InvalidNativeVerticalMotion {
                reason:
                    "kind/unit/sign combination is not canonical for the declared representation"
            }
        ));
    }

    #[test]
    fn eta_dot_production_path_remains_fail_closed_after_validation() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::EtaCoordinateVelocity,
            unit: NativeVerticalMotionUnit::PerSecond,
            sign: NativeVerticalMotionSign::PositiveEtaIncreasing,
            vertical_staggering: VerticalStaggering::LevelCenter,
                values: vec![1.0e-5, 0.0, 3.0e-5, 0.0],
            provenance: NativeVerticalMotionProvenance {
                source_id: "validated-etadot-production-disabled".to_string(),
            },
        };

        let error = reconstruct_vertical_geometry_with_motion(&snapshot, &native)
            .expect_err("validated eta-dot preprocessing must remain disabled in production");
        assert!(matches!(
            error,
            VerticalTransformError::InvalidNativeVerticalMotion {
                reason: "eta-dot preprocessing is validation-only; production normalization remains disabled until explicitly enabled after #70"
            }
        ));
    }

    #[test]
    fn geometric_vertical_motion_is_identity_with_explicit_upward_semantics() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::GeometricVelocity,
            unit: NativeVerticalMotionUnit::MeterPerSecond,
            sign: NativeVerticalMotionSign::PositiveUpward,
            vertical_staggering: VerticalStaggering::LevelCenter,
            values: vec![1.0, -2.0, 3.0, -4.0],
            provenance: NativeVerticalMotionProvenance {
                source_id: "already-geometric".to_string(),
            },
        };

        let normalized =
            normalize_vertical_motion(&snapshot, &geometry, &native).expect("identity normalize");
        assert_eq!(normalized.values_ms, native.values);
    }

    #[test]
    fn ambiguous_native_vertical_motion_semantics_fail_closed() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::PressureVelocityOmega,
            unit: NativeVerticalMotionUnit::MeterPerSecond,
            sign: NativeVerticalMotionSign::PositivePressureIncreasing,
            vertical_staggering: VerticalStaggering::LevelCenter,
            values: vec![0.0; 4],
            provenance: NativeVerticalMotionProvenance {
                source_id: "wrong-unit".to_string(),
            },
        };

        let error = normalize_vertical_motion(&snapshot, &geometry, &native)
            .expect_err("omega declared as m/s must fail");
        assert!(matches!(
            error,
            VerticalTransformError::InvalidNativeVerticalMotion { .. }
        ));
    }

    #[test]
    fn native_vertical_motion_wrong_shape_fails_closed() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::PressureVelocityOmega,
            unit: NativeVerticalMotionUnit::PascalPerSecond,
            sign: NativeVerticalMotionSign::PositivePressureIncreasing,
            vertical_staggering: VerticalStaggering::LevelInterface,
            values: vec![0.0; 4], // expected 2*1*(2+1) = 6
            provenance: NativeVerticalMotionProvenance {
                source_id: "wrong-shape".to_string(),
            },
        };

        let error = normalize_vertical_motion(&snapshot, &geometry, &native)
            .expect_err("wrong interface shape must fail");
        assert!(matches!(
            error,
            VerticalTransformError::ShapeMismatch {
                field: "native_vertical_motion",
                expected: 6,
                actual: 4
            }
        ));
    }

    #[test]
    fn eta_dot_interface_staggering_fails_closed() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::EtaCoordinateVelocity,
            unit: NativeVerticalMotionUnit::PerSecond,
            sign: NativeVerticalMotionSign::PositiveEtaIncreasing,
            vertical_staggering: VerticalStaggering::LevelInterface,
            values: vec![0.0; 6],
            provenance: NativeVerticalMotionProvenance {
                source_id: "wrong-etadot-staggering".to_string(),
            },
        };

        let error = normalize_vertical_motion(&snapshot, &geometry, &native)
            .expect_err("raw eta-dot on interfaces is ambiguous and must fail");
        assert!(matches!(
            error,
            VerticalTransformError::InvalidNativeVerticalMotion { .. }
        ));
    }

    fn eta_dot_snapshot(ordering: VerticalOrdering) -> Snapshot {
        let (a, b, interface_values, level_values) = match ordering {
            VerticalOrdering::Increasing => (
                vec![1_000.0, 2_000.0, 3_000.0, 0.0],
                vec![0.5, 0.6, 0.7, 1.0],
                vec![51_000.0, 62_000.0, 73_000.0, 100_000.0],
                vec![56_500.0, 67_500.0, 86_500.0],
            ),
            VerticalOrdering::Decreasing => (
                vec![0.0, 3_000.0, 2_000.0, 1_000.0],
                vec![1.0, 0.7, 0.6, 0.5],
                vec![100_000.0, 73_000.0, 62_000.0, 51_000.0],
                vec![86_500.0, 67_500.0, 56_500.0],
            ),
        };
        Snapshot {
            schema: SchemaIdentity::default(),
            horizontal_grid: HorizontalGrid {
                nx: 1,
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
                level_values,
                interface_values: Some(interface_values),
                hybrid_a_interface_pa: Some(a),
                hybrid_b_interface: Some(b),
                reference_surface_pressure_pa: Some(100_000.0),
                surface_pressure_dependency: Some(FieldId::SurfacePressure),
            },
            fields: vec![Field {
                id: FieldId::SurfacePressure,
                shape: vec![1, 1],
                axis_order: vec![Axis::X, Axis::Y],
                storage_order: StorageOrder::XFastest,
                unit: Unit::Pascal,
                sign: SignConvention::NonNegative,
                horizontal_staggering: HorizontalStaggering::CellCenter,
                vertical_staggering: VerticalStaggering::NotApplicable,
                time: field_time(TemporalKind::Instantaneous),
                values: vec![100_000.0],
            }],
        }
    }

    fn eta_dot_motion(values: Vec<f32>) -> NativeVerticalMotion {
        NativeVerticalMotion {
            kind: NativeVerticalMotionKind::EtaCoordinateVelocity,
            unit: NativeVerticalMotionUnit::PerSecond,
            sign: NativeVerticalMotionSign::PositiveEtaIncreasing,
            vertical_staggering: VerticalStaggering::LevelCenter,
            values,
            provenance: NativeVerticalMotionProvenance {
                source_id: "eta-dot-synthetic-column".to_string(),
            },
        }
    }

    #[test]
    fn eta_dot_transform_matches_pinned_calc_etadot_formula_hand_check() {
        // calc_etadot.f90 (v7.1.2) lines 545-557 with META=1/MDPDETA=1:
        //   ETAR(K) = 2*ETAR(K)*PS*(DAK/PS+DBK)/(DAK/P00+DBK); P00=101325.0
        //   ETAR(K) = ETAR(K) - ETAR(K-1) for K > 1
        // A = [1000, 2000, 3000, 0], B = [0.5, 0.6, 0.7, 1.0], PS = 100000.
        let p00 = FLEX_EXTRACT_CALC_ETADOT_REFERENCE_PRESSURE_PA;
        let ps = 100_000.0_f32;
        let (detadot_1, detadot_2, detadot_3) = (-1.0e-5_f32, -2.0e-5_f32, 3.0e-5_f32);

        let scaled_k =
            |deta: f32, dak: f32, dbk: f32| 2.0 * deta * ps * (dak / ps + dbk) / (dak / p00 + dbk);
        let expected_1 = scaled_k(detadot_1, 1_000.0, 0.1);
        let expected_2 = scaled_k(detadot_2, 1_000.0, 0.1) - expected_1;
        let expected_3 = scaled_k(detadot_3, -3_000.0, 0.3) - expected_2;

        for ordering in [VerticalOrdering::Increasing, VerticalOrdering::Decreasing] {
            let snapshot = eta_dot_snapshot(ordering);
            let values = match ordering {
                VerticalOrdering::Increasing => vec![detadot_1, detadot_2, detadot_3],
                VerticalOrdering::Decreasing => vec![detadot_3, detadot_2, detadot_1],
            };
            let result = eta_dot_to_pressure_velocity(&snapshot, &eta_dot_motion(values))
                .expect("validated eta-dot preprocessing");
            assert_eq!(result.values_interface_pa_s.len(), 4);

            let expected = match ordering {
                // Canonical interface order for Increasing: index 0 = top.
                VerticalOrdering::Increasing => vec![0.0, expected_1, expected_2, expected_3],
                // Canonical interface order for Decreasing: index 0 = surface.
                VerticalOrdering::Decreasing => vec![expected_3, expected_2, expected_1, 0.0],
            };
            for (actual, want) in result.values_interface_pa_s.iter().zip(&expected) {
                assert_relative_eq!(actual, want, max_relative = 1.0e-5);
            }
            assert_eq!(
                result.algorithm_id,
                "flex_extract_7_1_2_calc_etadot_meta_mdpdeta_v1"
            );
        }
    }

    #[test]
    fn eta_dot_output_composes_with_flexpart_geometry_consumers() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let motion = eta_dot_motion(vec![1.0e-5, -2.0e-5, 3.0e-5, -4.0e-5]);
        let result = eta_dot_to_pressure_velocity(&snapshot, &motion).expect("eta-dot transform");
        assert_eq!(result.values_interface_pa_s.len(), 6);

        // The produced interface Pa/s field is exactly the input shape that the
        // #30 omega-interface consumer requires; it must not be rejected.
        let geometric = pressure_velocity_interfaces_to_geometric(
            &snapshot,
            &geometry,
            &result.values_interface_pa_s,
        )
        .expect("validated pressure velocity composes with the existing pinmconv path");
        assert_eq!(geometric.len(), 6);
        assert!(geometric.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn eta_dot_rejects_non_canonical_semantics_fail_closed() {
        let snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
        let mut wrong_unit = eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5]);
        wrong_unit.unit = NativeVerticalMotionUnit::PascalPerSecond;
        assert!(matches!(
            eta_dot_to_pressure_velocity(&snapshot, &wrong_unit),
            Err(VerticalTransformError::InvalidNativeVerticalMotion { .. })
        ));

        let mut wrong_staggering = eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5]);
        wrong_staggering.vertical_staggering = VerticalStaggering::LevelInterface;
        assert!(matches!(
            eta_dot_to_pressure_velocity(&snapshot, &wrong_staggering),
            Err(VerticalTransformError::InvalidNativeVerticalMotion { .. })
        ));

        let mut unvalidated_sign = eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5]);
        unvalidated_sign.sign = NativeVerticalMotionSign::PositiveEtaDecreasing;
        assert!(matches!(
            eta_dot_to_pressure_velocity(&snapshot, &unvalidated_sign),
            Err(VerticalTransformError::InvalidNativeVerticalMotion { .. })
        ));

        let wrong_shape = eta_dot_motion(vec![-1.0e-5, -2.0e-5]);
        assert!(matches!(
            eta_dot_to_pressure_velocity(&snapshot, &wrong_shape),
            Err(VerticalTransformError::ShapeMismatch { .. })
        ));
    }

    #[test]
    fn eta_dot_rejects_non_finite_inputs_and_bad_coefficients_fail_closed() {
        let mut snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
        snapshot.vertical_coordinate.hybrid_a_interface_pa = Some(vec![1_000.0, 2_000.0, 3_000.0]);
        assert!(matches!(
            eta_dot_to_pressure_velocity(
                &snapshot,
                &eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5])
            ),
            Err(VerticalTransformError::Contract(_))
        ));

        let snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
        assert!(matches!(
            eta_dot_to_pressure_velocity(
                &snapshot,
                &eta_dot_motion(vec![f32::NAN, -2.0e-5, 3.0e-5])
            ),
            Err(VerticalTransformError::InvalidNativeVerticalMotionValue { index: 0, .. })
        ));

        let mut bad_pressure = eta_dot_snapshot(VerticalOrdering::Increasing);
        bad_pressure.fields[0].values = vec![f32::NEG_INFINITY];
        assert!(matches!(
            eta_dot_to_pressure_velocity(
                &bad_pressure,
                &eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5])
            ),
            Err(VerticalTransformError::Contract(_))
        ));
    }

    #[test]
    fn eta_dot_rejects_non_hybrid_coordinates_fail_closed() {
        let mut snapshot = eta_dot_snapshot(VerticalOrdering::Increasing);
        snapshot.vertical_coordinate.kind = VerticalCoordinateKind::Pressure;
        snapshot.vertical_coordinate.hybrid_a_interface_pa = None;
        snapshot.vertical_coordinate.hybrid_b_interface = None;
        snapshot.vertical_coordinate.reference_surface_pressure_pa = None;
        snapshot.vertical_coordinate.surface_pressure_dependency = None;
        snapshot.vertical_coordinate.interface_values = None;
        assert!(matches!(
            eta_dot_to_pressure_velocity(
                &snapshot,
                &eta_dot_motion(vec![-1.0e-5, -2.0e-5, 3.0e-5])
            ),
            Err(VerticalTransformError::UnsupportedVerticalCoordinate)
        ));
    }

    #[test]
    fn non_finite_native_vertical_motion_fails_closed() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::GeometricVelocity,
            unit: NativeVerticalMotionUnit::MeterPerSecond,
            sign: NativeVerticalMotionSign::PositiveUpward,
            vertical_staggering: VerticalStaggering::LevelCenter,
            values: vec![0.0, f32::NAN, 0.0, 0.0],
            provenance: NativeVerticalMotionProvenance {
                source_id: "nan-motion".to_string(),
            },
        };

        let error = normalize_vertical_motion(&snapshot, &geometry, &native)
            .expect_err("non-finite vertical motion must fail");
        assert!(matches!(
            error,
            VerticalTransformError::InvalidNativeVerticalMotionValue {
                index: 1,
                value
            } if value.is_nan()
        ));
    }

    #[test]
    fn vertical_motion_provenance_records_source_and_output_staggering() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::PressureVelocityOmega,
            unit: NativeVerticalMotionUnit::PascalPerSecond,
            sign: NativeVerticalMotionSign::PositivePressureIncreasing,
            vertical_staggering: VerticalStaggering::LevelInterface,
            values: vec![0.0, 0.0, -1.0, 0.0, -2.0, 0.0],
            provenance: NativeVerticalMotionProvenance {
                source_id: "provenance-test".to_string(),
            },
        };

        let normalized =
            normalize_vertical_motion(&snapshot, &geometry, &native).expect("normalize");
        assert_eq!(
            normalized.provenance.source_vertical_staggering,
            VerticalStaggering::LevelInterface
        );
        assert_eq!(
            normalized.provenance.output_vertical_staggering,
            VerticalStaggering::LevelInterface
        );
        assert_eq!(
            normalized.provenance.algorithm_id,
            "omega_interface_flexpart11_pinmconv_v1"
        );
        assert_eq!(normalized.provenance.source_native_motion_sha256.len(), 64);
        assert!(normalized
            .provenance
            .source_native_motion_sha256
            .chars()
            .all(|character| character.is_ascii_hexdigit()));

        assert_eq!(geometry.provenance.source_snapshot_sha256.len(), 64);
        assert!(geometry
            .provenance
            .source_snapshot_sha256
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn provenance_hashes_change_when_transform_inputs_change() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");

        let mut changed_snapshot = snapshot.clone();
        changed_snapshot
            .fields
            .iter_mut()
            .find(|field| field.id == FieldId::Temperature2m)
            .expect("temperature2m")
            .values[0] += 0.25;
        let changed_geometry =
            reconstruct_vertical_geometry(&changed_snapshot).expect("changed geometry");
        assert_ne!(
            geometry.provenance.source_snapshot_sha256,
            changed_geometry.provenance.source_snapshot_sha256
        );

        let native = NativeVerticalMotion {
            kind: NativeVerticalMotionKind::PressureVelocityOmega,
            unit: NativeVerticalMotionUnit::PascalPerSecond,
            sign: NativeVerticalMotionSign::PositivePressureIncreasing,
            vertical_staggering: VerticalStaggering::LevelInterface,
            values: vec![0.0, 0.0, -1.0, 0.0, -2.0, 0.0],
            provenance: NativeVerticalMotionProvenance {
                source_id: "hash-binding".to_string(),
            },
        };
        let first = normalize_vertical_motion(&snapshot, &geometry, &native).expect("normalize");
        let mut changed_native = native.clone();
        changed_native.values[2] = -1.25;
        let second =
            normalize_vertical_motion(&snapshot, &geometry, &changed_native).expect("normalize");
        assert_ne!(
            first.provenance.source_native_motion_sha256,
            second.provenance.source_native_motion_sha256
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
    fn derived_asl_geometry_overflow_fails_closed() {
        assert!(matches!(
            reconstruct_flexpart_w_heights(
                VerticalOrdering::Increasing,
                1,
                1,
                1,
                &[f32::MAX],
                &[f32::MAX],
            ),
            Err(VerticalTransformError::InvalidAbsoluteHeight { x: 0, y: 0, .. })
        ));
    }

    #[test]
    fn release_and_height_reference_arithmetic_overflow_fails_closed() {
        assert!(matches!(
            resolve_release_height(
                f32::MAX,
                VerticalReference::AboveGroundLevel,
                f32::MAX,
            ),
            Err(VerticalTransformError::InvalidReleaseHeight { .. })
        ));

        assert!(matches!(
            height_agl_to_asl(1, 1, 1, &[f32::MAX], &[f32::MAX]),
            Err(VerticalTransformError::InvalidAbsoluteHeight { .. })
        ));

        assert!(matches!(
            height_asl_to_agl(1, 1, 1, &[f32::MAX], &[-f32::MAX]),
            Err(VerticalTransformError::InvalidAbsoluteHeight { .. })
        ));
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
    #[test]
    fn hybrid_surface_interface_must_match_actual_local_surface_pressure() {
        let mut snapshot = hybrid_snapshot(VerticalOrdering::Increasing);
        let a = snapshot
            .vertical_coordinate
            .hybrid_a_interface_pa
            .as_mut()
            .expect("hybrid A");
        let b = snapshot
            .vertical_coordinate
            .hybrid_b_interface
            .as_mut()
            .expect("hybrid B");

        // This still matches the schema's 100 kPa reference interface:
        // 10 kPa + 0.9 * 100 kPa = 100 kPa. At the second column's actual
        // 90 kPa surface pressure it becomes 91 kPa and must fail closed
        // rather than pairing 91 kPa with the 0 m AGL W surface.
        a[2] = 10_000.0;
        b[2] = 0.9;

        let error = reconstruct_hybrid_pressure(&snapshot)
            .expect_err("surface hybrid interface must track each column's local ps");
        assert!(matches!(
            error,
            VerticalTransformError::SurfaceInterfacePressureMismatch {
                x: 1,
                y: 0,
                interface_pressure_pa,
                surface_pressure_pa,
            } if (interface_pressure_pa - 91_000.0).abs() < 0.1
                && (surface_pressure_pa - 90_000.0).abs() < 0.1
        ));
    }

    #[test]
    fn runtime_view_rejects_internal_corrupt_derived_shape() {
        let snapshot = geometry_snapshot(VerticalOrdering::Increasing);
        let mut geometry = reconstruct_vertical_geometry(&snapshot).expect("geometry");
        geometry.height_agl_m.pop();

        let error = geometry
            .runtime_view()
            .expect_err("corrupt internal runtime shape must fail");
        assert!(matches!(
            error,
            VerticalTransformError::RuntimeShapeMismatch {
                field: "height_agl_m",
                ..
            }
        ));
    }

}
